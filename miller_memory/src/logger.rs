// miller_memory/src/logger.rs
//
// Isolated logging primitives. miller_memory re-exports these so that
// downstream crates (miller_core) can consume a single canonical LogLevel
// rather than defining their own.

use std::time::SystemTime;

pub enum LogLevel {
    Info,
    Warn,
    Error,
    Debug,
}

pub fn log_event(level: LogLevel, context: &str, message: &str) {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let timestamp = format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    );
    let lvl_str = match level {
        LogLevel::Info  => "INFO",
        LogLevel::Warn  => "WARN",
        LogLevel::Error => "ERROR",
        LogLevel::Debug => "DEBUG",
    };
    println!("[{}] [{}] [{}] {}", timestamp, lvl_str, context.to_uppercase(), message);
}