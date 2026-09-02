//! Minimal edge corruption test - 10 nodes, 20 edges

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::native::v3::V3Backend;
use tempfile::TempDir;

#[test]
fn edge_corruption_minimal() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("minimal.db");

    println!("\n=== Minimal Edge Corruption Test ===\n");

    // Create
    println!("Creating...");
    let backend = V3Backend::create(&db_path).unwrap();

    for i in 1..=10 {
        backend
            .insert_node(sqlitegraph::backend::NodeSpec {
                kind: "N".to_string(),
                name: format!("n{}", i),
                file_path: None,
                data: serde_json::json!({"i": i}),
            })
            .unwrap();
    }

    for src in 1..=10 {
        for j in 1..=2 {
            let dst = (src + j) % 10 + 1;
            backend
                .insert_edge(sqlitegraph::backend::EdgeSpec {
                    from: src,
                    to: dst,
                    edge_type: String::new(),
                    data: serde_json::json!({}),
                })
                .unwrap();
        }
    }

    println!("Flushing...");
    backend.flush().unwrap();
    println!("  ✓ 10 nodes + 20 edges");
    drop(backend);

    // Reopen
    println!("Reopening...");
    match V3Backend::open(&db_path) {
        Ok(_) => println!("  ✓ Opened OK"),
        Err(e) => {
            println!("  ❌ Open failed: {:?}", e);
            panic!("Failed");
        }
    }

    println!("\n✅ Test passed!");
}
