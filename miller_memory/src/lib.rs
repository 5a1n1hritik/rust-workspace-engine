// miller_memory/src/lib.rs
//
// Crate entry point. Declares and wires up the four internal modules, then
// re-exports the public surface that workspace consumers (miller_core, etc.)
// need to import. Keeping this file to re-exports only means callers never
// have to reach into sub-modules by path:
//
//   use miller_memory::{MillerMemory, MemoryPayload, ScoredChunk, log_event, LogLevel};

mod config;
mod logger;
mod models;
mod client;

// Logging primitives — re-exported so miller_core can share the same
// LogLevel / log_event rather than duplicating them in miller_parser.
pub use logger::{log_event, LogLevel};

// Data models consumed by both miller_memory internals and external crates.
pub use models::{MemoryPayload, ScoredChunk};

// The primary engine type.
pub use client::MillerMemory;
































// use reqwest::Client;
// use serde::{Serialize, Deserialize};
// use serde_json::json;
// use std::time::Duration;

// use std::time::SystemTime;

// pub enum LogLevel { Info, Warn, Error, Debug }

// pub fn log_event(level: LogLevel, context: &str, message: &str) {
//     let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
//     let secs = now.as_secs();
//     let timestamp = format!("{:02}:{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60);
//     let lvl_str = match level {
//         LogLevel::Info => "INFO",
//         LogLevel::Warn => "WARN",
//         LogLevel::Error => "ERROR",
//         LogLevel::Debug => "DEBUG",
//     };
//     println!("[{}] [{}] [{}] {}", timestamp, lvl_str, context.to_uppercase(), message);
// }

// const QDRANT_URL: &str = "http://localhost:6333";
// const COLLECTION_NAME: &str = "miller_codebase";
// const VECTOR_DIMENSION: usize = 384; // Standard lightweight embedding size (e.g., All-MiniLM-L6-v2)

// #[derive(Debug, Serialize, Deserialize)]
// pub struct MemoryPayload {
//     pub file_path: String,
//     pub entity_name: String,
//     pub entity_type: String, // "function" ya "struct"
//     pub content: String,
// }

// pub struct MillerMemory {
//     client: Client,
// }

// impl MillerMemory {
//     /// Initialize client with proper connection timeouts
//     pub fn new() -> Self {
//         let client = Client::builder()
//             .timeout(Duration::from_secs(10))
//             .build()
//             .unwrap_or_else(|_| Client::new());
//         Self { client }
//     }

//     /// 1. Create Collection: Qdrant ke andar Miller ke liye memory space banana
//     pub async fn init_collection(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//         let url = format!("{}/collections/{}", QDRANT_URL, COLLECTION_NAME);
        
//         // Check if collection already exists to avoid overwriting
//         let check_resp = self.client.get(&url).send().await;
//         if let Ok(resp) = check_resp {
//             if resp.status().is_success() {
//                 log_event(LogLevel::Info, "memory", &format!("Collection '{}' already active.", COLLECTION_NAME));
//                 return Ok(());
//             }
//         }

//         log_event(LogLevel::Info, "memory", &format!("Creating fresh vector collection '{}'...", COLLECTION_NAME));
//         let payload = json!({
//             "vectors": {
//                 "size": VECTOR_DIMENSION,
//                 "distance": "Cosine" // Code similarity match karne ke liye best metric
//             }
//         });

//         let response = self.client.put(&url)
//             .json(&payload)
//             .send()
//             .await?;

//         if response.status().is_success() {
//             log_event(LogLevel::Info, "memory", "Qdrant Collection initialized successfully!");
//             Ok(())
//         } else {
//             Err(format!("Failed to create collection: {}", response.text().await?).into())
//         }
//     }

//     /// 2. Upsert Vector: Code chunk aur uske vector ko database mein save karna
//     pub async fn upsert_code_chunk(
//         &self,
//         id: u64,
//         vector: Vec<f64>,
//         payload: MemoryPayload,
//     ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//         let url = format!("{}/collections/{}/points", QDRANT_URL, COLLECTION_NAME);
        
//         let point_data = json!({
//             "points": [
//                 {
//                     "id": id,
//                     "vector": vector,
//                     "payload": payload
//                 }
//             ]
//         });

//         let response = self.client.put(&url)
//             .json(&point_data)
//             .send()
//             .await?;

//         if response.status().is_success() {
//             Ok(())
//         } else {
//             Err(format!("Failed to upsert point: {}", response.text().await?).into())
//         }
//     }

//     /// 3. Semantic Search: Code query vector ke basis par exact matching code dhoodhna
//     pub async fn search_similar_code(
//         &self,
//         query_vector: Vec<f64>,
//         limit: usize,
//     ) -> Result<Vec<MemoryPayload>, Box<dyn std::error::Error + Send + Sync>> {
//         let url = format!("{}/collections/{}/points/search", QDRANT_URL, COLLECTION_NAME);
        
//         let search_payload = json!({
//             "vector": query_vector,
//             "limit": limit,
//             "with_payload": true
//         });

//         let response = self.client.post(&url)
//             .json(&search_payload)
//             .send()
//             .await?;

//         if !response.status().is_success() {
//             return Err(format!("Search failed: {}", response.text().await?).into());
//         }

//         let result_json: serde_json::Value = response.json().await?;
//         let mut matched_payloads = Vec::new();

//         if let Some(results) = result_json["result"].as_array() {
//             for res in results {
//                 if let Some(payload_val) = res["payload"].as_object() {
//                     let payload_str = serde_json::to_string(payload_val)?;
//                     if let Ok(payload) = serde_json::from_str::<MemoryPayload>(&payload_str) {
//                         matched_payloads.push(payload);
//                     }
//                 }
//             }
//         }

//         Ok(matched_payloads)
//     }
// }
