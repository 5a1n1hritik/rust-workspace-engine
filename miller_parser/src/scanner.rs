use walkdir::{DirEntry, WalkDir};
use std::path::Path;
use std::fs;
use std::collections::HashMap;
use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};

// Humare padosi file `ast_graph.rs` se build function aur struct ko import kar rahe hain
use crate::ast_graph::{build_ast_graph, CodeNode}; 

const CACHE_FILE_NAME: &str = ".miller_cache.json";

// Single file ka system snapshot cache state save karne ke liye
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    pub file_path: String,
    pub sha256_hash: String,
    pub last_modified: u64,
}

// Pure codebase ka database cache map
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodebaseCache {
    pub files: HashMap<String, FileState>,
}

// Helper function: Faltu aur heavy directories ko ignore karne ke liye filter logic
fn is_hidden_or_ignored(entry: &DirEntry) -> bool {
    // Guardrail: Root path (depth 0) ko kabhi ignore nahi karna hai, chahe wo "." ya ".." ho
    if entry.depth() == 0 {
        return false;
    }
    let file_name = entry.file_name().to_str().unwrap_or("");
    // Hidden files (.git, .vscode) aur build targets ko scan nahi karna hai
    file_name.starts_with('.') || file_name == "target" || file_name == "node_modules"
}

// SHA-256 string signature hash generate karne wala logic
fn calculate_sha256(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

// Purani cache file ko disk se load karne ka kaam
fn load_cache(root_path: &Path) -> CodebaseCache {
    let cache_path = root_path.join(CACHE_FILE_NAME);
    if cache_path.exists() {
        if let Ok(content) = fs::read_to_string(&cache_path) {
            if let Ok(cache) = serde_json::from_str::<CodebaseCache>(&content) {
                return cache;
            }
        }
    }
    CodebaseCache::default()
}

// Nayi updated cache configuration database disk par flush karna
fn save_cache(root_path: &Path, cache: &CodebaseCache) {
    let cache_path = root_path.join(CACHE_FILE_NAME);
    if let Ok(content) = serde_json::to_string_pretty(cache) {
        let _ = fs::write(cache_path, content);
    }
}

// Pure folder tree ko recursive scan karke saare structs aur functions nikalna
pub fn scan_and_parse_project_incremental(root_path: &Path) -> (Vec<CodeNode>, usize) {
    let mut changed_nodes = Vec::new();
    let mut total_files_checked = 0;
    let mut files_skipped = 0;

    // let mut global_project_nodes = Vec::new();

    // println!("[Miller Parser] Initiating deep scan for directory: {:?}", root_path);

    // // WalkDir initialization with ignore filter
    // let walker = WalkDir::new(root_path)
    //     .into_iter()
    //     .filter_entry(|e| !is_hidden_or_ignored(e));

    // for entry in walker.filter_map(|e| e.ok()) {
    //     let path = entry.path();
        
    //     // Abhi ke liye hum sirf Rust (.rs) files ko filter kar rahe hain.
	// // Future me Python/TS ke liye yahan `&&` condition extend kar sakte hain.
    //     if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("rs") {
    //         // Hamari main scanner binary ya temporary files ko skip karo optimization ke liye
    //         if path.file_name().and_then(|s| s.to_str()) == Some("sandbox.rs") {
    //             continue;
    //         }

    //         println!("[Miller Parser] Extracting AST from: {:?}", path);
    //         let mut file_nodes = build_ast_graph(path);
    //         global_project_nodes.append(&mut file_nodes);
    //     }
    // }

    // println!("[Miller Parser] Scan Complete. Total Entities Extracted: {}", global_project_nodes.len());
    // global_project_nodes


    println!("[Miller Parser] Incremental background indexing scanner chalu ho raha hai...");

    let current_cache = load_cache(root_path);
    let mut updated_cache = CodebaseCache::default();

    let walker = WalkDir::new(root_path)
        .into_iter()
        .filter_entry(|e| !is_hidden_or_ignored(e));

    for entry in walker.filter_map(|e| e.ok()) {
        let path = entry.path();
        
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("rs") {
            let file_name = path.file_name().and_then(|s| s.to_str());
            
            // Core execution engine variables ko data indexing se safe door rakho
            if file_name == Some("sandbox.rs") || file_name == Some("sandbox_exec") {
                continue;
            }

            total_files_checked += 1;
            let file_path_str = path.to_string_lossy().into_owned();

            if let Ok(metadata) = fs::metadata(path) {
                if let Ok(content) = fs::read_to_string(path) {
                    let modified_time = metadata.modified()
                        .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)))
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    
                    let current_hash = calculate_sha256(&content);

                    // 🔍 Core Delta check algorithm
                    let mut needs_reindex = true;
                    if let Some(cached_file) = current_cache.files.get(&file_path_str) {
                        if cached_file.sha256_hash == current_hash && cached_file.last_modified == modified_time {
                            needs_reindex = false; // File changed nahi hui, index drop skip karo!
                        }
                    }

                    if needs_reindex {
                        println!("[Miller Parser] 🔄 Nayi file ya badlao mila. Code extract ho raha hai: {:?}", path);
                        let mut file_nodes = build_ast_graph(path);
                        changed_nodes.append(&mut file_nodes);
                    } else {
                        files_skipped += 1;
                    }

                    updated_cache.files.insert(
                        file_path_str.clone(),
                        FileState {
                            file_path: file_path_str,
                            sha256_hash: current_hash,
                            last_modified: modified_time,
                        },
                    );
                }
            }
        }
    }

    save_cache(root_path, &updated_cache);

    println!(
        "[Miller Parser] Scan Done. Total Checked: {} | Skipped (Unchanged): {} | Extracted New Items: {}",
        total_files_checked, files_skipped, changed_nodes.len()
    );

    (changed_nodes, files_skipped)
}
