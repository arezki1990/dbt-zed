//! Reopen Corruption Reproduction
//!
//! Reproduces the "used_bytes exceeds page boundary" corruption bug.

use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::backend::{EdgeSpec, GraphBackend, NodeSpec};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = "/tmp/v3_corruption_test.db";
    let _ = std::fs::remove_file(db_path); // Clean start

    println!("\n=== Creating database with 10K nodes, 50K edges ===\n");

    // Create and populate
    {
        let backend = V3Backend::create(db_path)?;

        // Insert 10K nodes
        println!("Inserting 10K nodes...");
        for i in 0..10_000 {
            backend.insert_node(NodeSpec {
                kind: "TestNode".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })?;
        }

        // Insert 50K edges
        println!("Inserting 50K edges...");
        for i in 0..50_000 {
            let from = (i % 10_000) as i64 + 1;
            let to = ((i * 7) % 10_000) as i64 + 1;
            if from != to {
                backend.insert_edge(EdgeSpec {
                    from,
                    to,
                    edge_type: "TestEdge".to_string(),
                    data: serde_json::json!({}),
                })?;
            }
        }

        println!("Flushing...");
        backend.flush()?;
        println!("Database created successfully");
    }

    println!("\n=== Attempting to reopen ===\n");

    // Try to reopen
    match V3Backend::open(db_path) {
        Ok(backend) => {
            println!("SUCCESS: Database reopened without corruption");

            // Try to read some data
            println!("Verifying data integrity...");
            match backend.get_node(sqlitegraph::SnapshotId::current(), 1) {
                Ok(node) => println!("Node 1: kind={}, name={}", node.kind, node.name),
                Err(e) => println!("ERROR reading node 1: {}", e),
            }
        }
        Err(e) => {
            println!("ERROR: Reopen failed with: {}", e);

            // Try to inspect the file
            if let Ok(contents) = std::fs::read(db_path) {
                println!(
                    "\nFile size: {} bytes ({} pages)",
                    contents.len(),
                    contents.len() / 4096
                );

                // Check first page header
                if contents.len() >= 4096 {
                    let first_page = &contents[0..4096];
                    println!("\nFirst page header (first 32 bytes):");
                    for i in (0..32).step_by(4) {
                        let end = (i + 4).min(32);
                        let bytes = &first_page[i..end];
                        let vals: Vec<u8> = bytes.to_vec();
                        println!("  [{}..{}]: {:?}", i, end, vals);
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
                            println!(
                                "  Page {}: used_bytes = {} (0x{:04x})",
                                page_idx, used_bytes, used_bytes
                            );
                        }
                    } else {
                        println!("No corrupted pages detected in used_bytes field");
                    }
                }
            }
        }
    }

    Ok(())
}
