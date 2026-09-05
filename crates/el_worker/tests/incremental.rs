//! The P4 proof: a full-engine incremental run against local DuckDB —
//! twice. Run 1 loads everything and stores the cursor; the source then
//! gains new rows and an update to an existing row; run 2 must extract
//! ONLY the delta, MERGE it, and land exactly the right final table.

use el_engine::state::WatermarkValue;

fn worker() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_zdbt-el-worker"))
}

fn run(project: &std::path::Path, pipeline: el_engine::spec::Pipeline) -> (u64, u64) {
    let request = el_engine::run::RunRequest {
        project_root: project.to_path_buf(),
        pipeline,
        worker: Some(worker()),
        driver: None,
        chunk_rows: 2,
    };
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let cancel = el_engine::CancelFlag::default();
    let report = el_engine::run::run_pipeline(&request, &tx, &cancel).unwrap();
    drop(tx);
    let mut rows_read = 0;
    while let Ok(Some(event)) = rx.try_next() {
        if let el_engine::ProgressEvent::StreamFinished {
            rows_read: r, ..
        } = event
        {
            rows_read = r;
        }
    }
    assert_eq!(report.streams_failed, 0, "run must succeed");
    (rows_read, report.rows_written)
}

fn scalar(db: &std::path::Path, sql: &str) -> String {
    let connection = duckdb::Connection::open(db).unwrap();
    connection
        .query_row(sql, [], |row| row.get::<_, String>(0))
        .unwrap()
}

#[test]
fn incremental_merge_moves_only_the_delta() {
    let project = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    // SAFETY: test-scoped env; the test binary runs this test in one thread.
    unsafe { std::env::set_var("ZDBT_EL_STATE_DIR", state_dir.path()) };

    let el = project.path().join("el");
    std::fs::create_dir_all(el.join("pipelines")).unwrap();
    let source_db = el.join("source.duckdb");
    let warehouse_db = el.join("warehouse.duckdb");

    // Seed the source: 4 orders with an updated_at cursor.
    {
        let connection = duckdb::Connection::open(&source_db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE main.orders (
                     id BIGINT, customer VARCHAR, amount DECIMAL(18,2),
                     updated_at TIMESTAMP);
                 INSERT INTO main.orders VALUES
                   (1,'acme',  100.00, TIMESTAMP '2026-01-01 10:00:00'),
                   (2,'globex', 200.00, TIMESTAMP '2026-01-02 10:00:00'),
                   (3,'initech',300.00, TIMESTAMP '2026-01-03 10:00:00'),
                   (4,'stark',  400.00, TIMESTAMP '2026-01-04 10:00:00');",
            )
            .unwrap();
    }

    std::fs::write(
        el.join("connections.yml"),
        "version: 1\nconnections:\n  src: { type: duckdb, path: el/source.duckdb }\n  wh: { type: duckdb, path: el/warehouse.duckdb }\n",
    )
    .unwrap();
    let pipeline_yaml = r#"version: 1
pipeline: inc
source: src
target: { connection: wh, schema: LANDING, table: '{stream}' }
streams:
- name: orders
  source: { schema: main, table: orders }
  mode: incremental
  primary_key: [id]
  update_key: updated_at
  columns:
  - { name: amount, cast: 'NUMBER(18,2)' }
"#;
    std::fs::write(el.join("pipelines").join("inc.yml"), pipeline_yaml).unwrap();
    let pipeline: el_engine::spec::Pipeline =
        el_engine::spec::load_pipeline(&el.join("pipelines").join("inc.yml")).unwrap();

    // ── Run 1: everything moves, cursor stored ──────────────────────────
    let (read1, written1) = run(project.path(), pipeline.clone());
    assert_eq!(read1, 4);
    assert_eq!(written1, 4);
    assert_eq!(
        scalar(&warehouse_db, "SELECT COUNT(*)::VARCHAR FROM LANDING.ORDERS"),
        "4"
    );
    let store = el_engine::state::StateStore::open(project.path()).unwrap();
    let cursor = store.watermark("inc", "orders").expect("cursor stored");
    assert_eq!(
        cursor,
        WatermarkValue::parse_scalar(
            "2026-01-04 10:00:00",
            &"TIMESTAMP_NTZ".parse().unwrap()
        )
        .unwrap()
    );
    drop(store);

    // ── Mutate the source: 2 new rows + 1 update (newer cursor) ─────────
    {
        let connection = duckdb::Connection::open(&source_db).unwrap();
        connection
            .execute_batch(
                "INSERT INTO main.orders VALUES
                   (5,'wayne', 500.00, TIMESTAMP '2026-01-05 10:00:00'),
                   (6,'oscorp',600.00, TIMESTAMP '2026-01-06 10:00:00');
                 UPDATE main.orders
                   SET amount = 111.00, updated_at = TIMESTAMP '2026-01-05 12:00:00'
                   WHERE id = 1;",
            )
            .unwrap();
    }

    // ── Run 2: only the delta (2 new + 1 updated = 3 rows) ──────────────
    let (read2, _) = run(project.path(), pipeline);
    assert_eq!(read2, 3, "cursor pushdown must skip unchanged rows");

    // Final table: 6 rows, id=1 updated not duplicated, amount merged.
    assert_eq!(
        scalar(&warehouse_db, "SELECT COUNT(*)::VARCHAR FROM LANDING.ORDERS"),
        "6"
    );
    assert_eq!(
        scalar(
            &warehouse_db,
            "SELECT COUNT(*)::VARCHAR FROM LANDING.ORDERS WHERE id = 1"
        ),
        "1",
        "MERGE must update, not duplicate"
    );
    assert_eq!(
        scalar(
            &warehouse_db,
            "SELECT amount::VARCHAR FROM LANDING.ORDERS WHERE id = 1"
        ),
        "111.00"
    );
    // Cursor advanced to the new max.
    let store = el_engine::state::StateStore::open(project.path()).unwrap();
    assert_eq!(
        store.watermark("inc", "orders").unwrap().to_string(),
        "2026-01-06 10:00:00"
    );

    // ── Run 3: nothing new — zero rows move, table unchanged ────────────
    let pipeline: el_engine::spec::Pipeline =
        el_engine::spec::load_pipeline(&el.join("pipelines").join("inc.yml")).unwrap();
    let (read3, _) = run(project.path(), pipeline);
    assert_eq!(read3, 0, "idempotent: no delta, no extraction");
    assert_eq!(
        scalar(&warehouse_db, "SELECT COUNT(*)::VARCHAR FROM LANDING.ORDERS"),
        "6"
    );

    unsafe { std::env::remove_var("ZDBT_EL_STATE_DIR") };
}
