use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use reqwest::Client;

mod config;
mod security;
mod network;
mod compiler;
mod orchestrator;
mod session;

use config::{HTTP_TIMEOUT, HTTP_POOL_IDLE, TARGET_FILE};
use security::is_safe;
use network::{get_ollama_embedding, generate_with_retry};
use compiler::{
    run_hardened_compile, run_hardened_execution, get_unique_job_dir,
    run_workspace_check_at, run_workspace_execution,
    extract_clean_error, Language, RuntimeConfig,
};
use orchestrator::{construct_initial_prompt, FILE_TAG_OPEN, FILE_TAG_SEP, FILE_TAG_CLOSE};
use miller_memory::{log_event, LogLevel};
use session::{AgentState, SessionState, save_session, load_last_session, clear_session};

use miller_parser::scanner::scan_and_parse_project_incremental;
use miller_memory::{MillerMemory, MemoryPayload};

// ── Retrieval quality guard ───────────────────────────────────────────────────

const MIN_RETRIEVAL_SCORE: f32 = 0.35;

// ═══════════════════════════════════════════════════════════════════════════════
// MULTI-FILE PARSER ENGINE
// ═══════════════════════════════════════════════════════════════════════════════

/// A single parsed file block from the LLM response.
#[derive(Debug, Clone)]
pub struct ParsedFile {
    /// Relative path as emitted by the model (already sanitised).
    pub rel_path: String,
    /// Raw file contents between the open and close tags.
    pub contents: String,
}

/// Parse ALL `<file path="...">...</file>` blocks out of `response`.
///
/// Uses only `str::find` / slice indexing — no external XML crate.
///
/// Edge cases handled:
/// - Whitespace inside the opening tag attribute.
/// - Leading/trailing whitespace in path values.
/// - Nested content that contains the literal string `</file>` in a comment
///   is NOT supported (the LLM is instructed never to nest tags), but the
///   parser is greedy-first-match safe for normal code.
/// - Returns an empty `Vec` if no valid blocks are found.
pub fn parse_file_blocks(response: &str) -> Vec<ParsedFile> {
    let mut files  = Vec::new();
    let mut cursor = 0usize;

    loop {
        // Locate the next opening tag prefix: `<file path="`
        let tag_start = match response[cursor..].find(FILE_TAG_OPEN) {
            Some(offset) => cursor + offset,
            None         => break,
        };

        // The path value starts immediately after FILE_TAG_OPEN.
        let path_value_start = tag_start + FILE_TAG_OPEN.len();

        // The path value ends at the closing `">` separator.
        let sep_pos = match response[path_value_start..].find(FILE_TAG_SEP) {
            Some(offset) => path_value_start + offset,
            None         => {
                // Malformed opening tag — skip past this match and continue.
                cursor = tag_start + FILE_TAG_OPEN.len();
                continue;
            }
        };

        let raw_path = response[path_value_start..sep_pos].trim();

        // Sanitise: strip leading `/`, `./`, and any `../` sequences so the
        // resolved path can never escape the workspace/job root.
        let safe_path = raw_path
            .trim_start_matches('/')
            .trim_start_matches("./");
        // Reject anything that still contains a `..` component after stripping.
        if safe_path.contains("..") || safe_path.is_empty() {
            cursor = sep_pos + FILE_TAG_SEP.len();
            continue;
        }

        // Content starts immediately after `">`.
        let content_start = sep_pos + FILE_TAG_SEP.len();

        // Content ends at the first `</file>` after content_start.
        let close_pos = match response[content_start..].find(FILE_TAG_CLOSE) {
            Some(offset) => content_start + offset,
            None         => {
                // No closing tag found — remainder is malformed; stop parsing.
                break;
            }
        };

        let contents = response[content_start..close_pos].to_string();
        // Trim a single leading newline that the model may insert after `">`
        // to keep indentation clean.
        let contents = contents
            .strip_prefix('\n')
            .unwrap_or(&contents)
            .to_string();

        files.push(ParsedFile {
            rel_path: safe_path.to_string(),
            contents,
        });

        // Advance past the closing tag for the next iteration.
        cursor = close_pos + FILE_TAG_CLOSE.len();
    }

    files
}

/// Write all parsed file blocks to disk under `root_dir`.
///
/// - Creates missing parent directories automatically.
/// - Returns a `Vec<String>` of the absolute host paths that were written,
///   in the same order as `files`.
/// - Returns an `Err` on the first I/O failure (partial writes are possible;
///   the caller should treat any error as a reason to re-prompt).
pub fn write_parsed_files(
    files: &[ParsedFile],
    root_dir: &str,
) -> Result<Vec<String>, std::io::Error> {
    let mut written = Vec::with_capacity(files.len());

    for pf in files {
        let abs = Path::new(root_dir).join(&pf.rel_path);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&abs, &pf.contents)?;
        written.push(abs.to_string_lossy().to_string());
    }

    Ok(written)
}

/// Reconstruct a multi-file tagged string from a slice of `ParsedFile`s.
///
/// Used to populate `SessionState::generated_code` so that a recovered session
/// can re-emit the same tagged format into the repair prompt, keeping the
/// round-trip format consistent.
pub fn files_to_tagged_repr(files: &[ParsedFile]) -> String {
    let mut out = String::new();
    for pf in files {
        out.push_str(&format!(
            "{}{}{}\n{}\n{}\n",
            FILE_TAG_OPEN, pf.rel_path, FILE_TAG_SEP,
            pf.contents,
            FILE_TAG_CLOSE,
        ));
    }
    out
}

/// Run the security scanner across all file contents in a parsed set.
/// Returns `false` and logs the offending path as soon as a violation is found.
fn scan_all_files(files: &[ParsedFile]) -> bool {
    for pf in files {
        if !is_safe(&pf.contents) {
            return false;
        }
    }
    true
}

/// Detect whether a Cargo.toml is present in the written file list and return
/// its **container-side** path (`/workspace/Cargo.toml`).
///
/// For Rust projects this is always `/workspace/Cargo.toml`; we verify the
/// model actually emitted it so we can give a clear error if it did not.
fn find_container_manifest(files: &[ParsedFile], lang: &Language) -> Option<String> {
    let descriptor = match lang {
        Language::Rust   => "Cargo.toml",
        Language::Python => "main.py",
    };
    files
        .iter()
        .find(|f| {
            // Accept the manifest at any depth (e.g. "Cargo.toml" or
            // "my_project/Cargo.toml"), but prefer the shallowest one.
            Path::new(&f.rel_path)
                .file_name()
                .map(|n| n == descriptor)
                .unwrap_or(false)
        })
        .map(|f| format!("/workspace/{}", f.rel_path))
}

// ═══════════════════════════════════════════════════════════════════════════════
// MAIN
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::builder()
        .timeout(HTTP_TIMEOUT)
        .pool_idle_timeout(HTTP_POOL_IDLE)
        .build()?;

    let memory_layer = MillerMemory::new();
    memory_layer.init_collection().await?;

    log_event(LogLevel::Info, "system", "=== MILLER: Local Autonomous Coding Framework ===");

    // ── CLI: external workspace path ──────────────────────────────────────────
    let args: Vec<String> = std::env::args().collect();
    let mut target_project_path: Option<PathBuf> = None;

    if args.len() > 1 {
        let path = PathBuf::from(&args[1]);
        if path.exists() {
            log_event(LogLevel::Info, "system", &format!("External workspace loaded: {:?}", path));
            target_project_path = Some(path);
        } else {
            log_event(LogLevel::Error, "system", &format!("Workspace path does not exist: {:?}", path));
            return Ok(());
        }
    } else {
        log_event(LogLevel::Warn, "system", "No external workspace attached. Running in scratchpad mode.");
    }

    let workspace_root: Option<String> = target_project_path
        .as_ref()
        .map(|p| p.to_string_lossy().to_string());

    // ── Background indexing thread ────────────────────────────────────────────
    if let Some(ref bg_path) = target_project_path {
        let bg_path   = bg_path.clone();
        let bg_client = client.clone();

        tokio::task::spawn(async move {
            log_event(LogLevel::Info, "background",
                &format!("Silent monitoring loop active: {:?}", bg_path));

            let scan_result = tokio::task::spawn_blocking(move || {
                scan_and_parse_project_incremental(&bg_path)
            }).await;

            let (changed_nodes, skipped_count) = match scan_result {
                Ok(r)  => r,
                Err(e) => {
                    log_event(LogLevel::Error, "background",
                        &format!("Scanner thread crashed: {}", e));
                    return;
                }
            };

            if skipped_count > 0 {
                log_event(LogLevel::Info, "background",
                    &format!("Fast skip operational: {} files unchanged.", skipped_count));
            }

            if changed_nodes.is_empty() {
                log_event(LogLevel::Info, "background", "Cache clean. Workspace state sync complete.");
                return;
            }

            log_event(LogLevel::Info, "background",
                &format!("Syncing {} modified items into local Qdrant...", changed_nodes.len()));

            let bg_memory  = MillerMemory::new();
            let mut pseudo_id = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            for (idx, chunk) in changed_nodes.iter().enumerate() {
                if let Ok(vector) = get_ollama_embedding(&bg_client, &chunk.source_code).await {
                    let payload = MemoryPayload {
                        file_path:   chunk.file_path.clone(),
                        entity_name: chunk.entity_name.clone(),
                        entity_type: chunk.entity_type.clone(),
                        content:     chunk.source_code.clone(),
                    };
                    if bg_memory.upsert_code_chunk(pseudo_id, vector, payload).await.is_ok() {
                        pseudo_id += 1;
                    }
                }
                if (idx + 1) % 30 == 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
            }
            log_event(LogLevel::Info, "background", "Vector embedding sync cycle completed.");
        });
    }

    // ── Crash recovery interceptor ────────────────────────────────────────────
    let mut original_task   = String::new();
    let mut current_prompt  = String::new();
    let mut attempts        = 0usize;
    let mut resumed_session = false;

    if let Some(last_session) = load_last_session() {
        log_event(LogLevel::Warn,  "recovery", "Detected a previously interrupted session checkpoint!");
        log_event(LogLevel::Info,  "recovery", &format!("Interrupted Task: \"{}\"", last_session.task));
        log_event(LogLevel::Info,  "recovery", &format!("Last Known State: {:?}", last_session.current_state));

        print!("Resume this session? (y/N): ");
        io::stdout().flush()?;
        let mut choice = String::new();
        io::stdin().read_line(&mut choice)?;

        if choice.trim().to_lowercase() == "y" {
            log_event(LogLevel::Info, "recovery", "Re-loading active session state machine...");
            original_task   = last_session.task.clone();
            attempts        = last_session.attempt;
            resumed_session = true;

            if last_session.current_state == AgentState::Failed
                && !last_session.last_error.is_empty()
            {
                // `generated_code` stores the tagged file repr from the last
                // attempt; feed it back into the repair prompt so the model
                // sees the same format it is expected to produce.
                current_prompt = orchestrator::build_repair_prompt(
                    &last_session.task,
                    &last_session.generated_code,
                    &last_session.last_error,
                    "Recovered Crash Error",
                );
            } else {
                current_prompt = last_session.generated_code.clone();
            }
        } else {
            log_event(LogLevel::Info, "recovery", "Discarding stale session tokens.");
            clear_session();
        }
    }

    // ── Task intake (skipped when resuming) ───────────────────────────────────
    let mut context_code_str = String::new();

    if original_task.is_empty() {
        match &workspace_root {
            Some(root) => log_event(LogLevel::Info, "system",
                &format!("Workspace mode active — project: {}", root)),
            None       => log_event(LogLevel::Info, "system",
                "Scratchpad mode active. Multi-file output will be written to a temporary job directory."),
        }

        print!("\nMiller ko task batao:\n> ");
        io::stdout().flush()?;
        io::stdin().read_line(&mut original_task)?;
        original_task = original_task.trim().to_string();

        // ── Semantic retrieval with quality guard ─────────────────────────
        log_event(LogLevel::Info, "retrieval", "Searching past code patterns from local Qdrant DB...");

        if let Ok(query_vector) = get_ollama_embedding(&client, &original_task).await {
            if let Ok(matched_chunks) = memory_layer.search_similar_code(query_vector, 2).await {
                let qualified: Vec<_> = matched_chunks
                    .into_iter()
                    .filter(|c| c.score >= MIN_RETRIEVAL_SCORE)
                    .collect();

                if qualified.is_empty() {
                    log_event(LogLevel::Info, "retrieval",
                        "No matches above similarity threshold. Proceeding without retrieved context.");
                } else {
                    log_event(LogLevel::Info, "retrieval",
                        &format!("{} qualified match(es) (score >= {:.2}). Injecting context.",
                            qualified.len(), MIN_RETRIEVAL_SCORE));

                    context_code_str.push_str("\n--- RELEVANT EXISTING CONTEXT CODE ---\n");
                    for chunk in &qualified {
                        if chunk.payload.file_path.contains("miller_core")
                            || chunk.payload.file_path.contains("miller_parser")
                        {
                            continue;
                        }
                        context_code_str.push_str(&format!(
                            "// From File: {}\n// Entity: {}\n{}\n\n",
                            chunk.payload.file_path,
                            chunk.payload.entity_name,
                            chunk.payload.content,
                        ));
                    }
                    context_code_str.push_str("---------------------------------------\n");
                }
            }
        }

        let active_language = Language::Rust;
        current_prompt = construct_initial_prompt(&context_code_str, &original_task, &active_language);
    }

    // ── Runtime config ────────────────────────────────────────────────────────
    let active_language = Language::Rust;
    let runtime_config  = RuntimeConfig::for_language(&active_language);

    if current_prompt.is_empty() {
        current_prompt = construct_initial_prompt(&context_code_str, &original_task, &active_language);
    }

    const MAX_ATTEMPTS: usize = 5;

    // ═══════════════════════════════════════════════════════════════════════════
    // AGENT REPAIR LOOP
    // ═══════════════════════════════════════════════════════════════════════════
    while attempts < MAX_ATTEMPTS {
        if !resumed_session {
            attempts += 1;
        }
        resumed_session = false;

        log_event(LogLevel::Info, "engine",
            &format!("Transitioning State: [AgentState::Generating] (Attempt {}/{})",
                attempts, MAX_ATTEMPTS));

        let mut active_session = SessionState {
            task:           original_task.clone(),
            attempt:        attempts,
            current_state:  AgentState::Generating,
            generated_code: current_prompt.clone(),
            last_error:     String::new(),
        };
        let _ = save_session(&active_session);

        // ── Step 1: Generate ──────────────────────────────────────────────────
        let ai_response = match generate_with_retry(&client, &current_prompt).await {
            Ok(text) => text,
            Err(e) => {
                log_event(LogLevel::Error, "network",
                    &format!("Ollama call permanently failed: {}", e));
                active_session.current_state = AgentState::Failed;
                active_session.last_error    = e.to_string();
                let _ = save_session(&active_session);
                break;
            }
        };

        // ── Step 2: Parse multi-file blocks ───────────────────────────────────
        log_event(LogLevel::Info, "engine", "Transitioning State: [AgentState::Sanitizing]");
        active_session.current_state = AgentState::Sanitizing;
        let _ = save_session(&active_session);

        let parsed_files = parse_file_blocks(&ai_response);

        if parsed_files.is_empty() {
            // The model did not follow the tagging protocol at all. Re-prompt
            // with an explicit reminder rather than entering the repair loop.
            log_event(LogLevel::Warn, "parser",
                "No <file path=\"...\"> blocks found in response. Re-prompting with format reminder.");

            current_prompt = format!(
                "Your previous response did not contain any valid <file path=\"...\"> blocks.\n\
                 You MUST wrap every file inside <file path=\"relative/path\">contents</file> tags.\n\
                 No markdown fences. No commentary outside the tags.\n\
                 Re-generate now.\n\
                 Task: {}",
                original_task,
            );
            continue;
        }

        log_event(LogLevel::Info, "parser",
            &format!("Parsed {} file block(s) from AI response.", parsed_files.len()));
        for pf in &parsed_files {
            log_event(LogLevel::Info, "parser",
                &format!("  → '{}' ({} bytes)", pf.rel_path, pf.contents.len()));
        }

        // ── Step 3: Security scan ─────────────────────────────────────────────
        log_event(LogLevel::Info, "engine", "Transitioning State: [AgentState::SecurityScanning]");

        if !scan_all_files(&parsed_files) {
            log_event(LogLevel::Error, "security",
                "Dangerous instruction sequence intercepted in generated file set. Task terminated.");
            active_session.current_state = AgentState::Failed;
            active_session.last_error    = "Security scan intercepted a rule infraction.".to_string();
            let _ = save_session(&active_session);
            break;
        }

        // ── Step 4: Write files to disk ───────────────────────────────────────
        // Determine the root directory to write into:
        //   - Workspace mode  → the real project root supplied via CLI.
        //   - Scratchpad mode → a fresh unique ephemeral job directory so that
        //     multi-file scratchpad projects (e.g. Cargo.toml + src/main.rs)
        //     are laid out exactly as the model intended.
        let write_root: String = match &workspace_root {
            Some(root) => root.clone(),
            None       => {
                let job_dir = get_unique_job_dir();
                fs::create_dir_all(&job_dir).map_err(|e| {
                    log_event(LogLevel::Error, "filesystem",
                        &format!("Failed to create scratchpad job directory: {}", e));
                    e
                })?;
                job_dir
            }
        };

        let written_paths = match write_parsed_files(&parsed_files, &write_root) {
            Ok(paths) => paths,
            Err(e) => {
                log_event(LogLevel::Error, "filesystem",
                    &format!("File write failure: {}", e));
                active_session.current_state = AgentState::Failed;
                active_session.last_error    = e.to_string();
                let _ = save_session(&active_session);
                break;
            }
        };

        for path in &written_paths {
            log_event(LogLevel::Info, "filesystem", &format!("Written → '{}'", path));
        }

        // Persist the tagged representation into the session so crash recovery
        // can reconstruct the repair prompt with the original file set.
        let files_repr = files_to_tagged_repr(&parsed_files);
        active_session.generated_code = files_repr.clone();

        // ── Step 5: Compile / Check ───────────────────────────────────────────
        log_event(LogLevel::Info, "engine", "Transitioning State: [AgentState::Compiling]");
        active_session.current_state = AgentState::Compiling;
        let _ = save_session(&active_session);

        let (check_ok, check_stderr) = if workspace_root.is_some() {
            // ── Workspace mode ────────────────────────────────────────────────
            // Detect the container-side manifest path from the written file set.
            let manifest = find_container_manifest(&parsed_files, &active_language)
                .unwrap_or_else(|| runtime_config.container_manifest_path(&active_language).to_string());

            log_event(LogLevel::Info, "compiler",
                &format!("Workspace check: cargo check (manifest: {})", manifest));

            match run_workspace_check_at(&write_root, &runtime_config, &manifest) {
                Ok(result) => result,
                Err(e) => {
                    log_event(LogLevel::Error, "compiler",
                        &format!("Critical workspace check failure: {}", e));
                    active_session.current_state = AgentState::Failed;
                    active_session.last_error    = e.to_string();
                    let _ = save_session(&active_session);
                    break;
                }
            }
        } else {
            // ── Scratchpad mode ───────────────────────────────────────────────
            // For a multi-file scratchpad (Cargo project), run `cargo check`
            // inside the container against the job directory. For a bare single
            // .rs file, fall back to `rustc`.
            let has_cargo_toml = parsed_files.iter()
                .any(|f| Path::new(&f.rel_path).file_name().map(|n| n == "Cargo.toml").unwrap_or(false));

            if has_cargo_toml {
                // Full Cargo project in scratchpad mode: use workspace check
                // against the job dir which now contains the full layout.
                // let manifest = format!("{}/Cargo.toml", write_root);
                let container_manifest = "/workspace/Cargo.toml".to_string();

                log_event(LogLevel::Info, "compiler",
                    &format!("Scratchpad cargo check (job dir: {})", write_root));

                match run_workspace_check_at(&write_root, &runtime_config, &container_manifest) {
                    Ok(result) => result,
                    Err(e) => {
                        log_event(LogLevel::Error, "compiler",
                            &format!("Critical scratchpad cargo check failure: {}", e));
                        active_session.current_state = AgentState::Failed;
                        active_session.last_error    = e.to_string();
                        let _ = save_session(&active_session);
                        // Clean up orphaned job dir.
                        compiler::safe_cleanup_job_dir(&write_root);
                        break;
                    }
                }
            } else {
                // Single-file scratchpad: original rustc path.
                // Find the primary source file (first .rs file in the set, or
                // fall back to TARGET_FILE if none parsed).
                let source_path = parsed_files.iter()
                    .find(|f| f.rel_path.ends_with(&format!(".{}", runtime_config.source_extension)))
                    .map(|f| format!("{}/{}", write_root, f.rel_path))
                    .unwrap_or_else(|| TARGET_FILE.to_string());

                log_event(LogLevel::Info, "compiler",
                    &format!("Scratchpad single-file compile: {}", source_path));

                match run_hardened_compile(&write_root, &runtime_config, &source_path) {
                    Ok(result) => result,
                    Err(e) => {
                        log_event(LogLevel::Error, "compiler",
                            &format!("Critical compile engine failure: {}", e));
                        active_session.current_state = AgentState::Failed;
                        active_session.last_error    = e.to_string();
                        let _ = save_session(&active_session);
                        break;
                    }
                }
            }
        };

        if !check_ok {
            let clean_error = extract_clean_error(&check_stderr);
            log_event(LogLevel::Error, "compiler", "Check/compilation failed. Error details captured.");
            active_session.current_state = AgentState::Failed;
            active_session.last_error    = clean_error.clone();
            let _ = save_session(&active_session);

            let error_type = if workspace_root.is_some() || parsed_files.iter().any(|f|
                Path::new(&f.rel_path).file_name().map(|n| n == "Cargo.toml").unwrap_or(false))
            {
                "Cargo Check Error"
            } else {
                "Compilation Error"
            };

            // Pass the full tagged file repr into the repair prompt so the
            // model can see which files it emitted and what broke.
            current_prompt = orchestrator::build_repair_prompt(
                &original_task, &files_repr, &clean_error, error_type,
            );
            // Clean up the failed scratchpad job dir before the next attempt.
            if workspace_root.is_none() {
                compiler::safe_cleanup_job_dir(&write_root);
            }
            continue;
        }

        // ── Step 6: Execute ───────────────────────────────────────────────────
        log_event(LogLevel::Info, "engine",
            "Transitioning State: [AgentState::Executing] inside container boundary...");
        active_session.current_state = AgentState::Executing;
        let _ = save_session(&active_session);

        let exec_result = if workspace_root.is_some() {
            // Workspace mode: mount and run.
            log_event(LogLevel::Info, "sandbox",
                &format!("Workspace execution: mounting '{}' as /workspace", write_root));
            run_workspace_execution(&write_root, &runtime_config, &active_language)
        } else {
            let has_cargo = parsed_files.iter()
                .any(|f| Path::new(&f.rel_path).file_name().map(|n| n == "Cargo.toml").unwrap_or(false));

            if has_cargo {
                // Multi-file scratchpad: use workspace execution against the
                // job dir which contains the full Cargo layout.
                log_event(LogLevel::Info, "sandbox",
                    &format!("Multi-file scratchpad execution (job dir: {})", write_root));
                run_workspace_execution(&write_root, &runtime_config, &active_language)
            } else {
                // Single-file scratchpad: re-compile into a fresh exec job dir,
                // then run. Two separate containers for isolation.
                let exec_job_dir = get_unique_job_dir();
                log_event(LogLevel::Info, "sandbox",
                    &format!("Single-file scratchpad execution (ephemeral jail: {})", exec_job_dir));

                let source_path = parsed_files.iter()
                    .find(|f| f.rel_path.ends_with(&format!(".{}", runtime_config.source_extension)))
                    .map(|f| format!("{}/{}", write_root, f.rel_path))
                    .unwrap_or_else(|| TARGET_FILE.to_string());

                if let Err(e) = run_hardened_compile(&exec_job_dir, &runtime_config, &source_path) {
                    log_event(LogLevel::Error, "compiler",
                        &format!("Re-compile for execution stage failed: {}", e));
                    compiler::safe_cleanup_job_dir(&write_root);
                    break;
                }
                // The check-phase job dir is no longer needed.
                compiler::safe_cleanup_job_dir(&write_root);
                run_hardened_execution(&exec_job_dir, &runtime_config)
            }
        };

        match exec_result {
            Ok(result) => {
                log_event(LogLevel::Info, "telemetry",
                    &format!("duration={}ms stdout_bytes={} stderr_bytes={} exit_code={:?}",
                        result.metrics.execution_duration_ms,
                        result.metrics.stdout_size,
                        result.metrics.stderr_size,
                        result.metrics.exit_code));

                if result.timed_out {
                    log_event(LogLevel::Error, "sandbox", &result.stderr);
                    active_session.current_state = AgentState::Failed;
                    active_session.last_error    = result.stderr.clone();
                    let _ = save_session(&active_session);
                    current_prompt = orchestrator::build_repair_prompt(
                        &original_task, &files_repr, &result.stderr, "Runtime Timeout Error");

                } else if result.limit_exceeded {
                    log_event(LogLevel::Error, "security", &result.stderr);
                    active_session.current_state = AgentState::Failed;
                    active_session.last_error    = result.stderr.clone();
                    let _ = save_session(&active_session);
                    current_prompt = orchestrator::build_repair_prompt(
                        &original_task, &files_repr, &result.stderr,
                        "Host Pipe Bomb Exploitation Intercept");

                } else if !result.stderr.is_empty() {
                    log_event(LogLevel::Error, "sandbox",
                        &format!("[Runtime Crash]\n{}", result.stderr));
                    active_session.current_state = AgentState::Failed;
                    active_session.last_error    = result.stderr.clone();
                    let _ = save_session(&active_session);
                    current_prompt = orchestrator::build_repair_prompt(
                        &original_task, &files_repr, &result.stderr, "Runtime Crash Error");

                } else {
                    log_event(LogLevel::Info, "engine",
                        &format!("[Execution Pass]\n{}", result.stdout));
                    log_event(LogLevel::Info, "engine",
                        "Transitioning State: [AgentState::Complete]. Processing loop ended successfully.");
                    clear_session();
                    break;
                }
            }
            Err(sandbox_err) => {
                log_event(LogLevel::Error, "sandbox",
                    &format!("Container execution failure: {}", sandbox_err));
                active_session.current_state = AgentState::Failed;
                active_session.last_error    = sandbox_err.clone();
                let _ = save_session(&active_session);
                break;
            }
        }
    }

    Ok(())
}


































// use std::fs;
// use std::io::{self, Write};
// use std::path::PathBuf;
// use reqwest::Client;

// // All modules definitions mapped clearly
// mod config;
// mod security;
// mod network;
// mod compiler;
// mod orchestrator;

// use config::{HTTP_TIMEOUT, HTTP_POOL_IDLE, TARGET_FILE};
// use security::{is_safe, sanitize_code};
// use network::{get_ollama_embedding, generate_with_retry};
// use compiler::{run_hardened_compile, run_hardened_execution, extract_clean_error, get_unique_job_dir};
// use orchestrator::{construct_initial_prompt};
// use miller_parser::{log_event, LogLevel};

// use miller_parser::scanner::scan_and_parse_project_incremental;
// use miller_memory::{MillerMemory, MemoryPayload};

// #[tokio::main]
// async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//     let client = Client::builder()
//         .timeout(HTTP_TIMEOUT)
//         .pool_idle_timeout(HTTP_POOL_IDLE)
//         .build()?;

//     let memory_layer = MillerMemory::new();
//     memory_layer.init_collection().await?;

//     log_event(LogLevel::Info, "system", "=== MILLER: Local Autonomous Coding Framework ===");
    
//     // Target Path Discovery Setup
//     let args: Vec<String> = std::env::args().collect();
//     let mut target_project_path: Option<PathBuf> = None;
    
//     if args.len() > 1 {
//         let path = PathBuf::from(&args[1]);
//         if path.exists() {
//             log_event(LogLevel::Info, "system", &format!("External project path loaded: {:?}", path));
//             target_project_path = Some(path);
//         } else {
//             log_event(LogLevel::Error, "system", &format!("Path exist nahi karta: {:?}", path));
//             return Ok(());
//         }
//     } else {
//         log_event(LogLevel::Warn, "system", "No external workspace attached. Background scanner is Idle.");
//     }

//     // Background Thread
//     if let Some(bg_path) = target_project_path {
//         let bg_client = client.clone();
//         tokio::task::spawn(async move {
//             log_event(LogLevel::Info, "background", &format!("Silent monitoring loop active: {:?}", bg_path));

//             let scan_result = tokio::task::spawn_blocking(move || {
//                 scan_and_parse_project_incremental(&bg_path)
//             })
//             .await;

//             let (changed_nodes, skipped_count) = match scan_result {
//                 Ok(result) => result,
//                 Err(e) => {
//                     log_event(LogLevel::Error, "background", &format!("Scanner thread crashed: {}", e));
//                     return;
//                 }
//             };
            
//             if skipped_count > 0 { 
//                 log_event(LogLevel::Info, "background", &format!("Fast skip operational: {} files unchanged.", skipped_count));
//             }

//             if changed_nodes.is_empty() { 
//                 log_event(LogLevel::Info, "background", "Cache clean. Workspace state sync complete."); 
//                 return; 
//             }

//             log_event(LogLevel::Info, "background", &format!("Syncing {} modified items into local Qdrant...", changed_nodes.len()));

//             let bg_memory = MillerMemory::new();
//             let mut pseudo_id = std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_secs();

//             for (idx, chunk) in changed_nodes.iter().enumerate() {
//                 if let Ok(vector) = get_ollama_embedding(&bg_client, &chunk.source_code).await {
//                     let payload = MemoryPayload {
//                         file_path: chunk.file_path.clone(),
//                         entity_name: chunk.entity_name.clone(),
//                         entity_type: chunk.entity_type.clone(),
//                         content: chunk.source_code.clone(),
//                     };
//                     if let Ok(_) = bg_memory.upsert_code_chunk(pseudo_id, vector, payload).await { pseudo_id += 1; }
//                 }
//                 if (idx + 1) % 30 == 0 { tokio::time::sleep(std::time::Duration::from_millis(400)).await; }
//             }
//             log_event(LogLevel::Info, "background", "Vector embedding database sync cycle completed!");
//         });
//     }
    
//     print!("\nMiller ko task batao:\n> ");
//     io::stdout().flush()?;

//     let mut original_task = String::new();
//     io::stdin().read_line(&mut original_task)?;
//     let original_task = original_task.trim();

//     log_event(LogLevel::Info, "retrieval", "Searching past code patterns from local Qdrant DB...");
//     let mut context_code_str = String::new();
    
//     if let Ok(query_vector) = get_ollama_embedding(&client, original_task).await {
//         if let Ok(matched_chunks) = memory_layer.search_similar_code(query_vector, 2).await {
//             if !matched_chunks.is_empty() {

//                 log_event(LogLevel::Info, "retrieval", "Valid structural matches found. Injecting blocks into prompt context...");

//                 context_code_str.push_str("\n--- RELEVANT EXISTING CONTEXT CODE ---\n");

//                 for chunk in matched_chunks {
//                     if chunk.file_path.contains("miller_core") || chunk.file_path.contains("miller_parser") { continue; }

//                     context_code_str.push_str(&format!("// From File: {}\n// Entity: {}\n{}\n\n", chunk.file_path, chunk.entity_name, chunk.content));
//                 }
//                 context_code_str.push_str("---------------------------------------\n");
//             }
//         }
//     }
    
//     let mut current_prompt = construct_initial_prompt(&context_code_str, original_task);
//     let mut attempts = 0;
//     const MAX_ATTEMPTS: usize = 5;

//     while attempts < MAX_ATTEMPTS {
//         attempts += 1;
//         log_event(LogLevel::Info, "engine", &format!("Generating code (Attempt {}/{})...", attempts, MAX_ATTEMPTS));
        
//         let ai_response = match generate_with_retry(&client, &current_prompt).await {
//             Ok(text) => text,
//             Err(e) => { 
//                 log_event(LogLevel::Error, "network", &format!("Ollama call permanently failed: {}", e)); 
//                 break; 
//             }
//         };

//         let code_to_write = match sanitize_code(&ai_response) {
//             Some(code) => code,
//             None => {
//                 log_event(LogLevel::Warn, "sanitizer", "Code standard structure block not captured. Re-prompting...");
//                 current_prompt = format!("Your previous response did not contain standard markdown code fences. Please regenerate and wrap properly.\nTask: {}", original_task);
//                 continue;
//             }
//         };

//         if !is_safe(&code_to_write) {
//             log_event(LogLevel::Error, "security", "Dangerous instruction sequence intercepted! Task terminated.");
//             break;
//         }

//         fs::write(TARGET_FILE, &code_to_write)?;
//         log_event(LogLevel::Info, "filesystem", &format!("Code successfully written to local cache: '{}'", TARGET_FILE));

//         // FIX: Explicit random non-deterministic dynamic unique job context alignment 
//         let active_job_dir = get_unique_job_dir();
//         log_event(LogLevel::Info, "compiler", &format!("Phase 1: Compiling code inside temporary isolated boundary: {}", active_job_dir));

//         match run_hardened_compile(&active_job_dir) {
//             Ok((true, _)) => {
//                 log_event(LogLevel::Info, "compiler", "Compilation successful! Phase 2: Launching isolated ephemeral sandbox...");
                
//                 match run_hardened_execution(&active_job_dir) {
//                     Ok(result) => {
//                         if result.timed_out {
//                             log_event(LogLevel::Error, "sandbox", &result.stderr);
//                             current_prompt = orchestrator::build_repair_prompt(original_task, &code_to_write, &result.stderr, "Runtime Timeout Error");
//                         } else if result.limit_exceeded {
//                             log_event(LogLevel::Error, "security", &result.stderr);
//                             current_prompt = orchestrator::build_repair_prompt(original_task, &code_to_write, &result.stderr, "Host Pipe Bomb Exploitation Intercept");
//                         } else if !result.stderr.is_empty() {
//                             println!("\n[Sandbox Runtime Crash]\n--- STDERR ---\n{}\n--------------", result.stderr);
//                             current_prompt = orchestrator::build_repair_prompt(original_task, &code_to_write, &result.stderr, "Runtime Crash Error");
//                         } else {
//                             println!("\n[Hardened Sandbox Execution Pass]\n--- STDOUT ---\n{}\n--------------", result.stdout);
//                             log_event(LogLevel::Info, "engine", "Processing Loop Cycle Ended Safely and Successfully.");
//                             break;
//                         }
//                     }
//                     Err(sandbox_err) => {
//                         log_event(LogLevel::Error, "sandbox", &format!("Container isolation failure: {}", sandbox_err));
//                         break;
//                     }
//                 }
//             }
//             Ok((false, compile_stderr)) => {
//                 let clean_error = extract_clean_error(&compile_stderr);
//                 log_event(LogLevel::Error, "compiler", "Logic failure details captured during isolated compilation phase.");
//                 current_prompt = orchestrator::build_repair_prompt(original_task, &code_to_write, &clean_error, "Compilation Error");
//             }
//             Err(e) => {
//                 log_event(LogLevel::Error, "compiler", &format!("Critical compile engine failure: {}", e));
//                 break;
//             }
//         }
//     }
//     Ok(())
// }

