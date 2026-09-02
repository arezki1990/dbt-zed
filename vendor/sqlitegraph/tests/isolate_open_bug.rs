//! Isolate the source of the 101 lookups bug

#[cfg(feature = "v3-forensics")]
use sqlitegraph::backend::native::v3::backend::V3Backend;
#[cfg(feature = "v3-forensics")]
use sqlitegraph::backend::native::v3::forensics::FORENSIC_COUNTERS;

#[test]
#[cfg(feature = "v3-forensics")]
fn test_isolate_open_vs_getnode() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("isolate.db");

    // Create DB with 100 nodes
    let backend = V3Backend::create(&db_path).unwrap();
    for i in 0..100 {
        backend
            .insert_node(NodeSpec {
                kind: format!("N{}", i),
                name: format!("n{}", i),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
    }
    backend.flush().unwrap();
    drop(backend);

    // Reset counters BEFORE open
    FORENSIC_COUNTERS.reset();

    println!("=== AFTER RESET, BEFORE OPEN ===");
    println!(
        "btree_lookup_calls: {}",
        FORENSIC_COUNTERS
            .btree_lookup_calls
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "node_decode_count: {}",
        FORENSIC_COUNTERS
            .node_decode_count
            .load(std::sync::atomic::Ordering::Relaxed)
    );

    // Open database
    let backend = V3Backend::open(&db_path).unwrap();

    println!("=== AFTER OPEN, BEFORE GET_NODE ===");
    println!(
        "btree_lookup_calls: {}",
        FORENSIC_COUNTERS
            .btree_lookup_calls
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "node_decode_count: {}",
        FORENSIC_COUNTERS
            .node_decode_count
            .load(std::sync::atomic::Ordering::Relaxed)
    );

    // Now do a single get_node
    let _ = backend.get_node(sqlitegraph::snapshot::SnapshotId::current(), 50);

    println!("=== AFTER GET_NODE ===");
    println!(
        "btree_lookup_calls: {}",
        FORENSIC_COUNTERS
            .btree_lookup_calls
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "node_decode_count: {}",
        FORENSIC_COUNTERS
            .node_decode_count
            .load(std::sync::atomic::Ordering::Relaxed)
    );
}
