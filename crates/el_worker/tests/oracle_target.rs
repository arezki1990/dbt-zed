//! The Oracle target end to end: a local DuckDB source loads into a real
//! Oracle schema, first as a full refresh (rename swap) and then as an
//! incremental MERGE whose watermark is read back from the target.
//!
//! Gated on live credentials (no client software is needed):
//!
//! ```text
//! EL_ORACLE_SMOKE_URL=host:1521/FREEPDB1 EL_ORACLE_SMOKE_USER=… \
//! EL_ORACLE_SMOKE_PASSWORD=… cargo test -p el_worker -- --ignored \
//! oracle_target --nocapture
//! ```

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
        profile_override: None,
    };
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let cancel = el_engine::CancelFlag::default();
    let report = el_engine::run::run_pipeline(&request, &tx, &cancel).unwrap();
    drop(tx);
    let mut rows_read = 0;
    while let Ok(Some(event)) = rx.try_next() {
        if let el_engine::ProgressEvent::StreamFinished { rows_read: r, .. } = event {
            rows_read = r;
        }
    }
    assert_eq!(report.streams_failed, 0, "run must succeed");
    (rows_read, report.rows_written)
}

#[test]
#[ignore]
fn oracle_target_swaps_then_merges() {
    let connect = std::env::var("EL_ORACLE_SMOKE_URL").expect("set EL_ORACLE_SMOKE_URL");
    let user = std::env::var("EL_ORACLE_SMOKE_USER").expect("set EL_ORACLE_SMOKE_USER");
    let password =
        std::env::var("EL_ORACLE_SMOKE_PASSWORD").expect("set EL_ORACLE_SMOKE_PASSWORD");

    let project = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    // SAFETY: test-scoped env; this test owns the process.
    unsafe { std::env::set_var("ZDBT_EL_STATE_DIR", state_dir.path()) };

    let el = project.path().join("el");
    std::fs::create_dir_all(el.join("pipelines")).unwrap();
    let source_db = el.join("source.duckdb");
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

    let conn = el_engine::connectors::oracle::connect(&user, &password, &connect).unwrap();
    let schema: String = conn
        .query_row("SELECT USER FROM DUAL", &[])
        .unwrap()
        .get(0)
        .unwrap();
    let drop_all = |table: &str| {
        let _ = conn.execute(
            &format!(
                "BEGIN EXECUTE IMMEDIATE 'DROP TABLE {table} PURGE'; \
                 EXCEPTION WHEN OTHERS THEN IF SQLCODE != -942 THEN RAISE; END IF; END;"
            ),
            &[],
        );
    };
    drop_all("ZDBT_EL_ORDERS");
    drop_all("ZDBT_EL_ORDERS__ZDBT_STAGING");
    drop_all("ZDBT_EL_ORDERS__ZDBT_OLD");

    std::fs::write(
        el.join("connections.yml"),
        "version: 1\nconnections:\n  \
         src: { type: duckdb, path: el/source.duckdb }\n  \
         ora:\n    type: oracle\n    user: ${EL_ORACLE_SMOKE_USER}\n    \
         password: ${EL_ORACLE_SMOKE_PASSWORD}\n    connect: ${EL_ORACLE_SMOKE_URL}\n",
    )
    .unwrap();
    let pipeline_yaml = format!(
        "version: 1
pipeline: ora
source: src
target: {{ connection: ora, schema: {schema}, table: ZDBT_EL_ORDERS }}
streams:
- name: orders
  source: {{ schema: main, table: orders }}
  mode: full_refresh
  primary_key: [id]
  update_key: updated_at
  columns:
  - {{ name: amount, cast: 'NUMBER(18,2)' }}
"
    );
    let path = el.join("pipelines").join("ora.yml");
    std::fs::write(&path, &pipeline_yaml).unwrap();
    let pipeline = el_engine::spec::load_pipeline(&path).unwrap();

    // Full refresh: staging is created, filled and renamed over the target.
    let (read, written) = run(project.path(), pipeline);
    assert_eq!((read, written), (4, 4));
    let count: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM \"{schema}\".\"ZDBT_EL_ORDERS\""),
            &[],
        )
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(count, 4);

    // Incremental: two new rows and one update move, and the watermark
    // comes back from the target.
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
    std::fs::write(
        &path,
        pipeline_yaml.replace("mode: full_refresh", "mode: incremental"),
    )
    .unwrap();
    let pipeline = el_engine::spec::load_pipeline(&path).unwrap();
    run(project.path(), pipeline);

    let count: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM \"{schema}\".\"ZDBT_EL_ORDERS\""),
            &[],
        )
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(count, 6, "MERGE must update, not duplicate");
    let amount: String = conn
        .query_row(
            &format!(
                "SELECT TO_CHAR(\"AMOUNT\", 'FM9999990.00') \
                 FROM \"{schema}\".\"ZDBT_EL_ORDERS\" WHERE \"ID\" = 1"
            ),
            &[],
        )
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(amount, "111.00");
    let store = el_engine::state::StateStore::open(project.path(), None).unwrap();
    assert_eq!(
        store.watermark("ora", "orders").unwrap().to_string(),
        "2026-01-06 10:00:00"
    );

    drop_all("ZDBT_EL_ORDERS");
    unsafe { std::env::remove_var("ZDBT_EL_STATE_DIR") };
}
