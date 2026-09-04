//! The Database explorer's offline catalog: a Database → Schema → Relation →
//! Column tree built purely from `target/manifest.json` and
//! `target/catalog.json`. No warehouse query, no dbt subprocess — this is a
//! map of dbt's world, as fresh as the artifacts on disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use gpui::SharedString;

pub struct DbCatalog {
    /// `metadata.adapter_type` from the manifest, lowercased ("snowflake").
    pub adapter: Option<SharedString>,
    /// `metadata.generated_at` from catalog.json, when present.
    pub catalog_generated_at: Option<SharedString>,
    /// False means no catalog.json: the tree still builds from the manifest,
    /// but no relation carries column types or stats.
    pub catalog_present: bool,
    pub databases: Vec<DbDatabase>,
}

pub struct DbDatabase {
    /// Case-folded UPPER — the manifest and catalog disagree on casing for
    /// nearly every relation, so the fold is what keeps one schema from
    /// splitting into two nodes.
    pub name: SharedString,
    pub schemas: Vec<DbSchema>,
}

pub struct DbSchema {
    pub name: SharedString,
    pub relations: Vec<DbRelation>,
}

pub struct DbRelation {
    pub unique_id: SharedString,
    /// Warehouse casing from the catalog when known, else the manifest alias.
    pub name: SharedString,
    pub kind: RelationKind,
    /// "VIEW" | "BASE TABLE" from catalog metadata, when known.
    pub object_type: Option<SharedString>,
    /// `relation_name` from the manifest — already quoted the way dbt itself
    /// addresses the relation, so safe to interpolate into `select * from`.
    pub fqn: SharedString,
    /// The defining file, relative to the project root.
    pub file_path: Option<PathBuf>,
    pub description: Option<SharedString>,
    pub owner: Option<SharedString>,
    /// From catalog stats; only physical tables carry one. Never invent 0
    /// for a view.
    pub row_count: Option<u64>,
    pub bytes: Option<u64>,
    pub columns: ColumnState,
}

/// `Unknown` is not an empty list: it means the catalog has no entry for the
/// relation (stale catalog, new model) and must render as "columns unknown",
/// never as "no columns".
pub enum ColumnState {
    Unknown,
    Known(Arc<Vec<DbColumn>>),
}

pub struct DbColumn {
    pub name: SharedString,
    pub data_type: Option<SharedString>,
    /// 1-based ordinal from the catalog.
    pub index: u32,
    /// Documentation from the manifest's column map, overlaid case-insensitively.
    pub description: Option<SharedString>,
}

pub enum RelationKind {
    Model { materialized: SharedString },
    Seed,
    Snapshot,
    Source { source_name: SharedString },
}

impl RelationKind {
    pub fn label(&self) -> &str {
        match self {
            RelationKind::Model { materialized } => materialized.as_ref(),
            RelationKind::Seed => "seed",
            RelationKind::Snapshot => "snapshot",
            RelationKind::Source { .. } => "source",
        }
    }
}

impl DbCatalog {
    pub fn relation_count(&self) -> usize {
        self.databases
            .iter()
            .flat_map(|db| &db.schemas)
            .map(|schema| schema.relations.len())
            .sum()
    }
}

fn string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// A `stats.<id>.value` number from a catalog entry.
fn stat_u64(entry: &serde_json::Value, id: &str) -> Option<u64> {
    let value = entry.get("stats")?.get(id)?.get("value")?;
    value
        .as_u64()
        .or_else(|| value.as_f64().map(|f| f.max(0.) as u64))
}

/// Builds the offline tree. Synchronous filesystem + JSON work only — call it
/// from a background task.
pub fn build_catalog(project_root: &Path) -> Result<DbCatalog> {
    let manifest: serde_json::Value = serde_json::from_reader(std::io::BufReader::new(
        std::fs::File::open(project_root.join("target").join("manifest.json"))
            .context("no target/manifest.json — run `dbt parse` first")?,
    ))
    .context("parsing target/manifest.json")?;

    let catalog: Option<serde_json::Value> =
        std::fs::File::open(project_root.join("target").join("catalog.json"))
            .ok()
            .and_then(|file| serde_json::from_reader(std::io::BufReader::new(file)).ok());

    let adapter = manifest
        .get("metadata")
        .and_then(|m| string(m, "adapter_type"))
        .map(|a| SharedString::from(a.to_lowercase()));
    let catalog_generated_at = catalog
        .as_ref()
        .and_then(|c| c.get("metadata"))
        .and_then(|m| string(m, "generated_at"))
        .map(SharedString::from);

    // Catalog entries by unique_id — names collide (verified: 3 model/source
    // pairs share a name in the reference project), unique_id never does.
    let mut catalog_by_uid: HashMap<String, &serde_json::Value> = HashMap::new();
    if let Some(catalog) = &catalog {
        for section in ["nodes", "sources"] {
            if let Some(map) = catalog.get(section).and_then(|v| v.as_object()) {
                for (uid, entry) in map {
                    catalog_by_uid.insert(uid.clone(), entry);
                }
            }
        }
    }

    // (UPPER db, UPPER schema) -> relations
    let mut grouped: HashMap<(String, String), Vec<DbRelation>> = HashMap::new();

    let mut add_relation = |uid: &str, node: &serde_json::Value, kind: RelationKind| {
        let Some(database) = string(node, "database") else {
            return;
        };
        let Some(schema) = string(node, "schema") else {
            return;
        };
        let alias = string(node, "alias")
            .or_else(|| string(node, "name"))
            .unwrap_or_else(|| uid.to_owned());
        let fqn = string(node, "relation_name")
            .unwrap_or_else(|| format!("{database}.{schema}.{alias}"));

        let entry = catalog_by_uid.get(uid);
        let meta = entry.and_then(|e| e.get("metadata"));
        let is_table = meta
            .and_then(|m| m.get("type"))
            .and_then(|t| t.as_str())
            .is_some_and(|t| t.eq_ignore_ascii_case("BASE TABLE"));

        // Manifest column docs, keyed lowercase for the case-insensitive overlay.
        let docs: HashMap<String, String> = node
            .get("columns")
            .and_then(|c| c.as_object())
            .map(|cols| {
                cols.iter()
                    .filter_map(|(name, col)| {
                        Some((name.to_lowercase(), string(col, "description")?))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let columns = match entry.and_then(|e| e.get("columns")).and_then(|c| c.as_object()) {
            Some(cols) => {
                let mut list: Vec<DbColumn> = cols
                    .values()
                    .filter_map(|col| {
                        let name = string(col, "name")?;
                        Some(DbColumn {
                            data_type: string(col, "type").map(SharedString::from),
                            index: col.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32,
                            description: docs.get(&name.to_lowercase()).cloned().map(Into::into),
                            name: name.into(),
                        })
                    })
                    .collect();
                list.sort_by_key(|col| col.index);
                ColumnState::Known(Arc::new(list))
            }
            None => ColumnState::Unknown,
        };

        let relation = DbRelation {
            unique_id: uid.to_owned().into(),
            name: meta
                .and_then(|m| string(m, "name"))
                .unwrap_or_else(|| alias.clone())
                .into(),
            kind,
            object_type: meta.and_then(|m| string(m, "type")).map(Into::into),
            fqn: fqn.into(),
            file_path: string(node, "original_file_path").map(PathBuf::from),
            description: string(node, "description").map(Into::into),
            owner: meta.and_then(|m| string(m, "owner")).map(Into::into),
            row_count: entry.and_then(|e| stat_u64(e, "row_count")).filter(|_| is_table),
            bytes: entry.and_then(|e| stat_u64(e, "bytes")).filter(|_| is_table),
            columns,
        };
        grouped
            .entry((database.to_uppercase(), schema.to_uppercase()))
            .or_default()
            .push(relation);
    };

    if let Some(nodes) = manifest.get("nodes").and_then(|n| n.as_object()) {
        for (uid, node) in nodes {
            let kind = match node.get("resource_type").and_then(|t| t.as_str()) {
                Some("model") => RelationKind::Model {
                    materialized: node
                        .get("config")
                        .and_then(|c| string(c, "materialized"))
                        .unwrap_or_else(|| "view".to_owned())
                        .into(),
                },
                Some("seed") => RelationKind::Seed,
                Some("snapshot") => RelationKind::Snapshot,
                _ => continue, // tests, operations, analyses
            };
            add_relation(uid, node, kind);
        }
    }
    if let Some(sources) = manifest.get("sources").and_then(|s| s.as_object()) {
        for (uid, node) in sources {
            let source_name = string(node, "source_name").unwrap_or_default();
            add_relation(
                uid,
                node,
                RelationKind::Source {
                    source_name: source_name.into(),
                },
            );
        }
    }

    // Regroup into the sorted tree.
    let mut by_db: HashMap<String, Vec<(String, Vec<DbRelation>)>> = HashMap::new();
    for ((db, schema), mut relations) in grouped {
        relations.sort_by(|a, b| a.name.cmp(&b.name));
        by_db.entry(db).or_default().push((schema, relations));
    }
    let mut databases: Vec<DbDatabase> = by_db
        .into_iter()
        .map(|(name, mut schemas)| {
            schemas.sort_by(|a, b| a.0.cmp(&b.0));
            DbDatabase {
                name: name.into(),
                schemas: schemas
                    .into_iter()
                    .map(|(name, relations)| DbSchema {
                        name: name.into(),
                        relations,
                    })
                    .collect(),
            }
        })
        .collect();
    databases.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(DbCatalog {
        adapter,
        catalog_generated_at,
        catalog_present: catalog.is_some(),
        databases,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_artifacts(dir: &Path, manifest: &str, catalog: Option<&str>) {
        let target = dir.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("manifest.json"), manifest).unwrap();
        if let Some(catalog) = catalog {
            std::fs::write(target.join("catalog.json"), catalog).unwrap();
        }
    }

    const MANIFEST: &str = r#"{
        "metadata": {"adapter_type": "snowflake"},
        "nodes": {
            "model.p.orders": {
                "resource_type": "model", "database": "analytics", "schema": "core",
                "name": "orders", "alias": "orders",
                "relation_name": "analytics.core.orders",
                "original_file_path": "models/core/orders.sql",
                "description": "All orders",
                "config": {"materialized": "table"},
                "columns": {"Order_ID": {"description": "pk"}}
            },
            "test.p.not_null_orders": {"resource_type": "test", "database": "analytics", "schema": "core", "name": "t"}
        },
        "sources": {
            "source.p.raw.orders": {
                "resource_type": "source", "database": "raw_db", "schema": "landing",
                "name": "orders", "source_name": "raw",
                "relation_name": "raw_db.landing.orders"
            }
        }
    }"#;

    const CATALOG: &str = r#"{
        "metadata": {"generated_at": "2026-09-04T00:00:00Z"},
        "nodes": {
            "model.p.orders": {
                "metadata": {"type": "BASE TABLE", "database": "ANALYTICS", "schema": "CORE", "name": "ORDERS", "owner": "ETL", "comment": null},
                "columns": {
                    "AMOUNT": {"type": "NUMBER", "index": 2, "name": "AMOUNT", "comment": null},
                    "ORDER_ID": {"type": "TEXT", "index": 1, "name": "ORDER_ID", "comment": null}
                },
                "stats": {
                    "row_count": {"id": "row_count", "value": 42, "include": true},
                    "bytes": {"id": "bytes", "value": 1024, "include": true}
                }
            }
        },
        "sources": {},
        "errors": null
    }"#;

    #[test]
    fn builds_tree_with_catalog_overlay() {
        let dir = tempfile::tempdir().unwrap();
        write_artifacts(dir.path(), MANIFEST, Some(CATALOG));
        let catalog = build_catalog(dir.path()).unwrap();

        assert!(catalog.catalog_present);
        assert_eq!(catalog.adapter.as_deref(), Some("snowflake"));
        assert_eq!(catalog.relation_count(), 2); // the test node is skipped

        // Case-folded grouping: manifest "analytics"/"core" and catalog
        // "ANALYTICS"/"CORE" land in one node.
        let db = catalog
            .databases
            .iter()
            .find(|db| db.name.as_ref() == "ANALYTICS")
            .unwrap();
        let schema = &db.schemas[0];
        assert_eq!(schema.name.as_ref(), "CORE");
        let orders = &schema.relations[0];
        assert_eq!(orders.name.as_ref(), "ORDERS"); // warehouse casing wins
        assert_eq!(orders.object_type.as_deref(), Some("BASE TABLE"));
        assert_eq!(orders.row_count, Some(42));
        assert_eq!(orders.fqn.as_ref(), "analytics.core.orders");

        // Columns ordered by ordinal, docs overlaid case-insensitively.
        let ColumnState::Known(cols) = &orders.columns else {
            panic!("catalog entry present, columns must be Known")
        };
        assert_eq!(cols[0].name.as_ref(), "ORDER_ID");
        assert_eq!(cols[0].description.as_deref(), Some("pk"));
        assert_eq!(cols[1].name.as_ref(), "AMOUNT");
        assert_eq!(cols[1].data_type.as_deref(), Some("NUMBER"));
    }

    #[test]
    fn missing_catalog_yields_unknown_columns_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        write_artifacts(dir.path(), MANIFEST, None);
        let catalog = build_catalog(dir.path()).unwrap();
        assert!(!catalog.catalog_present);
        let db = &catalog.databases[0];
        for schema in &db.schemas {
            for relation in &schema.relations {
                assert!(matches!(relation.columns, ColumnState::Unknown));
                assert!(relation.row_count.is_none());
            }
        }
    }

    #[test]
    fn sources_without_catalog_entry_stay_unknown() {
        let dir = tempfile::tempdir().unwrap();
        write_artifacts(dir.path(), MANIFEST, Some(CATALOG));
        let catalog = build_catalog(dir.path()).unwrap();
        let raw = catalog
            .databases
            .iter()
            .find(|db| db.name.as_ref() == "RAW_DB")
            .unwrap();
        let source = &raw.schemas[0].relations[0];
        assert!(matches!(source.kind, RelationKind::Source { .. }));
        assert!(matches!(source.columns, ColumnState::Unknown));
    }

    /// Full-fidelity run over a real project's artifacts; set
    /// `DBT_PROJECT_SMOKE` to the project root. Prints structure counts only.
    #[test]
    #[ignore]
    fn builds_from_a_real_project() {
        let root = std::env::var("DBT_PROJECT_SMOKE").expect("set DBT_PROJECT_SMOKE");
        let catalog = build_catalog(Path::new(&root)).unwrap();
        let schemas: usize = catalog.databases.iter().map(|d| d.schemas.len()).sum();
        let columns: usize = catalog
            .databases
            .iter()
            .flat_map(|d| &d.schemas)
            .flat_map(|s| &s.relations)
            .map(|r| match &r.columns {
                ColumnState::Known(cols) => cols.len(),
                ColumnState::Unknown => 0,
            })
            .sum();
        println!(
            "databases={} schemas={} relations={} columns={}",
            catalog.databases.len(),
            schemas,
            catalog.relation_count(),
            columns
        );
        assert!(catalog.relation_count() > 0);
    }
}
