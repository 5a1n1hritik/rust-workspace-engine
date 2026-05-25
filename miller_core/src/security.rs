// miller_core/src/security.rs
use regex::Regex;

pub const BLOCKED_PATTERNS: &[&str] = &[
    "remove_dir_all", "std::fs::remove_dir", "Command::new", 
    "std::process::Command", "unsafe", "std::net", "TcpStream", "TcpListener", "std::os::unix::fs"
];

// Security guardrail check
pub fn is_safe(code: &str) -> bool {
    !BLOCKED_PATTERNS.iter().any(|pattern| code.contains(pattern))
}

// Code content ko markdown fences se nikalne ka logic
pub fn sanitize_code(raw: &str) -> Option<String> {
    let re = Regex::new(r"(?s)```rust(.*?)```").ok()?;
    let mut code = if let Some(captures) = re.captures(raw) {
        captures.get(1)?.as_str().trim().to_string()
    } else {
        let re_fallback = Regex::new(r"(?s)```(.*?)```").ok()?;
        if let Some(captures) = re_fallback.captures(raw) {
            captures.get(1)?.as_str().trim().to_string()
        } else {
            return None;
        }
    };
    code = code.replace("\r\n", "\n");
    if code.trim().is_empty() { return None; }
    Some(code.trim().to_string())
}