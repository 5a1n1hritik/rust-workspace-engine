// miller_core/src/compiler.rs
use std::process::Command;
// use std::fs;
use crate::config::{TARGET_FILE, EXEC_NAME};

pub fn run_compiler_test() -> Result<bool, std::io::Error> {
    let output = Command::new("rustc")
        .arg(TARGET_FILE)
        .arg("-o")
        .arg(EXEC_NAME)
        .output()?;
    Ok(output.status.success())
}

pub fn run_sandbox_execution() -> Result<String, String> {
    let output = match Command::new(EXEC_NAME).output() {
        Ok(out) => out,
        Err(e) => return Err(format!("Failed to spawn sandbox execution binary: {}", e)),
    };
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

pub fn get_clean_compiler_error() -> String {
    // Agar compilation fail hoti hai toh logs read karne ke liye helper
    let output = Command::new("rustc").arg(TARGET_FILE).arg("-o").arg(EXEC_NAME).output();
    if let Ok(out) = output {
        let stderr = String::from_utf8_lossy(&out.stderr);
        stderr.lines()
            .filter(|line| line.contains("error") || line.contains("-->") || line.contains("|") || line.contains("help:"))
            .take(25)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        "Unknown compilation engine error".to_string()
    }
}