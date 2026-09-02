//! Debug test to understand scenario C overflow
//!
//! Run with: cargo test --features native-v3 debug_scenario_c -- --nocapture

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::native::v3::V3Backend;
use tempfile::TempDir;

#[test]
fn debug_scenario_c() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("debug_scenario_c.db");

    println!("\n=== Debug Scenario C ===\n");

    let backend = V3Backend::create(&db_path).unwrap();

    // Exact scenario C data
    let kind_list = vec![
        "Function",
        "Struct",
        "Enum",
        "Trait",
        "Impl",
        "Module",
        "Variable",
        "Parameter",
        "Return",
        "Field",
        "Method",
        "Class",
        "Interface",
        "Package",
        "Import",
        "Export",
        "Type",
        "Const",
        "Static",
        "Macro",
    ];

    let name_patterns = vec![
        "process_data_",
        "handle_",
        "validate_",
        "parse_",
        "format_",
        "encode_",
        "decode_",
        "transform_",
        "compute_",
        "calculate_",
        "retrieve_",
        "store_",
        "fetch_",
        "query_",
        "update_",
        "delete_",
    ];

    // Insert first 1000 nodes with flush every 100
    for batch in 0..10 {
        for i in 0..100 {
            let idx = batch * 100 + i;
            let kind = kind_list[idx % kind_list.len()].to_string();
            let name_prefix = name_patterns[idx % name_patterns.len()];
            let name = format!("{}{}", name_prefix, idx);
            let data = serde_json::json!({
                "index": idx,
                "kind_index": idx % kind_list.len(),
                "name_index": idx % name_patterns.len(),
            });

            let total_len = 2 + kind.len() + name.len() + data.to_string().len();
            let is_external = total_len > 64;

            match backend.insert_node(sqlitegraph::backend::NodeSpec {
                kind: kind.clone(),
                name: name.clone(),
                file_path: None,
                data: data.clone(),
            }) {
                Ok(id) => {
                    if idx % 100 == 0 {
                        println!(
                            "Node {} (id={}): kind={}, name={}, total_len={}, external={}, json_len={}",
                            idx,
                            id,
                            kind.len(),
                            name.len(),
                            total_len,
                            is_external,
                            data.to_string().len()
                        );
                    }
                }
                Err(e) => {
                    println!("  ❌ FAILED at node {}: {:?}", idx, e);
                    panic!("Failed at node {}", idx);
                }
            }
        }

        println!("  Flush after batch {}", batch);
        backend.flush().unwrap();
    }

    println!("\n✓ All 1000 nodes inserted successfully");

    // Try to reopen
    drop(backend);
    println!("Reopening database...");
    let _backend2 = V3Backend::open(&db_path).unwrap();
    println!("✓ Database reopened successfully");

    println!("\n✓ Debug test passed!");
}
