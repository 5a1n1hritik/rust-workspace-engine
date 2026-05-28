use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use reqwest::Client;

mod config;
mod security;
mod network;
mod compiler;
mod orchestrator;
mod session;

use config::{HTTP_TIMEOUT, HTTP_POOL_IDLE, TARGET_FILE};
use security::{is_safe, sanitize_code};
use network::{get_ollama_embedding, generate_with_retry};
use compiler::{
    run_hardened_compile, run_hardened_execution, extract_clean_error,
    get_unique_job_dir, Language, RuntimeConfig,
};
use orchestrator::construct_initial_prompt;
// use miller_parser::{log_event, LogLevel};
use miller_memory::{log_event, LogLevel};
use session::{AgentState, SessionState, save_session, load_last_session, clear_session}; 

use miller_parser::scanner::scan_and_parse_project_incremental;
use miller_memory::{MillerMemory, MemoryPayload};

// ── Retrieval Quality Guard ───────────────────────────────────────────────────
/// Minimum cosine-similarity score for a vector match to be injected into the prompt.
/// Matches below this threshold are discarded to prevent low-quality context pollution.
const MIN_RETRIEVAL_SCORE: f32 = 0.35;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::builder()
        .timeout(HTTP_TIMEOUT)
        .pool_idle_timeout(HTTP_POOL_IDLE)
        .build()?;

    let memory_layer = MillerMemory::new();
    memory_layer.init_collection().await?;

    log_event(LogLevel::Info, "system", "=== MILLER: Local Autonomous Coding Framework ===");

    // ── Target path discovery ─────────────────────────────────────────────────
    let args: Vec<String> = std::env::args().collect();
    let mut target_project_path: Option<PathBuf> = None;

    if args.len() > 1 {
        let path = PathBuf::from(&args[1]);
        if path.exists() {
            log_event(LogLevel::Info, "system", &format!("External project path loaded: {:?}", path));
            target_project_path = Some(path);
        } else {
            log_event(LogLevel::Error, "system", &format!("Path does not exist: {:?}", path));
            return Ok(());
        }
    } else {
        log_event(LogLevel::Warn, "system", "No external workspace attached. Background scanner is idle.");
    }

    // ── Background indexing thread ────────────────────────────────────────────
    if let Some(bg_path) = target_project_path {
        let bg_client = client.clone();
        tokio::task::spawn(async move {
            log_event(
                LogLevel::Info, "background",
                &format!("Silent monitoring loop active: {:?}", bg_path),
            );

            let scan_result = tokio::task::spawn_blocking(move || {
                scan_and_parse_project_incremental(&bg_path)
            })
            .await;

            let (changed_nodes, skipped_count) = match scan_result {
                Ok(result) => result,
                Err(e) => {
                    log_event(LogLevel::Error, "background", &format!("Scanner thread crashed: {}", e));
                    return;
                }
            };

            if skipped_count > 0 {
                log_event(
                    LogLevel::Info, "background",
                    &format!("Fast skip operational: {} files unchanged.", skipped_count),
                );
            }

            if changed_nodes.is_empty() {
                log_event(LogLevel::Info, "background", "Cache clean. Workspace state sync complete.");
                return;
            }

            log_event(
                LogLevel::Info, "background",
                &format!("Syncing {} modified items into local Qdrant...", changed_nodes.len()),
            );

            let bg_memory = MillerMemory::new();
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
            log_event(LogLevel::Info, "background", "Vector embedding database sync cycle completed.");
        });
    }

    // ── CRASH RECOVERY INTERCEPTOR LAYER ──────────────────────────────────
    let mut original_task = String::new();
    let mut current_prompt = String::new();
    let mut attempts = 0;
    let mut resumed_session = false;

    if let Some(last_session) = load_last_session() {
        log_event(LogLevel::Warn, "recovery", "Detected a previously interrupted or crashed session checkpoint!");
        log_event(LogLevel::Info, "recovery", &format!("Interrupted Task: \"{}\"", last_session.task));
        log_event(LogLevel::Info, "recovery", &format!("Last Known State: {:?}", last_session.current_state));
        print!("Do you want to resume this session? (y/N): ");
        io::stdout().flush()?;

        let mut user_choice = String::new();
        io::stdin().read_line(&mut user_choice)?;
        
        if user_choice.trim().to_lowercase() == "y" {
            log_event(LogLevel::Info, "recovery", "Re-loading active session metrics state machine...");
            original_task = last_session.task;
            attempts = last_session.attempt;
            resumed_session = true;

            // State recovering checkpoint routing
            if last_session.current_state == AgentState::Failed && !last_session.last_error.is_empty() {
                current_prompt = orchestrator::build_repair_prompt(
                    &original_task, &last_session.generated_code, &last_session.last_error, "Recovered Crash Error"
                );
            } else {
                current_prompt = last_session.generated_code; // Pipeline fallthrough retry
            }
        } else {
            log_event(LogLevel::Info, "recovery", "Discarding stale session tokens.");
            clear_session();
        }
    }

    let mut context_code_str = String::new();
    // If not recovering, proceed with a standard interactive prompt intake session
    if original_task.is_empty() {
        // ── Task intake ───────────────────────────────────────────────────────────
        print!("\nMiller ko task batao:\n> ");
        io::stdout().flush()?;

        // let mut original_task = String::new();
        io::stdin().read_line(&mut original_task)?;
        let original_task = original_task.trim();

        // ── Semantic retrieval with quality guard ─────────────────────────────────
        log_event(LogLevel::Info, "retrieval", "Searching past code patterns from local Qdrant DB...");

        if let Ok(query_vector) = get_ollama_embedding(&client, original_task).await {
            if let Ok(matched_chunks) = memory_layer.search_similar_code(query_vector, 2).await {
                // ── Quality Guard: discard matches below the similarity threshold ──
                let qualified: Vec<_> = matched_chunks
                    .into_iter()
                    .filter(|chunk| chunk.score >= MIN_RETRIEVAL_SCORE)
                    .collect();

                if qualified.is_empty() {
                    log_event(
                        LogLevel::Info, "retrieval",
                        "No matches above similarity threshold. Proceeding without retrieved context.",
                    );
                } else {
                    log_event(
                        LogLevel::Info, "retrieval",
                        &format!(
                            "{} qualified match(es) found (score >= {:.2}). Injecting into prompt context.",
                            qualified.len(),
                            MIN_RETRIEVAL_SCORE
                        ),
                    );

                    context_code_str.push_str("\n--- RELEVANT EXISTING CONTEXT CODE ---\n");
                    for chunk in &qualified {
                        // Skip internal miller framework files to prevent self-referential noise.
                        if chunk.payload.file_path.contains("miller_core")
                            || chunk.payload.file_path.contains("miller_parser")
                        {
                            continue;
                        }
                        context_code_str.push_str(&format!(
                            "// From File: {}\n// Entity: {}\n{}\n\n",
                            chunk.payload.file_path, 
                            chunk.payload.entity_name, 
                            chunk.payload.content
                        ));
                    }
                    context_code_str.push_str("---------------------------------------\n");
                }
            }
        }

        let active_language = Language::Rust;
        current_prompt = construct_initial_prompt(&context_code_str, &original_task, &active_language);
    }

    // ── Language selection (Rust default; extend here for multi-language) ─────
    let active_language = Language::Rust;
    let runtime_config  = RuntimeConfig::for_language(&active_language);

    if current_prompt.is_empty() {
        current_prompt = construct_initial_prompt(&context_code_str, &original_task, &active_language);
    }
    
    const MAX_ATTEMPTS: usize = 5;

    // ── Agent repair loop ─────────────────────────────────────────────────────
    while attempts < MAX_ATTEMPTS {
        if !resumed_session {
            attempts += 1;
        }
        resumed_session = false; // Reset toggle instantly to allow linear steps next iteration

        log_event(LogLevel::Info, "engine", &format!("Transitioning State: [AgentState::Generating] (Attempt {}/{})...", attempts, MAX_ATTEMPTS));

        let mut active_session = SessionState {
            task: original_task.clone(),
            attempt: attempts,
            current_state: AgentState::Generating,
            generated_code: current_prompt.clone(),
            last_error: String::new(),
        };
        let _ = save_session(&active_session);

        let ai_response = match generate_with_retry(&client, &current_prompt).await {
            Ok(text) => text,
            Err(e) => {
                log_event(LogLevel::Error, "network", &format!("Ollama call permanently failed: {}", e));
                active_session.current_state = AgentState::Failed;
                active_session.last_error = e.to_string();
                let _ = save_session(&active_session);
                break;
            }
        };

        log_event(LogLevel::Info, "engine", "Transitioning State: [AgentState::Sanitizing]");
        active_session.current_state = AgentState::Sanitizing;
        let _ = save_session(&active_session);

        let code_to_write = match sanitize_code(&ai_response) {
            Some(code) => code,
            None => {
                log_event(
                    LogLevel::Warn, "sanitizer",
                    "Code block not captured in response. Re-prompting...",
                );
                current_prompt = format!("Your previous response did not contain standard markdown code fences. Please regenerate and wrap the code properly.\nTask: {}", original_task);
                continue;
            }
        };

        log_event(LogLevel::Info, "engine", "Transitioning State: [AgentState::SecurityScanning]");
        if !is_safe(&code_to_write) {
            log_event(LogLevel::Error, "security", "Dangerous instruction sequence intercepted. Task terminated.");
            active_session.current_state = AgentState::Failed;
            active_session.last_error = "Security Scan Intercepted Rule Infraction".to_string();
            let _ = save_session(&active_session);
            break;
        }

        fs::write(TARGET_FILE, &code_to_write)?;
        log_event(
            LogLevel::Info, "filesystem",
            &format!("Code written to host cache: '{}'", TARGET_FILE),
        );

        let active_job_dir = get_unique_job_dir();
        log_event(LogLevel::Info, "engine", &format!("Transitioning State: [AgentState::Compiling] inside directory: {}", active_job_dir));
        active_session.current_state = AgentState::Compiling;
        active_session.generated_code = code_to_write.clone();
        let _ = save_session(&active_session);

        match run_hardened_compile(&active_job_dir, &runtime_config, TARGET_FILE) {
            Ok((true, _)) => {
                log_event(LogLevel::Info, "engine", "Transitioning State: [AgentState::Executing] inside ephemeral container boundary...");
                active_session.current_state = AgentState::Executing;
                let _ = save_session(&active_session);

                match run_hardened_execution(&active_job_dir, &runtime_config) {
                    Ok(result) => {
                        log_event(LogLevel::Info, "telemetry", &format!(
                            "duration={}ms stdout_bytes={} stderr_bytes={} exit_code={:?}",
                            result.metrics.execution_duration_ms, result.metrics.stdout_size, result.metrics.stderr_size, result.metrics.exit_code,
                        ));

                        if result.timed_out {
                            log_event(LogLevel::Error, "sandbox", &result.stderr);
                            active_session.current_state = AgentState::Failed;
                            active_session.last_error = result.stderr.clone();
                            let _ = save_session(&active_session);

                            current_prompt = orchestrator::build_repair_prompt(&original_task, &code_to_write, &result.stderr, "Runtime Timeout Error");
                        } else if result.limit_exceeded {
                            log_event(LogLevel::Error, "security", &result.stderr);
                            active_session.current_state = AgentState::Failed;
                            active_session.last_error = result.stderr.clone();
                            let _ = save_session(&active_session);

                            current_prompt = orchestrator::build_repair_prompt(&original_task, &code_to_write, &result.stderr, "Host Pipe Bomb Exploitation Intercept");
                        } else if !result.stderr.is_empty() {
                            log_event(LogLevel::Error, "sandbox", &format!("[Runtime Crash]\n{}", result.stderr));
                            active_session.current_state = AgentState::Failed;
                            active_session.last_error = result.stderr.clone();
                            let _ = save_session(&active_session);

                            current_prompt = orchestrator::build_repair_prompt(&original_task, &code_to_write, &result.stderr, "Runtime Crash Error");
                        } else {
                            log_event(LogLevel::Info, "engine", &format!("[Sandbox Execution Pass]\n{}", result.stdout));
                            log_event(LogLevel::Info, "engine", "Transitioning State: [AgentState::Complete]. Processing loop ended successfully.");
                            clear_session(); // Wipe persistence cache on absolute success state
                            break;
                        }
                    }
                    Err(sandbox_err) => {
                        log_event(LogLevel::Error, "sandbox", &format!("Container isolation failure: {}", sandbox_err));
                        break;
                    }
                }
            }
            Ok((false, compile_stderr)) => {
                let clean_error = extract_clean_error(&compile_stderr);
                log_event(LogLevel::Error, "compiler", "Compilation failed. Error details captured.");
                active_session.current_state = AgentState::Failed;
                active_session.last_error = clean_error.clone();
                let _ = save_session(&active_session);

                current_prompt = orchestrator::build_repair_prompt(&original_task, &code_to_write, &clean_error, "Compilation Error");
            }
            Err(e) => {
                log_event(LogLevel::Error, "compiler", &format!("Critical compile engine failure: {}", e));
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

