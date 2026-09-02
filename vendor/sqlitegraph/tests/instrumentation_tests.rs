use serde_json::json;
use sqlitegraph::{
    GraphEdgeCreate, GraphEntity, GraphEntityCreate, SqliteGraph, SqliteGraphError,
    bulk_insert_edges, bulk_insert_entities,
};

fn make_entity(name: &str) -> GraphEntity {
    GraphEntity {
        id: 0,
        kind: "unit".to_string(),
        name: name.to_string(),
        file_path: None,
        data: json!({}),
    }
}

fn make_entity_create(name: &str) -> GraphEntityCreate {
    GraphEntityCreate {
        kind: "unit".to_string(),
        name: name.to_string(),
        file_path: None,
        data: json!({}),
    }
}

#[test]
fn test_prepare_counter_increments() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;
    graph.reset_metrics();
    let before = graph.metrics_snapshot();

    graph.list_entity_ids()?;

    let after = graph.metrics_snapshot();
    assert_eq!(after.prepare_count - before.prepare_count, 1);
    assert_eq!(after.prepare_cache_misses - before.prepare_cache_misses, 1);
    assert_eq!(after.execute_count - before.execute_count, 1);
    Ok(())
}

#[test]
fn test_execute_counter_increments() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;
    graph.reset_metrics();
    let before = graph.metrics_snapshot();

    let entity = make_entity("execute_counter");
    graph.insert_entity(&entity)?;

    let after = graph.metrics_snapshot();
    // insert_entity issues two `execute()` calls: one to insert the row
    // into `graph_entities`, one to auto-register the kind as a label in
    // `graph_labels` so subsequent `match_triples` label-filtered edge
    // patterns find the node. The label INSERT is `OR IGNORE` and is
    // skipped only when `kind` is empty (which is not the case here).
    assert_eq!(before.execute_count + 2, after.execute_count);
    assert_eq!(before.prepare_count, after.prepare_count);
    Ok(())
}

#[test]
fn test_tx_counters() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;
    graph.reset_metrics();

    let before = graph.metrics_snapshot();
    bulk_insert_entities(&graph, &[make_entity_create("tx_ok")])?;
    let after_commit = graph.metrics_snapshot();
    assert_eq!(before.tx_begin_count + 1, after_commit.tx_begin_count);
    assert_eq!(before.tx_commit_count + 1, after_commit.tx_commit_count);
    assert_eq!(before.tx_rollback_count, after_commit.tx_rollback_count);

    let failure = bulk_insert_edges(
        &graph,
        &[GraphEdgeCreate {
            from_id: 9999,
            to_id: 10000,
            edge_type: "nope".to_string(),
            data: json!({}),
        }],
    );
    assert!(failure.is_err());

    let after_rollback = graph.metrics_snapshot();
    assert_eq!(
        after_commit.tx_begin_count + 1,
        after_rollback.tx_begin_count
    );
    assert_eq!(after_commit.tx_commit_count, after_rollback.tx_commit_count);
    assert_eq!(
        after_commit.tx_rollback_count + 1,
        after_rollback.tx_rollback_count
    );
    Ok(())
}

#[test]
fn test_metrics_reset() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;
    let entity = make_entity("reset_target");
    graph.insert_entity(&entity)?;
    let pre_reset = graph.metrics_snapshot();
    assert!(pre_reset.execute_count > 0 || pre_reset.prepare_count > 0);

    graph.reset_metrics();
    let after_reset = graph.metrics_snapshot();
    assert_eq!(0, after_reset.prepare_count);
    assert_eq!(0, after_reset.execute_count);
    assert_eq!(0, after_reset.tx_begin_count);
    assert_eq!(0, after_reset.tx_commit_count);
    assert_eq!(0, after_reset.tx_rollback_count);
    assert_eq!(0, after_reset.prepare_cache_hits);
    assert_eq!(0, after_reset.prepare_cache_misses);
    Ok(())
}

#[test]
fn test_metrics_snapshot_is_stable() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;
    graph.reset_metrics();

    let first = graph.metrics_snapshot();
    let second = graph.metrics_snapshot();
    assert_eq!(first, second);

    graph.list_entity_ids()?;
    let third = graph.metrics_snapshot();
    assert_ne!(second, third);
    Ok(())
}

#[test]
fn test_bulk_insert_metrics_regression() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;
    graph.reset_metrics();

    bulk_insert_entities(
        &graph,
        &[make_entity_create("reg_a"), make_entity_create("reg_b")],
    )?;

    let snapshot = graph.metrics_snapshot();
    assert_eq!(1, snapshot.prepare_count);
    assert_eq!(4, snapshot.execute_count);
    assert_eq!(1, snapshot.tx_begin_count);
    assert_eq!(1, snapshot.tx_commit_count);
    assert_eq!(0, snapshot.tx_rollback_count);
    Ok(())
}

#[test]
fn test_cached_prepares_register_hits() -> Result<(), SqliteGraphError> {
    let graph = SqliteGraph::open_in_memory()?;
    graph.reset_metrics();

    graph.list_entity_ids()?;
    graph.list_entity_ids()?;

    let snapshot = graph.metrics_snapshot();
    assert_eq!(1, snapshot.prepare_count);
    assert_eq!(1, snapshot.prepare_cache_misses);
    assert_eq!(1, snapshot.prepare_cache_hits);
    Ok(())
}
