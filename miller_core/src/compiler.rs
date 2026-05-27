use std::process::Command;
use std::fs;
use std::path::Path;
use std::time::{Instant, Duration};
use std::thread;

// Industry standard execution metrics layout
pub struct SandboxExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

const TMP_JOB_DIR: &str = "/tmp/miller_sandbox_job";
const IMAGE_NAME: &str = "miller-rust-runner";

/// Phase 1: Compile the generated code inside a secure immutable container boundary
pub fn run_hardened_compile() -> Result<bool, std::io::Error> {
    // Ensure temporary job folder alignment
    if !Path::new(TMP_JOB_DIR).exists() {
        fs::create_dir_all(TMP_JOB_DIR)?;
    }
    
    // Copy generated code file into the shared transit container path
    if Path::new("sandbox.rs").exists() {
        fs::copy("sandbox.rs", format!("{}/main.rs", TMP_JOB_DIR))?;
    } else {
        return Ok(false);
    }

    // Secure Ephemeral Compilation command injection matrix
    let output = Command::new("docker")
        .args(&[
            "run", "--rm",
            "--network", "none",
            "--memory", "512m", // Compiler needs a bit more memory than raw execution
            "-v", &format!("{}:/workspace:rw", TMP_JOB_DIR),
            IMAGE_NAME,
            "rustc", "main.rs", "-o", "app",
        ])
        .output()?;

    Ok(output.status.success())
}

/// Phase 2: Execute the compiled binary inside a strictly sandboxed, read-only ephemeral jail
pub fn run_hardened_execution() -> Result<SandboxExecutionResult, String> {
    // Target command setup with strict security primitives (no-new-privileges, capability drop, fork-bomb limits)
    let mut child = match Command::new("docker")
        .args(&[
            "run", "--rm",
            "--init",
            "--network", "none",
            "--memory", "256m",
            "--cpus", "0.5",
            "--pids-limit", "64",
            "--cap-drop", "ALL",
            "--security-opt", "no-new-privileges",
            "--read-only",
            "--tmpfs", "/tmp:rw,noexec,nosuid,size=64m",
            "-v", &format!("{}:/workspace:rw", TMP_JOB_DIR),
            IMAGE_NAME,
            "./app",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn() 
    {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to spawn sandbox container execution: {}", e)),
    };

    let timeout = Duration::from_secs(3);
    let start_time = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child.wait_with_output().map_err(|e| e.to_string())?;
                // Cleanup temp execution storage residue safely
                let _ = fs::remove_dir_all(TMP_JOB_DIR);
                
                return Ok(SandboxExecutionResult {
                    stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                    timed_out: false,
                });
            }
            Ok(None) => {
                if start_time.elapsed() >= timeout {
                    let _ = child.kill(); // Force kill container execution block
                    let _ = fs::remove_dir_all(TMP_JOB_DIR);
                    return Ok(SandboxExecutionResult {
                        stdout: String::new(),
                        stderr: "TIMEOUT ERROR: Process force-killed. Execution exceeded 3s security ceiling.".to_string(),
                        timed_out: true,
                    });
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = fs::remove_dir_all(TMP_JOB_DIR);
                return Err(format!("Sandbox monitoring error: {}", e));
            }
        }
    }
}

/// Extraction helper to pull errors if validation flags break down
pub fn get_clean_compiler_error() -> String {
    let output = Command::new("docker")
        .args(&["run", "--rm", "-v", &format!("{}:/workspace:rw", TMP_JOB_DIR), IMAGE_NAME, "rustc", "main.rs", "-o", "app"])
        .output();

    if let Ok(out) = output {
        let stderr = String::from_utf8_lossy(&out.stderr);
        stderr.lines()
            .filter(|line| line.contains("error") || line.contains("-->") || line.contains("|") || line.contains("help:"))
            .take(25)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        "Unknown container compiler malfunction".to_string()
    }
}

// pub fn run_compiler_test() -> Result<bool, std::io::Error> {
//     let output = Command::new("rustc")
//         .arg(TARGET_FILE)
//         .arg("-o")
//         .arg(EXEC_NAME)
//         .output()?;
//     Ok(output.status.success())
// }

// pub fn run_sandbox_execution() -> Result<String, String> {
//     let output = match Command::new(EXEC_NAME).output() {
//         Ok(out) => out,
//         Err(e) => return Err(format!("Failed to spawn sandbox execution binary: {}", e)),
//     };
//     if output.status.success() {
//         Ok(String::from_utf8_lossy(&output.stdout).to_string())
//     } else {
//         Err(String::from_utf8_lossy(&output.stderr).to_string())
//     }
// }

// pub fn get_clean_compiler_error() -> String {
//     // Agar compilation fail hoti hai toh logs read karne ke liye helper
//     let output = Command::new("rustc").arg(TARGET_FILE).arg("-o").arg(EXEC_NAME).output();
//     if let Ok(out) = output {
//         let stderr = String::from_utf8_lossy(&out.stderr);
//         stderr.lines()
//             .filter(|line| line.contains("error") || line.contains("-->") || line.contains("|") || line.contains("help:"))
//             .take(25)
//             .collect::<Vec<_>>()
//             .join("\n")
//     } else {
//         "Unknown compilation engine error".to_string()
//     }
// }