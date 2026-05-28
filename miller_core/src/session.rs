// miller_core/src/session.rs

use std::fs;
use std::path::Path;
use serde::{Serialize, Deserialize};

const SESSION_DIR: &str = ".sessions";
const LATEST_SESSION_FILE: &str = ".sessions/latest.json";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum AgentState {
    Generating,
    Sanitizing,
    SecurityScanning,
    Compiling,
    Executing,
    Parsing,
    Embedding,
    Persisting,
    Recovering,
    Failed,
    Complete,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionState {
    pub task: String,
    pub attempt: usize,
    pub current_state: AgentState,
    pub generated_code: String,
    pub last_error: String,
}

/// Automates directory guardrails and persists the current state machine dump to disk
pub fn save_session(state: &SessionState) -> Result<(), std::io::Error> {
    if !Path::new(SESSION_DIR).exists() {
        fs::create_dir_all(SESSION_DIR)?;
    }

    let json_data = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        
    fs::write(LATEST_SESSION_FILE, json_data)?;
    Ok(())
}

/// Tries to fetch the last aborted or crashed session checkpoint from disk
pub fn load_last_session() -> Option<SessionState> {
    if !Path::new(LATEST_SESSION_FILE).exists() {
        return None;
    }

    let json_data = fs::read_to_string(LATEST_SESSION_FILE).ok()?;
    serde_json::from_str(&json_data).ok()
}

/// Cleans up the checkpoint once the agent successfully terminates or fails completely
pub fn clear_session() {
    if Path::new(LATEST_SESSION_FILE).exists() {
        let _ = fs::remove_file(LATEST_SESSION_FILE);
    }
}