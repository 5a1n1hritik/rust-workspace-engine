use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tree_sitter::{Parser, Query, QueryCursor};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeNode {
    pub file_path: String,
    pub entity_name: String,
    pub entity_type: String, // "struct" ya "function"
    pub content: String,
    pub dependencies: Vec<String>, // Yeh function kis dusre function ko call kar raha hai (Graph Edges)
}

pub fn build_ast_graph(file_path: &Path) -> Vec<CodeNode> {
    let source_code = fs::read_to_string(file_path).expect("File read karne mein error aayi");
    let source_bytes = source_code.as_bytes();
    
    let mut parser = Parser::new();
    let language = tree_sitter_rust::language();
    parser.set_language(language).expect("Rust grammar load nahi hui");
    
    let tree = parser.parse(&source_code, None).expect("AST Tree parse nahi hua");
    let root_node = tree.root_node();

    // Query 1: Structs aur Functions ko extract karne ke liye
    let entity_query_str = "
        (function_item name: (identifier) @fn.name) @fn.body
        (struct_item name: (type_identifier) @struct.name) @struct.body
    ";
    
    let entity_query = Query::new(language, entity_query_str).unwrap();
    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&entity_query, root_node, source_bytes);

    let mut graph_nodes = Vec::new();

    for mat in matches {
        let mut entity_name = String::new();
        let mut content = String::new();
        let mut entity_type = "unknown";
        let mut target_node = None; // Dependency scan karne ke liye node save karenge

        for capture in mat.captures {
            let capture_name = entity_query.capture_names()[capture.index as usize].as_str();
            let text = capture.node.utf8_text(source_bytes).unwrap_or("").to_string();

            match capture_name {
                "fn.name" => {
                    entity_name = text;
                    entity_type = "function";
                }
                "fn.body" => {
                    content = text;
                    target_node = Some(capture.node);
                }
                "struct.name" => {
                    entity_name = text;
                    entity_type = "struct";
                }
                "struct.body" => {
                    content = text;
                    target_node = Some(capture.node);
                }
                _ => {}
            }
        }

        // Agar yeh node ek function hai, toh iske andar ki dependencies (Function calls) nikalte hain
        let mut dependencies = HashSet::new();
        if entity_type == "function" {
            if let Some(node) = target_node {
                // Query 2: Function ke andar call hone wale dusre functions dhoondhna
                let dep_query_str = "(call_expression function: (identifier) @call.name)";
                if let Ok(dep_query) = Query::new(language, dep_query_str) {
                    let mut dep_cursor = QueryCursor::new();
                    let dep_matches = dep_cursor.matches(&dep_query, node, source_bytes);
                    
                    for dep_mat in dep_matches {
                        for dep_cap in dep_mat.captures {
                            let dep_name = dep_cap.node.utf8_text(source_bytes).unwrap_or("").to_string();
                            dependencies.insert(dep_name);
                        }
                    }
                }
            }
        }

        graph_nodes.push(CodeNode {
            file_path: file_path.to_string_lossy().into_owned(),
            entity_name,
            entity_type: entity_type.to_string(),
            content,
            dependencies: dependencies.into_iter().collect(),
        });
    }

    graph_nodes
}
