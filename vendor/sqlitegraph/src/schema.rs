use rusqlite::{Connection, OptionalExtension};

use crate::errors::SqliteGraphError;

pub const BASE_SCHEMA_VERSION: i64 = 1;

struct MigrationStep {
    target_version: i64,
    statements: &'static [&'static str],
}

const MIGRATION_STEPS: &[MigrationStep] = &[
    MigrationStep {
        target_version: 2,
        statements: &[
            "CREATE TABLE IF NOT EXISTS graph_meta_history(version INTEGER NOT NULL, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)",
            "INSERT INTO graph_meta_history(version) VALUES(2)",
        ],
    },
    MigrationStep {
        target_version: 3,
        statements: &[
            // HNSW index metadata table
            "CREATE TABLE IF NOT EXISTS hnsw_indexes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                dimension INTEGER NOT NULL,
                m INTEGER NOT NULL,
                ef_construction INTEGER NOT NULL,
                distance_metric TEXT NOT NULL,
                vector_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            // HNSW vectors table
            "CREATE TABLE IF NOT EXISTS hnsw_vectors (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                index_id INTEGER NOT NULL,
                vector_data BLOB NOT NULL,
                metadata TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (index_id) REFERENCES hnsw_indexes(id) ON DELETE CASCADE
            )",
            // HNSW layers table
            "CREATE TABLE IF NOT EXISTS hnsw_layers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                index_id INTEGER NOT NULL,
                layer_level INTEGER NOT NULL,
                node_id INTEGER NOT NULL,
                connections BLOB NOT NULL,
                FOREIGN KEY (index_id) REFERENCES hnsw_indexes(id) ON DELETE CASCADE,
                UNIQUE(index_id, layer_level, node_id)
            )",
            // HNSW entry points table
            "CREATE TABLE IF NOT EXISTS hnsw_entry_points (
                index_id INTEGER NOT NULL,
                node_id INTEGER NOT NULL,
                PRIMARY KEY (index_id, node_id),
                FOREIGN KEY (index_id) REFERENCES hnsw_indexes(id) ON DELETE CASCADE
            )",
            // Indexes for performance
            "CREATE INDEX IF NOT EXISTS idx_hnsw_vectors_index ON hnsw_vectors(index_id)",
            "CREATE INDEX IF NOT EXISTS idx_hnsw_layers_index ON hnsw_layers(index_id, layer_level)",
            "CREATE INDEX IF NOT EXISTS idx_hnsw_entry_points_index ON hnsw_entry_points(index_id)",
            "INSERT INTO graph_meta_history(version) VALUES(3)",
        ],
    },
    MigrationStep {
        target_version: 4,
        statements: &[
            "CREATE INDEX IF NOT EXISTS idx_entities_kind ON graph_entities(kind)",
            "CREATE INDEX IF NOT EXISTS idx_entities_kind_name ON graph_entities(kind, name)",
            "INSERT INTO graph_meta_history(version) VALUES(4)",
        ],
    },
    MigrationStep {
        target_version: 5,
        // Enforce label uniqueness so `INSERT OR IGNORE INTO graph_labels`
        // actually dedupes (the original base schema had no UNIQUE
        // constraint, so duplicates silently accumulated). The DELETE step
        // collapses any existing duplicates by keeping the lowest rowid
        // per (entity_id, label).
        statements: &[
            "DELETE FROM graph_labels WHERE rowid NOT IN (SELECT MIN(rowid) FROM graph_labels GROUP BY entity_id, label)",
            "CREATE UNIQUE INDEX IF NOT EXISTS uq_graph_labels_entity_label ON graph_labels(entity_id, label)",
            "INSERT INTO graph_meta_history(version) VALUES(5)",
        ],
    },
    MigrationStep {
        target_version: 6,
        statements: &[
            "ALTER TABLE hnsw_entry_points ADD COLUMN order_idx INTEGER NOT NULL DEFAULT 0",
            "INSERT INTO graph_meta_history(version) VALUES(6)",
        ],
    },
    MigrationStep {
        target_version: 7,
        statements: &[
            // MVCC: Add conflict resolution to csr_manifest (only if missing)
            "ALTER TABLE csr_manifest ADD COLUMN conflict_resolution TEXT NOT NULL DEFAULT 'last-write-wins'",
            // MVCC: Add versioning to csr_shards (only if missing)
            "ALTER TABLE csr_shards ADD COLUMN version INTEGER NOT NULL DEFAULT 1",
            "ALTER TABLE csr_shards ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE csr_shards ADD COLUMN visible_at INTEGER NOT NULL DEFAULT 0",
            // MVCC: Add snapshot isolation to hnsw_vectors (only if missing)
            "ALTER TABLE hnsw_vectors ADD COLUMN snapshot_id TEXT NOT NULL DEFAULT 'default'",
            "ALTER TABLE hnsw_vectors ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0",
            // MVCC: Add read cursor tracking to consumer_group_state (only if missing)
            "ALTER TABLE consumer_group_state ADD COLUMN read_cursor INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE consumer_group_state ADD COLUMN cursor_snapshot TEXT NOT NULL DEFAULT 'default'",
            "INSERT INTO graph_meta_history(version) VALUES(7)",
        ],
    },
    MigrationStep {
        target_version: 8,
        statements: &[
            // MVCC: Create snapshots table for tracking named snapshots
            "CREATE TABLE IF NOT EXISTS snapshots (
                snapshot_id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                description TEXT
            )",
            // MVCC: Add snapshot_id to graph_entities for batch tracking
            "ALTER TABLE graph_entities ADD COLUMN snapshot_id TEXT NOT NULL DEFAULT 'default'",
            // MVCC: Add snapshot_id to graph_edges for batch tracking
            "ALTER TABLE graph_edges ADD COLUMN snapshot_id TEXT NOT NULL DEFAULT 'default'",
            // MVCC: Add created_at to graph_entities for time-travel queries
            "ALTER TABLE graph_entities ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0",
            // MVCC: Add created_at to graph_edges for time-travel queries
            "ALTER TABLE graph_edges ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0",
            // Indexes for snapshot queries
            "CREATE INDEX IF NOT EXISTS idx_graph_entities_snapshot ON graph_entities(snapshot_id)",
            "CREATE INDEX IF NOT EXISTS idx_graph_edges_snapshot ON graph_edges(snapshot_id)",
            "CREATE INDEX IF NOT EXISTS idx_graph_entities_created ON graph_entities(created_at)",
            "CREATE INDEX IF NOT EXISTS idx_graph_edges_created ON graph_edges(created_at)",
            "INSERT INTO graph_meta_history(version) VALUES(8)",
        ],
    },
    MigrationStep {
        target_version: 9,
        statements: &[
            // SCALE: Create pre-aggregated snapshot stats table
            "CREATE TABLE IF NOT EXISTS snapshot_stats (
                snapshot_id TEXT PRIMARY KEY,
                entity_count INTEGER NOT NULL DEFAULT 0,
                edge_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (snapshot_id) REFERENCES snapshots(snapshot_id) ON DELETE CASCADE
            )",
            // SCALE: Composite index for (snapshot_id, created_at) queries
            "CREATE INDEX IF NOT EXISTS idx_graph_entities_snapshot_created ON graph_entities(snapshot_id, created_at)",
            "CREATE INDEX IF NOT EXISTS idx_graph_edges_snapshot_created ON graph_edges(snapshot_id, created_at)",
            // SCALE: Index for time-range queries
            "CREATE INDEX IF NOT EXISTS idx_snapshot_stats_created_at ON snapshot_stats(created_at)",
            "INSERT INTO graph_meta_history(version) VALUES(9)",
        ],
    },
    MigrationStep {
        target_version: 10,
        statements: &[
            "ALTER TABLE graph_meta ADD COLUMN authoritative_version INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE graph_meta ADD COLUMN materialized_version INTEGER NOT NULL DEFAULT 0",
            "UPDATE graph_meta
             SET authoritative_version = COALESCE(authoritative_version, 0),
                 materialized_version = COALESCE(materialized_version, 0)
             WHERE id = 1",
            "INSERT INTO graph_meta_history(version) VALUES(10)",
        ],
    },
];

pub const SCHEMA_VERSION: i64 = BASE_SCHEMA_VERSION + MIGRATION_STEPS.len() as i64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub from_version: i64,
    pub to_version: i64,
    pub statements: Vec<&'static str>,
    pub dry_run: bool,
}

pub fn ensure_schema(conn: &Connection) -> Result<(), SqliteGraphError> {
    ensure_base_schema(conn)?;
    ensure_meta(conn)?;
    ensure_native_v3_tables(conn)?; // Initialize native-v3 infrastructure
    run_pending_migrations(conn, false)?;
    Ok(())
}

pub fn ensure_schema_without_migrations(conn: &Connection) -> Result<(), SqliteGraphError> {
    ensure_base_schema(conn)?;
    ensure_meta(conn)?;
    Ok(())
}

fn ensure_base_schema(conn: &Connection) -> Result<(), SqliteGraphError> {
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = ON;
        CREATE TABLE IF NOT EXISTS graph_entities (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            kind      TEXT NOT NULL,
            name      TEXT NOT NULL,
            file_path TEXT,
            data      TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS graph_edges (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            from_id   INTEGER NOT NULL,
            to_id     INTEGER NOT NULL,
            edge_type TEXT NOT NULL,
            data      TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS graph_labels (
            entity_id INTEGER NOT NULL,
            label     TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS graph_properties (
            entity_id INTEGER NOT NULL,
            key       TEXT NOT NULL,
            value     TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_edges_from ON graph_edges(from_id);
        CREATE INDEX IF NOT EXISTS idx_edges_to ON graph_edges(to_id);
        CREATE INDEX IF NOT EXISTS idx_edges_type ON graph_edges(edge_type);
        CREATE INDEX IF NOT EXISTS idx_labels_label ON graph_labels(label);
        CREATE INDEX IF NOT EXISTS idx_labels_label_entity_id ON graph_labels(label, entity_id);
        CREATE INDEX IF NOT EXISTS idx_props_key_value ON graph_properties(key, value);
        CREATE INDEX IF NOT EXISTS idx_props_key_value_entity_id ON graph_properties(key, value, entity_id);
        CREATE INDEX IF NOT EXISTS idx_entities_kind_id ON graph_entities(kind, id);
        CREATE TABLE IF NOT EXISTS graph_meta (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            schema_version INTEGER NOT NULL,
            authoritative_version INTEGER NOT NULL DEFAULT 0,
            materialized_version INTEGER NOT NULL DEFAULT 0
        );
        "#,
    )
    .map_err(|e| SqliteGraphError::schema(e.to_string()))
}

pub fn read_schema_version(conn: &Connection) -> Result<i64, SqliteGraphError> {
    conn.query_row(
        "SELECT schema_version FROM graph_meta WHERE id=1",
        [],
        |row| row.get(0),
    )
    .map_err(|e| SqliteGraphError::schema(e.to_string()))
}

pub fn run_pending_migrations(
    conn: &Connection,
    dry_run: bool,
) -> Result<MigrationReport, SqliteGraphError> {
    let current = read_schema_version(conn)?;
    let mut statements: Vec<&'static str> = Vec::new();
    let mut target = current;
    for step in MIGRATION_STEPS {
        if step.target_version > current {
            target = step.target_version;
            statements.extend_from_slice(step.statements);
        }
    }
    if statements.is_empty() {
        return Ok(MigrationReport {
            from_version: current,
            to_version: current,
            statements,
            dry_run,
        });
    }
    if dry_run {
        return Ok(MigrationReport {
            from_version: current,
            to_version: target,
            statements,
            dry_run,
        });
    }
    conn.execute("BEGIN IMMEDIATE", [])
        .map_err(|e| SqliteGraphError::schema(e.to_string()))?;
    let result: Result<(), SqliteGraphError> = (|| {
        for sql in statements.iter().copied() {
            // Ignore duplicate column errors for MVCC migration (v7)
            match conn.execute(sql, []) {
                Ok(_) => {}
                Err(e) => {
                    // Check if this is a duplicate column error (already exists in new schema)
                    if e.to_string().contains("duplicate column name") {
                        // Column already exists - skip, this is expected for v7 migration
                        continue;
                    }
                    return Err(SqliteGraphError::schema(e.to_string()));
                }
            }
        }
        conn.execute(
            "UPDATE graph_meta SET schema_version=?1 WHERE id=1",
            [target],
        )
        .map_err(|e| SqliteGraphError::schema(e.to_string()))?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute("COMMIT", [])
                .map_err(|e| SqliteGraphError::schema(e.to_string()))?;
        }
        Err(err) => {
            let _ = conn.execute("ROLLBACK", []);
            return Err(err);
        }
    }
    Ok(MigrationReport {
        from_version: current,
        to_version: target,
        statements,
        dry_run,
    })
}

fn ensure_meta(conn: &Connection) -> Result<(), SqliteGraphError> {
    let version: Option<i64> = conn
        .query_row(
            "SELECT schema_version FROM graph_meta WHERE id=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| SqliteGraphError::schema(e.to_string()))?;
    match version {
        Some(existing) => {
            if existing > SCHEMA_VERSION {
                return Err(SqliteGraphError::schema(format!(
                    "database schema version {existing} is newer than supported {SCHEMA_VERSION}"
                )));
            }
            if existing < BASE_SCHEMA_VERSION {
                conn.execute(
                    "UPDATE graph_meta SET schema_version=?1 WHERE id=1",
                    [BASE_SCHEMA_VERSION],
                )
                .map_err(|e| SqliteGraphError::schema(e.to_string()))?;
            }
        }
        None => {
            conn.execute(
                "INSERT INTO graph_meta(id, schema_version, authoritative_version, materialized_version)
                 VALUES(1, ?1, 0, 0)",
                [BASE_SCHEMA_VERSION],
            )
            .map_err(|e| SqliteGraphError::schema(e.to_string()))?;
        }
    }
    Ok(())
}

/// Initialize native-v3 infrastructure tables (CSR, HNSW, FTS5, property filters, consumer groups).
fn ensure_native_v3_tables(conn: &Connection) -> Result<(), SqliteGraphError> {
    // CSR manifest for sharding metadata
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
    .map_err(|e| {
        SqliteGraphError::SchemaError(format!("Failed to create csr_manifest table: {}", e))
    })?;

    // CSR shards for fast traversal (MVCC: versioned reads)
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
        SqliteGraphError::SchemaError(format!("Failed to create csr_shards table: {}", e))
    })?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS csr_edge_types (
            type_id INTEGER PRIMARY KEY,
            edge_type TEXT NOT NULL UNIQUE
        )",
        [],
    )
    .map_err(|e| {
        SqliteGraphError::SchemaError(format!("Failed to create csr_edge_types table: {}", e))
    })?;

    // HNSW vectors table (MVCC: snapshot isolation)
    // NOTE: HNSW tables (hnsw_indexes, hnsw_vectors, hnsw_layers, hnsw_entry_points)
    // are created by migration v3, not here. The native-v3 schema below was removed
    // because it created incompatible tables that broke existing HNSW tests.
    // Migration v3 creates the schema with id/metadata columns that HNSW code expects.

    // FTS5 content table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS token_content (
            token_id INTEGER PRIMARY KEY,
            content_type TEXT NOT NULL,
            content_text TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| {
        SqliteGraphError::SchemaError(format!("Failed to create token_content table: {}", e))
    })?;

    // FTS5 virtual table for full-text search
    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS token_content_fts USING fts5(
            token_id,
            content_type,
            content_text,
            tokenize='porter'
        )",
        [],
    )
    .map_err(|e| SqliteGraphError::SchemaError(format!("Failed to create FTS5 table: {}", e)))?;

    // FTS5 triggers for automatic synchronization
    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS token_content_insert AFTER INSERT ON token_content BEGIN
              INSERT INTO token_content_fts(rowid, token_id, content_type, content_text)
              VALUES (new.rowid, new.token_id, new.content_type, new.content_text);
             END",
        [],
    )
    .map_err(|e| {
        SqliteGraphError::SchemaError(format!("Failed to create FTS5 insert trigger: {}", e))
    })?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS token_content_delete AFTER DELETE ON token_content BEGIN
              DELETE FROM token_content_fts WHERE rowid = old.rowid;
             END",
        [],
    )
    .map_err(|e| {
        SqliteGraphError::SchemaError(format!("Failed to create FTS5 delete trigger: {}", e))
    })?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS token_content_update AFTER UPDATE ON token_content BEGIN
              DELETE FROM token_content_fts WHERE rowid = old.rowid;
              INSERT INTO token_content_fts(rowid, token_id, content_type, content_text)
              VALUES (new.rowid, new.token_id, new.content_type, new.content_text);
             END",
        [],
    )
    .map_err(|e| {
        SqliteGraphError::SchemaError(format!("Failed to create FTS5 update trigger: {}", e))
    })?;

    // Property filters table for rich attribute queries
    conn.execute(
        "CREATE TABLE IF NOT EXISTS token_attributes (
            token_id INTEGER NOT NULL,
            attr_name TEXT NOT NULL,
            attr_type TEXT NOT NULL,
            attr_value TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (token_id, attr_name)
        )",
        [],
    )
    .map_err(|e| {
        SqliteGraphError::SchemaError(format!("Failed to create token_attributes table: {}", e))
    })?;

    // Indexes for attribute query performance
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_token_attributes_name ON token_attributes(attr_name)",
        [],
    )
    .map_err(|e| {
        SqliteGraphError::SchemaError(format!(
            "Failed to create token_attributes name index: {}",
            e
        ))
    })?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_token_attributes_name_value ON token_attributes(attr_name, attr_value)",
        [],
    ).map_err(|e| SqliteGraphError::SchemaError(
        format!("Failed to create token_attributes name/value index: {}", e)
    ))?;

    // Consumer groups for reliable pub/sub
    conn.execute(
        "CREATE TABLE IF NOT EXISTS consumer_groups (
            name TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| {
        SqliteGraphError::SchemaError(format!("Failed to create consumer_groups table: {}", e))
    })?;

    // Consumer group state tracking (MVCC: read cursor isolation)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS consumer_group_state (
            group_name TEXT NOT NULL,
            topic TEXT NOT NULL,
            last_offset INTEGER NOT NULL DEFAULT 0,
            read_cursor INTEGER NOT NULL DEFAULT 0,
            cursor_snapshot TEXT NOT NULL DEFAULT 'default',
            pending_offsets TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (group_name, topic),
            FOREIGN KEY (group_name) REFERENCES consumer_groups(name) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| {
        SqliteGraphError::SchemaError(format!(
            "Failed to create consumer_group_state table: {}",
            e
        ))
    })?;

    Ok(())
}
