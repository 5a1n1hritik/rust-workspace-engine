// miller_memory/src/client.rs
//
// Core Qdrant client engine. All HTTP interaction with the vector database
// lives here and nowhere else. The three public methods form a stable interface:
//
//   init_collection   – idempotent collection bootstrap
//   upsert_code_chunk – insert or overwrite a single vector point
//   search_similar_code – ANN search returning scored results
//
// search_similar_code now returns Vec<ScoredChunk> instead of Vec<MemoryPayload>
// so that callers (miller_core/main.rs) can apply similarity-score filtering
// without re-parsing the raw Qdrant response themselves.

use reqwest::Client;
use serde_json::json;
use std::time::Duration;

use crate::config::{COLLECTION_NAME, QDRANT_URL, VECTOR_DIMENSION};
use crate::logger::{log_event, LogLevel};
use crate::models::{MemoryPayload, ScoredChunk};

pub struct MillerMemory {
    client: Client,
}

impl MillerMemory {
    /// Build a `MillerMemory` instance with sensible connection timeouts.
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }

    // ── 1. Collection bootstrap ───────────────────────────────────────────────

    /// Ensure the Qdrant collection exists. Safe to call repeatedly; it checks
    /// for an existing collection before attempting creation so it never
    /// overwrites live data.
    pub async fn init_collection(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/collections/{}", QDRANT_URL, COLLECTION_NAME);

        let check = self.client.get(&url).send().await;
        if let Ok(resp) = check {
            if resp.status().is_success() {
                log_event(
                    LogLevel::Info,
                    "memory",
                    &format!("Collection '{}' already active.", COLLECTION_NAME),
                );
                return Ok(());
            }
        }

        log_event(
            LogLevel::Info,
            "memory",
            &format!("Creating fresh vector collection '{}'...", COLLECTION_NAME),
        );

        let payload = json!({
            "vectors": {
                "size": VECTOR_DIMENSION,
                // Cosine is the best metric for semantic code similarity.
                "distance": "Cosine"
            }
        });

        let response = self.client.put(&url).json(&payload).send().await?;

        if response.status().is_success() {
            log_event(LogLevel::Info, "memory", "Qdrant collection initialized successfully.");
            Ok(())
        } else {
            Err(format!(
                "Failed to create collection: {}",
                response.text().await?
            )
            .into())
        }
    }

    // ── 2. Upsert ─────────────────────────────────────────────────────────────

    /// Insert or overwrite a single code-chunk vector point.
    /// `id` is the caller-managed numeric point identifier; callers are
    /// responsible for ensuring uniqueness within the collection.
    pub async fn upsert_code_chunk(
        &self,
        id: u64,
        vector: Vec<f64>,
        payload: MemoryPayload,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/collections/{}/points", QDRANT_URL, COLLECTION_NAME);

        let body = json!({
            "points": [
                {
                    "id":      id,
                    "vector":  vector,
                    "payload": payload
                }
            ]
        });

        let response = self.client.put(&url).json(&body).send().await?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("Failed to upsert point {}: {}", id, response.text().await?).into())
        }
    }

    // ── 3. Semantic search ────────────────────────────────────────────────────

    /// Approximate nearest-neighbour search over the code-chunk collection.
    ///
    /// Returns up to `limit` results as `ScoredChunk` values ordered by
    /// descending cosine similarity. The `score` field on each result is the
    /// raw Qdrant similarity value in [0.0, 1.0] and can be used directly for
    /// quality-threshold filtering by the caller:
    ///
    /// ```rust
    /// let qualified: Vec<_> = memory
    ///     .search_similar_code(query_vec, 5).await?
    ///     .into_iter()
    ///     .filter(|c| c.score >= 0.35)
    ///     .collect();
    /// ```
    pub async fn search_similar_code(
        &self,
        query_vector: Vec<f64>,
        limit: usize,
    ) -> Result<Vec<ScoredChunk>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "{}/collections/{}/points/search",
            QDRANT_URL, COLLECTION_NAME
        );

        let search_body = json!({
            "vector":       query_vector,
            "limit":        limit,
            "with_payload": true
        });

        let response = self.client.post(&url).json(&search_body).send().await?;

        if !response.status().is_success() {
            return Err(
                format!("Search failed: {}", response.text().await?).into()
            );
        }

        let result_json: serde_json::Value = response.json().await?;
        let mut scored_chunks: Vec<ScoredChunk> = Vec::new();

        if let Some(results) = result_json["result"].as_array() {
            for res in results {
                // Extract the similarity score Qdrant returns on each hit.
                // Fall back to 0.0 so the quality guard in main.rs will
                // naturally discard any malformed result rather than panicking.
                let score = res["score"].as_f64().unwrap_or(0.0) as f32;

                if let Some(payload_obj) = res["payload"].as_object() {
                    let payload_str = serde_json::to_string(payload_obj)?;
                    match serde_json::from_str::<MemoryPayload>(&payload_str) {
                        Ok(payload) => scored_chunks.push(ScoredChunk { score, payload }),
                        Err(e) => log_event(
                            LogLevel::Warn,
                            "memory",
                            &format!("Skipping malformed payload during search deserialization: {}", e),
                        ),
                    }
                }
            }
        }

        Ok(scored_chunks)
    }
}

impl Default for MillerMemory {
    fn default() -> Self {
        Self::new()
    }
}