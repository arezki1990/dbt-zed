//! Stage timing test that directly instruments open_with_cache_capacity
//!
//! This is a temporary instrumented version to measure open() stages.

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::NodeSpec;
use sqlitegraph::backend::native::v3::V3Backend;
use std::io::Write;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== V3 OPEN() DETAILED STAGE TIMING ===\n");

    // Test both small and medium datasets
    for (name, node_count) in [("small", 1_000), ("medium", 10_000)] {
        println!(
            "--- {} DATASET ({} nodes) ---",
            name.to_uppercase(),
            node_count
        );

        let temp_dir = tempfile::tempdir()?;
        let db_path = temp_dir.path().join("test.db");

        // Create database (not timed)
        print!("Creating database... ");
        let _ = std::io::stdout().flush();
        let backend = V3Backend::create(&db_path)?;
        for i in 0..node_count {
            backend.insert_node(NodeSpec {
                kind: "TestKind".to_string(),
                name: format!("node_{:05}", i),
                file_path: None,
                data: serde_json::json!({"value": i}),
            })?;
        }
        backend.flush_to_disk()?;
        drop(backend);
        println!("Done");

        // Run the instrumented open
        let open_total = Instant::now();
        let _backend = V3Backend::open(&db_path)?;
        let open_total = open_total.elapsed();

        println!(
            "Total open() time: {:.2} ms",
            open_total.as_secs_f64() * 1000.0
        );
        println!();
    }

    Ok(())
}
