use std::fs;
use std::process::Command;
use std::io::{self, Write};
use std::time::Duration;
use std::path::Path;            // New import
use reqwest::Client;
use serde_json::json;
use regex::Regex;

// Use miller_parser function
use miller_parser::ast_graph::build_ast_graph;

const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
const MODEL_NAME: &str = "qwen2.5-coder:3b";
const TARGET_FILE: &str = "sandbox.rs";
const EXEC_NAME: &str = "./sandbox_exec";

// 🔒 Security: Stochastic parrot ko root access nahi dena hai!
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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ⚙️ Production HTTP Client: Defend against freezes and timeouts
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .pool_idle_timeout(Duration::from_secs(30))
        .build()?;
    
    println!("=== MILLER: Production-Grade Autonomous Engine ===");
    print!("\nMiller ko task batao:\n> ");
    io::stdout().flush()?;
    
    let mut original_task = String::new();
    io::stdin().read_line(&mut original_task)?;
    let original_task = original_task.trim();
    
    // Initial System Prompt
    let mut current_prompt = format!(
        "You are Miller, a world-class systems engineer writing pure Rust code. \
        You must provide the complete code strictly inside [CODE_START] and [CODE_END] tags. \
        Do NOT write markdown code blocks like ```rust. Just raw text inside tags.\n\
        Task: {}", 
        original_task
    );

    let mut attempts = 0;
    const MAX_ATTEMPTS: usize = 5;

    while attempts < MAX_ATTEMPTS {
        attempts += 1;
        println!("\n[Miller] Generating code (Attempt {}/{})...", attempts, MAX_ATTEMPTS);
        
        // 🚀 2. Robust Request Layer with internal timeout handling
        let ai_response = match generate_with_retry(&client, &current_prompt).await {
            Ok(text) => text,
            Err(e) => {
                println!("❌ [Network Error] Ollama call permanently failed: {}", e);
                break;
            }
        };

        // 🎯 1. Strict Extraction & Sanitization Layer
        let code_to_write = match sanitize_code(&ai_response) {
            Some(code) => code,
            None => {
                println!("⚠️ [Sanitizer] Failed to extract clean code from tags. Regenerating...");
                current_prompt = format!(
                    "Your previous response did not contain the strict [CODE_START] and [CODE_END] tags. \
                    Please regenerate the full code and wrap it properly.\nTask: {}", original_task
                );
                continue;
            }
        };

        // 🔒 6. Security Scanner Layer
        if !is_safe(&code_to_write) {
            println!("🚨 [Security Breach] Generated code contains malicious or blocked patterns! Aborting block.");
            println!("----------------------------------------\n{}\n----------------------------------------", code_to_write);
            current_prompt = format!(
                "CRITICAL: The code you generated failed our security scan due to blocked system calls (e.g., unsafe, Command, remove_dir_all). \
                Rewrite the code without using any malicious or unsafe calls.\nTask: {}", original_task
            );
            continue;
        }

        // 📝 Safe to write now
        fs::write(TARGET_FILE, &code_to_write)?;
        println!("[Filesystem] Code safely written to '{}'", TARGET_FILE);

        // 🛠️ 3. Compilation Validation Layer
        println!("[Compiler] Running rustc validation...");
        let compile_output = Command::new("rustc")
            .arg(TARGET_FILE)
            .arg("-o")
            .arg("sandbox_exec")
            .output()?;

        if compile_output.status.success() {
            println!("🎉 [Success] Code compiled successfully! Moving to Execution Sandbox...");
            
            // 🏃‍♂️ 5. Execution Sandbox & Behavioral Validation
            match run_sandbox_execution() {
                Ok(stdout) => {
                    println!("\n🚀 [Sandbox Execution Pass]");
                    println!("--- STDOUT ---");
                    println!("{}", stdout);
                    println!("--------------");
                    
                    // 👁️ 👁️ 👁️ NEW: EYE OF MILLER (AST PARSER GRAPH INTERACTIVE VIEW) 👁️ 👁️ 👁️
                    println!("\n[Miller Parser] Analysing Generated Code Structure...");
                    let path = Path::new(TARGET_FILE);
                    let nodes = build_ast_graph(path);
                    
                    println!("\n========== LIVE AST CODE GRAPH ==========");
                    if nodes.is_empty() {
                        println!("No structural functions or structs detected.");
                    } else {
                        for node in nodes {
                            println!("📍 Type: [{}] | Name: {}", node.entity_type.to_uppercase(), node.entity_name);
                            if !node.dependencies.is_empty() {
                                println!("   └── Calls: {:?}", node.dependencies);
                            }
                        }
                    }
                    println!("=========================================");

                    // Cleanup binaries
                    let _ = fs::remove_file("sandbox_exec");
                    break;
                }
                Err(stderr) => {
                    println!("\n❌ [Sandbox Runtime Error] Code compiled but failed during execution.");
                    println!("--- STDERR ---");
                    println!("{}", stderr);
                    println!("--------------");
                    
                    // 🔄 4. Stateless Repair Prompt
                    current_prompt = build_repair_prompt(original_task, &code_to_write, &stderr, "Runtime/Execution Error");
                }
            }
        } else {
            // Compilation failed -> Extract clean errors
            let raw_stderr = String::from_utf8_lossy(&compile_output.stderr);
            let clean_error = extract_compiler_error(&raw_stderr);
            
            println!("\n❌ [Compile Error] Found syntax or type errors!");
            println!("------------------- CLEAN ERROR -------------------\n{}", clean_error);
            println!("---------------------------------------------------");

            // 🔄 4. Stateless Repair Prompt
            current_prompt = build_repair_prompt(original_task, &code_to_write, &clean_error, "Compilation Error");
        }
    }

    if attempts >= MAX_ATTEMPTS {
        println!("\n[Miller] Max repair cycles exhausted. Code could not be fully healed automatically.");
    }

    Ok(())
}

// ================= ARCHITECTURAL FUNCTIONS =================

/// 🎯 1. Strict Sanitizer: Fails early, no dangerous fallbacks.
fn sanitize_code(raw: &str) -> Option<String> {
    let re = Regex::new(r"(?si)\[CODE_START\](.*?)\[CODE_END\]").ok()?;
    let captures = re.captures(raw)?;
    let mut code = captures.get(1)?.as_str().trim().to_string();

    // Remove rogue markdown fences if any nested inside tags
    code = code.replace("
```rust", "");
    code = code.replace("```", "");
    code = code.replace("\r\n", "\n");

    if code.trim().is_empty() {
        return None;
    }
    Some(code.trim().to_string())
}

/// 🚀 2. Robust Client with internal retry logic for Unstable Local LLMs
async fn generate_with_retry(client: &Client, prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
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

/// 🔍 3. Error Extractor: Deletes unicode garbage, filters only core rustc notes
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

/// 🔄 4. Stateless Repair Prompt Builder: Prevents infinite toxic context bloating
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

/// 🏃‍♂️ 5. Execution Sandbox: Validates behavioral correctness beyond just compilation
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

/// 🔒 6. Security Guardrail
fn is_safe(code: &str) -> bool {
    !BLOCKED_PATTERNS.iter().any(|pattern| code.contains(pattern))
}
