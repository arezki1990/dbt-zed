use super::*;
use crate::backend::{BackendDirection, EdgeSpec, GraphBackend, NeighborQuery, NodeSpec};

#[test]
fn test_edge_type_serialization_roundtrip() {
    let mut cluster = V3EdgeCluster::new(1, Direction::Outgoing, 100);
    cluster.add_edge(2, Some("TEST_TYPE".to_string()));

    assert_eq!(cluster.edges.len(), 1);
    let edge = &cluster.edges[0];
    assert!(
        !edge.edge_data.is_empty(),
        "edge_data should not be empty when edge_type is set"
    );

    let extracted = V3EdgeCluster::extract_edge_type(&edge.edge_data);
    assert_eq!(extracted, Some("TEST_TYPE".to_string()));

    let serialized = cluster.serialize().unwrap();
    let deserialized = V3EdgeCluster::deserialize(&serialized, 100).unwrap();

    assert_eq!(deserialized.edges.len(), 1);
    let deser_edge = &deserialized.edges[0];
    let deser_type = V3EdgeCluster::extract_edge_type(&deser_edge.edge_data);
    assert_eq!(
        deser_type,
        Some("TEST_TYPE".to_string()),
        "edge_type should survive serialization roundtrip"
    );
}

#[test]
fn test_edges_with_types_extraction() {
    let mut cluster = V3EdgeCluster::new(1, Direction::Outgoing, 100);
    cluster.add_edge(2, Some("CALLS".to_string()));
    cluster.add_edge(3, Some("USES".to_string()));
    cluster.add_edge(4, None);

    let edges_with_types = cluster.edges_with_types();
    assert_eq!(edges_with_types.len(), 3);
    assert_eq!(edges_with_types[0].0, 2);
    assert_eq!(edges_with_types[0].1, Some("CALLS".to_string()));
    assert_eq!(edges_with_types[1].0, 3);
    assert_eq!(edges_with_types[1].1, Some("USES".to_string()));
    assert_eq!(edges_with_types[2].0, 4);
    assert_eq!(edges_with_types[2].1, None);
}

#[test]
fn test_weighted_edges() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let backend = crate::backend::native::v3::V3Backend::create(&db_path).unwrap();

    backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "n1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "n2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let n1_id = backend.entity_ids().unwrap()[0];
    let n2_id = backend.entity_ids().unwrap()[1];

    backend
        .insert_edge(EdgeSpec {
            from: n1_id,
            to: n2_id,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({ "weight": 2.5 }),
        })
        .unwrap();

    let query = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: None,
    };
    let neighbors = backend
        .neighbors_weighted(crate::SnapshotId::current(), n1_id, query.clone())
        .unwrap();
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0].0, n2_id);
    assert_eq!(neighbors[0].1, 2.5);

    let query_filtered = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: Some("CALLS".to_string()),
    };
    let neighbors_filt = backend
        .neighbors_weighted(crate::SnapshotId::current(), n1_id, query_filtered)
        .unwrap();
    assert_eq!(neighbors_filt.len(), 1);
    assert_eq!(neighbors_filt[0].1, 2.5);

    backend
        .batch_insert_edges_with_weights(vec![(n2_id, n1_id, 4.2, Some("RETURNS".to_string()))])
        .unwrap();

    let query_incoming = NeighborQuery {
        direction: BackendDirection::Outgoing,
        edge_type: Some("RETURNS".to_string()),
    };
    let neighbors_incoming = backend
        .neighbors_weighted(crate::SnapshotId::current(), n2_id, query_incoming)
        .unwrap();
    assert_eq!(neighbors_incoming.len(), 1);
    assert_eq!(neighbors_incoming[0].0, n1_id);
    assert_eq!(neighbors_incoming[0].1, 4.2);
}

#[test]
fn test_weighted_neighbors_are_sorted_after_reopen() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("sorted_weighted_neighbors.graph");
    std::fs::write(&db_path, vec![0u8; 4096]).expect("Failed to create db file");

    {
        let (edge_store, _allocator) = create_test_edge_store(Some(db_path.clone()));
        edge_store
            .insert_edge_weighted(1, 11, Direction::Outgoing, None, 0.2)
            .expect("Insert failed");
        edge_store
            .insert_edge_weighted(1, 12, Direction::Outgoing, None, 0.9)
            .expect("Insert failed");
        edge_store
            .insert_edge_weighted(1, 13, Direction::Outgoing, None, 0.5)
            .expect("Insert failed");
        edge_store.flush(None).expect("Flush failed");
    }

    {
        let (recovered_store, _allocator) = create_test_edge_store(Some(db_path.clone()));
        let neighbors = recovered_store
            .neighbors_weighted(1, Direction::Outgoing)
            .expect("Failed to recover weighted neighbors");
        assert_eq!(neighbors.as_ref(), &[(12, 0.9), (13, 0.5), (11, 0.2)]);
    }
}

#[test]
fn test_bulk_warm_weighted_neighbors_populates_cache() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("warm_weighted_neighbors.graph");
    std::fs::write(&db_path, vec![0u8; 4096]).expect("Failed to create db file");

    {
        let (edge_store, _allocator) = create_test_edge_store(Some(db_path.clone()));
        for src in 1..=32 {
            edge_store
                .insert_edge_weighted(src, 1000 + src, Direction::Outgoing, None, 0.7)
                .expect("Insert failed");
        }
        edge_store.flush(None).expect("Flush failed");
    }

    {
        let (recovered_store, _allocator) = create_test_edge_store(Some(db_path.clone()));
        assert_eq!(recovered_store.cache_weighted.read().len(), 0);
        assert_eq!(recovered_store.cache.read().len(), 0);

        let warmed = recovered_store
            .warm_weighted_neighbors(&(1..=32).collect::<Vec<_>>(), Direction::Outgoing)
            .expect("warm failed");
        assert_eq!(warmed, 32);
        assert_eq!(recovered_store.cache_weighted.read().len(), 32);
        assert_eq!(recovered_store.cache.read().len(), 32);

        let before = recovered_store.cache_stats();
        let neighbors = recovered_store
            .neighbors_weighted(1, Direction::Outgoing)
            .expect("neighbors failed");
        let after = recovered_store.cache_stats();

        assert_eq!(neighbors.as_ref(), &[(1001, 0.7)]);
        assert_eq!(after.0, before.0 + 1, "warm cache should produce a hit");
        assert_eq!(after.1, before.1, "warm cache should avoid a miss");
    }
}

#[test]
fn test_large_edge_cluster_survives_flush_and_reopen() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("large_edge_cluster.graph");
    std::fs::write(&db_path, vec![0u8; 4096]).expect("Failed to create db file");

    {
        let (edge_store, _allocator) = create_test_edge_store(Some(db_path.clone()));
        for dst in 2..=2500 {
            edge_store
                .insert_edge_weighted(1, dst, Direction::Outgoing, Some("LINK".to_string()), 1.5)
                .expect("Insert failed");
        }
        edge_store.flush(None).expect("Flush failed");
    }

    {
        let (recovered_store, _allocator) = create_test_edge_store(Some(db_path.clone()));
        let neighbors = recovered_store
            .neighbors_weighted(1, Direction::Outgoing)
            .expect("Failed to recover weighted neighbors");
        assert_eq!(
            neighbors.len(),
            2499,
            "all oversized-cluster edges must survive reopen"
        );
        assert_eq!(neighbors[0], (2, 1.5));
        assert_eq!(neighbors[2498], (2500, 1.5));
    }
}

#[test]
fn test_small_clusters_are_packed_and_reopen_correctly() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("packed_small_clusters.graph");
    std::fs::write(&db_path, vec![0u8; 4096]).expect("Failed to create db file");

    {
        let (edge_store, _allocator) = create_test_edge_store(Some(db_path.clone()));
        for src in 1..=128 {
            edge_store
                .insert_edge_weighted(src, 10_000 + src, Direction::Outgoing, None, 0.5)
                .expect("Insert failed");
            edge_store
                .insert_edge_weighted(10_000 + src, src, Direction::Incoming, None, 0.5)
                .expect("Insert failed");
        }
        edge_store.flush(None).expect("Flush failed");
    }

    let file_size = std::fs::metadata(&db_path).expect("metadata").len();
    assert!(
        file_size < 65_536,
        "packed sparse clusters should stay well below one-page-per-cluster size, got {} bytes",
        file_size
    );

    {
        let (recovered_store, _allocator) = create_test_edge_store(Some(db_path.clone()));
        for src in 1..=128 {
            let outgoing = recovered_store
                .neighbors_weighted(src, Direction::Outgoing)
                .expect("Failed to recover outgoing neighbors");
            assert_eq!(outgoing.as_ref(), &[(10_000 + src, 0.5)]);

            let incoming = recovered_store
                .neighbors_weighted(10_000 + src, Direction::Incoming)
                .expect("Failed to recover incoming neighbors");
            assert_eq!(incoming.as_ref(), &[(src, 0.5)]);
        }
    }
}
