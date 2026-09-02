//! Temporary test to capture the exact BufferTooSmall error details

use sqlitegraph::{EdgeSpec, GraphConfig, NodeSpec, SnapshotId, open_graph};

fn test_capture_buffer_too_small_error() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Capturing BufferTooSmall Error Details ===");

    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("buffer_error_test.db");

    // Step 1: Create database with stress conditions
    println!("STEP 1: Creating database...");
    let config = GraphConfig::native();
    let graph = open_graph(&db_path, &config)?;

    // Create fewer nodes to get to the error faster
    let mut node_ids = Vec::new();
    for i in 1..=20 {
        let node_id = graph.insert_node(NodeSpec {
            kind: "TestNode".to_string(),
            name: format!("node_{}", i),
            file_path: None,
            data: serde_json::json!({"index": i}),
        })?;
        node_ids.push(node_id);
    }
    println!("✅ Created {} nodes", node_ids.len());

    // Step 2: Create edges to trigger cluster growth
    println!("STEP 2: Creating edges...");
    for (i, &node_id) in node_ids.iter().enumerate() {
        for j in 1..=4 {
            let target_index = (i + j * 7) % node_ids.len();
            if target_index != i {
                graph.insert_edge(EdgeSpec {
                    from: node_id,
                    to: node_ids[target_index],
                    edge_type: format!("edge_type_{}", j),
                    data: serde_json::json!({"edge_index": j}),
                })?;
            }
        }
    }
    println!("✅ Created edges");

    // Step 3: Close database
    println!("STEP 3: Closing database...");
    drop(graph);

    // Step 4: Reopen database
    println!("STEP 4: Reopening database...");
    let mut reopen_config = GraphConfig::native();
    reopen_config.native.create_if_missing = false;

    let graph_reopened = open_graph(&db_path, &reopen_config)?;
    println!("✅ Database reopened");

    // Step 5: Try neighbor queries to trigger error
    println!("STEP 5: Testing neighbor queries...");
    for (i, &node_id) in node_ids.iter().enumerate() {
        match graph_reopened.neighbors(SnapshotId::current(), node_id, Default::default()) {
            Ok(neighbors) => {
                println!("✅ Node {}: {} neighbors", node_id, neighbors.len());
            }
            Err(e) => {
                println!("❌ ERROR on node {}: {}", node_id, e);

                // Check if this contains our target error
                let error_str = e.to_string();
                if error_str.contains("Buffer too small") {
                    println!("🎯 FOUND TARGET ERROR: {}", error_str);

                    // Try to extract size and min_size values
                    if let Some(size_str) = extract_error_value(&error_str, "size: ") {
                        println!("  Size: {}", size_str);
                    }
                    if let Some(min_size_str) = extract_error_value(&error_str, "min_size: ") {
                        println!("  Min Size: {}", min_size_str);
                    }

                    return Err(Box::new(e));
                }
            }
        }

        // Stop after first error to keep output manageable
        if i >= 5 {
            break;
        }
    }

    println!("=== No BufferTooSmall error found ===");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    test_capture_buffer_too_small_error()
}

fn extract_error_value<'a>(error_str: &'a str, prefix: &str) -> Option<&'a str> {
    let start = error_str.find(prefix)?;
    let after_prefix = &error_str[start + prefix.len()..];
    let end = after_prefix.find([',', ')'])?;
    Some(&after_prefix[..end])
}
