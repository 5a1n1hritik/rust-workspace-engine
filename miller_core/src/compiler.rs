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

/// Per-language runtime configuration. All Docker invocation details live here;
/// nothing in the execution flow is allowed to hardcode language-specific strings.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Docker image to use for both compile and run phases.
    pub image_name: &'static str,
    /// argv slice for the compile step (empty = interpreted, no compile phase).
    /// Placeholders: `{INPUT}` → `/workspace/main.<ext>`, `{OUTPUT}` → `/workspace/app`
    pub compile_args: Option<Vec<&'static str>>,
    /// argv slice for the run step.
    /// Placeholder: `{BINARY}` → `/workspace/app` (compiled) or source file (interpreted).
    pub run_args: Vec<&'static str>,
    /// Source file extension written into the job directory.
    pub source_extension: &'static str,
}

impl RuntimeConfig {
    pub fn for_language(lang: &Language) -> Self {
        match lang {
            Language::Rust => RuntimeConfig {
                image_name: "miller-rust-runner",
                compile_args: Some(vec!["rustc", "{INPUT}", "-o", "{OUTPUT}"]),
                run_args: vec!["{BINARY}"],
                source_extension: "rs",
            },
            Language::Python => RuntimeConfig {
                image_name: "miller-python-runner",
                compile_args: None, // interpreted — no compile phase
                run_args: vec!["python3", "{INPUT}"],
                source_extension: "py",
            },
        }
    }

    /// Resolve compile argv with concrete job paths.
    pub fn resolved_compile_args(&self, job_dir: &str) -> Option<Vec<String>> {
        self.compile_args.as_ref().map(|args| {
            args.iter().map(|a| {
                a.replace("{INPUT}",  &format!("{}/main.{}", job_dir, self.source_extension))
                 .replace("{OUTPUT}", &format!("{}/app", job_dir))
            }).collect()
        })
    }

    /// Resolve run argv with concrete job paths.
    pub fn resolved_run_args(&self, job_dir: &str) -> Vec<String> {
        self.run_args.iter().map(|a| {
            a.replace("{BINARY}", &format!("{}/app", job_dir))
             .replace("{INPUT}",  &format!("{}/main.{}", job_dir, self.source_extension))
        }).collect()
    }
}

// ── Telemetry ─────────────────────────────────────────────────────────────────

/// Structured metrics captured for every sandbox execution cycle.
#[derive(Debug, Default)]
pub struct SandboxMetrics {
    /// Wall-clock time from container spawn to process exit (or kill), in milliseconds.
    pub execution_duration_ms: u128,
    /// Bytes captured on stdout (capped at MAX_BUFFER_SIZE).
    pub stdout_size: usize,
    /// Bytes captured on stderr (capped at MAX_BUFFER_SIZE).
    pub stderr_size: usize,
    /// Process exit code; `None` if the process was killed (timeout / limit breach).
    pub exit_code: Option<i32>,
}

// ── Execution Result ──────────────────────────────────────────────────────────

/// Industry-standard execution metrics layout — now includes structured telemetry.
pub struct SandboxExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub limit_exceeded: bool,
    pub metrics: SandboxMetrics,
}

// ── Constants ─────────────────────────────────────────────────────────────────

const MAX_BUFFER_SIZE: usize = 1024 * 1024; // 1 MB strict host-pipe ceiling

// ── Job Directory Helpers ─────────────────────────────────────────────────────

/// Generate a unique ephemeral job directory path per execution.
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
        // Asynchronous container unmount cooling window
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("[Janitor warning] Permanent lock on resource directory: {}", job_dir);
}

// ── Phase 1: Compile ──────────────────────────────────────────────────────────

/// Compile the generated source inside a secure immutable container boundary.
/// For interpreted languages (Python), this phase is skipped and returns `Ok((true, ""))`.
pub fn run_hardened_compile(
    job_dir: &str,
    config: &RuntimeConfig,
    source_file_on_host: &str,
) -> Result<(bool, String), std::io::Error> {
    if !Path::new(job_dir).exists() {
        fs::create_dir_all(job_dir)?;
    }

    // Copy source into the shared transit container path.
    let dest = format!("{}/main.{}", job_dir, config.source_extension);
    if Path::new(source_file_on_host).exists() {
        fs::copy(source_file_on_host, &dest)?;
    } else {
        return Ok((false, format!("{} missing on host cache", source_file_on_host)));
    }

    // Interpreted languages have no compile phase.
    let compile_argv = match config.resolved_compile_args(job_dir) {
        Some(args) => args,
        None => return Ok((true, String::new())),
    };

    // Hardened compile: cap-drop, non-root enforcement, explicit workspace execution.
    let output = Command::new("docker")
        .args(&[
            "run", "--rm",
            "--network",      "none",
            "--memory",       "512m",
            "--cpus",         "0.5",
            "--pids-limit",   "100",
            "--cap-drop",     "ALL",
            "--security-opt", "no-new-privileges",
            "-v", &format!("{}:/workspace:rw", job_dir),
            config.image_name,
        ])
        .args(&compile_argv)
        .output()?;

    let stderr_logs = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((output.status.success(), stderr_logs))
}

// ── Phase 2: Execute ──────────────────────────────────────────────────────────

/// Execute the compiled (or interpreted) program inside a strictly sandboxed ephemeral jail.
pub fn run_hardened_execution(
    job_dir: &str,
    config: &RuntimeConfig,
) -> Result<SandboxExecutionResult, String> {
    let run_argv = config.resolved_run_args(job_dir);

    let start_time = Instant::now();

    let mut child = match Command::new("docker")
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
            "-v", &format!("{}:/workspace:rw", job_dir),
            config.image_name,
        ])
        .args(&run_argv)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            safe_cleanup_job_dir(job_dir);
            return Err(format!("Failed to spawn sandbox container: {}", e));
        }
    };

    let mut stdout_pipe = child.stdout.take()
        .ok_or("Failed to capture stdout pipe handle")?;
    let mut stderr_pipe = child.stderr.take()
        .ok_or("Failed to capture stderr pipe handle")?;

    let timeout_duration = Duration::from_secs(3);

    let mut stdout_buffer = Vec::new();
    let mut stderr_buffer  = Vec::new();
    let mut chunk           = [0u8; 4096];
    let mut limit_exceeded  = false;
    let mut timed_out       = false;
    let mut exit_code: Option<i32> = None;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code();
                let _ = stdout_pipe.read_to_end(&mut stdout_buffer);
                let _ = stderr_pipe.read_to_end(&mut stderr_buffer);
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
                        stdout_buffer.extend_from_slice(&chunk[..n]);
                        if stdout_buffer.len() >= MAX_BUFFER_SIZE {
                            let _ = child.kill();
                            limit_exceeded = true;
                            break;
                        }
                    }
                }

                if let Ok(n) = stderr_pipe.read(&mut chunk) {
                    if n > 0 {
                        stderr_buffer.extend_from_slice(&chunk[..n]);
                        if stderr_buffer.len() >= MAX_BUFFER_SIZE {
                            let _ = child.kill();
                            limit_exceeded = true;
                            break;
                        }
                    }
                }

                // High-precision low-latency processing tick
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                safe_cleanup_job_dir(job_dir);
                return Err(format!("Sandbox monitoring error: {}", e));
            }
        }
    }

    let execution_duration_ms = start_time.elapsed().as_millis();
    safe_cleanup_job_dir(job_dir);

    let stdout_size = stdout_buffer.len();
    let stderr_size  = stderr_buffer.len();

    let clean_stdout = String::from_utf8_lossy(&stdout_buffer).to_string();
    let mut clean_stderr = String::from_utf8_lossy(&stderr_buffer).to_string();

    if limit_exceeded {
        clean_stderr.push_str(
            "\n[SECURITY ALERT] HOST PIPE BOMB BLOCKED: Execution terminated. \
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

/// Extract the most actionable lines from a raw compiler stderr stream.
pub fn extract_clean_error(raw_stderr: &str) -> String {
    raw_stderr
        .lines()
        .filter(|line| {
            line.contains("error")
                || line.contains("-->")
                || line.contains('|')
                || line.contains("help:")
        })
        .take(25)
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
