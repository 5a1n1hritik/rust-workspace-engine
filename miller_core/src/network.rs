// miller_core/src/network.rs
use reqwest::Client;
use serde_json::json;
use crate::config::{EMBED_URL, EMBED_MODEL, OLLAMA_URL, MODEL_NAME};

// Inter-module Ollama Vector Embedding Core
pub async fn get_ollama_embedding(
    client: &Client,
    text: &str
) -> Result<Vec<f64>, Box<dyn std::error::Error + Send + Sync>> {
    // 500 characters truncation guardrail to prevent context length errors
    let safe_text = if text.len() > 500 {
        match text.char_indices().nth(500) {
            Some((idx, _)) => &text[..idx],
            None => text,
        }
    } else {
        text
    };

    let response = client.post(EMBED_URL)
        .json(&json!({ "model": EMBED_MODEL, "prompt": safe_text }))
        .send()
        .await?;
    
    let json_val: serde_json::Value = response.json().await?;

    if let Some(err) = json_val.get("error") {
        eprintln!("[Ollama Debug] Ollama returned error: {:?}", err);
    }

    if let Some(embedding_array) = json_val["embedding"].as_array() {
        let vector: Vec<f64> = embedding_array.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect();
        Ok(vector)
    } else {
        eprintln!("[Ollama Debug] Unexpected JSON format: {:?}", json_val);
        Err("Failed to extract valid embedding vectors from Ollama".into())
    }
}

// Robust generation client with internal retry logic
pub async fn generate_with_retry(
    client: &Client,
    prompt: &str
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    const MAX_RETRIES: usize = 3;
    for attempt in 1..=MAX_RETRIES {
        let response = client.post(OLLAMA_URL)
            .json(&json!({
                "model": MODEL_NAME,
                "prompt": prompt,
                "stream": false
            }))
            .send()
            .await;

        match response {
            Ok(resp) => {
                if let Ok(json_data) = resp.json::<serde_json::Value>().await {
                    if let Some(text) = json_data["response"].as_str() {
                        return Ok(text.to_string());
                    }
                }
            }
            Err(e) => println!("[Client Retry {}] Connection glitch: {}", attempt, e),
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    Err("Ollama connections permanently dropped.".into())
}