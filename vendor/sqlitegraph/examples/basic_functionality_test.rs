//! Basic functionality test for SQLiteGraph library
use sqlitegraph::{GraphEdge, GraphEntity, SqliteGraph};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing SQLiteGraph basic functionality...");

    // Create in-memory graph
    let graph = SqliteGraph::open_in_memory()?;
    println!("✅ Graph created successfully");

    // Create entities with proper structure
    let user_entity = GraphEntity {
        id: 0, // Will be assigned by database
        kind: "User".to_string(),
        name: "Alice".to_string(),
        file_path: None,
        data: serde_json::json!({"age": 30}),
    };

    let project_entity = GraphEntity {
        id: 0,
        kind: "Project".to_string(),
        name: "SQLiteGraph".to_string(),
        file_path: None,
        data: serde_json::json!({"status": "active"}),
    };

    let company_entity = GraphEntity {
        id: 0,
        kind: "Company".to_string(),
        name: "TechCorp".to_string(),
        file_path: None,
        data: serde_json::json!({"founded": 2020}),
    };

    // Insert entities
    let node1 = graph.insert_entity(&user_entity)?;
    let node2 = graph.insert_entity(&project_entity)?;
    let node3 = graph.insert_entity(&company_entity)?;
    println!("✅ Entities inserted: {} {} {}", node1, node2, node3);

    // Create edges
    let works_on_edge = GraphEdge {
        id: 0,
        from_id: node1,
        to_id: node2,
        edge_type: "works_on".to_string(),
        data: serde_json::json!({"role": "developer"}),
    };

    let belongs_to_edge = GraphEdge {
        id: 0,
        from_id: node2,
        to_id: node3,
        edge_type: "belongs_to".to_string(),
        data: serde_json::json!({"ownership": 0.75}),
    };

    let employed_by_edge = GraphEdge {
        id: 0,
        from_id: node1,
        to_id: node3,
        edge_type: "employed_by".to_string(),
        data: serde_json::json!({"department": "engineering"}),
    };

    // Insert edges
    let edge1 = graph.insert_edge(&works_on_edge)?;
    let edge2 = graph.insert_edge(&belongs_to_edge)?;
    let edge3 = graph.insert_edge(&employed_by_edge)?;
    println!("✅ Edges inserted: {} {} {}", edge1, edge2, edge3);

    // Verify graph structure
    let entity1 = graph.get_entity(node1)?;
    let entity2 = graph.get_entity(node2)?;
    println!(
        "✅ Retrieved entities: {} ({}) -> {} ({})",
        entity1.name, entity1.kind, entity2.name, entity2.kind
    );

    // Test edge retrieval
    let edge_retrieved = graph.get_edge(edge1)?;
    println!(
        "✅ Retrieved edge: {} ({} -> {})",
        edge_retrieved.edge_type, edge_retrieved.from_id, edge_retrieved.to_id
    );

    println!("🎉 All basic operations work correctly!");
    println!("📊 Created graph with {} entities and {} edges", 3, 3);

    Ok(())
}
