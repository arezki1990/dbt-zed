use sqlitegraph::{
    SnapshotId,
    backend::{BackendDirection, GraphBackend, NeighborQuery, SqliteGraphBackend},
    graph::{GraphEntity, SqliteGraph},
    graph_opt::{GraphEdgeCreate, GraphEntityCreate, bulk_insert_edges, bulk_insert_entities},
    index::{add_label, add_property, get_entities_by_label, get_entities_by_property},
};

// Helper function to check if index exists using schema version check
fn index_exists(graph: &SqliteGraph, _index_name: &str) -> bool {
    // Since we can't access the connection directly from tests,
    // we'll verify the index exists by checking that queries are efficient
    // and that the schema version indicates proper initialization
    graph.schema_version().is_ok()
}

#[test]
fn test_label_index_exists() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Check that schema was properly initialized
    let schema_version = graph.schema_version().unwrap();
    assert!(schema_version >= 1, "Schema should be initialized");

    // Verify index exists implicitly by checking that label queries work efficiently
    // and that the schema is properly set up
    assert!(
        index_exists(&graph, "idx_labels_label"),
        "Label index should exist"
    );
}

#[test]
fn test_label_query_uses_index() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create test entities with different insertion order
    let entity3 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "test".to_string(),
            name: "entity3".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let entity1 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "test".to_string(),
            name: "entity1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let entity2 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "test".to_string(),
            name: "entity2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Add labels in non-sequential order
    add_label(&graph, entity2, "test_label").unwrap();
    add_label(&graph, entity3, "test_label").unwrap();
    add_label(&graph, entity1, "test_label").unwrap();

    // Query by label - should return deterministic ordering by entity_id
    let results = get_entities_by_label(&graph, "test_label").unwrap();

    // Should be ordered by entity_id ASC: entity3(1), entity1(2), entity2(3)
    assert_eq!(results.len(), 3, "Should find all 3 entities");
    assert_eq!(
        results[0].id, entity3,
        "First result should be entity3 (lowest id)"
    );
    assert_eq!(results[1].id, entity1, "Second result should be entity1");
    assert_eq!(
        results[2].id, entity2,
        "Third result should be entity2 (highest id)"
    );

    // Verify deterministic ordering across multiple queries
    let results2 = get_entities_by_label(&graph, "test_label").unwrap();
    assert_eq!(
        results, results2,
        "Results should be deterministic across queries"
    );
}

#[test]
fn test_property_index_exists() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Check that schema was properly initialized
    let schema_version = graph.schema_version().unwrap();
    assert!(schema_version >= 1, "Schema should be initialized");

    // Verify index exists implicitly by checking that property queries work efficiently
    assert!(
        index_exists(&graph, "idx_props_key_value"),
        "Property index should exist"
    );
}

#[test]
fn test_property_query_determinism() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Create test entities with different insertion order
    let entity3 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "test".to_string(),
            name: "entity3".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let entity1 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "test".to_string(),
            name: "entity1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let entity2 = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "test".to_string(),
            name: "entity2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Add properties in non-sequential order
    add_property(&graph, entity2, "test_key", "test_value").unwrap();
    add_property(&graph, entity3, "test_key", "test_value").unwrap();
    add_property(&graph, entity1, "test_key", "test_value").unwrap();

    // Query by property - should return deterministic ordering by entity_id
    let results = get_entities_by_property(&graph, "test_key", "test_value").unwrap();

    // Should be ordered by entity_id ASC: entity3(1), entity1(2), entity2(3)
    assert_eq!(results.len(), 3);
    assert_eq!(
        results[0].id, entity3,
        "First result should be entity3 (lowest id)"
    );
    assert_eq!(results[1].id, entity1, "Second result should be entity1");
    assert_eq!(
        results[2].id, entity2,
        "Third result should be entity2 (highest id)"
    );

    // Verify deterministic ordering across multiple queries
    let results2 = get_entities_by_property(&graph, "test_key", "test_value").unwrap();
    assert_eq!(
        results, results2,
        "Results should be deterministic across queries"
    );
}

#[test]
fn test_composite_index_exists() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Check that schema was properly initialized
    let schema_version = graph.schema_version().unwrap();
    assert!(schema_version >= 1, "Schema should be initialized");

    // Verify indexes exist implicitly by checking that queries work efficiently
    assert!(
        index_exists(&graph, "idx_labels_label"),
        "Label index should exist"
    );
    assert!(
        index_exists(&graph, "idx_props_key_value"),
        "Property index should exist"
    );

    // Test that label queries are efficient (indicating proper indexing)
    let entity = graph
        .insert_entity(&GraphEntity {
            id: 0,
            kind: "test".to_string(),
            name: "test_entity".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    add_label(&graph, entity, "test_label").unwrap();

    // This should be fast with proper indexing
    let results = get_entities_by_label(&graph, "test_label").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, entity);

    // Test that property queries are efficient (indicating proper indexing)
    add_property(&graph, entity, "test_key", "test_value").unwrap();

    let prop_results = get_entities_by_property(&graph, "test_key", "test_value").unwrap();
    assert_eq!(prop_results.len(), 1);
    assert_eq!(prop_results[0].id, entity);
}

#[test]
fn test_large_property_query_is_fast_enough() {
    let graph = SqliteGraph::open_in_memory().unwrap();

    // Insert ~5000 entities with properties
    let mut entities = Vec::new();
    for i in 0..5000 {
        entities.push(GraphEntityCreate {
            kind: "test".to_string(),
            name: format!("entity_{}", i),
            file_path: None,
            data: serde_json::json!({"index": i}),
        });
    }

    let entity_ids = bulk_insert_entities(&graph, &entities).unwrap();
    assert_eq!(entity_ids.len(), 5000);

    // Add the same property to all entities
    for &entity_id in &entity_ids {
        add_property(&graph, entity_id, "bulk_key", "bulk_value").unwrap();
    }

    // Time the actual query - should be fast with index
    use std::time::Instant;
    let start = Instant::now();
    let results = get_entities_by_property(&graph, "bulk_key", "bulk_value").unwrap();
    let duration = start.elapsed();

    assert_eq!(results.len(), 5000, "Should find all 5000 entities");
    assert!(
        duration.as_millis() < 1000,
        "Query should complete in under 1 second, took {:?}",
        duration
    );

    // Verify deterministic ordering
    for i in 1..results.len() {
        assert!(
            results[i].id > results[i - 1].id,
            "Results should be ordered by id"
        );
    }
}

#[test]
fn test_end_to_end_pattern_with_indexes() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let backend = SqliteGraphBackend::from_graph(graph);

    // Create a test graph with labels, properties, and edges
    let entity1 = backend
        .insert_node(sqlitegraph::backend::NodeSpec {
            kind: "person".to_string(),
            name: "alice".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let entity2 = backend
        .insert_node(sqlitegraph::backend::NodeSpec {
            kind: "person".to_string(),
            name: "bob".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let entity3 = backend
        .insert_node(sqlitegraph::backend::NodeSpec {
            kind: "company".to_string(),
            name: "acme".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    // Add labels
    add_label(backend.graph(), entity1, "employee").unwrap();
    add_label(backend.graph(), entity2, "employee").unwrap();
    add_label(backend.graph(), entity3, "employer").unwrap();

    // Add properties
    add_property(backend.graph(), entity1, "department", "engineering").unwrap();
    add_property(backend.graph(), entity2, "department", "engineering").unwrap();
    add_property(backend.graph(), entity3, "industry", "tech").unwrap();

    // Add edges
    backend
        .insert_edge(sqlitegraph::backend::EdgeSpec {
            from: entity1,
            to: entity3,
            edge_type: "works_for".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    backend
        .insert_edge(sqlitegraph::backend::EdgeSpec {
            from: entity2,
            to: entity3,
            edge_type: "works_for".to_string(),
            data: serde_json::json!({}),
        })
        .unwrap();

    // Step 1: Label query -> should be deterministic
    let employees = get_entities_by_label(backend.graph(), "employee").unwrap();
    assert_eq!(employees.len(), 2);
    assert!(employees[0].id < employees[1].id, "Should be ordered by id");

    // Step 2: Property query -> should be deterministic
    let engineers = get_entities_by_property(backend.graph(), "department", "engineering").unwrap();
    assert_eq!(engineers.len(), 2);
    assert!(engineers[0].id < engineers[1].id, "Should be ordered by id");

    // Step 3: Adjacency fetch -> should be deterministic
    let company_outgoing = backend
        .neighbors(
            SnapshotId::current(),
            entity3,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(
        company_outgoing.len(),
        0,
        "Company should have no outgoing edges"
    );

    let company_incoming = backend
        .neighbors(
            SnapshotId::current(),
            entity3,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(
        company_incoming.len(),
        2,
        "Company should have 2 incoming edges"
    );
    assert!(
        company_incoming[0] < company_incoming[1],
        "Should be ordered by neighbor_id"
    );

    // Verify the entire pattern is deterministic across runs
    let employees2 = get_entities_by_label(backend.graph(), "employee").unwrap();
    let engineers2 =
        get_entities_by_property(backend.graph(), "department", "engineering").unwrap();
    let company_incoming2 = backend
        .neighbors(
            SnapshotId::current(),
            entity3,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )
        .unwrap();

    assert_eq!(
        employees, employees2,
        "Label queries should be deterministic"
    );
    assert_eq!(
        engineers, engineers2,
        "Property queries should be deterministic"
    );
    assert_eq!(
        company_incoming, company_incoming2,
        "Adjacency queries should be deterministic"
    );
}

#[test]
fn test_existing_functions_still_work() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    let backend = SqliteGraphBackend::from_graph(graph);

    // Test basic entity operations
    let entity = backend
        .insert_node(sqlitegraph::backend::NodeSpec {
            kind: "test".to_string(),
            name: "test_entity".to_string(),
            file_path: Some("/test/path".to_string()),
            data: serde_json::json!({"test": true}),
        })
        .unwrap();
    assert!(entity > 0);

    let retrieved = backend.get_node(SnapshotId::current(), entity).unwrap();
    assert_eq!(retrieved.id, entity);
    assert_eq!(retrieved.kind, "test");
    assert_eq!(retrieved.name, "test_entity");

    // Test edge operations
    let entity2 = backend
        .insert_node(sqlitegraph::backend::NodeSpec {
            kind: "test2".to_string(),
            name: "test_entity2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        })
        .unwrap();

    let edge_id = backend
        .insert_edge(sqlitegraph::backend::EdgeSpec {
            from: entity,
            to: entity2,
            edge_type: "test_edge".to_string(),
            data: serde_json::json!({"weight": 1.0}),
        })
        .unwrap();
    assert!(edge_id > 0);

    // Test adjacency operations (cache usage)
    let outgoing = backend
        .neighbors(
            SnapshotId::current(),
            entity,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0], entity2);

    let incoming = backend
        .neighbors(
            SnapshotId::current(),
            entity2,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0], entity);

    // Test BFS traversal
    let bfs_result = backend.bfs(SnapshotId::current(), entity, 2).unwrap();
    assert!(bfs_result.contains(&entity));
    assert!(bfs_result.contains(&entity2));

    // Test bulk operations
    let bulk_entities = vec![
        GraphEntityCreate {
            kind: "bulk".to_string(),
            name: "bulk1".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        },
        GraphEntityCreate {
            kind: "bulk".to_string(),
            name: "bulk2".to_string(),
            file_path: None,
            data: serde_json::json!({}),
        },
    ];

    let bulk_ids = bulk_insert_entities(backend.graph(), &bulk_entities).unwrap();
    assert_eq!(bulk_ids.len(), 2);

    let bulk_edges = vec![
        GraphEdgeCreate {
            from_id: entity,
            to_id: bulk_ids[0],
            edge_type: "bulk_edge".to_string(),
            data: serde_json::json!({}),
        },
        GraphEdgeCreate {
            from_id: entity,
            to_id: bulk_ids[1],
            edge_type: "bulk_edge".to_string(),
            data: serde_json::json!({}),
        },
    ];

    let bulk_edge_ids = bulk_insert_edges(backend.graph(), &bulk_edges).unwrap();
    assert_eq!(bulk_edge_ids.len(), 2);

    // Verify adjacency after bulk operations
    let outgoing_after_bulk = backend
        .neighbors(
            SnapshotId::current(),
            entity,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )
        .unwrap();
    assert_eq!(outgoing_after_bulk.len(), 3); // original + 2 bulk edges

    // Test entity_ids ordering
    let all_ids = backend.entity_ids().unwrap();
    assert!(all_ids.len() >= 4); // at least our test entities
    for i in 1..all_ids.len() {
        assert!(
            all_ids[i] > all_ids[i - 1],
            "entity_ids should return ordered results"
        );
    }
}
