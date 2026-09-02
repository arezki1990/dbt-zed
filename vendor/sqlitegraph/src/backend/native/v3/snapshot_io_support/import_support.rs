use super::V3Backend;
use crate::SqliteGraphError;
use crate::backend::{EdgeSpec, GraphBackend, ImportMetadata, NodeSpec};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

type LabelsByEntity = HashMap<i64, Vec<String>>;
type PropsByEntity = HashMap<i64, Vec<(String, serde_json::Value)>>;

pub(super) fn snapshot_import_support(
    backend: &V3Backend,
    import_dir: &Path,
) -> Result<ImportMetadata, SqliteGraphError> {
    let snapshot_file = import_dir.join("snapshot.json");
    if !snapshot_file.exists() {
        return Err(SqliteGraphError::connection(format!(
            "Snapshot file not found: {}",
            snapshot_file.display()
        )));
    }

    let (mut labels_by_entity, mut props_by_entity, entity_records, edge_records) =
        parse_snapshot_records(&snapshot_file)?;

    if entity_records.is_empty() && edge_records.is_empty() {
        return Ok(ImportMetadata {
            snapshot_path: snapshot_file,
            entities_imported: 0,
            edges_imported: 0,
        });
    }

    let (id_map, entities_imported) = import_entities(
        backend,
        &entity_records,
        &mut labels_by_entity,
        &mut props_by_entity,
    )?;
    let edges_imported = import_edges(backend, &edge_records, &id_map)?;

    Ok(ImportMetadata {
        snapshot_path: snapshot_file,
        entities_imported,
        edges_imported,
    })
}

fn parse_snapshot_records(
    snapshot_file: &Path,
) -> Result<
    (
        LabelsByEntity,
        PropsByEntity,
        Vec<serde_json::Value>,
        Vec<serde_json::Value>,
    ),
    SqliteGraphError,
> {
    let file = File::open(snapshot_file)
        .map_err(|e| SqliteGraphError::connection(format!("Failed to open snapshot: {}", e)))?;
    let reader = BufReader::new(file);

    let mut labels_by_entity: LabelsByEntity = HashMap::new();
    let mut props_by_entity: PropsByEntity = HashMap::new();
    let mut entity_records = Vec::new();
    let mut edge_records = Vec::new();

    for line in reader.lines() {
        let line = line
            .map_err(|e| SqliteGraphError::invalid_input(format!("Failed to read line: {}", e)))?;
        if line.trim().is_empty() {
            continue;
        }
        let record: serde_json::Value = serde_json::from_str(&line).map_err(|e| {
            SqliteGraphError::invalid_input(format!("Failed to parse JSONL record: {}", e))
        })?;
        let rec_type = record.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match rec_type {
            "entity" => entity_records.push(record),
            "edge" => edge_records.push(record),
            "label" => record_label(&record, &mut labels_by_entity)?,
            "property" => record_property(&record, &mut props_by_entity)?,
            "" => {
                return Err(SqliteGraphError::invalid_input(
                    "JSONL record missing `type` field".to_string(),
                ));
            }
            other => {
                return Err(SqliteGraphError::invalid_input(format!(
                    "unknown JSONL record type: {other}"
                )));
            }
        }
    }

    Ok((
        labels_by_entity,
        props_by_entity,
        entity_records,
        edge_records,
    ))
}

fn record_label(
    record: &serde_json::Value,
    labels_by_entity: &mut LabelsByEntity,
) -> Result<(), SqliteGraphError> {
    let entity_id = record
        .get("entity_id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            SqliteGraphError::invalid_input("label record missing entity_id".to_string())
        })?;
    let label = record
        .get("label")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SqliteGraphError::invalid_input("label record missing label".to_string()))?
        .to_string();
    labels_by_entity.entry(entity_id).or_default().push(label);
    Ok(())
}

fn record_property(
    record: &serde_json::Value,
    props_by_entity: &mut PropsByEntity,
) -> Result<(), SqliteGraphError> {
    let entity_id = record
        .get("entity_id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            SqliteGraphError::invalid_input("property record missing entity_id".to_string())
        })?;
    let key = record
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SqliteGraphError::invalid_input("property record missing key".to_string()))?
        .to_string();
    let raw_value = record
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            SqliteGraphError::invalid_input("property record missing value".to_string())
        })?;
    let parsed: serde_json::Value =
        serde_json::from_str(raw_value).unwrap_or(serde_json::Value::String(raw_value.to_string()));
    props_by_entity
        .entry(entity_id)
        .or_default()
        .push((key, parsed));
    Ok(())
}

fn import_entities(
    backend: &V3Backend,
    entity_records: &[serde_json::Value],
    labels_by_entity: &mut LabelsByEntity,
    props_by_entity: &mut PropsByEntity,
) -> Result<(HashMap<i64, i64>, u64), SqliteGraphError> {
    let mut id_map = HashMap::new();
    let mut entities_imported = 0;

    for rec in entity_records {
        let original_id = rec.get("id").and_then(|v| v.as_i64()).ok_or_else(|| {
            SqliteGraphError::invalid_input("entity record missing id".to_string())
        })?;
        let kind = rec
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("Entity")
            .to_string();
        let name = rec
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let file_path = rec
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let data = merge_entity_import_data(
            rec.get("data").cloned().unwrap_or(serde_json::Value::Null),
            labels_by_entity.remove(&original_id),
            props_by_entity.remove(&original_id),
        );

        let node_id = backend.insert_node(NodeSpec {
            kind,
            name,
            file_path,
            data,
        })?;
        id_map.insert(original_id, node_id);
        entities_imported += 1;
    }

    Ok((id_map, entities_imported))
}

fn merge_entity_import_data(
    mut data: serde_json::Value,
    extra_labels: Option<Vec<String>>,
    extra_props: Option<Vec<(String, serde_json::Value)>>,
) -> serde_json::Value {
    if extra_labels.is_none() && extra_props.is_none() {
        return data;
    }

    if !data.is_object() {
        data = serde_json::Value::Object(serde_json::Map::new());
    }

    if let Some(obj) = data.as_object_mut() {
        if let Some(labels) = extra_labels {
            obj.insert(
                "_labels".to_string(),
                serde_json::Value::Array(
                    labels.into_iter().map(serde_json::Value::String).collect(),
                ),
            );
        }
        if let Some(props) = extra_props {
            for (k, v) in props {
                obj.insert(k, v);
            }
        }
    }

    data
}

fn import_edges(
    backend: &V3Backend,
    edge_records: &[serde_json::Value],
    id_map: &HashMap<i64, i64>,
) -> Result<u64, SqliteGraphError> {
    let mut edges_imported = 0;

    for rec in edge_records {
        let from_original = rec.get("from_id").and_then(|v| v.as_i64()).ok_or_else(|| {
            SqliteGraphError::invalid_input("edge record missing from_id".to_string())
        })?;
        let to_original = rec.get("to_id").and_then(|v| v.as_i64()).ok_or_else(|| {
            SqliteGraphError::invalid_input("edge record missing to_id".to_string())
        })?;
        let edge_type = rec
            .get("edge_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let data = rec.get("data").cloned().unwrap_or(serde_json::Value::Null);

        let from = id_map.get(&from_original).copied().unwrap_or(from_original);
        let to = id_map.get(&to_original).copied().unwrap_or(to_original);

        backend.insert_edge(EdgeSpec {
            from,
            to,
            edge_type,
            data,
        })?;
        edges_imported += 1;
    }

    Ok(edges_imported)
}
