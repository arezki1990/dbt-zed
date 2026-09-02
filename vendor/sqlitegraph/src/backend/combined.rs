use rusqlite::OptionalExtension;

use crate::{
    SqliteGraphError,
    backend::{
        BackendDirection, BackupResult, GraphBackend, ImportMetadata, PubSubEvent,
        SnapshotMetadata, SqliteGraphBackend, SubscriptionFilter,
    },
    config::{CombinedConfig, CombinedReadMode},
    graph::{GraphEntity, SqliteGraph},
    multi_hop::ChainStep,
    pattern::{PatternMatch, PatternQuery},
    pattern_engine::{PatternTriple, TripleMatch},
    snapshot::SnapshotId,
};

use super::sqlite::types::{EdgeSpec, NeighborQuery, NodeSpec};

fn encode_csr_adjacency(entries: &[(u32, f32, u32)]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(entries.len() * 12);
    for &(dst, weight, flags) in entries {
        buf.extend_from_slice(&dst.to_le_bytes());
        buf.extend_from_slice(&weight.to_le_bytes());
        buf.extend_from_slice(&flags.to_le_bytes());
    }
    buf
}

fn decode_csr_adjacency(blob: &[u8]) -> Result<Vec<(u32, f32, u32)>, SqliteGraphError> {
    if !blob.len().is_multiple_of(12) {
        return Err(SqliteGraphError::GraphCorruption(format!(
            "combined CSR shard_data has invalid length {} (expected 12-byte records)",
            blob.len()
        )));
    }

    let mut entries = Vec::with_capacity(blob.len() / 12);
    for chunk in blob.chunks_exact(12) {
        entries.push((
            u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
            f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
            u32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]),
        ));
    }
    Ok(entries)
}

/// SQLite-authoritative backend mode.
///
/// In the current Phase 2 slice, this is an explicit composition point over the
/// SQLite backend. It preserves today's SQLite execution behavior while giving
/// combined mode its own public type and future materialization seam.
pub struct CombinedGraphBackend {
    sqlite: SqliteGraphBackend,
    config: CombinedConfig,
}

impl CombinedGraphBackend {
    const CSR_SHARD_OUTGOING: i64 = 0;
    const CSR_SHARD_INCOMING: i64 = 1;

    /// Create a new combined backend with an in-memory SQLite authority store.
    pub fn in_memory() -> Result<Self, SqliteGraphError> {
        Self::in_memory_with_config(CombinedConfig::default())
    }

    /// Create a new combined backend with an in-memory SQLite authority store
    /// and explicit combined-mode configuration.
    pub fn in_memory_with_config(config: CombinedConfig) -> Result<Self, SqliteGraphError> {
        Ok(Self {
            sqlite: SqliteGraphBackend::in_memory()?,
            config,
        })
    }

    /// Create a combined backend from an existing SQLite backend.
    pub fn from_sqlite_backend(sqlite: SqliteGraphBackend) -> Self {
        Self::from_sqlite_backend_with_config(sqlite, CombinedConfig::default())
    }

    /// Create a combined backend from an existing SQLite backend and
    /// combined-mode configuration.
    pub fn from_sqlite_backend_with_config(
        sqlite: SqliteGraphBackend,
        config: CombinedConfig,
    ) -> Self {
        Self { sqlite, config }
    }

    /// Create a combined backend from an existing `SqliteGraph`.
    pub fn from_graph(graph: SqliteGraph) -> Self {
        Self::from_graph_with_config(graph, CombinedConfig::default())
    }

    /// Create a combined backend from an existing `SqliteGraph` and
    /// combined-mode configuration.
    pub fn from_graph_with_config(graph: SqliteGraph, config: CombinedConfig) -> Self {
        Self::from_sqlite_backend_with_config(SqliteGraphBackend::from_graph(graph), config)
    }

    /// Access the authoritative SQLite backend used by combined mode.
    pub fn sqlite_backend(&self) -> &SqliteGraphBackend {
        &self.sqlite
    }

    /// Access the authoritative SQLite graph used by combined mode.
    pub fn graph(&self) -> &SqliteGraph {
        self.sqlite.graph()
    }

    fn materialized_maintenance_enabled(&self) -> bool {
        self.config.read_mode == CombinedReadMode::PreferMaterialized
    }

    fn ensure_csr_tables(
        conn: &crate::graph::ConnectionWrapper<'_>,
    ) -> Result<(), SqliteGraphError> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS csr_manifest (
                shard_id INTEGER PRIMARY KEY,
                source_start INTEGER NOT NULL,
                source_end INTEGER NOT NULL,
                node_count INTEGER NOT NULL,
                edge_count INTEGER NOT NULL,
                conflict_resolution TEXT NOT NULL DEFAULT 'last-write-wins',
                created_at INTEGER NOT NULL
            )",
            [],
        )
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
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
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS csr_edge_types (
                type_id INTEGER PRIMARY KEY,
                edge_type TEXT NOT NULL UNIQUE
            )",
            [],
        )
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        Ok(())
    }

    fn ensure_edge_type_ids(
        conn: &crate::graph::ConnectionWrapper<'_>,
        edge_types: &std::collections::BTreeSet<String>,
    ) -> Result<std::collections::HashMap<String, u32>, SqliteGraphError> {
        let mut existing = std::collections::HashMap::with_capacity(edge_types.len());
        let mut lookup_stmt = conn
            .prepare_cached("SELECT type_id FROM csr_edge_types WHERE edge_type = ?1")
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        let mut insert_stmt = conn
            .prepare_cached("INSERT OR IGNORE INTO csr_edge_types (edge_type) VALUES (?1)")
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        for edge_type in edge_types {
            let existing_id = lookup_stmt
                .query_row(rusqlite::params![edge_type], |row| row.get::<_, i64>(0))
                .optional()
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;
            let type_id = match existing_id {
                Some(type_id) => type_id as u32,
                None => {
                    insert_stmt
                        .execute(rusqlite::params![edge_type])
                        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                    lookup_stmt
                        .query_row(rusqlite::params![edge_type], |row| row.get::<_, i64>(0))
                        .map_err(|e| SqliteGraphError::query(e.to_string()))?
                        as u32
                }
            };
            existing.insert(edge_type.clone(), type_id);
        }

        if existing.len() != edge_types.len() {
            return Err(SqliteGraphError::query(
                "Failed to resolve all combined CSR edge type ids".to_string(),
            ));
        }

        Ok(existing)
    }

    fn load_materialized_row_entries(
        conn: &crate::graph::ConnectionWrapper<'_>,
        node_id: i64,
        direction: BackendDirection,
    ) -> Result<Vec<(u32, f32, u32)>, SqliteGraphError> {
        let mut stmt = match conn.prepare_cached(
            "SELECT shard_data
             FROM csr_shards
             WHERE shard_id = ?1 AND node_id = ?2
             ORDER BY version DESC
             LIMIT 1",
        ) {
            Ok(stmt) => stmt,
            Err(e) if e.to_string().contains("no such table: csr_shards") => return Ok(Vec::new()),
            Err(e) => {
                return Err(SqliteGraphError::query(format!(
                    "Failed to prepare combined CSR row lookup: {}",
                    e
                )));
            }
        };

        let blob = stmt
            .query_row(
                rusqlite::params![Self::csr_shard_id(direction), node_id],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()
            .map_err(|e| {
                SqliteGraphError::query(format!("Failed to execute combined CSR row lookup: {}", e))
            })?
            .flatten();

        match blob {
            Some(blob) => decode_csr_adjacency(&blob),
            None => Ok(Vec::new()),
        }
    }

    fn write_materialized_encoded_row(
        conn: &crate::graph::ConnectionWrapper<'_>,
        node_id: i64,
        direction: BackendDirection,
        version: i64,
        timestamp: i64,
        entries: &[(u32, f32, u32)],
    ) -> Result<(), SqliteGraphError> {
        let blob = encode_csr_adjacency(entries);
        conn.execute(
            "INSERT OR REPLACE INTO csr_shards
             (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                Self::csr_shard_id(direction),
                node_id,
                blob,
                version,
                timestamp,
                version
            ],
        )
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        Ok(())
    }

    fn insert_encoded_entry(entries: &mut Vec<(u32, f32, u32)>, entry: (u32, f32, u32)) {
        let insert_at = entries.partition_point(|(dst, _, type_id)| {
            *dst < entry.0 || (*dst == entry.0 && *type_id <= entry.2)
        });
        entries.insert(insert_at, entry);
    }

    fn remove_encoded_entries(entries: &mut Vec<(u32, f32, u32)>, dst: u32) {
        entries.retain(|(entry_dst, _, _)| *entry_dst != dst);
    }

    fn refresh_inserted_edge_materialized_rows(
        &self,
        edge: &EdgeSpec,
    ) -> Result<(), SqliteGraphError> {
        if !self.materialized_maintenance_enabled() {
            return Ok(());
        }

        let version = self.graph().get_authoritative_version()?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let conn = self.graph().connection();
        conn.underlying()
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        let refresh_result: Result<(), SqliteGraphError> = (|| {
            Self::ensure_csr_tables(&conn)?;

            let mut edge_types = std::collections::BTreeSet::new();
            edge_types.insert(edge.edge_type.clone());
            let type_ids = Self::ensure_edge_type_ids(&conn, &edge_types)?;
            let type_id = *type_ids
                .get(&edge.edge_type)
                .expect("combined CSR edge type id missing");

            let mut outgoing_entries =
                Self::load_materialized_row_entries(&conn, edge.from, BackendDirection::Outgoing)?;
            Self::insert_encoded_entry(&mut outgoing_entries, (edge.to as u32, 1.0, type_id));
            Self::write_materialized_encoded_row(
                &conn,
                edge.from,
                BackendDirection::Outgoing,
                version,
                timestamp,
                &outgoing_entries,
            )?;

            let mut incoming_entries =
                Self::load_materialized_row_entries(&conn, edge.to, BackendDirection::Incoming)?;
            Self::insert_encoded_entry(&mut incoming_entries, (edge.from as u32, 1.0, type_id));
            Self::write_materialized_encoded_row(
                &conn,
                edge.to,
                BackendDirection::Incoming,
                version,
                timestamp,
                &incoming_entries,
            )?;

            conn.execute(
                "UPDATE graph_meta SET materialized_version = ?1 WHERE id = 1",
                rusqlite::params![version],
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
            Ok(())
        })();

        match refresh_result {
            Ok(()) => {
                conn.underlying()
                    .execute_batch("COMMIT")
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                Ok(())
            }
            Err(err) => {
                let _ = conn.underlying().execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    fn refresh_deleted_entity_materialized_rows(
        &self,
        id: i64,
        outgoing_nodes: &[i64],
        incoming_nodes: &[i64],
    ) -> Result<(), SqliteGraphError> {
        if !self.materialized_maintenance_enabled() {
            return Ok(());
        }

        let version = self.graph().get_authoritative_version()?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let conn = self.graph().connection();
        conn.underlying()
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        let refresh_result: Result<(), SqliteGraphError> = (|| {
            Self::ensure_csr_tables(&conn)?;

            for &node_id in outgoing_nodes {
                let mut entries = Self::load_materialized_row_entries(
                    &conn,
                    node_id,
                    BackendDirection::Outgoing,
                )?;
                Self::remove_encoded_entries(&mut entries, id as u32);
                Self::write_materialized_encoded_row(
                    &conn,
                    node_id,
                    BackendDirection::Outgoing,
                    version,
                    timestamp,
                    &entries,
                )?;
            }

            for &node_id in incoming_nodes {
                let mut entries = Self::load_materialized_row_entries(
                    &conn,
                    node_id,
                    BackendDirection::Incoming,
                )?;
                Self::remove_encoded_entries(&mut entries, id as u32);
                Self::write_materialized_encoded_row(
                    &conn,
                    node_id,
                    BackendDirection::Incoming,
                    version,
                    timestamp,
                    &entries,
                )?;
            }

            Self::write_materialized_encoded_row(
                &conn,
                id,
                BackendDirection::Outgoing,
                version,
                timestamp,
                &[],
            )?;
            Self::write_materialized_encoded_row(
                &conn,
                id,
                BackendDirection::Incoming,
                version,
                timestamp,
                &[],
            )?;

            conn.execute(
                "UPDATE graph_meta SET materialized_version = ?1 WHERE id = 1",
                rusqlite::params![version],
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
            Ok(())
        })();

        match refresh_result {
            Ok(()) => {
                conn.underlying()
                    .execute_batch("COMMIT")
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                Ok(())
            }
            Err(err) => {
                let _ = conn.underlying().execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    fn sync_materialized_version_only(&self) -> Result<(), SqliteGraphError> {
        if !self.materialized_maintenance_enabled() {
            return Ok(());
        }
        let version = self.graph().get_authoritative_version()?;
        self.graph().set_materialized_version(version)
    }

    /// Rebuild and publish combined-mode materialized traversal views from
    /// authoritative SQLite truth.
    ///
    /// Returns the published materialized version, which matches the current
    /// authoritative SQLite version at publish time.
    pub fn publish_materialized_views(&self) -> Result<i64, SqliteGraphError> {
        let authoritative_version = self.graph().get_authoritative_version()?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let conn = self.graph().connection();
        conn.underlying()
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        let publish_result: Result<(), SqliteGraphError> = (|| {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS csr_manifest (
                    shard_id INTEGER PRIMARY KEY,
                    source_start INTEGER NOT NULL,
                    source_end INTEGER NOT NULL,
                    node_count INTEGER NOT NULL,
                    edge_count INTEGER NOT NULL,
                    conflict_resolution TEXT NOT NULL DEFAULT 'last-write-wins',
                    created_at INTEGER NOT NULL
                )",
                [],
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
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
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS csr_edge_types (
                    type_id INTEGER PRIMARY KEY,
                    edge_type TEXT NOT NULL UNIQUE
                )",
                [],
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

            let mut rows = conn
                .prepare_cached(
                    "SELECT from_id, to_id, edge_type
                     FROM graph_edges
                     ORDER BY from_id, to_id, edge_type, id",
                )
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;

            let edge_rows = rows
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;

            let mut outgoing: std::collections::BTreeMap<i64, Vec<(u32, f32, String)>> =
                std::collections::BTreeMap::new();
            let mut incoming: std::collections::BTreeMap<i64, Vec<(u32, f32, String)>> =
                std::collections::BTreeMap::new();
            let mut edge_types = std::collections::BTreeSet::new();
            let mut min_source = i64::MAX;
            let mut max_source = i64::MIN;
            let mut edge_count = 0_i64;

            for row in edge_rows {
                let (from_id, to_id, edge_type) =
                    row.map_err(|e| SqliteGraphError::query(e.to_string()))?;
                outgoing
                    .entry(from_id)
                    .or_default()
                    .push((to_id as u32, 1.0, edge_type.clone()));
                incoming
                    .entry(to_id)
                    .or_default()
                    .push((from_id as u32, 1.0, edge_type.clone()));
                edge_types.insert(edge_type);
                min_source = min_source.min(from_id).min(to_id);
                max_source = max_source.max(from_id).max(to_id);
                edge_count += 1;
            }

            let mut existing_type_ids = std::collections::HashMap::new();
            let mut next_type_id = 1_i64;
            {
                let mut stmt = conn
                    .prepare_cached(
                        "SELECT type_id, edge_type FROM csr_edge_types ORDER BY type_id",
                    )
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                for row in rows {
                    let (type_id, edge_type) =
                        row.map_err(|e| SqliteGraphError::query(e.to_string()))?;
                    next_type_id = next_type_id.max(type_id + 1);
                    existing_type_ids.insert(edge_type, type_id as u32);
                }
            }
            for edge_type in edge_types {
                if existing_type_ids.contains_key(&edge_type) {
                    continue;
                }
                conn.execute(
                    "INSERT INTO csr_edge_types (type_id, edge_type) VALUES (?1, ?2)",
                    rusqlite::params![next_type_id, edge_type],
                )
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                existing_type_ids.insert(edge_type, next_type_id as u32);
                next_type_id += 1;
            }

            let insert_rows = |shard_id: i64,
                               map: &std::collections::BTreeMap<i64, Vec<(u32, f32, String)>>|
             -> Result<(), SqliteGraphError> {
                for (node_id, entries) in map {
                    let encoded_entries: Vec<(u32, f32, u32)> = entries
                        .iter()
                        .map(|(dst, weight, edge_type)| {
                            let type_id = *existing_type_ids
                                .get(edge_type)
                                .expect("combined CSR edge type id missing");
                            (*dst, *weight, type_id)
                        })
                        .collect();
                    let blob = encode_csr_adjacency(&encoded_entries);
                    conn.execute(
                        "INSERT OR REPLACE INTO csr_shards
                         (shard_id, node_id, shard_data, version, created_at, visible_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            shard_id,
                            *node_id,
                            blob,
                            authoritative_version,
                            timestamp,
                            authoritative_version
                        ],
                    )
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                }
                Ok(())
            };

            insert_rows(Self::CSR_SHARD_OUTGOING, &outgoing)?;
            insert_rows(Self::CSR_SHARD_INCOMING, &incoming)?;

            let source_start = if min_source == i64::MAX {
                0
            } else {
                min_source
            };
            let source_end = if max_source == i64::MIN {
                0
            } else {
                max_source + 1
            };
            let node_count = outgoing
                .keys()
                .chain(incoming.keys())
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len() as i64;

            conn.execute(
                "INSERT INTO csr_manifest
                 (shard_id, source_start, source_end, node_count, edge_count, conflict_resolution, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'last-write-wins', ?6)
                 ON CONFLICT(shard_id) DO UPDATE SET
                    source_start = excluded.source_start,
                    source_end = excluded.source_end,
                    node_count = excluded.node_count,
                    edge_count = excluded.edge_count,
                    conflict_resolution = excluded.conflict_resolution,
                    created_at = excluded.created_at",
                rusqlite::params![
                    Self::CSR_SHARD_OUTGOING,
                    source_start,
                    source_end,
                    node_count,
                    edge_count,
                    timestamp
                ],
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
            conn.execute(
                "INSERT INTO csr_manifest
                 (shard_id, source_start, source_end, node_count, edge_count, conflict_resolution, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'last-write-wins', ?6)
                 ON CONFLICT(shard_id) DO UPDATE SET
                    source_start = excluded.source_start,
                    source_end = excluded.source_end,
                    node_count = excluded.node_count,
                    edge_count = excluded.edge_count,
                    conflict_resolution = excluded.conflict_resolution,
                    created_at = excluded.created_at",
                rusqlite::params![
                    Self::CSR_SHARD_INCOMING,
                    source_start,
                    source_end,
                    node_count,
                    edge_count,
                    timestamp
                ],
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

            conn.execute(
                "UPDATE graph_meta SET materialized_version = ?1 WHERE id = 1",
                rusqlite::params![authoritative_version],
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

            Ok(())
        })();

        match publish_result {
            Ok(()) => {
                conn.underlying()
                    .execute_batch("COMMIT")
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                Ok(authoritative_version)
            }
            Err(err) => {
                let _ = conn.underlying().execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    fn should_prefer_materialized_reads(
        &self,
        snapshot_id: SnapshotId,
        query: Option<&NeighborQuery>,
    ) -> bool {
        if self.config.read_mode != CombinedReadMode::PreferMaterialized {
            return false;
        }
        if snapshot_id != SnapshotId::current() {
            return false;
        }
        let authoritative_version = match self.graph().get_authoritative_version() {
            Ok(version) => version,
            Err(_) => return false,
        };
        let materialized_version = match self.graph().get_materialized_version() {
            Ok(version) => version,
            Err(_) => return false,
        };
        if materialized_version < authoritative_version {
            return false;
        }
        query.is_none_or(|query| query.edge_type.is_none())
    }

    fn csr_shard_id(direction: BackendDirection) -> i64 {
        match direction {
            BackendDirection::Outgoing => Self::CSR_SHARD_OUTGOING,
            BackendDirection::Incoming => Self::CSR_SHARD_INCOMING,
        }
    }

    fn load_materialized_neighbors(
        &self,
        node_id: i64,
        direction: BackendDirection,
    ) -> Result<Option<Vec<i64>>, SqliteGraphError> {
        let visible_version = self.graph().get_materialized_version()?;
        let conn = self.graph().connection();
        let mut stmt = match conn.prepare_cached(
            "SELECT shard_data
             FROM csr_shards
             WHERE shard_id = ?1 AND node_id = ?2 AND visible_at <= ?3
             ORDER BY version DESC
             LIMIT 1",
        ) {
            Ok(stmt) => stmt,
            Err(e) if e.to_string().contains("no such table: csr_shards") => return Ok(None),
            Err(e) => {
                return Err(SqliteGraphError::query(format!(
                    "Failed to prepare combined CSR lookup: {}",
                    e
                )));
            }
        };

        let blob = stmt
            .query_row(
                rusqlite::params![Self::csr_shard_id(direction), node_id, visible_version],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()
            .map_err(|e| {
                SqliteGraphError::query(format!("Failed to execute combined CSR lookup: {}", e))
            })?;

        let Some(blob) = blob else {
            return Ok(None);
        };

        let Some(blob) = blob else {
            return Ok(Some(Vec::new()));
        };

        if blob.len() % 12 != 0 {
            return Err(SqliteGraphError::GraphCorruption(format!(
                "combined CSR shard_data for node {} has invalid length {} (expected 12-byte records)",
                node_id,
                blob.len()
            )));
        }

        let mut neighbors = Vec::with_capacity(blob.len() / 12);
        for chunk in blob.chunks_exact(12) {
            let dst = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as i64;
            neighbors.push(dst);
        }
        Ok(Some(neighbors))
    }

    fn bfs_materialized_or_fallback(
        &self,
        start: i64,
        depth: u32,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        let mut visited = Vec::new();
        let mut seen = ahash::AHashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back((start, 0_u32));
        seen.insert(start);

        while let Some((node, d)) = queue.pop_front() {
            visited.push(node);
            if d >= depth {
                continue;
            }

            let neighbors =
                match self.load_materialized_neighbors(node, BackendDirection::Outgoing)? {
                    Some(neighbors) => neighbors,
                    None => self.sqlite.neighbors(
                        SnapshotId::current(),
                        node,
                        NeighborQuery {
                            direction: BackendDirection::Outgoing,
                            edge_type: None,
                        },
                    )?,
                };

            for next in neighbors {
                if seen.insert(next) {
                    queue.push_back((next, d + 1));
                }
            }
        }

        Ok(visited)
    }

    fn k_hop_materialized_or_fallback(
        &self,
        start: i64,
        depth: u32,
        direction: BackendDirection,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        if depth == 0 {
            return Ok(Vec::new());
        }

        let mut visited = ahash::AHashSet::new();
        let mut queue = std::collections::VecDeque::new();
        let mut ordered = Vec::new();

        visited.insert(start);
        queue.push_back((start, 0_u32));

        while let Some((node, level)) = queue.pop_front() {
            if level == depth {
                continue;
            }

            let neighbors = match self.load_materialized_neighbors(node, direction)? {
                Some(neighbors) => neighbors,
                None => self.sqlite.neighbors(
                    SnapshotId::current(),
                    node,
                    NeighborQuery {
                        direction,
                        edge_type: None,
                    },
                )?,
            };

            for neighbor in neighbors {
                if visited.insert(neighbor) {
                    ordered.push((level + 1, neighbor));
                    queue.push_back((neighbor, level + 1));
                }
            }
        }

        ordered.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        Ok(ordered.into_iter().map(|(_, node)| node).collect())
    }

    fn node_degree_materialized_or_fallback(
        &self,
        node: i64,
    ) -> Result<(usize, usize), SqliteGraphError> {
        let outgoing = match self.load_materialized_neighbors(node, BackendDirection::Outgoing)? {
            Some(neighbors) => neighbors.len(),
            None => self
                .sqlite
                .neighbors(
                    SnapshotId::current(),
                    node,
                    NeighborQuery {
                        direction: BackendDirection::Outgoing,
                        edge_type: None,
                    },
                )?
                .len(),
        };

        let incoming = match self.load_materialized_neighbors(node, BackendDirection::Incoming)? {
            Some(neighbors) => neighbors.len(),
            None => self
                .sqlite
                .neighbors(
                    SnapshotId::current(),
                    node,
                    NeighborQuery {
                        direction: BackendDirection::Incoming,
                        edge_type: None,
                    },
                )?
                .len(),
        };

        Ok((outgoing, incoming))
    }

    fn shortest_path_materialized_or_fallback(
        &self,
        start: i64,
        end: i64,
    ) -> Result<Option<Vec<i64>>, SqliteGraphError> {
        let _ = self.sqlite.get_node(SnapshotId::current(), start)?;
        let _ = self.sqlite.get_node(SnapshotId::current(), end)?;

        if start == end {
            return Ok(Some(vec![start]));
        }

        let mut parents = ahash::AHashMap::new();
        let mut seen = ahash::AHashSet::new();
        let mut queue = std::collections::VecDeque::new();

        seen.insert(start);
        queue.push_back(start);

        let mut found = false;
        while let Some(node) = queue.pop_front() {
            let neighbors =
                match self.load_materialized_neighbors(node, BackendDirection::Outgoing)? {
                    Some(neighbors) => neighbors,
                    None => self.sqlite.neighbors(
                        SnapshotId::current(),
                        node,
                        NeighborQuery {
                            direction: BackendDirection::Outgoing,
                            edge_type: None,
                        },
                    )?,
                };

            for next in neighbors {
                if seen.insert(next) {
                    parents.insert(next, node);
                    if next == end {
                        found = true;
                        break;
                    }
                    queue.push_back(next);
                }
            }
            if found {
                break;
            }
        }

        if !found {
            return Ok(None);
        }

        let mut path = vec![end];
        let mut current = end;
        while let Some(&parent) = parents.get(&current) {
            path.push(parent);
            if parent == start {
                break;
            }
            current = parent;
        }

        if path.last().map(|last| *last != start).unwrap_or(true) {
            return Ok(None);
        }

        path.reverse();
        Ok(Some(path))
    }
}

impl GraphBackend for CombinedGraphBackend {
    fn insert_node(&self, node: NodeSpec) -> Result<i64, SqliteGraphError> {
        let id = self.sqlite.insert_node(node)?;
        self.sync_materialized_version_only()?;
        Ok(id)
    }

    fn insert_edge(&self, edge: EdgeSpec) -> Result<i64, SqliteGraphError> {
        let id = self.sqlite.insert_edge(edge.clone())?;
        self.refresh_inserted_edge_materialized_rows(&edge)?;
        Ok(id)
    }

    fn update_node(&self, node_id: i64, node: NodeSpec) -> Result<i64, SqliteGraphError> {
        let id = self.sqlite.update_node(node_id, node)?;
        self.sync_materialized_version_only()?;
        Ok(id)
    }

    fn delete_entity(&self, id: i64) -> Result<(), SqliteGraphError> {
        let incoming = self.sqlite.neighbors(
            SnapshotId::current(),
            id,
            NeighborQuery {
                direction: BackendDirection::Incoming,
                edge_type: None,
            },
        )?;
        let outgoing = self.sqlite.neighbors(
            SnapshotId::current(),
            id,
            NeighborQuery {
                direction: BackendDirection::Outgoing,
                edge_type: None,
            },
        )?;

        self.sqlite.delete_entity(id)?;

        self.refresh_deleted_entity_materialized_rows(id, &incoming, &outgoing)
    }

    fn entity_ids(&self) -> Result<Vec<i64>, SqliteGraphError> {
        self.sqlite.entity_ids()
    }

    fn get_node(&self, snapshot_id: SnapshotId, id: i64) -> Result<GraphEntity, SqliteGraphError> {
        self.sqlite.get_node(snapshot_id, id)
    }

    fn neighbors(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
        query: NeighborQuery,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        if self.should_prefer_materialized_reads(snapshot_id, Some(&query))
            && let Some(neighbors) = self.load_materialized_neighbors(node, query.direction)?
        {
            return Ok(neighbors);
        }
        self.sqlite.neighbors(snapshot_id, node, query)
    }

    fn bfs(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        if self.should_prefer_materialized_reads(snapshot_id, None) {
            return self.bfs_materialized_or_fallback(start, depth);
        }
        self.sqlite.bfs(snapshot_id, start, depth)
    }

    fn shortest_path(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        end: i64,
    ) -> Result<Option<Vec<i64>>, SqliteGraphError> {
        if self.should_prefer_materialized_reads(snapshot_id, None) {
            return self.shortest_path_materialized_or_fallback(start, end);
        }
        self.sqlite.shortest_path(snapshot_id, start, end)
    }

    fn node_degree(
        &self,
        snapshot_id: SnapshotId,
        node: i64,
    ) -> Result<(usize, usize), SqliteGraphError> {
        if self.should_prefer_materialized_reads(snapshot_id, None) {
            return self.node_degree_materialized_or_fallback(node);
        }
        self.sqlite.node_degree(snapshot_id, node)
    }

    fn k_hop(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        if self.should_prefer_materialized_reads(snapshot_id, None) {
            return self.k_hop_materialized_or_fallback(start, depth, direction);
        }
        self.sqlite.k_hop(snapshot_id, start, depth, direction)
    }

    fn k_hop_filtered(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.sqlite
            .k_hop_filtered(snapshot_id, start, depth, direction, allowed_edge_types)
    }

    fn bfs_filtered(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        depth: u32,
        direction: BackendDirection,
        allowed_edge_types: &[&str],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.sqlite
            .bfs_filtered(snapshot_id, start, depth, direction, allowed_edge_types)
    }

    fn shortest_path_filtered(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        end: i64,
        allowed_edge_types: &[&str],
    ) -> Result<Option<Vec<i64>>, SqliteGraphError> {
        self.sqlite
            .shortest_path_filtered(snapshot_id, start, end, allowed_edge_types)
    }

    fn chain_query(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        chain: &[ChainStep],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.sqlite.chain_query(snapshot_id, start, chain)
    }

    fn pattern_search(
        &self,
        snapshot_id: SnapshotId,
        start: i64,
        pattern: &PatternQuery,
    ) -> Result<Vec<PatternMatch>, SqliteGraphError> {
        self.sqlite.pattern_search(snapshot_id, start, pattern)
    }

    fn match_triples(&self, pattern: &PatternTriple) -> Result<Vec<TripleMatch>, SqliteGraphError> {
        self.sqlite.match_triples(pattern)
    }

    fn vector_search(
        &self,
        snapshot: SnapshotId,
        index_name: &str,
        vector: &[f32],
        k: usize,
    ) -> Result<Vec<(i64, f32)>, SqliteGraphError> {
        self.sqlite.vector_search(snapshot, index_name, vector, k)
    }

    fn get_graph_ref(&self) -> Option<&SqliteGraph> {
        Some(self.graph())
    }

    fn checkpoint(&self) -> Result<(), SqliteGraphError> {
        self.sqlite.checkpoint()
    }

    fn flush(&self) -> Result<(), SqliteGraphError> {
        self.sqlite.flush()
    }

    fn backup(&self, backup_dir: &std::path::Path) -> Result<BackupResult, SqliteGraphError> {
        self.sqlite.backup(backup_dir)
    }

    fn snapshot_export(
        &self,
        export_dir: &std::path::Path,
    ) -> Result<SnapshotMetadata, SqliteGraphError> {
        self.sqlite.snapshot_export(export_dir)
    }

    fn snapshot_import(
        &self,
        import_dir: &std::path::Path,
    ) -> Result<ImportMetadata, SqliteGraphError> {
        self.sqlite.snapshot_import(import_dir)
    }

    fn kv_get(
        &self,
        snapshot_id: SnapshotId,
        key: &[u8],
    ) -> Result<Option<crate::backend::native::types::KvValue>, SqliteGraphError> {
        self.sqlite.kv_get(snapshot_id, key)
    }

    fn kv_set(
        &self,
        key: Vec<u8>,
        value: crate::backend::native::types::KvValue,
        ttl_seconds: Option<u64>,
    ) -> Result<(), SqliteGraphError> {
        self.sqlite.kv_set(key, value, ttl_seconds)
    }

    fn kv_delete(&self, key: &[u8]) -> Result<(), SqliteGraphError> {
        self.sqlite.kv_delete(key)
    }

    fn subscribe(
        &self,
        filter: SubscriptionFilter,
    ) -> Result<(u64, std::sync::mpsc::Receiver<PubSubEvent>), SqliteGraphError> {
        self.sqlite.subscribe(filter)
    }

    fn unsubscribe(&self, subscriber_id: u64) -> Result<bool, SqliteGraphError> {
        self.sqlite.unsubscribe(subscriber_id)
    }

    fn kv_prefix_scan(
        &self,
        snapshot_id: SnapshotId,
        prefix: &[u8],
    ) -> Result<Vec<(Vec<u8>, crate::backend::native::types::KvValue)>, SqliteGraphError> {
        self.sqlite.kv_prefix_scan(snapshot_id, prefix)
    }

    fn query_nodes_by_kind(
        &self,
        snapshot_id: SnapshotId,
        kind: &str,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.sqlite.query_nodes_by_kind(snapshot_id, kind)
    }

    fn query_nodes_by_name_pattern(
        &self,
        snapshot_id: SnapshotId,
        pattern: &str,
    ) -> Result<Vec<i64>, SqliteGraphError> {
        self.sqlite
            .query_nodes_by_name_pattern(snapshot_id, pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{EdgeSpec, GraphBackend, NodeSpec};

    fn read_materialized_row(
        backend: &CombinedGraphBackend,
        node_id: i64,
        direction: BackendDirection,
    ) -> Vec<i64> {
        backend
            .load_materialized_neighbors(node_id, direction)
            .expect("load materialized row")
            .unwrap_or_default()
    }

    #[test]
    fn test_combined_backend_prefers_materialized_neighbors_when_enabled() {
        let backend = CombinedGraphBackend::in_memory_with_config(
            CombinedConfig::new().with_materialized_reads(),
        )
        .expect("combined backend");

        let a = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "A".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let b = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "B".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let c = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "C".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: b,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: c,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();

        let conn = backend.graph().connection();
        let version = backend.graph().get_authoritative_version().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4)",
            rusqlite::params![
                0_i64,
                a,
                encode_csr_adjacency(&[(c as u32, 0.9, 0), (b as u32, 0.1, 0)]),
                version
            ],
        )
        .unwrap();
        backend
            .graph()
            .set_materialized_version(backend.graph().get_authoritative_version().unwrap())
            .unwrap();

        let neighbors = backend
            .neighbors(
                SnapshotId::current(),
                a,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                },
            )
            .unwrap();

        assert_eq!(neighbors, vec![c, b]);
    }

    #[test]
    fn test_combined_backend_materialized_bfs_falls_back_per_node() {
        let backend = CombinedGraphBackend::in_memory_with_config(
            CombinedConfig::new().with_materialized_reads(),
        )
        .expect("combined backend");

        let a = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "A".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let b = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "B".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let c = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "C".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: b,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: b,
                to: c,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();

        let conn = backend.graph().connection();
        let version = backend.graph().get_authoritative_version().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4)",
            rusqlite::params![0_i64, a, encode_csr_adjacency(&[(b as u32, 1.0, 0)]), version],
        )
        .unwrap();
        backend
            .graph()
            .set_materialized_version(backend.graph().get_authoritative_version().unwrap())
            .unwrap();

        let visited = backend.bfs(SnapshotId::current(), a, 2).unwrap();
        assert_eq!(visited, vec![a, b, c]);
    }

    #[test]
    fn test_combined_backend_materialized_k_hop_matches_sqlite_ordering() {
        let backend = CombinedGraphBackend::in_memory_with_config(
            CombinedConfig::new().with_materialized_reads(),
        )
        .expect("combined backend");

        let a = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "A".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let b = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "B".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let c = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "C".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let d = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "D".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: b,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: c,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: c,
                to: d,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();

        let conn = backend.graph().connection();
        let version = backend.graph().get_authoritative_version().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4)",
            rusqlite::params![
                0_i64,
                a,
                encode_csr_adjacency(&[(c as u32, 0.9, 0), (b as u32, 0.1, 0)]),
                version
            ],
        )
        .unwrap();
        backend
            .graph()
            .set_materialized_version(backend.graph().get_authoritative_version().unwrap())
            .unwrap();

        let nodes = backend
            .k_hop(SnapshotId::current(), a, 2, BackendDirection::Outgoing)
            .unwrap();
        assert_eq!(nodes, vec![b, c, d]);
    }

    #[test]
    fn test_combined_backend_materialized_node_degree_falls_back_per_direction() {
        let backend = CombinedGraphBackend::in_memory_with_config(
            CombinedConfig::new().with_materialized_reads(),
        )
        .expect("combined backend");

        let a = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "A".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let b = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "B".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let c = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "C".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: b,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: c,
                to: a,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();

        let conn = backend.graph().connection();
        let version = backend.graph().get_authoritative_version().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4)",
            rusqlite::params![0_i64, a, encode_csr_adjacency(&[(b as u32, 1.0, 0)]), version],
        )
        .unwrap();
        backend
            .graph()
            .set_materialized_version(backend.graph().get_authoritative_version().unwrap())
            .unwrap();

        let degree = backend.node_degree(SnapshotId::current(), a).unwrap();
        assert_eq!(degree, (1, 1));
    }

    #[test]
    fn test_combined_backend_ignores_stale_materialized_rows() {
        let backend = CombinedGraphBackend::in_memory_with_config(
            CombinedConfig::new().with_materialized_reads(),
        )
        .expect("combined backend");

        let a = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "A".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let b = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "B".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let c = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "C".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: b,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();

        let conn = backend.graph().connection();
        conn.execute(
            "INSERT INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, 1, 1, 1)",
            rusqlite::params![0_i64, a, encode_csr_adjacency(&[(c as u32, 1.0, 0)])],
        )
        .unwrap();
        backend.graph().set_materialized_version(1).unwrap();

        let neighbors = backend
            .neighbors(
                SnapshotId::current(),
                a,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                },
            )
            .unwrap();

        assert_eq!(neighbors, vec![b]);
    }

    #[test]
    fn test_combined_backend_publish_materialized_views_updates_freshness() {
        let backend = CombinedGraphBackend::in_memory_with_config(
            CombinedConfig::new().with_materialized_reads(),
        )
        .expect("combined backend");

        let a = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "A".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let b = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "B".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: b,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();

        let published = backend.publish_materialized_views().unwrap();
        assert_eq!(
            published,
            backend.graph().get_authoritative_version().unwrap()
        );
        assert_eq!(
            published,
            backend.graph().get_materialized_version().unwrap()
        );

        let neighbors = backend
            .neighbors(
                SnapshotId::current(),
                a,
                NeighborQuery {
                    direction: BackendDirection::Outgoing,
                    edge_type: None,
                },
            )
            .unwrap();
        assert_eq!(neighbors, vec![b]);
    }

    #[test]
    fn test_combined_backend_incremental_node_writes_keep_versions_in_sync() {
        let backend = CombinedGraphBackend::in_memory_with_config(
            CombinedConfig::new().with_materialized_reads(),
        )
        .expect("combined backend");

        let a = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "A".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        assert_eq!(
            backend.graph().get_authoritative_version().unwrap(),
            backend.graph().get_materialized_version().unwrap()
        );

        backend
            .update_node(
                a,
                NodeSpec {
                    kind: "Node".into(),
                    name: "A2".into(),
                    file_path: None,
                    data: serde_json::json!({"label": "updated"}),
                },
            )
            .unwrap();
        assert_eq!(
            backend.graph().get_authoritative_version().unwrap(),
            backend.graph().get_materialized_version().unwrap()
        );
    }

    #[test]
    fn test_combined_backend_incremental_edge_insert_refreshes_affected_rows() {
        let backend = CombinedGraphBackend::in_memory_with_config(
            CombinedConfig::new().with_materialized_reads(),
        )
        .expect("combined backend");

        let a = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "A".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let b = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "B".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let c = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "C".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: b,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();

        assert_eq!(
            backend.graph().get_authoritative_version().unwrap(),
            backend.graph().get_materialized_version().unwrap()
        );
        assert_eq!(
            read_materialized_row(&backend, a, BackendDirection::Outgoing),
            vec![b]
        );
        assert_eq!(
            read_materialized_row(&backend, b, BackendDirection::Incoming),
            vec![a]
        );

        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: c,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();

        assert_eq!(
            backend.graph().get_authoritative_version().unwrap(),
            backend.graph().get_materialized_version().unwrap()
        );
        assert_eq!(
            read_materialized_row(&backend, a, BackendDirection::Outgoing),
            vec![b, c]
        );
        assert_eq!(
            read_materialized_row(&backend, c, BackendDirection::Incoming),
            vec![a]
        );
    }

    #[test]
    fn test_combined_backend_incremental_delete_refreshes_neighbor_rows() {
        let backend = CombinedGraphBackend::in_memory_with_config(
            CombinedConfig::new().with_materialized_reads(),
        )
        .expect("combined backend");

        let a = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "A".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let b = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "B".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let c = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "C".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: b,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: b,
                to: c,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();

        assert_eq!(
            read_materialized_row(&backend, a, BackendDirection::Outgoing),
            vec![b]
        );
        assert_eq!(
            read_materialized_row(&backend, c, BackendDirection::Incoming),
            vec![b]
        );

        backend.delete_entity(b).unwrap();

        assert_eq!(
            backend.graph().get_authoritative_version().unwrap(),
            backend.graph().get_materialized_version().unwrap()
        );
        assert_eq!(
            read_materialized_row(&backend, a, BackendDirection::Outgoing),
            Vec::<i64>::new()
        );
        assert_eq!(
            read_materialized_row(&backend, c, BackendDirection::Incoming),
            Vec::<i64>::new()
        );
        assert_eq!(
            read_materialized_row(&backend, b, BackendDirection::Outgoing),
            Vec::<i64>::new()
        );
        assert_eq!(
            read_materialized_row(&backend, b, BackendDirection::Incoming),
            Vec::<i64>::new()
        );
    }

    #[test]
    fn test_combined_backend_materialized_shortest_path_matches_sqlite_semantics() {
        let backend = CombinedGraphBackend::in_memory_with_config(
            CombinedConfig::new().with_materialized_reads(),
        )
        .expect("combined backend");

        let a = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "A".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let b = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "B".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let c = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "C".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();
        let d = backend
            .insert_node(NodeSpec {
                kind: "Node".into(),
                name: "D".into(),
                file_path: None,
                data: serde_json::json!({}),
            })
            .unwrap();

        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: b,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: b,
                to: d,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: a,
                to: c,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();
        backend
            .insert_edge(EdgeSpec {
                from: c,
                to: d,
                edge_type: "LINKS".into(),
                data: serde_json::json!({}),
            })
            .unwrap();

        let conn = backend.graph().connection();
        let version = backend.graph().get_authoritative_version().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4)",
            rusqlite::params![
                0_i64,
                a,
                encode_csr_adjacency(&[(b as u32, 1.0, 0), (c as u32, 1.0, 0)]),
                version
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4)",
            rusqlite::params![0_i64, b, encode_csr_adjacency(&[(d as u32, 1.0, 0)]), version],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO csr_shards (shard_id, node_id, shard_data, version, created_at, visible_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4)",
            rusqlite::params![0_i64, c, encode_csr_adjacency(&[(d as u32, 1.0, 0)]), version],
        )
        .unwrap();
        backend
            .graph()
            .set_materialized_version(backend.graph().get_authoritative_version().unwrap())
            .unwrap();

        let path = backend.shortest_path(SnapshotId::current(), a, d).unwrap();
        assert_eq!(path, Some(vec![a, b, d]));
    }
}
