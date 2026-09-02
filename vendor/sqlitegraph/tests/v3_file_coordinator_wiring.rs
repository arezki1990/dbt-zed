//! Phase 1 TDD regression tests: FileCoordinator wiring into V3Backend.
//!
//! These tests verify that:
//! 1. V3Backend::open() and V3Backend::create() wire a shared FileCoordinator
//!    into NodeStore, BTreeManager, and V3EdgeStore so that cold-path reads
//!    reuse a persistent file handle instead of open/seek/read/close per miss.
//! 2. Concurrent reads from multiple threads do not deadlock (RwLock allows
//!    concurrent readers).
//!
//! **TDD discipline:** These tests were written BEFORE the wiring fix.
//! `test_v3_file_coordinator_wired` fails pre-fix (no coordinator set),
//! passes post-fix. `test_v3_concurrent_reads` would deadlock under a Mutex
//! if read_page serialized all readers; under RwLock it passes.

use sqlitegraph::{NodeSpec, SnapshotId, backend::GraphBackend, backend::native::v3::V3Backend};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::TempDir;

/// Create a V3 DB with `count` nodes, returning the path.
fn make_db_with_nodes(dir: &TempDir, count: i64) -> std::path::PathBuf {
    let db_path = dir.path().join("fc_wiring.graph");
    {
        let backend = V3Backend::create(&db_path).expect("create backend");
        for i in 0..count {
            backend
                .insert_node(NodeSpec {
                    kind: "BenchNode".to_string(),
                    name: format!("node_{i}"),
                    file_path: None,
                    data: serde_json::json!({"idx": i}),
                })
                .expect("insert node");
        }
        backend.flush_to_disk().expect("flush");
    }
    db_path
}

/// Regression test: after create() then reopen via open(), the FileCoordinator
/// must be present on the backend (shared across node_store, its btree, and
/// edge_store). Verified via the public `has_file_coordinator()` getter.
#[test]
fn test_v3_file_coordinator_wired() {
    let temp_dir = TempDir::new().expect("tempdir");
    let db_path = make_db_with_nodes(&temp_dir, 100);

    // Reopen — this is the production path that must wire the coordinator.
    let backend = V3Backend::open(&db_path).expect("open backend");

    // The coordinator must be present. Pre-fix this returns false.
    assert!(
        backend.has_file_coordinator(),
        "FileCoordinator is not wired into V3Backend::open() — \
         cold-path reads will do open/seek/read/close per cache miss"
    );

    // Sanity: a real read works and returns the expected node.
    let entity = backend
        .get_node(SnapshotId(0), 1)
        .expect("get_node must succeed");
    assert_eq!(entity.id, 1);
}

/// Regression test: the create() path must also wire the coordinator.
#[test]
fn test_v3_file_coordinator_wired_on_create() {
    let temp_dir = TempDir::new().expect("tempdir");
    let db_path = temp_dir.path().join("fc_create.graph");

    let backend = V3Backend::create(&db_path).expect("create backend");
    assert!(
        backend.has_file_coordinator(),
        "FileCoordinator is not wired into V3Backend::create() — \
         fresh backends must share a coordinator too"
    );
}

/// Regression test: 4 threads doing get_node simultaneously must all succeed
/// with no deadlock. Under the old Mutex inner lock, concurrent read_page
/// calls would serialize (not deadlock, but no concurrency). Under a bug
/// where read_page took a write lock, this could deadlock or panic.
/// Under the correct RwLock, all 4 readers proceed concurrently.
#[test]
fn test_v3_concurrent_reads() {
    let temp_dir = TempDir::new().expect("tempdir");
    let db_path = make_db_with_nodes(&temp_dir, 100);

    let backend = Arc::new(V3Backend::open(&db_path).expect("open backend"));
    let barrier = Arc::new(Barrier::new(4));
    let success = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let b = Arc::clone(&backend);
            let bar = Arc::clone(&barrier);
            let ok = Arc::clone(&success);
            let err = Arc::clone(&errors);
            thread::spawn(move || {
                // Synchronize start so all threads hit get_node at once.
                bar.wait();
                // Each thread reads 25 distinct nodes.
                for node_id in 1..=25i64 {
                    match b.get_node(SnapshotId(0), node_id) {
                        Ok(entity) => {
                            if entity.id == node_id {
                                ok.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Err(_e) => {
                            err.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("worker thread panicked");
    }

    assert_eq!(
        errors.load(Ordering::Relaxed),
        0,
        "concurrent reads produced errors — possible deadlock or lock contention bug"
    );
    assert_eq!(
        success.load(Ordering::Relaxed),
        100,
        "not all concurrent reads returned the correct node id"
    );
}
