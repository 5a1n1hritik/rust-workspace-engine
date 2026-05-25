

## Ye is commit se pehle ka code hai fully tested runing code.

**Git Commit:**
```text
git commit -m "refactor: modularize miller_core into srp modules config, security, network, isolate compiler and orchestration logics out of main.rs into srp modules" 
```


**Code file: `miller_core/src/main.rs`**
```Rust
use std::fs;
use std::process::Command;
use std::io::{self, Write};
use std::time::Duration;
use std::path::PathBuf;
use reqwest::Client;
use serde_json::json;
use regex::Regex;

// Use miller_parser function
use miller_parser::scanner::scan_and_parse_project_incremental;
use miller_memory::{MillerMemory, MemoryPayload};

const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
const EMBED_URL: &str = "http://localhost:11434/api/embeddings";
const MODEL_NAME: &str = "qwen2.5-coder:3b";
const EMBED_MODEL: &str = "all-minilm"; // Fixed lightweight 384-dim embedding model
const TARGET_FILE: &str = "sandbox.rs";
const EXEC_NAME: &str = "./sandbox_exec";

// Security: Stochastic parrot ko root access nahi dena hai!
const BLOCKED_PATTERNS: &[&str] = &[
    "remove_dir_all",
    "std::fs::remove_dir",
    "Command::new",
    "std::process::Command",
    "unsafe",
    "std::net",
    "TcpStream",
    "TcpListener",
    "std::os::unix::fs",
];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Production HTTP Client: Defend against freezes and timeouts
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .pool_idle_timeout(Duration::from_secs(30))
        .build()?;


    // Initialize Local Qdrant Memory Layer
    let memory_layer = MillerMemory::new();
    memory_layer.init_collection().await?;

    println!("=== MILLER: Local Autonomous Coding Framework ===");
    
    let args: Vec<String> = std::env::args().collect();
    let mut target_project_path: Option<PathBuf> = None;
    
    if args.len() > 1 {
        let path = PathBuf::from(&args[1]);
        if path.exists() {
            println!("[System] External project path loaded: {:?}", path);
            target_project_path = Some(path);
        } else {
            println!("[Error] Path exist nahi karta: {:?}", path);
            return Ok(());
        }
    } else {
        println!("[System] No external workspace attached. Background scanner is Idle.");
    }

    // TOKIO ASYNC BACKGROUND SCANNER RUN (The Cursor Way)
    if let Some(bg_path) = target_project_path {
        let bg_client = client.clone();
        // let bg_path = target_project_path.clone();
        
        tokio::task::spawn(async move {
            println!("\n[Background Worker] Silent monitoring loop active: {:?}", bg_path);

            let scan_result = tokio::task::spawn_blocking(move || {
                scan_and_parse_project_incremental(&bg_path)
            })
            .await;

            let (changed_nodes, skipped_count) = match scan_result {
                Ok(result) => result,
                Err(e) => {
                    eprintln!("[Background Worker] Scanner thread crashed: {}", e);
                    return;
                }
            };
            
            if skipped_count > 0 {
                println!("[Background Worker] Fast skip operational: {} files unchanged.", skipped_count);
            }

            if changed_nodes.is_empty() {
                println!(
                    "\n[Background Worker] Cache clean. Total code memory state is fully synchronous."
                );
                return;
            }

            println!(
                "[Background Worker] Syncing {} modified items into local Qdrant...",
                changed_nodes.len()
            );

            let bg_memory = MillerMemory::new();

            let mut pseudo_id = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            const BATCH_SIZE: usize = 30;

            for (idx, chunk) in changed_nodes.iter().enumerate() {
                match get_ollama_embedding(&bg_client, &chunk.content).await {
                    Ok(vector) => {
                        let payload = MemoryPayload {
                            file_path: chunk.file_path.clone(),
                            entity_name: chunk.entity_name.clone(),
                            entity_type: chunk.entity_type.clone(),
                            content: chunk.content.clone(),
                        };

                        match bg_memory
                            .upsert_code_chunk(pseudo_id, vector, payload)
                            .await
                        {
                            Ok(_) => {
                                pseudo_id += 1;
                            }
                            Err(e) => {
                                eprintln!(
                                    "[Background Worker] Failed to store vector chunk: {}",
                                    e
                                );
                            }
                        }
                    }

                    Err(e) => {
                        eprintln!(
                            "[Background Worker] Embedding generation failed: {}",
                            e
                        );
                    }
                }

                if (idx + 1) % BATCH_SIZE == 0 {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
            }

            println!(
                "\n[Background Worker] Vector embedding database sync cycle completed!"
            );
        });
    }

    print!("\nMiller ko task batao:\n> ");
    io::stdout().flush()?;
    
    let mut original_task = String::new();
    io::stdin().read_line(&mut original_task)?;
    let original_task = original_task.trim();

    // RETRIEVING CONTEXT FROM MEMORY (DATABASE) BASED ON THE USER TASK
    println!("[Retrieval] Searching past code patterns from local Qdrant DB...");
    let mut context_code_str = String::new();
    
    // User ke prompt ka query embedding generate karo
    if let Ok(query_vector) = get_ollama_embedding(&client, original_task).await {
        // Qdrant DB se top 2 sabse matching code chunks nikal lo
        if let Ok(matched_chunks) = memory_layer.search_similar_code(query_vector, 2).await {
            if !matched_chunks.is_empty() {
                println!("[Retrieval Match] Valid structural matches found. Injecting code context memory blocks...");
                context_code_str.push_str("\n--- RELEVANT EXISTING CONTEXT CODE ---\n");
                for chunk in matched_chunks {
                    if chunk.file_path.contains("miller_core") || chunk.file_path.contains("miller_parser") {
                        continue;
                    }
                    println!("   -> Found {} in '{}'", chunk.entity_name, chunk.file_path);
                    context_code_str.push_str(&format!(
                        "// From File: {}\n// Entity: {}\n{}\n\n",
                        chunk.file_path, chunk.entity_name, chunk.content
                    ));
                }
                context_code_str.push_str("---------------------------------------\n");
            } else {
                println!("[Retrieval] Database configuration direct zero hits. Initializing safe baseline creation...");
            }
        }
    }
    
    // Initial System Prompt that prompt for ollama 7b module
    // let mut current_prompt = format!(
    //     "You are Miller, a world-class systems engineer writing pure Rust code. \
    //     You must provide the complete code strictly inside [CODE_START] and [CODE_END] tags. \
    //     Do NOT write markdown code blocks like ```rust. Just raw text inside tags.\n\
    //     Task: {}", 
    //     original_task
    // );

    // that prompt for ollama 3b module.
    let mut current_prompt = format!(
        "You are Miller, an expert Rust engineer. Return ONLY the executable Rust code requested.\n\
        You MUST wrap the code inside a standard markdown code block like this:\n\
        ```rust\n\
        // code here\n\
        ```\n\
        Do not include any introductory or concluding text.\n\
        {}\n\
        Task: {}",
        context_code_str, // inject old matching code in prompt!
        original_task
    );

    let mut attempts = 0;
    const MAX_ATTEMPTS: usize = 5;

    while attempts < MAX_ATTEMPTS {
        attempts += 1;
        println!("\n[Miller] Generating code (Attempt {}/{})...", attempts, MAX_ATTEMPTS);
        
        // Robust Request Layer with internal timeout handling
        let ai_response = match generate_with_retry(&client, &current_prompt).await {
            Ok(text) => text,
            Err(e) => {
                println!("[Network Error] Ollama call permanently failed: {}", e);
                break;
            }
        };

        // Strict Extraction & Sanitization Layer
        let code_to_write = match sanitize_code(&ai_response) {
            Some(code) => code,
            None => {
                println!("[Sanitizer] Code structure standard block not captured. Re-prompt alignment structural fix active...");
                current_prompt = format!(
                    "Your previous response did not contain standard markdown code fences \
                    Please regenerate the full code and wrap it properly.\nTask: {}", original_task
                );
                continue;
            }
        };

        // Security Scanner Layer
        if !is_safe(&code_to_write) {
            println!("[Security Breach] Dangerous instruction sequence intercepted! Task terminated.");
            println!("----------------------------------------\n{}\n----------------------------------------", code_to_write);
            current_prompt = format!(
                "CRITICAL: The code you generated failed our security scan due to blocked system calls (e.g., unsafe, Command, remove_dir_all). \
                Rewrite the code without using any malicious or unsafe calls.\nTask: {}", original_task
            );
            continue;
        }

        // Safe to write now
        fs::write(TARGET_FILE, &code_to_write)?;
        println!("[Filesystem] Code successfully flushed into sandbox storage file: '{}'", TARGET_FILE);

        // Compilation Validation Layer
        println!("[Compiler] Running rustc validation...");
        let compile_output = Command::new("rustc")
            .arg(TARGET_FILE)
            .arg("-o")
            .arg("sandbox_exec")
            .output()?;

        if compile_output.status.success() {
            println!("[Success] Code compiled successfully! Moving to Execution Sandbox...");
            
            // Execution Sandbox & Behavioral Validation
            match run_sandbox_execution() {
                Ok(stdout) => {
                    println!("\n[Sandbox Execution Pass]");
                    println!("--- STDOUT ---");
                    println!("{}", stdout);
                    println!("--------------");

                    // Cleanup binaries
                    let _ = fs::remove_file("sandbox_exec");
                    println!("\nProcessing Loop Cycle Ended Successfully.");
                    break;
                }
                Err(stderr) => {
                    println!("\n[Sandbox Runtime Error] Executable container crash or execution failure.");
                    println!("--- STDERR ---");
                    println!("{}", stderr);
                    println!("--------------");
                    
                    // Stateless Repair Prompt
                    current_prompt = build_repair_prompt(original_task, &code_to_write, &stderr, "Runtime/Execution Error");
                }
            }
        } else {
            // Compilation failed -> Extract clean errors
            let raw_stderr = String::from_utf8_lossy(&compile_output.stderr);
            let clean_error = extract_compiler_error(&raw_stderr);
            
            println!("\n[Compile Error] Logic failure details captured.");
            println!("------------------- CLEAN ERROR -------------------\n{}", clean_error);
            println!("---------------------------------------------------");

            // Stateless Repair Prompt
            current_prompt = build_repair_prompt(original_task, &code_to_write, &clean_error, "Compilation Error");
        }
    }

    if attempts >= MAX_ATTEMPTS {
        println!("\n[Miller] Max repair cycles exhausted. Code could not be fully healed automatically.");
    }

    Ok(())
}

// === INTER-MODULE OLLAMA VECTOR ENGINE ===
async fn get_ollama_embedding(
    client: &Client,
    text: &str
) -> Result<Vec<f64>, Box<dyn std::error::Error + Send + Sync>> {
    // SAFE TRUNCATION: slice to maximum 500 characters to prevent Ollama context length overflow
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
        .send().await?;

    let json_val: serde_json::Value = response.json().await?;

    // Agar Ollama koi explicit error bhej raha hai:
    if let Some(err) = json_val.get("error") {
        eprintln!("[Ollama Debug] Ollama returned error: {:?}", err);
    }
    
    if let Some(embedding_array) = json_val["embedding"].as_array() {
        let vector: Vec<f64> = embedding_array.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect();
        Ok(vector)
    } else {
        // Agar format badal gaya hai toh pure response ke keys check karein
        eprintln!("[Ollama Debug] Unexpected JSON format: {:?}", json_val);
        Err("Failed to extract valid embedding vectors from Ollama".into())
    }
}

// ================= ARCHITECTURAL FUNCTIONS =================

fn sanitize_code(raw: &str) -> Option<String> {
    // 3B model standard markdown fences use karega, hum use hi capture karenge
    let re = Regex::new(r"(?s)```rust(.*?)```").ok()?;
    
    let mut code = if let Some(captures) = re.captures(raw) {
        captures.get(1)?.as_str().trim().to_string()
    } else {
        // Fallback: Agar bina language tag ke sirf ``` diya ho
        let re_fallback = Regex::new(r"(?s)```(.*?)```").ok()?;
        if let Some(captures) = re_fallback.captures(raw) {
            captures.get(1)?.as_str().trim().to_string()
        } else {
            return None;
        }
    };

    code = code.replace("\r\n", "\n");

    if code.trim().is_empty() {
        return None;
    }
    Some(code.trim().to_string())
}

/// Robust Client with internal retry logic for Unstable Local LLMs
async fn generate_with_retry(
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
                match resp.json::<serde_json::Value>().await {
                    Ok(json_data) => {
                        if let Some(text) = json_data["response"].as_str() {
                            return Ok(text.to_string());
                        }
                    }
                    Err(e) => println!("[Client Retry {}] JSON corruption: {}", attempt, e),
                }
            }
            Err(e) => println!("[Client Retry {}] Connection glitch: {}", attempt, e),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Err("Ollama connections permanently dropped.".into())
}

/// Error Extractor: Deletes unicode garbage, filters only core rustc notes
fn extract_compiler_error(stderr: &str) -> String {
    stderr.lines()
        .filter(|line| {
            line.contains("error") 
            || line.contains("-->") 
            || line.contains("|") 
            || line.contains("help:")
        })
        .take(25)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Stateless Repair Prompt Builder: Prevents infinite toxic context bloating
fn build_repair_prompt(original_task: &str, broken_code: &str, error_log: &str, error_type: &str) -> String {
    format!(
r#"You are an expert systems engineer. The previous Rust code you generated failed during {}.

ORIGINAL TASK:
{}

FAILED CODE:
[CODE_START]
{}
[CODE_END]

{} LOG:
{}

Fix the issue completely. Return the ENTIRE updated Rust code strictly inside [CODE_START] and [CODE_END] tags without any markdown code fences."#,
        error_type, original_task, broken_code, error_type, error_log
    )
}

/// Execution Sandbox: Validates behavioral correctness beyond just compilation
fn run_sandbox_execution() -> Result<String, String> {
    let output = match Command::new(EXEC_NAME).output() {
        Ok(out) => out,
        Err(e) => return Err(format!("Failed to spawn sandbox execution binary: {}", e)),
    };

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

/// Security Guardrail
fn is_safe(code: &str) -> bool {
    !BLOCKED_PATTERNS.iter().any(|pattern| code.contains(pattern))
}

```

---

