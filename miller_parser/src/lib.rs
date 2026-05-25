pub mod ast_graph;
pub mod scanner;

use std::time::SystemTime;

pub enum LogLevel {
    Info,
    Warn,
    Error,
    Debug,
}

impl LogLevel {
    fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Debug => "DEBUG",
        }
    }
}

// System timestamp helper
fn get_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    
    // Simple timestamp rendering formatting
    let secs = now.as_secs();
    let hours = (secs / 3600) % 24;
    let minutes = (secs / 60) % 60;
    let seconds = secs % 60;
    
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

// Industry Standard Structured Logger Layout Matrix
pub fn log_event(level: LogLevel, context: &str, message: &str) {
    let timestamp = get_timestamp();
    println!(
        "[{}] [{}] [{}] {}",
        timestamp,
        level.as_str(),
        context.to_uppercase(),
        message
    );
}