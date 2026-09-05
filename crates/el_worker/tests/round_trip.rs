//! Full round-trip through the REAL worker binary: seed a demo db, spawn
//! `zdbt-el-worker extract`, read chunks back over the JSON+IPC protocol.

use el_engine::connectors::Extractor as _;
use el_engine::connectors::remote::RemoteExtractor;

#[test]
fn worker_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("demo.duckdb");
    el_engine::connectors::duckdb::create_demo_db(&db).unwrap();

    let worker = std::path::Path::new(env!("CARGO_BIN_EXE_zdbt-el-worker"));
    let mut extractor =
        RemoteExtractor::spawn_duckdb(worker, &db, Some("main"), "demo_orders", 2).unwrap();

    let schema = extractor.schema().unwrap();
    assert!(schema.len() >= 4, "{schema:?}");

    let mut total = 0usize;
    let mut chunks = 0usize;
    while let Some(chunk) = extractor.next_chunk().unwrap() {
        total += chunk.height();
        chunks += 1;
    }
    assert_eq!(total, 5);
    assert!(chunks >= 2, "chunk-rows 2 over 5 rows must chunk");
}
