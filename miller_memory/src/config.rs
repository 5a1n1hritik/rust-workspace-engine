// miller_memory/src/config.rs

pub const QDRANT_URL: &str = "http://localhost:6333";
pub const COLLECTION_NAME: &str = "miller_codebase";

/// Standard lightweight embedding dimension (e.g. All-MiniLM-L6-v2).
pub const VECTOR_DIMENSION: usize = 384;