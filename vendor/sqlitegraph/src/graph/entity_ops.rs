//! Entity CRUD operations for SqliteGraph.

use rusqlite::{OptionalExtension, params};

use crate::errors::SqliteGraphError;

use super::{
    SqliteGraph,
    types::{GraphEntity, row_to_entity, validate_entity},
};

impl SqliteGraph {
    pub fn insert_entity(&self, entity: &GraphEntity) -> Result<i64, SqliteGraphError> {
        validate_entity(entity)?;
        let data = serde_json::to_string(&entity.data)
            .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
        let conn = self.connection();
        conn.execute(
            "INSERT INTO graph_entities(kind, name, file_path, data) VALUES(?1, ?2, ?3, ?4)",
            params![
                entity.kind.as_str(),
                entity.name.as_str(),
                entity.file_path.as_deref(),
                data,
            ],
        )
        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        let id = conn.last_insert_rowid();
        // Auto-register `kind` as a label so `match_triples` can answer
        // label-filtered edge queries (e.g. `(a:User)-[:KNOWS]->(b:User)`)
        // without callers having to call `index::add_label` separately.
        // `INSERT OR IGNORE` is safe against duplicates if the row already
        // exists (e.g. re-running migrations or future kind/label sync).
        if !entity.kind.is_empty() {
            conn.execute(
                "INSERT OR IGNORE INTO graph_labels(entity_id, label) VALUES(?1, ?2)",
                params![id, entity.kind.as_str()],
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        }
        self.bump_authoritative_version()?;
        Ok(id)
    }

    /// Insert many entities atomically inside a single transaction.
    ///
    /// Returns the rowids of the inserted entities in the same order as the
    /// input. Empty input returns an empty vector without opening a
    /// transaction. On any error, the transaction is rolled back and the
    /// database is left untouched.
    pub fn insert_entities_bulk(
        &self,
        entities: &[GraphEntity],
    ) -> Result<Vec<i64>, SqliteGraphError> {
        if entities.is_empty() {
            return Ok(Vec::new());
        }
        for entity in entities {
            validate_entity(entity)?;
        }
        let conn = self.connection();
        conn.underlying()
            .execute_batch("BEGIN")
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;

        let mut ids = Vec::with_capacity(entities.len());
        let insert_result: Result<(), SqliteGraphError> = (|| {
            let mut entity_stmt = conn
                .prepare_cached(
                    "INSERT INTO graph_entities(kind, name, file_path, data) VALUES(?1, ?2, ?3, ?4)",
                )
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;
            let mut label_stmt = conn
                .prepare_cached(
                    "INSERT OR IGNORE INTO graph_labels(entity_id, label) VALUES(?1, ?2)",
                )
                .map_err(|e| SqliteGraphError::query(e.to_string()))?;
            for entity in entities {
                let data = serde_json::to_string(&entity.data)
                    .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
                entity_stmt
                    .execute(params![
                        entity.kind.as_str(),
                        entity.name.as_str(),
                        entity.file_path.as_deref(),
                        data,
                    ])
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                let id = conn.last_insert_rowid();
                if !entity.kind.is_empty() {
                    label_stmt
                        .execute(params![id, entity.kind.as_str()])
                        .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                }
                ids.push(id);
            }
            Ok(())
        })();

        match insert_result {
            Ok(()) => {
                conn.underlying()
                    .execute_batch("COMMIT")
                    .map_err(|e| SqliteGraphError::query(e.to_string()))?;
                self.bump_authoritative_version()?;
                Ok(ids)
            }
            Err(err) => {
                let _ = conn.underlying().execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    pub fn get_entity(&self, id: i64) -> Result<GraphEntity, SqliteGraphError> {
        self.connection()
            .query_row(
                "SELECT id, kind, name, file_path, data FROM graph_entities WHERE id=?1",
                params![id],
                row_to_entity,
            )
            .map_err(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => {
                    SqliteGraphError::not_found(format!("entity {id}"))
                }
                other => SqliteGraphError::query(other.to_string()),
            })
    }

    pub fn update_entity(&self, entity: &GraphEntity) -> Result<(), SqliteGraphError> {
        if entity.id <= 0 {
            return Err(SqliteGraphError::invalid_input(
                "entity id must be positive for update",
            ));
        }
        validate_entity(entity)?;
        let data = serde_json::to_string(&entity.data)
            .map_err(|e| SqliteGraphError::invalid_input(e.to_string()))?;
        let affected = self
            .connection()
            .execute(
                "UPDATE graph_entities SET kind=?1, name=?2, file_path=?3, data=?4 WHERE id=?5",
                params![
                    entity.kind.as_str(),
                    entity.name.as_str(),
                    entity.file_path.as_deref(),
                    data,
                    entity.id,
                ],
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        if affected == 0 {
            return Err(SqliteGraphError::not_found(format!("entity {}", entity.id)));
        }
        self.bump_authoritative_version()?;
        Ok(())
    }

    pub fn delete_entity(&self, id: i64) -> Result<(), SqliteGraphError> {
        let affected = self
            .connection()
            .execute("DELETE FROM graph_entities WHERE id=?1", params![id])
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        if affected == 0 {
            return Err(SqliteGraphError::not_found(format!("entity {id}")));
        }
        self.connection()
            .execute(
                "DELETE FROM graph_edges WHERE from_id=?1 OR to_id=?1",
                params![id],
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        self.invalidate_caches();
        self.bump_authoritative_version()?;
        Ok(())
    }

    pub fn list_entity_ids(&self) -> Result<Vec<i64>, SqliteGraphError> {
        self.all_entity_ids()
    }

    /// Find all entities of a given kind.
    ///
    /// Uses the `idx_entities_kind` index for efficient lookup.
    pub fn find_entities_by_kind(&self, kind: &str) -> Result<Vec<GraphEntity>, SqliteGraphError> {
        let conn = self.connection();
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, kind, name, file_path, data FROM graph_entities WHERE kind = ?1 ORDER BY id",
            )
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        let rows = stmt
            .query_map(params![kind], row_to_entity)
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        let mut entities = Vec::new();
        for row in rows {
            entities.push(row.map_err(|e| SqliteGraphError::query(e.to_string()))?);
        }
        Ok(entities)
    }

    /// Find a single entity by kind and exact name.
    ///
    /// Uses the `idx_entities_kind_name` composite index for efficient lookup.
    /// Returns `None` if no entity matches.
    pub fn find_entity_by_kind_and_name(
        &self,
        kind: &str,
        name: &str,
    ) -> Result<Option<GraphEntity>, SqliteGraphError> {
        let conn = self.connection();
        let result = conn
            .query_row(
                "SELECT id, kind, name, file_path, data FROM graph_entities WHERE kind = ?1 AND name = ?2",
                params![kind, name],
                row_to_entity,
            )
            .optional()
            .map_err(|e| SqliteGraphError::query(e.to_string()))?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteGraph;

    fn test_graph() -> SqliteGraph {
        SqliteGraph::open_in_memory().expect("Failed to open in-memory graph")
    }

    fn make_entity(kind: &str, name: &str) -> GraphEntity {
        GraphEntity {
            id: 0,
            kind: kind.to_string(),
            name: name.to_string(),
            file_path: None,
            data: serde_json::json!({}),
        }
    }

    #[test]
    fn find_entities_by_kind_returns_matching() -> Result<(), SqliteGraphError> {
        let graph = test_graph();
        graph.insert_entity(&make_entity("agent", "hermes"))?;
        graph.insert_entity(&make_entity("agent", "claude1"))?;
        graph.insert_entity(&make_entity("tool", "magellan"))?;

        let agents = graph.find_entities_by_kind("agent")?;
        assert_eq!(agents.len(), 2);
        let names: Vec<&str> = agents.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"hermes"));
        assert!(names.contains(&"claude1"));

        let tools = graph.find_entities_by_kind("tool")?;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "magellan");

        let missing = graph.find_entities_by_kind("nonexistent")?;
        assert!(missing.is_empty());

        Ok(())
    }

    #[test]
    fn find_entity_by_kind_and_name_exacts() -> Result<(), SqliteGraphError> {
        let graph = test_graph();
        graph.insert_entity(&make_entity("agent", "hermes"))?;
        graph.insert_entity(&make_entity("tool", "hermes"))?;

        let found = graph.find_entity_by_kind_and_name("agent", "hermes")?;
        assert!(found.is_some());
        assert_eq!(found.as_ref().unwrap().kind, "agent");
        assert_eq!(found.as_ref().unwrap().name, "hermes");

        let wrong_kind = graph.find_entity_by_kind_and_name("tool", "hermes")?;
        assert!(wrong_kind.is_some());
        assert_eq!(wrong_kind.as_ref().unwrap().kind, "tool");

        let no_match = graph.find_entity_by_kind_and_name("agent", "nonexistent")?;
        assert!(no_match.is_none());

        let no_kind = graph.find_entity_by_kind_and_name("nonexistent", "hermes")?;
        assert!(no_kind.is_none());

        Ok(())
    }

    #[test]
    fn find_entities_preserves_order_and_fields() -> Result<(), SqliteGraphError> {
        let graph = test_graph();
        graph.insert_entity(&make_entity("fn", "alpha"))?;
        graph.insert_entity(&make_entity("fn", "beta"))?;
        graph.insert_entity(&make_entity("fn", "gamma"))?;

        let fns = graph.find_entities_by_kind("fn")?;
        assert_eq!(fns.len(), 3);
        assert!(fns[0].id < fns[1].id);
        assert!(fns[1].id < fns[2].id);

        Ok(())
    }
}
