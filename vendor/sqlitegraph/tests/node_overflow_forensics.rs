//! Minimal reproduction for node page overflow bug
//!
//! Run with: cargo test --features native-v3 node_overflow_forensics -- --nocapture

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::native::v3::V3Backend;
use tempfile::TempDir;

#[test]
fn node_overflow_forensics() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("overflow_test.db");

    println!("\n=== Node Overflow Forensics ===\n");

    let backend = V3Backend::create(&db_path).unwrap();

    // Scenario: Insert nodes matching Scenario C from regression sweep
    // This uses longer name patterns and varied kinds to trigger overflow

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

    for i in 0..100 {
        let kind = kind_list[i % kind_list.len()].to_string();
        let name_prefix = name_patterns[i % name_patterns.len()];
        let name = format!("{}{}", name_prefix, i);
        let data = serde_json::json!({
            "index": i,
            "kind_index": i % kind_list.len(),
            "name_index": i % name_patterns.len(),
        });

        println!(
            "Inserting node {}: kind='{}' ({} bytes), name='{}' ({} bytes), JSON size ~{} bytes",
            i,
            kind,
            kind.len(),
            name,
            name.len(),
            data.to_string().len()
        );

        match backend.insert_node(sqlitegraph::backend::NodeSpec {
            kind: kind.clone(),
            name: name.clone(),
            file_path: None,
            data: data.clone(),
        }) {
            Ok(id) => println!("  ✓ Inserted as node_id={}", id),
            Err(e) => {
                println!("  ❌ FAILED at node {}: {:?}", i, e);
                panic!("Node insertion failed at {}", i);
            }
        }

        if (i + 1) % 10 == 0 {
            println!("  Flushing...");
            backend.flush().unwrap();
        }
    }

    println!("\n=== All 100 nodes inserted successfully ===\n");

    // Try to reopen
    drop(backend);
    println!("Reopening database...");
    let backend2 = V3Backend::open(&db_path).unwrap();
    println!("✓ Database reopened successfully");

    // Verify a few nodes
    use sqlitegraph::snapshot::SnapshotId;
    for i in [0, 50, 99] {
        let node = backend2
            .get_node(SnapshotId::current(), (i + 1) as i64)
            .unwrap();
        println!("Node {}: kind={}, name={}", i, node.kind, node.name);
    }

    println!("\n✓ Forensics test passed!");
}
