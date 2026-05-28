use std::process::{Command, Stdio};
use std::fs;
use std::path::Path;
use std::time::{Instant, Duration};
use std::io::Read;

// ── Language Abstraction ──────────────────────────────────────────────────────

/// Supported sandbox execution languages.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum Language {
    Rust,
    Python,
}

/// Per-language runtime configuration.
///
/// Two execution modes:
///
/// 1. **Scratchpad mode** — single generated source file compiled with
///    `compile_args` and executed with `run_args`.
/// 2. **Workspace mode** — full project directory mounted at `/workspace`;
///    verified with `workspace_check_args`, run with `workspace_run_args`.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub image_name: &'static str,

    // ── Scratchpad-mode fields ────────────────────────────────────────────────
    /// `None` = interpreted language; no compile phase in scratchpad mode.
    /// Placeholders: `{INPUT}` → job_dir/main.<ext>, `{OUTPUT}` → job_dir/app
    pub compile_args: Option<Vec<&'static str>>,
    /// Placeholders: `{BINARY}` → job_dir/app, `{INPUT}` → job_dir/main.<ext>
    pub run_args: Vec<&'static str>,
    pub source_extension: &'static str,

    // ── Workspace-mode fields ─────────────────────────────────────────────────
    /// Placeholders: `{MANIFEST}` → container-side descriptor path,
    ///               `{WORKSPACE}` → `/workspace`
    pub workspace_check_args: Vec<&'static str>,
    /// Placeholder: `{MANIFEST}` / `{WORKSPACE}` as above.
    pub workspace_run_args: Vec<&'static str>,
}

impl RuntimeConfig {
    pub fn for_language(lang: &Language) -> Self {
        match lang {
            Language::Rust => RuntimeConfig {
                image_name: "miller-rust-runner",
                compile_args: Some(vec!["rustc", "{INPUT}", "-o", "{OUTPUT}"]),
                run_args: vec!["{BINARY}"],
                source_extension: "rs",
                workspace_check_args: vec![
                    "cargo", "check", "--manifest-path", "{MANIFEST}",
                ],
                workspace_run_args: vec![
                    "cargo", "run", "--manifest-path", "{MANIFEST}",
                ],
            },
            Language::Python => RuntimeConfig {
                image_name: "miller-python-runner",
                compile_args: None,
                run_args: vec!["python3", "{INPUT}"],
                source_extension: "py",
                workspace_check_args: vec![
                    "python3", "-m", "py_compile", "{INPUT}",
                ],
                workspace_run_args: vec![
                    "python3", "{WORKSPACE}/main.py",
                ],
            },
        }
    }

    // ── Scratchpad resolution ─────────────────────────────────────────────────

    pub fn resolved_compile_args(&self, job_dir: &str) -> Option<Vec<String>> {
        self.compile_args.as_ref().map(|args| {
            args.iter().map(|a| {
                a.replace("{INPUT}",  &format!("{}/main.{}", job_dir, self.source_extension))
                 .replace("{OUTPUT}", &format!("{}/app", job_dir))
            }).collect()
        })
    }

    pub fn resolved_run_args(&self, job_dir: &str) -> Vec<String> {
        self.run_args.iter().map(|a| {
            a.replace("{BINARY}", &format!("{}/app", job_dir))
             .replace("{INPUT}",  &format!("{}/main.{}", job_dir, self.source_extension))
        }).collect()
    }

    // ── Workspace resolution ──────────────────────────────────────────────────

    pub fn resolved_workspace_check_args(
        &self,
        manifest_path: &str,
        workspace_container_root: &str,
    ) -> Vec<String> {
        self.workspace_check_args.iter().map(|a| {
            a.replace("{MANIFEST}",  manifest_path)
             .replace("{INPUT}",     manifest_path)
             .replace("{WORKSPACE}", workspace_container_root)
        }).collect()
    }

    pub fn resolved_workspace_run_args(
        &self,
        manifest_path: &str,
        workspace_container_root: &str,
    ) -> Vec<String> {
        self.workspace_run_args.iter().map(|a| {
            a.replace("{MANIFEST}",  manifest_path)
             .replace("{WORKSPACE}", workspace_container_root)
        }).collect()
    }

    /// Container-side path to the project descriptor.
    /// Rust → `/workspace/Cargo.toml`; Python → `/workspace/main.py`
    pub fn container_manifest_path(&self, lang: &Language) -> &'static str {
        match lang {
            Language::Rust   => "/workspace/Cargo.toml",
            Language::Python => "/workspace/main.py",
        }
    }
}

// ── Telemetry ─────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct SandboxMetrics {
    pub execution_duration_ms: u128,
    pub stdout_size:           usize,
    pub stderr_size:           usize,
    pub exit_code:             Option<i32>,
}

// ── Execution Result ──────────────────────────────────────────────────────────

pub struct SandboxExecutionResult {
    pub stdout:        String,
    pub stderr:        String,
    pub timed_out:     bool,
    pub limit_exceeded: bool,
    pub metrics:       SandboxMetrics,
}

// ── Constants ─────────────────────────────────────────────────────────────────

const MAX_BUFFER_SIZE:     usize = 1024 * 1024; // 1 MB
const CONTAINER_WORKSPACE: &str  = "/workspace";

// ── Job Directory Helpers ─────────────────────────────────────────────────────

pub fn get_unique_job_dir() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("/tmp/miller_job_{}", now)
}

pub fn safe_cleanup_job_dir(job_dir: &str) {
    for _ in 0..3 {
        if fs::remove_dir_all(job_dir).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("[Janitor warning] Permanent lock on resource directory: {}", job_dir);
}

// ═══════════════════════════════════════════════════════════════════════════════
// WORKSPACE MODE
// ═══════════════════════════════════════════════════════════════════════════════

/// Workspace check — mounts `workspace_root` at `/workspace` and runs the
/// language-native static checker.
///
/// This variant uses the **default** container manifest path derived from
/// `RuntimeConfig::container_manifest_path` (e.g. `/workspace/Cargo.toml`).
/// Use `run_workspace_check_at` when the multi-file parser has already
/// confirmed a specific manifest location.
#[allow(dead_code)]
pub fn run_workspace_check(
    workspace_root: &str,
    config: &RuntimeConfig,
    lang: &Language,
) -> Result<(bool, String), std::io::Error> {
    let manifest = config.container_manifest_path(lang);
    run_workspace_check_at(workspace_root, config, manifest)
}

/// Workspace check with an **explicit** container-side manifest path.
///
/// Called by main.rs after the multi-file parser has written files and can
/// confirm that `Cargo.toml` exists at a known location (always
/// `/workspace/Cargo.toml` for a properly generated Rust project).
/// Separating this from the default variant keeps the caller in full control
/// of the manifest path without requiring a mutable `RuntimeConfig`.
pub fn run_workspace_check_at(
    workspace_root: &str,
    config: &RuntimeConfig,
    manifest_path: &str,
) -> Result<(bool, String), std::io::Error> {
    let check_argv = config.resolved_workspace_check_args(manifest_path, CONTAINER_WORKSPACE);

    let output = Command::new("docker")
        .args(&[
            "run", "--rm",
            "--network",      "none",
            "--memory",       "512m",
            "--cpus",         "0.5",
            "--pids-limit",   "100",
            "--cap-drop",     "ALL",
            "--security-opt", "no-new-privileges",
            "-v", &format!("{}:{}:rw", workspace_root, CONTAINER_WORKSPACE),
            config.image_name,
        ])
        .args(&check_argv)
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((output.status.success(), stderr))
}

/// Workspace execution — runs the project from inside the container.
/// `read-only` is intentionally absent: `cargo run` writes to `target/`.
pub fn run_workspace_execution(
    workspace_root: &str,
    config: &RuntimeConfig,
    lang: &Language,
) -> Result<SandboxExecutionResult, String> {
    let manifest = config.container_manifest_path(lang);
    let run_argv = config.resolved_workspace_run_args(manifest, CONTAINER_WORKSPACE);
    let start    = Instant::now();

    let child = match Command::new("docker")
        .args(&[
            "run", "--rm",
            "--init",
            "--network",      "none",
            "--memory",       "256m",
            "--cpus",         "0.5",
            "--pids-limit",   "64",
            "--cap-drop",     "ALL",
            "--security-opt", "no-new-privileges",
            "--tmpfs",        "/tmp:rw,noexec,nosuid,size=64m",
            "-v", &format!("{}:{}:rw", workspace_root, CONTAINER_WORKSPACE),
            config.image_name,
        ])
        .args(&run_argv)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c)  => c,
        Err(e) => return Err(format!("Failed to spawn workspace container: {}", e)),
    };

    stream_child_output(child, start, None)
}

// ═══════════════════════════════════════════════════════════════════════════════
// SCRATCHPAD MODE
// ═══════════════════════════════════════════════════════════════════════════════

/// Scratchpad compile — copies `source_file_on_host` into `job_dir` and
/// compiles it. Interpreted languages skip the compile step.
pub fn run_hardened_compile(
    job_dir: &str,
    config: &RuntimeConfig,
    source_file_on_host: &str,
) -> Result<(bool, String), std::io::Error> {
    if !Path::new(job_dir).exists() {
        fs::create_dir_all(job_dir)?;
    }

    let dest = format!("{}/main.{}", job_dir, config.source_extension);
    if Path::new(source_file_on_host).exists() {
        fs::copy(source_file_on_host, &dest)?;
    } else {
        return Ok((false, format!("{} missing on host cache", source_file_on_host)));
    }

    let compile_argv = match config.resolved_compile_args(job_dir) {
        Some(args) => args,
        None       => return Ok((true, String::new())),
    };

    let output = Command::new("docker")
        .args(&[
            "run", "--rm",
            "--network",      "none",
            "--memory",       "512m",
            "--cpus",         "0.5",
            "--pids-limit",   "100",
            "--cap-drop",     "ALL",
            "--security-opt", "no-new-privileges",
            "-v", &format!("{}:{}:rw", job_dir, CONTAINER_WORKSPACE),
            config.image_name,
        ])
        .args(&compile_argv)
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((output.status.success(), stderr))
}

/// Scratchpad execution — runs the compiled binary in a read-only ephemeral
/// jail. Cleans up `job_dir` on exit.
pub fn run_hardened_execution(
    job_dir: &str,
    config: &RuntimeConfig,
) -> Result<SandboxExecutionResult, String> {
    let run_argv = config.resolved_run_args(job_dir);
    let start    = Instant::now();

    let child = match Command::new("docker")
        .args(&[
            "run", "--rm",
            "--init",
            "--network",      "none",
            "--memory",       "256m",
            "--cpus",         "0.5",
            "--pids-limit",   "64",
            "--cap-drop",     "ALL",
            "--security-opt", "no-new-privileges",
            "--read-only",
            "--tmpfs",        "/tmp:rw,noexec,nosuid,size=64m",
            "-v", &format!("{}:{}:rw", job_dir, CONTAINER_WORKSPACE),
            config.image_name,
        ])
        .args(&run_argv)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c)  => c,
        Err(e) => {
            safe_cleanup_job_dir(job_dir);
            return Err(format!("Failed to spawn sandbox container: {}", e));
        }
    };

    stream_child_output(child, start, Some(job_dir))
}

// ── Shared streaming watchdog ─────────────────────────────────────────────────

/// Poll a child process with bounded buffers and a hard timeout.
/// `cleanup_dir` — `Some(path)` removes the directory after exit (scratchpad).
fn stream_child_output(
    mut child: std::process::Child,
    start_time: Instant,
    cleanup_dir: Option<&str>,
) -> Result<SandboxExecutionResult, String> {
    let mut stdout_pipe = child.stdout.take()
        .ok_or("Failed to capture stdout pipe")?;
    let mut stderr_pipe = child.stderr.take()
        .ok_or("Failed to capture stderr pipe")?;

    let timeout_duration = Duration::from_secs(120); // generous for `cargo run`

    let mut stdout_buf     = Vec::new();
    let mut stderr_buf     = Vec::new();
    let mut chunk          = [0u8; 4096];
    let mut limit_exceeded = false;
    let mut timed_out      = false;
    let mut exit_code: Option<i32> = None;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code();
                let _ = stdout_pipe.read_to_end(&mut stdout_buf);
                let _ = stderr_pipe.read_to_end(&mut stderr_buf);
                break;
            }
            Ok(None) => {
                if start_time.elapsed() >= timeout_duration {
                    let _ = child.kill();
                    timed_out = true;
                    break;
                }

                if let Ok(n) = stdout_pipe.read(&mut chunk) {
                    if n > 0 {
                        stdout_buf.extend_from_slice(&chunk[..n]);
                        if stdout_buf.len() >= MAX_BUFFER_SIZE {
                            let _ = child.kill();
                            limit_exceeded = true;
                            break;
                        }
                    }
                }

                if let Ok(n) = stderr_pipe.read(&mut chunk) {
                    if n > 0 {
                        stderr_buf.extend_from_slice(&chunk[..n]);
                        if stderr_buf.len() >= MAX_BUFFER_SIZE {
                            let _ = child.kill();
                            limit_exceeded = true;
                            break;
                        }
                    }
                }

                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                if let Some(dir) = cleanup_dir {
                    safe_cleanup_job_dir(dir);
                }
                return Err(format!("Sandbox monitoring error: {}", e));
            }
        }
    }

    let execution_duration_ms = start_time.elapsed().as_millis();

    if let Some(dir) = cleanup_dir {
        safe_cleanup_job_dir(dir);
    }

    let stdout_size = stdout_buf.len();
    let stderr_size  = stderr_buf.len();

    let clean_stdout = String::from_utf8_lossy(&stdout_buf).to_string();
    let mut clean_stderr = String::from_utf8_lossy(&stderr_buf).to_string();

    if limit_exceeded {
        clean_stderr.push_str(
            "\n[SECURITY ALERT] HOST PIPE BOMB BLOCKED: \
             Stdout/Stderr threshold of 1 MB breached.",
        );
    }

    Ok(SandboxExecutionResult {
        stdout: clean_stdout,
        stderr: clean_stderr,
        timed_out,
        limit_exceeded,
        metrics: SandboxMetrics {
            execution_duration_ms,
            stdout_size,
            stderr_size,
            exit_code,
        },
    })
}

// ── Error Extraction ──────────────────────────────────────────────────────────

/// Extract the most actionable lines from a raw compiler/checker stderr stream.
pub fn extract_clean_error(raw_stderr: &str) -> String {
    raw_stderr
        .lines()
        .filter(|line| {
            line.contains("error")
                || line.contains("warning")
                || line.contains("-->")
                || line.contains('|')
                || line.contains("help:")
                || line.contains("note:")
        })
        .take(40)
        .collect::<Vec<_>>()
        .join("\n")
}


















// use std::process::{Command, Stdio};
// use std::fs;
// use std::path::Path;
// use std::time::{Instant, Duration};
// use std::io::Read;

// // Industry standard execution metrics layout
// pub struct SandboxExecutionResult {
//     pub stdout: String,
//     pub stderr: String,
//     pub timed_out: bool,
//     pub limit_exceeded: bool,
// }

// const IMAGE_NAME: &str = "miller-rust-runner";
// const MAX_BUFFER_SIZE: usize = 1024 * 1024;   // 1MB Strict Host-Pipe Ceiling Limit

// // Helper: Har execution ke liye ek unique dynamic job directory path generate karna
// pub fn get_unique_job_dir() -> String {
//     let now = std::time::SystemTime::now()
//         .duration_since(std::time::SystemTime::UNIX_EPOCH)
//         .unwrap_or_default()
//         .as_nanos();
//     format!("/tmp/miller_job_{}", now)
// }

// pub fn safe_cleanup_job_dir(job_dir: &str) {
//     for _ in 0..3 {
//         if fs::remove_dir_all(job_dir).is_ok(){
//             return;
//         }
//         std::thread::sleep(Duration::from_millis(100));  // Asynchronous container unmount cooling window
//     }
//     eprintln!("[Janitor warning] Permanent look on resource directory: {}", job_dir);
// }

// /// Phase 1: Compile the generated code inside a secure immutable container boundary
// pub fn run_hardened_compile(job_dir: &str) -> Result<(bool, String), std::io::Error> {
//     // Ensure temporary job folder alignment
//     if !Path::new(job_dir).exists() {
//         fs::create_dir_all(job_dir)?;
//     }
    
//     // Copy generated code file into the shared transit container path
//     if Path::new("sandbox.rs").exists() {
//         fs::copy("sandbox.rs", format!("{}/main.rs", job_dir))?;
//     } else {
//         return Ok((false, "sandbox.rs file missing on host cache".to_string()));
//     }

//     // 🛡️ Hardened Compile Phase: Added cap-drop, non-root enforcement, and explicit workspace execution
//     let output = Command::new("docker")
//         .args(&[
//             "run", "--rm",
//             "--network", "none",
//             "--memory", "512m",
//             "--cpus", "0.5",
//             "--pids-limit", "100",
//             "--cap-drop", "ALL",
//             "--security-opt", "no-new-privileges",
//             "-v", &format!("{}:/workspace:rw", job_dir),
//             IMAGE_NAME,
//             "rustc", "/workspace/main.rs", "-o", "/workspace/app",
//         ])
//         .output()?;

//     let stderr_logs = String::from_utf8_lossy(&output.stderr).to_string();
//     Ok((output.status.success(), stderr_logs))
// }

// /// Phase 2: Execute the compiled binary inside a strictly sandboxed, read-only ephemeral jail
// pub fn run_hardened_execution(job_dir: &str) -> Result<SandboxExecutionResult, String> {
//     // Target command setup with strict security primitives (no-new-privileges, capability drop, fork-bomb limits)
//     let mut child = match Command::new("docker")
//         .args(&[
//             "run", "--rm",
//             "--init",
//             "--network", "none",
//             "--memory", "256m",
//             "--cpus", "0.5",
//             "--pids-limit", "64",
//             "--cap-drop", "ALL",
//             "--security-opt", "no-new-privileges",
//             "--read-only",
//             "--tmpfs", "/tmp:rw,noexec,nosuid,size=64m",
//             "-v", &format!("{}:/workspace:rw", job_dir),
//             IMAGE_NAME,
//             "/workspace/app",
//         ])
//         .stdout(Stdio::piped())
//         .stderr(Stdio::piped())
//         .spawn() 
//     {
//         Ok(c) => c,
//         Err(e) => {
//             safe_cleanup_job_dir(job_dir);
//             return Err(format!("Failed to spawn sandbox container execution: {}", e));
//         }
//     };

//     // Extract raw pipe handles out of child supervisor scope safely
//     let mut stdout_pipe = child.stdout.take().ok_or("Failed to capture stdout stream pipe handle")?;
//     let mut stderr_pipe = child.stderr.take().ok_or("Failed to capture stderr stream pipe handle")?;

//     let timeout_duration = Duration::from_secs(3);
//     let start_time = Instant::now();

//     let mut stdout_buffer = Vec::new();
//     let mut stderr_buffer = Vec::new();
    
//     let mut chunk_reader = [0u8; 4096]; // 4KB chunk parsing cycles
//     let mut limit_exceeded = false;
//     let mut timed_out = false;

//     // Set read streams to non-blocking or chunk poll loop manually for precise telemetry capping
//     loop {
//         // Check structural process exit code state
//         match child.try_wait() {
//             Ok(Some(_)) => {
//                 // Read remaining byte trails inside pipes safely
//                 let _ = stdout_pipe.read_to_end(&mut stdout_buffer);
//                 let _ = stderr_pipe.read_to_end(&mut stderr_buffer);
//                 break;
//             }
//             Ok(None) => {
//                 // Hard ceiling runtime watchdog trigger
//                 if start_time.elapsed() >= timeout_duration {
//                     let _ = child.kill();
//                     timed_out = true;
//                     break;
//                 }

//                 // OUTPUT_GUARD STREAM CONSUMER MATRIX (Host Memory Shield)
//                 if let Ok(bytes_read) = stdout_pipe.read(&mut chunk_reader) {
//                     if bytes_read > 0 {
//                         stdout_buffer.extend_from_slice(&chunk_reader[..bytes_read]);
//                         if stdout_buffer.len() >= MAX_BUFFER_SIZE {
//                             let _ = child.kill();
//                             limit_exceeded = true;
//                             break;
//                         }
//                     }
//                 }

//                 if let Ok(bytes_read) = stderr_pipe.read(&mut chunk_reader) {
//                     if bytes_read > 0 {
//                         stderr_buffer.extend_from_slice(&chunk_reader[..bytes_read]);
//                         if stderr_buffer.len() >= MAX_BUFFER_SIZE {
//                             let _ = child.kill();
//                             limit_exceeded = true;
//                             break;
//                         }
//                     }
//                 }

//                 std::thread::sleep(Duration::from_millis(20)); // High precision low-latency processing tick
//             }
//             Err(e) => {
//                 safe_cleanup_job_dir(job_dir);
//                 return Err(format!("Sandbox monitoring error: {}", e));
//             }
//         }
//     }

//     // Flush cleanup worker
//     safe_cleanup_job_dir(job_dir);

//     let clean_stdout = String::from_utf8_lossy(&stdout_buffer).to_string();
//     let mut clean_stderr = String::from_utf8_lossy(&stderr_buffer).to_string();

//     if limit_exceeded {
//         clean_stderr.push_str("\n[SECURITY ALERT] HOST PIPE BOMB BLOCKED: Execution terminated. Stdout/Stderr threshold limit of 1MB breached.");
//     }

//     Ok(SandboxExecutionResult {
//         stdout: clean_stdout,
//         stderr: clean_stderr,
//         timed_out,
//         limit_exceeded,
//     })
// }

// /// Optimisation Fix: Extractor grabs existing stderr instead of recompiling everything
// pub fn extract_clean_error(raw_stderr: &str) -> String {
//     raw_stderr.lines()
//         .filter(|line| line.contains("error") || line.contains("-->") || line.contains("|") || line.contains("help:"))
//         .take(25)
//         .collect::<Vec<_>>()
//         .join("\n")
// }
