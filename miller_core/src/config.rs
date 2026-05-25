// miller_core/src/config.rs
use std::time::Duration;

pub const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
pub const EMBED_URL: &str = "http://localhost:11434/api/embeddings";
pub const MODEL_NAME: &str = "qwen2.5-coder:3b";
pub const EMBED_MODEL: &str = "all-minilm"; 
pub const TARGET_FILE: &str = "sandbox.rs";
pub const EXEC_NAME: &str = "./sandbox_exec";

pub const HTTP_TIMEOUT: Duration = Duration::from_secs(120);
pub const HTTP_POOL_IDLE: Duration = Duration::from_secs(30);