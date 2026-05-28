// miller_memory/src/models.rs
//
// Data models shared across the crate and re-exported to workspace consumers.
// ScoredChunk is the fix for the main.rs data-flow discrepancy: it carries the
// Qdrant cosine similarity score alongside the payload so callers can apply
// quality-threshold filtering (e.g. chunk.score >= 0.35) without any extra
// deserialization step.

use serde::{Deserialize, Serialize};

/// Structured metadata stored in Qdrant alongside each code vector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPayload {
    pub file_path:   String,
    pub entity_name: String,
    /// e.g. "function" or "struct"
    pub entity_type: String,
    pub content:     String,
}

/// A search result returned by `MillerMemory::search_similar_code`.
/// Bundles the raw Qdrant cosine similarity score with the associated payload
/// so that the caller can apply score-based filtering directly.
///
/// ```rust
/// let qualified: Vec<_> = results
///     .into_iter()
///     .filter(|c| c.score >= 0.35)
///     .collect();
/// ```
#[derive(Debug, Clone)]
pub struct ScoredChunk {
    /// Cosine similarity in [0.0, 1.0]; higher is more similar.
    pub score:   f32,
    pub payload: MemoryPayload,
}