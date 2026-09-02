use super::{
    CSR_SHARD_INCOMING, CSR_SHARD_OUTGOING, EdgeDirection, HashMap, SqliteGraphError, V3Backend,
    map_v3_error,
};
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

type CsrRuntimeEntry = (u32, f32, String);
type CsrRuntimeRow = (i64, Vec<CsrRuntimeEntry>);

impl V3Backend {
    pub(crate) fn rebuild_csr_runtime_views(
        &self,
        visible_version: u64,
    ) -> Result<(), SqliteGraphError> {
        let node_ids = self.get_all_node_ids()?;

        let (outgoing_rows, incoming_rows, distinct_types) = {
            let edge_store = self.edge_store.read();
            let edge_types = edge_store.edge_types_lock().read();
            let mut distinct_types = BTreeSet::new();

            let collect_rows =
                |direction: EdgeDirection| -> Result<Vec<CsrRuntimeRow>, SqliteGraphError> {
                    node_ids
                        .iter()
                        .map(|&node_id| {
                            let weighted = edge_store
                                .neighbors_weighted(node_id, direction)
                                .map_err(map_v3_error)?;
                            let entries = weighted
                                .iter()
                                .filter_map(|&(dst, weight)| {
                                    edge_types
                                        .get(&(node_id, dst, direction))
                                        .cloned()
                                        .map(|edge_type| (dst as u32, weight, edge_type))
                                })
                                .collect();
                            Ok((node_id, entries))
                        })
                        .collect()
                };

            let outgoing_rows = collect_rows(EdgeDirection::Outgoing)?;
            let incoming_rows = collect_rows(EdgeDirection::Incoming)?;

            for (_, entries) in outgoing_rows.iter().chain(incoming_rows.iter()) {
                for (_, _, edge_type) in entries {
                    distinct_types.insert(edge_type.clone());
                }
            }

            (outgoing_rows, incoming_rows, distinct_types)
        };

        let type_ids = self.ensure_csr_edge_type_ids(distinct_types.into_iter().collect())?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let conn = self.sqlite_conn.lock();
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
        .map_err(|e| {
            SqliteGraphError::connection(format!("Failed to ensure csr_shards table: {}", e))
        })?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS csr_edge_types (
                type_id INTEGER PRIMARY KEY,
                edge_type TEXT NOT NULL UNIQUE
            )",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::connection(format!("Failed to ensure csr_edge_types table: {}", e))
        })?;

        let insert_rows = |shard_id: i64,
                           rows: &Vec<(i64, Vec<(u32, f32, String)>)>|
         -> Result<(), SqliteGraphError> {
            for (node_id, entries) in rows {
                let encoded_entries: Vec<(u32, f32, u32)> = entries
                    .iter()
                    .map(|&(dst, weight, ref edge_type)| {
                        let type_id = *type_ids.get(edge_type).expect("CSR edge type id missing");
                        (dst, weight, type_id)
                    })
                    .collect();
                let blob = super::encode_csr_adjacency(&encoded_entries);
                conn.execute(
                    "INSERT OR REPLACE INTO csr_shards
                         (shard_id, node_id, shard_data, version, created_at, visible_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        shard_id,
                        *node_id,
                        blob,
                        visible_version as i64,
                        timestamp,
                        visible_version as i64
                    ],
                )
                .map_err(|e| {
                    SqliteGraphError::connection(format!("Failed to write CSR shard row: {}", e))
                })?;
            }
            Ok(())
        };

        insert_rows(CSR_SHARD_OUTGOING, &outgoing_rows)?;
        insert_rows(CSR_SHARD_INCOMING, &incoming_rows)?;

        Ok(())
    }

    pub(crate) fn ensure_csr_edge_type_ids(
        &self,
        edge_types: Vec<String>,
    ) -> Result<HashMap<String, u32>, SqliteGraphError> {
        let conn = self.sqlite_conn.lock();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS csr_edge_types (
                type_id INTEGER PRIMARY KEY,
                edge_type TEXT NOT NULL UNIQUE
            )",
            [],
        )
        .map_err(|e| {
            SqliteGraphError::connection(format!("Failed to ensure csr_edge_types table: {}", e))
        })?;

        let mut existing = HashMap::new();
        let mut next_type_id = 1_u32;
        {
            let mut stmt = conn
                .prepare("SELECT type_id, edge_type FROM csr_edge_types")
                .map_err(|e| {
                    SqliteGraphError::query(format!("Failed to prepare CSR edge type scan: {}", e))
                })?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, i64>(0)? as u32, row.get::<_, String>(1)?))
                })
                .map_err(|e| {
                    SqliteGraphError::query(format!("Failed to scan CSR edge types: {}", e))
                })?;
            for row in rows {
                let (type_id, edge_type) = row.map_err(|e| {
                    SqliteGraphError::query(format!("Failed to read CSR edge type row: {}", e))
                })?;
                next_type_id = next_type_id.max(type_id.saturating_add(1));
                existing.insert(edge_type, type_id);
            }
        }

        for edge_type in edge_types {
            if existing.contains_key(&edge_type) {
                continue;
            }
            conn.execute(
                "INSERT INTO csr_edge_types (type_id, edge_type) VALUES (?1, ?2)",
                rusqlite::params![next_type_id as i64, edge_type],
            )
            .map_err(|e| {
                SqliteGraphError::connection(format!("Failed to insert CSR edge type: {}", e))
            })?;
            existing.insert(edge_type, next_type_id);
            next_type_id = next_type_id.saturating_add(1);
        }

        Ok(existing)
    }
}
