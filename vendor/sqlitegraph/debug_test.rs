use sqlitegraph::{
    index::{add_label, get_entities_by_label},
    graph::{GraphEntity, SqliteGraph},
};

fn main() {
    let graph = SqliteGraph::open_in_memory().unwrap();
    
    // Create test entities with different insertion order
    let entity3 = graph.insert_entity(&GraphEntity {
        id: 0,
        kind: "test".to_string(),
        name: "entity3".to_string(),
        file_path: None,
        data: serde_json::json!({}),
    }).unwrap();
    
    let entity1 = graph.insert_entity(&GraphEntity {
        id: 0,
        kind: "test".to_string(),
        name: "entity1".to_string(),
        file_path: None,
        data: serde_json::json!({}),
    }).unwrap();
    
    let entity2 = graph.insert_entity(&GraphEntity {
        id: 0,
        kind: "test".to_string(),
        name: "entity2".to_string(),
        file_path: None,
        data: serde_json::json!({}),
    }).unwrap();
    
    println!("Created entities: {} (entity1), {} (entity2), {} (entity3)", entity1, entity2, entity3);
    
    // Add labels in non-sequential order
    add_label(&graph, entity2, "test_label").unwrap();
    add_label(&graph, entity3, "test_label").unwrap();
    add_label(&graph, entity1, "test_label").unwrap();
    
    println!("Added labels to all entities");
    
    // Query by label
    let results = get_entities_by_label(&graph, "test_label").unwrap();
    println!("Found {} entities with label 'test_label'", results.len());
    for (i, entity) in results.iter().enumerate() {
        println!("  Result {}: id={}, name={}", i, entity.id, entity.name);
    }
}