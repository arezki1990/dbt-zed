use super::*;
use tempfile::TempDir;

#[test]
fn test_v3_snapshot_import() {
    use std::io::Write;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    let snapshot_path = temp_dir.path().join("snapshot.json");
    let mut file = std::fs::File::create(&snapshot_path).unwrap();
    writeln!(
        file,
        r#"{{"type":"entity","id":1,"kind":"Class","name":"NodeA","file_path":null,"data":{{"score":42}}}}"#
    )
    .unwrap();
    writeln!(
        file,
        r#"{{"type":"entity","id":2,"kind":"Class","name":"NodeB","file_path":"src/b.rs","data":{{"score":7}}}}"#
    )
    .unwrap();
    writeln!(
        file,
        r#"{{"type":"entity","id":3,"kind":"Method","name":"NodeC","file_path":null,"data":{{}}}}"#
    )
    .unwrap();
    writeln!(
        file,
        r#"{{"type":"edge","id":10,"from_id":1,"to_id":2,"edge_type":"calls","data":{{"weight":2.0}}}}"#
    )
    .unwrap();
    writeln!(
        file,
        r#"{{"type":"edge","id":11,"from_id":2,"to_id":3,"edge_type":"defines","data":{{}}}}"#
    )
    .unwrap();
    writeln!(file, r#"{{"type":"label","entity_id":1,"label":"public"}}"#).unwrap();
    writeln!(
        file,
        r#"{{"type":"property","entity_id":1,"key":"lang","value":"\"rust\""}}"#
    )
    .unwrap();
    drop(file);

    let meta = backend.snapshot_import(temp_dir.path()).unwrap();
    assert_eq!(meta.entities_imported, 3, "three entities should import");
    assert_eq!(meta.edges_imported, 2, "two edges should import");

    let header = backend.header.read();
    assert_eq!(header.node_count, 3, "header node_count should be 3");
    assert_eq!(header.edge_count, 2, "header edge_count should be 2");
    drop(header);

    let ids = backend.entity_ids().unwrap();
    assert_eq!(ids.len(), 3, "entity_ids should report 3 nodes");

    use crate::snapshot::SnapshotId;
    let node = backend.get_node(SnapshotId::current(), ids[0]).unwrap();
    assert_eq!(node.kind, "Class");
    assert_eq!(node.name, "NodeA");
    assert_eq!(node.data.get("score").and_then(|v| v.as_i64()), Some(42));
    assert_eq!(
        node.data
            .get("_labels")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(1),
        "merged label should be present in data"
    );
    assert_eq!(
        node.data.get("lang").and_then(|v| v.as_str()),
        Some("rust"),
        "merged property should be present in data"
    );
}

#[test]
fn test_v3_snapshot_import_missing_file() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let backend = V3Backend::create(&db_path).unwrap();
    let result = backend.snapshot_import(temp_dir.path());
    assert!(result.is_err(), "import without snapshot.json should fail");
}

#[test]
fn test_snapshot_creation() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    let snapshot_name = "snapshot_v1";
    let lsn_v1 = backend.create_snapshot(snapshot_name).unwrap();
    assert!(lsn_v1 > 0, "Snapshot LSN should be positive");

    backend
        .insert_node(NodeSpec {
            kind: "Agent".to_string(),
            name: "agent1".to_string(),
            file_path: None,
            data: serde_json::json!({"version": 1}),
        })
        .unwrap();

    let snapshot_name_v2 = "snapshot_v2";
    let lsn_v2 = backend.create_snapshot(snapshot_name_v2).unwrap();
    assert!(lsn_v2 > lsn_v1, "Snapshot LSNs should increment");

    backend
        .insert_node(NodeSpec {
            kind: "Agent".to_string(),
            name: "agent2".to_string(),
            file_path: None,
            data: serde_json::json!({"version": 2}),
        })
        .unwrap();

    let retrieved_lsn = backend.get_snapshot_lsn(snapshot_name);
    assert_eq!(retrieved_lsn, Some(lsn_v1), "Should retrieve stored LSN");

    let retrieved_lsn_v2 = backend.get_snapshot_lsn(snapshot_name_v2);
    assert_eq!(retrieved_lsn_v2, Some(lsn_v2), "Should retrieve second LSN");

    let snapshots = backend.list_snapshots();
    assert_eq!(snapshots.len(), 2, "Should have 2 snapshots");
    assert!(
        snapshots
            .iter()
            .any(|(name, lsn)| name == snapshot_name && *lsn == lsn_v1)
    );

    backend.delete_snapshot(snapshot_name).unwrap();
    let snapshots_after_delete = backend.list_snapshots();
    assert_eq!(
        snapshots_after_delete.len(),
        1,
        "Should have 1 snapshot after delete"
    );
    assert!(
        !snapshots_after_delete
            .iter()
            .any(|(name, _)| name == snapshot_name)
    );

    let result = backend.delete_snapshot("nonexistent");
    assert!(
        result.is_err(),
        "Deleting non-existent snapshot should fail"
    );
}

#[test]
fn test_snapshot_version_increments() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.graph");
    let backend = V3Backend::create(&db_path).unwrap();

    let version_0 = backend.current_snapshot_version();
    assert_eq!(version_0, 1, "Initial version should be 1");

    backend
        .insert_node(NodeSpec {
            kind: "Node".to_string(),
            name: "node1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let version_1 = backend.current_snapshot_version();
    assert_eq!(version_1, 2, "Version should increment after insert");

    let node_id = backend.entity_ids().unwrap()[0];
    backend
        .update_node(
            node_id,
            NodeSpec {
                kind: "Node".to_string(),
                name: "node1".to_string(),
                file_path: None,
                data: serde_json::json!({"updated": true}),
            },
        )
        .unwrap();

    let version_2 = backend.current_snapshot_version();
    assert_eq!(version_2, 3, "Version should increment after update");

    backend.delete_entity(node_id).unwrap();

    let version_3 = backend.current_snapshot_version();
    assert_eq!(version_3, 4, "Version should increment after delete");
}

#[test]
fn test_current_snapshot_validation() {
    use crate::SnapshotId;

    let current = SnapshotId::current();
    assert!(V3Backend::require_current_snapshot(current).is_ok());

    let historical = SnapshotId::from_lsn(12345);
    assert!(V3Backend::require_current_snapshot(historical).is_ok());

    let invalid = SnapshotId::invalid();
    assert!(V3Backend::require_current_snapshot(invalid).is_err());
}
