//! Stage-by-stage timing decomposition of V3Backend::open()
//!
//! This instrumented version measures each stage of open() separately
//! to identify the dominant cost center.

use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::NodeSpec;
use sqlitegraph::backend::native::v3::V3Backend;
use std::io::Write;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== V3 OPEN() STAGE TIMING DECOMPOSITION ===\n");

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

        // Check index file size for context
        let index_path = db_path.with_extension("v3index");
        if index_path.exists() {
            let metadata = std::fs::metadata(&index_path)?;
            println!("Index file size: {:.2} KB", metadata.len() as f64 / 1024.0);
        }

        // STAGE-BY-STAGE OPEN TIMING
        println!("\nOpen() stage breakdown:");

        // STAGE A: File open / header read
        let stage_a_start = Instant::now();
        let mut _file = std::fs::File::open(&db_path)?;
        let stage_a = stage_a_start.elapsed();
        println!(
            "  A. File::open()              {:.2} µs",
            stage_a.as_secs_f64() * 1_000_000.0
        );

        // STAGE B: Header bytes read
        let stage_b_start = Instant::now();
        use sqlitegraph::backend::native::v3::V3_HEADER_SIZE;
        let mut _header_bytes = vec![0u8; V3_HEADER_SIZE as usize];
        // Read from the already-open file for timing
        std::io::Read::read_exact(&mut _file, &mut _header_bytes)?;
        let stage_b = stage_b_start.elapsed();
        println!(
            "  B. Read header bytes         {:.2} µs",
            stage_b.as_secs_f64() * 1_000_000.0
        );

        // STAGE C: Header parse
        let stage_c_start = Instant::now();
        let _header = sqlitegraph::backend::native::v3::header::PersistentHeaderV3::from_bytes(
            &_header_bytes,
        )?;
        let stage_c = stage_c_start.elapsed();
        println!(
            "  C. Parse header              {:.2} µs",
            stage_c.as_secs_f64() * 1_000_000.0
        );

        // STAGE D: Full open (includes all other stages)
        let stage_d_start = Instant::now();
        let _backend = V3Backend::open(&db_path)?;
        let stage_d = stage_d_start.elapsed();
        println!(
            "  D. V3Backend::open() TOTAL  {:.2} ms",
            stage_d.as_secs_f64() * 1000.0
        );

        println!("  Stage D breakdown needed (requires instrumentation in backend.rs)");
        println!();
    }

    Ok(())
}
