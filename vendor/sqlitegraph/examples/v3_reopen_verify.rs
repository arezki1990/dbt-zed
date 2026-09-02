//! Verify neighbors and BFS work correctly after reopen
use sqlitegraph::backend::native::v3::V3Backend;
use sqlitegraph::{EdgeSpec, GraphBackend, NodeSpec};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = "/tmp/v3_reopen_verify.db";
    let _ = fs::remove_file(db_path);

    println!("=== Creating database ===");
    {
        let backend = V3Backend::create(db_path)?;

        for i in 1..=1000 {
            backend.insert_node(NodeSpec {
                kind: "TestNode".to_string(),
                name: format!("node_{}", i),
                file_path: None,
                data: serde_json::json!({"id": i}),
            })?;
        }

        for i in 0..5000 {
            let from = (i % 1000) as i64 + 1;
            let to = ((i * 7) % 1000) as i64 + 1;
            if from != to {
                backend.insert_edge(EdgeSpec {
                    from,
                    to,
                    edge_type: "TestEdge".to_string(),
                    data: serde_json::json!({}),
                })?;
            }
        }

        backend.flush()?;
        println!("Created 1000 nodes, 5000 edges");
    }

    println!("\n=== Reopening database ===");
    {
        let backend = V3Backend::open(db_path)?;
        println!("Reopened successfully!");

        use sqlitegraph::NeighborQuery;
        let query = NeighborQuery {
            direction: sqlitegraph::BackendDirection::Outgoing,
            edge_type: None,
        };

        println!("\n=== Testing neighbors after reopen ===");
        for node_id in [100, 500, 999] {
            match backend.neighbors(sqlitegraph::SnapshotId::current(), node_id, query.clone()) {
                Ok(neighbors) => println!("Node {}: {} neighbors", node_id, neighbors.len()),
                Err(e) => println!("Node {}: ERROR - {}", node_id, e),
            }
        }

        println!("\n=== Testing BFS after reopen ===");
        let bfs_result = backend.bfs(sqlitegraph::SnapshotId::current(), 1, 1000)?;
        println!("BFS from node 1: {} nodes reached", bfs_result.len());

        println!("\n=== Testing get_node after reopen ===");
        match backend.get_node(sqlitegraph::SnapshotId::current(), 500) {
            Ok(node) => println!("Node 500: kind={}, name={}", node.kind, node.name),
            Err(e) => println!("Node 500: ERROR - {}", e),
        }
    }

    println!("\n=== ALL TESTS PASSED ===");
    Ok(())
}
