pub mod ast_graph;

#[cfg(test)]
mod tests {
    use super::ast_graph::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn test_graph_extraction() {
        // Ek dummy Rust code banate hain test karne ke liye
        let test_code = "
            struct Database { url: String }
            
            fn connect_db() {
                log_status();
                parse_url();
            }

            fn log_status() {
                // logs something
            }
        ";

        // Temporary file create karte hain
        let file_path = Path::new("dummy_test.rs");
        fs::write(file_path, test_code).unwrap();

        // Parser chalate hain
        let graph = build_ast_graph(file_path);
        
        for node in &graph {
            println!("---");
            println!("Type: {}", node.entity_type);
            println!("Name: {}", node.entity_name);
            println!("Calls/Dependencies: {:?}", node.dependencies);
        }

        // Cleanup
        fs::remove_file(file_path).unwrap();
        
        assert_eq!(graph.len(), 3); // 1 struct + 2 functions
    }
}
