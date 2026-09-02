use super::csr_support::encode_csr_adjacency;
use super::*;
use tempfile::TempDir;

#[test]
fn test_bfs_uses_csr_for_outgoing_unfiltered() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("csr_bfs.graph");
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
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: n3,
            to: n4,
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
            0_i64,
            n1,
            encode_csr_adjacency(&[(n3 as u32, 0.7, 0), (n2 as u32, 0.2, 0)]),
            manual_version as i64
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?4)",
        rusqlite::params![
            0_i64,
            n3,
            encode_csr_adjacency(&[(n4 as u32, 0.9, 0)]),
            manual_version as i64
        ],
    )
    .unwrap();
    drop(conn);
    backend.set_graph_version(manual_version);

    let bfs = backend.bfs(SnapshotId::current(), n1, 2).unwrap();
    assert_eq!(bfs, vec![n1, n3, n2, n4]);
}

#[test]
fn test_shortest_path_uses_csr_for_outgoing_unfiltered() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("csr_shortest_path.graph");
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
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: n3,
            to: n4,
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: n2,
            to: n4,
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
            0_i64,
            n1,
            encode_csr_adjacency(&[(n3 as u32, 0.9, 0), (n2 as u32, 0.2, 0)]),
            manual_version as i64
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?4)",
        rusqlite::params![
            0_i64,
            n3,
            encode_csr_adjacency(&[(n4 as u32, 0.8, 0)]),
            manual_version as i64
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?4)",
        rusqlite::params![
            0_i64,
            n2,
            encode_csr_adjacency(&[(n4 as u32, 0.1, 0)]),
            manual_version as i64
        ],
    )
    .unwrap();
    drop(conn);
    backend.set_graph_version(manual_version);

    let path = backend
        .shortest_path(SnapshotId::current(), n1, n4)
        .unwrap()
        .unwrap();
    assert_eq!(path, vec![n1, n3, n4]);
}

#[test]
fn test_k_hop_uses_csr_for_outgoing_unfiltered() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("csr_k_hop.graph");
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
            edge_type: "CALLS".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();
    backend
        .insert_edge(EdgeSpec {
            from: n3,
            to: n4,
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
            0_i64,
            n1,
            encode_csr_adjacency(&[(n3 as u32, 0.7, 0), (n2 as u32, 0.2, 0)]),
            manual_version as i64
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?4)",
        rusqlite::params![
            0_i64,
            n3,
            encode_csr_adjacency(&[(n4 as u32, 0.9, 0)]),
            manual_version as i64
        ],
    )
    .unwrap();
    drop(conn);
    backend.set_graph_version(manual_version);

    let hop = backend
        .k_hop(SnapshotId::current(), n1, 2, BackendDirection::Outgoing)
        .unwrap();
    assert_eq!(hop, vec![n3, n2, n4]);
}

#[test]
fn test_node_degree_uses_csr_for_outgoing_unfiltered() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("csr_node_degree.graph");
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
            from: n3,
            to: n1,
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
            0_i64,
            n1,
            encode_csr_adjacency(&[(n2 as u32, 0.7, 0), (n3 as u32, 0.3, 0)]),
            manual_version as i64
        ],
    )
    .unwrap();
    drop(conn);
    backend.set_graph_version(manual_version);

    let degree = backend.node_degree(SnapshotId::current(), n1).unwrap();
    assert_eq!(degree, (2, 1));
}
