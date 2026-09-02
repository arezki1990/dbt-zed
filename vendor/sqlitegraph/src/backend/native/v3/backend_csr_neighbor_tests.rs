use super::csr_support::encode_csr_adjacency;
use super::*;
use tempfile::TempDir;

#[test]
fn test_neighbors_shared_uses_csr_for_outgoing_unfiltered() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("csr_neighbors.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    let n1 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let n2 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let n3 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n3".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: n1,
            to: n2,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: n1,
            to: n3,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    let manual_version = backend.current_snapshot_version() + 1;
    let csr_blob = encode_csr_adjacency(&[(n3 as u32, 0.7, 0), (n2 as u32, 0.2, 0)]);
    let conn = backend.sqlite_conn.lock();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS csr_shards (
                shard_id INTEGER NOT NULL,
                node_id INTEGER NOT NULL,
                shard_data BLOB,
                version INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                visible_at INTEGER NOT NULL,
                PRIMARY KEY (shard_id, node_id, version)
            )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?4)",
        rusqlite::params![0_i64, n1, csr_blob, manual_version as i64],
    )
    .unwrap();
    drop(conn);
    backend.set_graph_version(manual_version);

    let neighbors = backend
        .neighbors_shared(
            SnapshotId::current(),
            n1,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(&*neighbors, &[n3, n2]);

    let weighted = backend
        .neighbors_weighted_shared(
            SnapshotId::current(),
            n1,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(&*weighted, &[(n3, 0.7), (n2, 0.2)]);
}

#[test]
fn test_neighbors_shared_keeps_filtered_queries_on_edge_store() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("csr_fallback.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    let n1 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let n2 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let n3 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n3".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: n1,
            to: n2,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: n1,
            to: n3,
            edge_type: "USES".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    let csr_blob = encode_csr_adjacency(&[(n3 as u32, 0.7, 0), (n2 as u32, 0.2, 0)]);
    let conn = backend.sqlite_conn.lock();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS csr_shards (
                shard_id INTEGER NOT NULL,
                node_id INTEGER NOT NULL,
                shard_data BLOB,
                version INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                visible_at INTEGER NOT NULL,
                PRIMARY KEY (shard_id, node_id, version)
            )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
                 VALUES (?1, ?2, ?3, 1, 0, 0)",
        rusqlite::params![0_i64, n1, csr_blob],
    )
    .unwrap();
    drop(conn);

    let filtered = backend
        .neighbors_shared(
            SnapshotId::current(),
            n1,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("CALLS".to_string()),
            },
        )
        .unwrap();
    assert_eq!(&*filtered, &[n2]);
}

#[test]
fn test_neighbors_shared_uses_csr_for_incoming_unfiltered() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("csr_incoming_neighbors.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    let n1 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let n2 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let n3 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n3".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: n1,
            to: n3,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: n2,
            to: n3,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    let manual_version = backend.current_snapshot_version() + 1;
    let conn = backend.sqlite_conn.lock();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS csr_shards (
                shard_id INTEGER NOT NULL,
                node_id INTEGER NOT NULL,
                shard_data BLOB,
                version INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                visible_at INTEGER NOT NULL,
                PRIMARY KEY (shard_id, node_id, version)
            )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?4)",
        rusqlite::params![
            1_i64,
            n3,
            encode_csr_adjacency(&[(n2 as u32, 0.9, 0), (n1 as u32, 0.4, 0)]),
            manual_version as i64
        ],
    )
    .unwrap();
    drop(conn);
    backend.set_graph_version(manual_version);

    let incoming = backend
        .neighbors_shared(
            SnapshotId::current(),
            n3,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(&*incoming, &[n2, n1]);
}

#[test]
fn test_neighbors_shared_uses_csr_for_typed_outgoing() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("csr_typed_neighbors.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    let n1 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let n2 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let n3 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n3".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();
    let n4 = backend
        .insert_node(NodeSpec {
            kind: "Fn".to_string(),
            name: "n4".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(EdgeSpec {
            from: n1,
            to: n2,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: n1,
            to: n3,
            edge_type: "USES".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: n1,
            to: n4,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    let manual_version = backend.current_snapshot_version() + 1;
    let conn = backend.sqlite_conn.lock();
    let calls_type_id: i64 = conn
        .query_row(
            "SELECT type_id FROM csr_edge_types WHERE edge_type = 'CALLS'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let uses_type_id: i64 = conn
        .query_row(
            "SELECT type_id FROM csr_edge_types WHERE edge_type = 'USES'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS csr_shards (
                shard_id INTEGER NOT NULL,
                node_id INTEGER NOT NULL,
                shard_data BLOB,
                version INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                visible_at INTEGER NOT NULL,
                PRIMARY KEY (shard_id, node_id, version)
            )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?4)",
        rusqlite::params![
            0_i64,
            n1,
            encode_csr_adjacency(&[
                (n4 as u32, 0.9, calls_type_id as u32),
                (n3 as u32, 0.7, uses_type_id as u32),
                (n2 as u32, 0.5, calls_type_id as u32),
            ]),
            manual_version as i64
        ],
    )
    .unwrap();
    drop(conn);
    backend.set_graph_version(manual_version);

    let typed = backend
        .neighbors_shared(
            SnapshotId::current(),
            n1,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: Some("CALLS".to_string()),
            },
        )
        .unwrap();
    assert_eq!(&*typed, &[n4, n2]);
}
