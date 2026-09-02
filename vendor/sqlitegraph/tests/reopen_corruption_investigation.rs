//! Reopen Corruption Investigation
//!
//! Reproduces the "used_bytes exceeds page boundary" corruption bug.

use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{EdgeSpec, GraphBackend, NodeSpec};
use tempfile::TempDir;

#[test]
#[ignore] // Run with: cargo test --features native-v3 reopen_corruption -- --nocapture
fn investigate_reopen_corruption() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("v3.db");

    println!("\n=== Creating database with 10K nodes, 50K edges ===\n");

    // Create and populate
    {
        let backend = V3Backend::create(&db_path).unwrap();

        // Insert 10K nodes
        println!("Inserting nodes...");
        for i in 0..10_000 {
            backend
                .insert_node(NodeSpec {
                    kind: "TestNode".to_string(),
                    name: format!("node_{}", i),
                    file_path: None,
                    data: serde_json::json!({"id": i}),
                })
                .unwrap();
        }

        // Insert 50K edges
        println!("Inserting edges...");
        for i in 0..50_000 {
            let from = (i % 10_000) as i64 + 1;
            let to = ((i * 7) % 10_000) as i64 + 1;
            if from != to {
                backend
                    .insert_edge(EdgeSpec {
                        from,
                        to,
                        edge_type: "TestEdge".to_string(),
                        data: serde_json::json!({}),
                    })
                    .unwrap();
            }
        }

        println!("Flushing...");
        backend.flush().unwrap();
        println!("Database created successfully");
    }

    println!("\n=== Attempting to reopen ===\n");

    // Try to reopen
    match V3Backend::open(&db_path) {
        Ok(_) => println!("SUCCESS: Database reopened without corruption"),
        Err(e) => {
            println!("ERROR: Reopen failed with: {}", e);

            // Try to inspect the file
            if let Ok(contents) = std::fs::read(&db_path) {
                println!("\nFile size: {} bytes", contents.len());

                // Check first page header
                if contents.len() >= 4096 {
                    let first_page = &contents[0..4096];
                    println!("First 32 bytes (header):");
                    for i in (0..32).step_by(4) {
                        let end = (i + 4).min(32);
                        println!("  [{}..{}]: {:?}", i, end, &first_page[i..end]);
                    }

                    // Check used_bytes field at offset 18-19
                    let used_bytes_bytes = &first_page[18..20];
                    let used_bytes = u16::from_be_bytes([used_bytes_bytes[0], used_bytes_bytes[1]]);
                    println!(
                        "\nused_bytes field (offset 18-19): {:?} = {}",
                        used_bytes_bytes, used_bytes
                    );

                    // Scan all pages for suspicious used_bytes values
                    println!("\nScanning for corrupted pages (used_bytes > 4000)...");
                    let page_count = contents.len() / 4096;
                    let mut corrupted_pages = Vec::new();

                    for page_idx in 0..page_count {
                        let offset = page_idx * 4096;
                        if offset + 32 <= contents.len() {
                            let page_bytes = &contents[offset..];
                            let used_bytes_bytes = &page_bytes[18..20];
                            let used_bytes =
                                u16::from_be_bytes([used_bytes_bytes[0], used_bytes_bytes[1]]);

                            if used_bytes > 4000 {
                                corrupted_pages.push((page_idx, used_bytes));
                            }
                        }
                    }

                    if !corrupted_pages.is_empty() {
                        println!(
                            "Found {} potentially corrupted pages:",
                            corrupted_pages.len()
                        );
                        for (page_idx, used_bytes) in corrupted_pages.iter().take(10) {
                            println!("  Page {}: used_bytes = {}", page_idx, used_bytes);
                        }
                    } else {
                        println!("No corrupted pages detected in used_bytes field");
                    }
                }
            }
        }
    }
}
