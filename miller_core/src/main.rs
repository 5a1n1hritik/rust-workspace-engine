// miller_core/src/main.rs
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use reqwest::Client;

// All modules definitions mapped clearly
mod config;
mod security;
mod network;
mod compiler;
mod orchestrator;

use config::{HTTP_TIMEOUT, HTTP_POOL_IDLE, TARGET_FILE, EXEC_NAME};
use security::{is_safe, sanitize_code};
use network::{get_ollama_embedding, generate_with_retry};
use compiler::{run_compiler_test, run_sandbox_execution, get_clean_compiler_error};
use orchestrator::{build_repair_prompt, construct_initial_prompt};
use miller_parser::{log_event, LogLevel};

use miller_parser::scanner::scan_and_parse_project_incremental;
use miller_memory::{MillerMemory, MemoryPayload};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::builder()
        .timeout(HTTP_TIMEOUT)
        .pool_idle_timeout(HTTP_POOL_IDLE)
        .build()?;

    let memory_layer = MillerMemory::new();
    memory_layer.init_collection().await?;

    log_event(LogLevel::Info, "system", "=== MILLER: Local Autonomous Coding Framework ===");
    
    // Target Path Discovery Setup
    let args: Vec<String> = std::env::args().collect();
    let mut target_project_path: Option<PathBuf> = None;
    
    if args.len() > 1 {
        let path = PathBuf::from(&args[1]);
        if path.exists() {
            log_event(LogLevel::Info, "system", &format!("External project path loaded: {:?}", path));
            target_project_path = Some(path);
        } else {
            log_event(LogLevel::Error, "system", &format!("Path exist nahi karta: {:?}", path));
            return Ok(());
        }
    } else {
        log_event(LogLevel::Warn, "system", "No external workspace attached. Background scanner is Idle.");
    }

    // Background Thread
    if let Some(bg_path) = target_project_path {
        let bg_client = client.clone();
        tokio::task::spawn(async move {
            log_event(LogLevel::Info, "background", &format!("Silent monitoring loop active: {:?}", bg_path));

            let scan_result = tokio::task::spawn_blocking(move || {
                scan_and_parse_project_incremental(&bg_path)
            })
            .await;

            let (changed_nodes, skipped_count) = match scan_result {
                Ok(result) => result,
                Err(e) => {
                    log_event(LogLevel::Error, "background", &format!("Scanner thread crashed: {}", e));
                    return;
                }
            };
            
            if skipped_count > 0 { 
                log_event(LogLevel::Info, "background", &format!("Fast skip operational: {} files unchanged.", skipped_count));
            }

            if changed_nodes.is_empty() { 
                log_event(LogLevel::Info, "background", "Cache clean. Workspace state sync complete."); 
                return; 
            }

            log_event(LogLevel::Info, "background", &format!("Syncing {} modified items into local Qdrant...", changed_nodes.len()));

            let bg_memory = MillerMemory::new();
            let mut pseudo_id = std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_secs();

            for (idx, chunk) in changed_nodes.iter().enumerate() {
                if let Ok(vector) = get_ollama_embedding(&bg_client, &chunk.source_code).await {
                    let payload = MemoryPayload {
                        file_path: chunk.file_path.clone(),
                        entity_name: chunk.entity_name.clone(),
                        entity_type: chunk.entity_type.clone(),
                        content: chunk.source_code.clone(),
                    };
                    if let Ok(_) = bg_memory.upsert_code_chunk(pseudo_id, vector, payload).await { pseudo_id += 1; }
                }
                if (idx + 1) % 30 == 0 { tokio::time::sleep(std::time::Duration::from_millis(400)).await; }
            }
            log_event(LogLevel::Info, "background", "Vector embedding database sync cycle completed!");
        });
    }
    
    print!("\nMiller ko task batao:\n> ");
    io::stdout().flush()?;

    let mut original_task = String::new();
    io::stdin().read_line(&mut original_task)?;
    let original_task = original_task.trim();

    log_event(LogLevel::Info, "retrieval", "Searching past code patterns from local Qdrant DB...");
    let mut context_code_str = String::new();
    
    if let Ok(query_vector) = get_ollama_embedding(&client, original_task).await {
        if let Ok(matched_chunks) = memory_layer.search_similar_code(query_vector, 2).await {
            if !matched_chunks.is_empty() {

                log_event(LogLevel::Info, "retrieval", "Valid structural matches found. Injecting blocks into prompt context...");

                context_code_str.push_str("\n--- RELEVANT EXISTING CONTEXT CODE ---\n");

                for chunk in matched_chunks {
                    if chunk.file_path.contains("miller_core") || chunk.file_path.contains("miller_parser") { continue; }

                    context_code_str.push_str(&format!("// From File: {}\n// Entity: {}\n{}\n\n", chunk.file_path, chunk.entity_name, chunk.content));
                }
                context_code_str.push_str("---------------------------------------\n");
            }
        }
    }
    
    let mut current_prompt = construct_initial_prompt(&context_code_str, original_task);
    let mut attempts = 0;
    const MAX_ATTEMPTS: usize = 5;

    while attempts < MAX_ATTEMPTS {
        attempts += 1;
        log_event(LogLevel::Info, "engine", &format!("Generating code (Attempt {}/{})...", attempts, MAX_ATTEMPTS));
        
        let ai_response = match generate_with_retry(&client, &current_prompt).await {
            Ok(text) => text,
            Err(e) => { 
                log_event(LogLevel::Error, "network", &format!("Ollama call permanently failed: {}", e)); 
                break; 
            }
        };

        let code_to_write = match sanitize_code(&ai_response) {
            Some(code) => code,
            None => {
                log_event(LogLevel::Warn, "sanitizer", "Code standard structure block not captured. Re-prompting...");
                current_prompt = format!("Your previous response did not contain standard markdown code fences. Please regenerate and wrap properly.\nTask: {}", original_task);
                continue;
            }
        };

        if !is_safe(&code_to_write) {
            log_event(LogLevel::Error, "security", "Dangerous instruction sequence intercepted! Task terminated.");
            break;
        }

        fs::write(TARGET_FILE, &code_to_write)?;
        log_event(LogLevel::Info, "filesystem", &format!("Code successfully written to sandbox: '{}'", TARGET_FILE));

        log_event(LogLevel::Info, "compiler", "Running rustc validation...");
        match run_compiler_test() {
            Ok(true) => {
                log_event(LogLevel::Info, "compiler", "Code compiled successfully! Moving to Execution Sandbox...");
                match run_sandbox_execution() {
                    Ok(stdout) => {
                        println!("\n[Sandbox Execution Pass]\n--- STDOUT ---\n{}\n--------------", stdout);
                        let _ = fs::remove_file(EXEC_NAME);
                        log_event(LogLevel::Info, "engine", "Processing Loop Cycle Ended Successfully.");
                        break;
                    }
                    Err(stderr) => {
                        log_event(LogLevel::Error, "sandbox", "Executable container crash.");
                        current_prompt = build_repair_prompt(original_task, &code_to_write, &stderr, "Runtime/Execution Error");
                    }
                }
            }
            _ => {
                let clean_error = get_clean_compiler_error();
                log_event(LogLevel::Error, "compiler", "Logic failure details captured during compilation.");
                current_prompt = build_repair_prompt(original_task, &code_to_write, &clean_error, "Compilation Error");
            }
        }
    }
    Ok(())
}

