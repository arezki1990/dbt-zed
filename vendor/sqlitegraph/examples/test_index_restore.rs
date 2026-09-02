use sqlitegraph::backend::GraphBackend;
use sqlitegraph::backend::NodeSpec;
use sqlitegraph::backend::native::v3::V3Backend;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("test.db");

    println!("=== COLD PATH INDEX RESTORATION TEST ===");

    // Create database
    print!("Creating 1000 nodes... ");
    let backend = V3Backend::create(&db_path)?;
    for i in 0..1000 {
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

    // Check if index file exists
    let index_path = db_path.with_extension("v3index");
    println!("Index file exists: {}", index_path.exists());
    if index_path.exists() {
        let metadata = std::fs::metadata(&index_path)?;
        println!("Index file size: {} bytes", metadata.len());
    }

    // First open
    print!("First open... ");
    let start = Instant::now();
    let backend = V3Backend::open(&db_path)?;
    let first_open = start.elapsed();
    println!("{:.2} ms", first_open.as_secs_f64() * 1000.0);

    // Second open (should restore from sidecar)
    drop(backend);
    println!("Before second open, checking header...");
    let mut file = std::fs::File::open(&db_path)?;
    use std::io::Read;
    let mut header_bytes = vec![0u8; sqlitegraph::backend::native::v3::V3_HEADER_SIZE as usize];
    file.read_exact(&mut header_bytes)?;
    println!("  file exists: true");

    print!("Second open... ");
    let start = Instant::now();
    let _backend = V3Backend::open(&db_path)?;
    let second_open = start.elapsed();
    println!("{:.2} ms", second_open.as_secs_f64() * 1000.0);

    Ok(())
}
