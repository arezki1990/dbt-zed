use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    SqliteGraphError,
    fault_injection::{self, FaultPoint},
    graph::SqliteGraph,
};

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DumpRecord {
    Entity {
        id: i64,
        kind: String,
        name: String,
        file_path: Option<String>,
        data: Value,
    },
    Edge {
        id: i64,
        from_id: i64,
        to_id: i64,
        edge_type: String,
        data: Value,
    },
    Label {
        entity_id: i64,
        label: String,
    },
    Property {
        entity_id: i64,
        key: String,
        value: String,
    },
}

pub fn dump_graph_to_path<P: AsRef<Path>>(
    graph: &SqliteGraph,
    path: P,
) -> Result<(), SqliteGraphError> {
    let file =
        File::create(path.as_ref()).map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
    dump_graph_to_writer(graph, BufWriter::new(file))
}

pub fn dump_graph_to_writer<W: Write>(
    graph: &SqliteGraph,
    mut writer: W,
) -> Result<(), SqliteGraphError> {
    for id in graph.list_entity_ids()? {
        let entity = graph.get_entity(id)?;
        write_record(
            &mut writer,
            &DumpRecord::Entity {
                id: entity.id,
                kind: entity.kind,
                name: entity.name,
                file_path: entity.file_path,
                data: entity.data,
            },
        )?;
    }
    dump_edges(graph, &mut writer)?;
    dump_labels(graph, &mut writer)?;
    dump_properties(graph, &mut writer)?;
    Ok(())
}

pub fn load_graph_from_path<P: AsRef<Path>>(
    graph: &SqliteGraph,
    path: P,
) -> Result<(), SqliteGraphError> {
    let file =
        File::open(path.as_ref()).map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
    load_graph_from_reader(graph, BufReader::new(file))
}

pub fn load_graph_from_reader<R: BufRead>(
    graph: &SqliteGraph,
    reader: R,
) -> Result<(), SqliteGraphError> {
    let conn = graph.connection();
    conn.execute("BEGIN IMMEDIATE", [])
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let result: Result<(), SqliteGraphError> = (|| {
        conn.execute("DELETE FROM graph_labels", [])
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        conn.execute("DELETE FROM graph_properties", [])
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        conn.execute("DELETE FROM graph_edges", [])
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        conn.execute("DELETE FROM graph_entities", [])
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        let mut stmt_entity = conn
            .prepare_cached(
                "INSERT INTO graph_entities(id,kind,name,file_path,data) VALUES(?1,?2,?3,?4,?5)",
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        let mut stmt_edge = conn
            .prepare_cached(
                "INSERT INTO graph_edges(id,from_id,to_id,edge_type,data) VALUES(?1,?2,?3,?4,?5)",
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        let mut stmt_label = conn
            .prepare_cached("INSERT INTO graph_labels(entity_id,label) VALUES(?1,?2)")
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        let mut stmt_property = conn
            .prepare_cached("INSERT INTO graph_properties(entity_id,key,value) VALUES(?1,?2,?3)")
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        for line in reader.lines() {
            let line = line.map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
            if line.trim().is_empty() {
                continue;
            }
            let record: DumpRecord = serde_json::from_str(&line)
                .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
            match record {
                DumpRecord::Entity {
                    id,
                    kind,
                    name,
                    file_path,
                    data,
                } => {
                    let payload = serde_json::to_string(&data)
                        .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
                    stmt_entity
                        .execute(rusqlite::params![id, kind, name, file_path, payload])
                        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                }
                DumpRecord::Edge {
                    id,
                    from_id,
                    to_id,
                    edge_type,
                    data,
                } => {
                    let payload = serde_json::to_string(&data)
                        .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
                    stmt_edge
                        .execute(rusqlite::params![id, from_id, to_id, edge_type, payload])
                        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                }
                DumpRecord::Label { entity_id, label } => {
                    stmt_label
                        .execute(rusqlite::params![entity_id, label])
                        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                }
                DumpRecord::Property {
                    entity_id,
                    key,
                    value,
                } => {
                    stmt_property
                        .execute(rusqlite::params![entity_id, key, value])
                        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                }
            }
        }
        Ok(())
    })();
    let result = result.and_then(|_| {
        fault_injection::check_fault(FaultPoint::RecoveryLoadBeforeCommit)?;
        Ok(())
    });
    match result {
        Ok(()) => {
            conn.execute("COMMIT", [])
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;
            graph.invalidate_caches();
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute("ROLLBACK", []);
            Err(err)
        }
    }
}

fn dump_edges<W: Write>(graph: &SqliteGraph, writer: &mut W) -> Result<(), SqliteGraphError> {
    let conn = graph.connection();
    let mut stmt = conn
        .prepare_cached("SELECT id FROM graph_edges ORDER BY id")
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    for row in rows {
        let id = row.map_err(|e| SqliteGraphError::query(e.to_string()))?;
        let edge = graph.get_edge(id)?;
        write_record(
            writer,
            &DumpRecord::Edge {
                id: edge.id,
                from_id: edge.from_id,
                to_id: edge.to_id,
                edge_type: edge.edge_type,
                data: edge.data,
            },
        )?;
    }
    Ok(())
}

fn dump_labels<W: Write>(graph: &SqliteGraph, writer: &mut W) -> Result<(), SqliteGraphError> {
    let conn = graph.connection();
    let mut stmt = conn
        .prepare_cached("SELECT entity_id, label FROM graph_labels ORDER BY entity_id, label")
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    for row in rows {
        let (entity_id, label) = row.map_err(|e| SqliteGraphError::query(e.to_string()))?;
        write_record(writer, &DumpRecord::Label { entity_id, label })?;
    }
    Ok(())
}

fn dump_properties<W: Write>(graph: &SqliteGraph, writer: &mut W) -> Result<(), SqliteGraphError> {
    let conn = graph.connection();
    let mut stmt = conn
        .prepare_cached(
            "SELECT entity_id, key, value FROM graph_properties ORDER BY entity_id, key, value",
        )
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
    for row in rows {
        let (entity_id, key, value) = row.map_err(|e| SqliteGraphError::query(e.to_string()))?;
        write_record(
            writer,
            &DumpRecord::Property {
                entity_id,
                key,
                value,
            },
        )?;
    }
    Ok(())
}

fn write_record<W: Write>(writer: &mut W, record: &DumpRecord) -> Result<(), SqliteGraphError> {
    serde_json::to_writer(&mut *writer, record)
        .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
    writer
        .write_all(b"\n")
        .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))
}
