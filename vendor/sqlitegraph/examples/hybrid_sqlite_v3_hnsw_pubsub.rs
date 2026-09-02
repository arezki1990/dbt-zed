use rusqlite::{Connection, params};
use serde_json::json;
use sqlitegraph::backend::{
    BackendDirection, EdgeSpec, GraphBackend, KvValue, NeighborQuery, NodeSpec, SubscriptionFilter,
};
use sqlitegraph::hnsw::{DistanceMetric, HnswConfig};
use sqlitegraph::{NativeGraphBackend, SnapshotId, SqliteGraph};
use std::collections::HashMap;
use std::fs;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::temp_dir().join(format!("sqlitegraph-hybrid-demo-{}", std::process::id()));
    fs::create_dir_all(&base)?;

    let sqlite_path = base.join("application.sqlite");
    let graph_path = base.join("metadata.graph");

    // 1. Normal SQLite tables hold application records.
    let sql = Connection::open(&sqlite_path)?;
    sql.execute_batch(
        r#"
        CREATE TABLE documents (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            body TEXT NOT NULL
        );
        "#,
    )?;
    sql.execute(
        "INSERT INTO documents (id, title, body) VALUES (?1, ?2, ?3)",
        params![
            1_i64,
            "SQLiteGraph architecture",
            "SQLite stores durable records, V3 stores graph metadata, HNSW stores vectors."
        ],
    )?;
    sql.execute(
        "INSERT INTO documents (id, title, body) VALUES (?1, ?2, ?3)",
        params![
            2_i64,
            "Grounded coding workflow",
            "Code intelligence links symbols, files, notes, and provenance into a graph."
        ],
    )?;
    sql.execute(
        "INSERT INTO documents (id, title, body) VALUES (?1, ?2, ?3)",
        params![
            3_i64,
            "Bread baking notes",
            "Hydration, fermentation, flour, and oven spring belong to a different topic."
        ],
    )?;

    // 2. Native V3 stores graph-shaped metadata over those SQLite rows.
    let graph = NativeGraphBackend::create_with_wal(&graph_path, true)?;
    let (subscription_id, rx) = graph.subscribe(SubscriptionFilter::all())?;

    let doc_1 = graph.insert_node(NodeSpec {
        kind: "Document".to_string(),
        name: "doc:1".to_string(),
        file_path: None,
        data: json!({"sqlite_id": 1, "topic": "architecture"}),
    })?;
    let doc_2 = graph.insert_node(NodeSpec {
        kind: "Document".to_string(),
        name: "doc:2".to_string(),
        file_path: None,
        data: json!({"sqlite_id": 2, "topic": "coding"}),
    })?;
    let doc_3 = graph.insert_node(NodeSpec {
        kind: "Document".to_string(),
        name: "doc:3".to_string(),
        file_path: None,
        data: json!({"sqlite_id": 3, "topic": "cooking"}),
    })?;

    graph.insert_edge(EdgeSpec {
        from: doc_1,
        to: doc_2,
        edge_type: "MENTIONS".to_string(),
        data: json!({"reason": "graph metadata explains grounded coding"}),
    })?;
    graph.insert_edge(EdgeSpec {
        from: doc_2,
        to: doc_1,
        edge_type: "USES".to_string(),
        data: json!({"reason": "workflow uses sqlitegraph"}),
    })?;
    graph.insert_edge(EdgeSpec {
        from: doc_3,
        to: doc_1,
        edge_type: "UNRELATED_EXAMPLE".to_string(),
        data: json!({}),
    })?;

    graph.kv_set(b"last_indexed_doc".to_vec(), KvValue::Integer(2), None)?;
    let event = rx.recv_timeout(Duration::from_secs(1))?;
    graph.unsubscribe(subscription_id)?;
    graph.flush_to_disk()?;

    // 3. HNSW stores vector similarity in the SQLite-backed graph database.
    let vector_graph = SqliteGraph::open(&sqlite_path)?;
    let config = HnswConfig::new(3, 16, 200, DistanceMetric::Cosine);
    let mut vector_to_sqlite_id = HashMap::new();
    {
        let mut indexes = vector_graph.hnsw_index_persistent("document_embeddings", config)?;
        let hnsw = indexes
            .get_mut("document_embeddings")
            .ok_or("HNSW index was not registered")?;
        let v1 = hnsw.insert_vector(&[0.95, 0.80, 0.05], Some(json!({"sqlite_id": 1})))?;
        let v2 = hnsw.insert_vector(&[0.90, 0.85, 0.10], Some(json!({"sqlite_id": 2})))?;
        let v3 = hnsw.insert_vector(&[0.05, 0.10, 0.95], Some(json!({"sqlite_id": 3})))?;
        vector_to_sqlite_id.insert(v1, 1_i64);
        vector_to_sqlite_id.insert(v2, 2_i64);
        vector_to_sqlite_id.insert(v3, 3_i64);
    }

    let nearest = vector_graph.get_hnsw_index_ref("document_embeddings", |hnsw| {
        hnsw.search(&[0.92, 0.82, 0.05], 2)
    })??;

    // 4. Combine all layers: vector candidates -> SQLite rows -> V3 graph expansion.
    println!("Hybrid sqlitegraph runtime demo");
    println!("workspace: {}", base.display());
    println!("pub/sub event from V3: {event:?}");
    println!();

    for (vector_id, distance) in nearest {
        let sqlite_id = vector_to_sqlite_id
            .get(&vector_id)
            .ok_or("missing vector-to-document mapping")?;
        let title: String = sql.query_row(
            "SELECT title FROM documents WHERE id = ?1",
            params![sqlite_id],
            |row| row.get(0),
        )?;

        let graph_node_id = match sqlite_id {
            1 => doc_1,
            2 => doc_2,
            3 => doc_3,
            _ => unreachable!("demo only inserts three documents"),
        };
        let related = graph.neighbors(
            SnapshotId::current(),
            graph_node_id,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )?;

        println!(
            "- vector_id={vector_id}, sqlite_id={sqlite_id}, distance={distance:.4}, title={title:?}, graph_neighbors={related:?}"
        );
    }

    println!();
    println!(
        "V3 document nodes: {:?}",
        graph.query_nodes_by_kind(SnapshotId::current(), "Document")?
    );

    Ok(())
}
