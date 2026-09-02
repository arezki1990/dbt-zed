use rusqlite::Connection;
use sqlitegraph::schema::{SCHEMA_VERSION, ensure_schema};

#[test]
fn test_schema_creates_label_and_property_tables() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_schema(&conn).unwrap();
    assert!(table_exists(&conn, "graph_labels"));
    assert!(table_exists(&conn, "graph_properties"));
}

#[test]
fn test_schema_meta_records_version() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_schema(&conn).unwrap();
    let version: i64 = conn
        .prepare("SELECT schema_version FROM graph_meta WHERE id=1")
        .unwrap()
        .query_row([], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
}

#[test]
fn test_future_schema_version_errors() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE graph_meta(id INTEGER PRIMARY KEY, schema_version INTEGER NOT NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO graph_meta(id, schema_version) VALUES(1, ?1)",
        [SCHEMA_VERSION + 10],
    )
    .unwrap();
    let err = ensure_schema(&conn).expect_err("expected version error");
    assert!(err.to_string().contains("database schema version"));
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.prepare("SELECT name FROM sqlite_master WHERE name=?1")
        .unwrap()
        .exists([name])
        .unwrap()
}
