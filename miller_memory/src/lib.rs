use reqwest::Client;
use serde::{Serialize, Deserialize};
use serde_json::json;
use std::time::Duration;

const QDRANT_URL: &str = "http://localhost:6333";
const COLLECTION_NAME: &str = "miller_codebase";
const VECTOR_DIMENSION: usize = 384; // 📌 Standard lightweight embedding size (e.g., All-MiniLM-L6-v2)

#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryPayload {
    pub file_path: String,
    pub entity_name: String,
    pub entity_type: String, // "function" ya "struct"
    pub content: String,
}

pub struct MillerMemory {
    client: Client,
}

impl MillerMemory {
    /// 🏗️ Initialize client with proper connection timeouts
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }

    /// 🛠️ 1. Create Collection: Qdrant ke andar Miller ke liye memory space banana
    pub async fn init_collection(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/collections/{}", QDRANT_URL, COLLECTION_NAME);
        
        // Check if collection already exists to avoid overwriting
        let check_resp = self.client.get(&url).send().await;
        if let Ok(resp) = check_resp {
            if resp.status().is_success() {
                println!("[Memory] Collection '{}' already active.", COLLECTION_NAME);
                return Ok(());
            }
        }

        println!("[Memory] Creating fresh vector collection '{}'...", COLLECTION_NAME);
        let payload = json!({
            "vectors": {
                "size": VECTOR_DIMENSION,
                "distance": "Cosine" // Code similarity match karne ke liye best metric
            }
        });

        let response = self.client.put(&url)
            .json(&payload)
            .send()
            .await?;

        if response.status().is_success() {
            println!("🎉 [Memory] Qdrant Collection initialized successfully!");
            Ok(())
        } else {
            Err(format!("Failed to create collection: {}", response.text().await?).into())
        }
    }

    /// 📥 2. Upsert Vector: Code chunk aur uske vector ko database mein save karna
    pub async fn upsert_code_chunk(
        &self,
        id: u64,
        vector: Vec<f64>,
        payload: MemoryPayload,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/collections/{}/points", QDRANT_URL, COLLECTION_NAME);
        
        let point_data = json!({
            "points": [
                {
                    "id": id,
                    "vector": vector,
                    "payload": payload
                }
            ]
        });

        let response = self.client.put(&url)
            .json(&point_data)
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("Failed to upsert point: {}", response.text().await?).into())
        }
    }

    /// 🔍 3. Semantic Search: Code query vector ke basis par exact matching code dhoodhna
    pub async fn search_similar_code(
        &self,
        query_vector: Vec<f64>,
        limit: usize,
    ) -> Result<Vec<MemoryPayload>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/collections/{}/points/search", QDRANT_URL, COLLECTION_NAME);
        
        let search_payload = json!({
            "vector": query_vector,
            "limit": limit,
            "with_payload": true
        });

        let response = self.client.post(&url)
            .json(&search_payload)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(format!("Search failed: {}", response.text().await?).into());
        }

        let result_json: serde_json::Value = response.json().await?;
        let mut matched_payloads = Vec::new();

        if let Some(results) = result_json["result"].as_array() {
            for res in results {
                if let Some(payload_val) = res["payload"].as_object() {
                    let payload_str = serde_json::to_string(payload_val)?;
                    if let Ok(payload) = serde_json::from_str::<MemoryPayload>(&payload_str) {
                        matched_payloads.push(payload);
                    }
                }
            }
        }

        Ok(matched_payloads)
    }
}
