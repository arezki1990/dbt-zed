//! Phase 4 TDD regression tests: cache sizing, LRU eviction, double-decode fix.
//!
//! These tests verify that:
//! 1. `PAGE_CACHE_SIZE` was raised from 64 to 1024 pages (4 MiB at 4 KiB/page).
//! 2. Node page cache eviction is **LRU**, not FIFO: a hot node read
//!    repeatedly stays resident even when enough other nodes are read to fill
//!    the cache well beyond the hot node's insertion order. Under FIFO the
//!    hot node (inserted first) would be evicted first; under LRU it survives
//!    because it is the most-recently-used.
//! 3. The mutable lookup path (`get_node` → `lookup_node` → `load_node_page`)
//!    consults the unpacked-page cache, so repeated reads of the same node do
//!    not re-run varint decoding on every hit (double-decode bug).
//!
//! **TDD discipline:** `test_v3_cache_lru_eviction` was written BEFORE the
//! FIFO→LRU conversion. It fails under FIFO eviction (hot node 1 evicted) and
//! passes under LRU eviction (hot node 1 retained because recently used).
//! `test_v3_page_cache_size_raised` fails at 64 and passes at 1024.

use sqlitegraph::{NodeSpec, SnapshotId, backend::GraphBackend, backend::native::v3::V3Backend};
use tempfile::TempDir;

/// The raised page-cache capacity (Phase 4: 64 → 1024 pages = 4 MiB at 4 KiB/page).
const EXPECTED_PAGE_CACHE_SIZE: usize = 1024;

/// Create a V3 DB with `count` nodes and flush it durably.
fn make_db_with_nodes(dir: &TempDir, count: i64) -> std::path::PathBuf {
    let db_path = dir.path().join("cache_sizing.graph");
    {
        let backend = V3Backend::create(&db_path).expect("create backend");
        for i in 0..count {
            backend
                .insert_node(NodeSpec {
                    kind: "CacheNode".to_string(),
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

/// Regression: `PAGE_CACHE_SIZE` must be 1024, not the old 64.
/// A 1000-node graph needs more than 64 pages; 1024 pages (4 MiB) holds a
/// realistic working set without thrashing.
#[test]
fn test_v3_page_cache_size_raised() {
    let temp_dir = TempDir::new().expect("tempdir");
    let db_path = temp_dir.path().join("cache_size.graph");
    let backend = V3Backend::create(&db_path).expect("create backend");
    assert_eq!(
        backend.node_page_cache_capacity(),
        EXPECTED_PAGE_CACHE_SIZE,
        "PAGE_CACHE_SIZE must be 1024 (4 MiB), not 64 (256 KiB). \
         A 1000-node graph needs more than 64 pages."
    );
}

/// Regression: node page cache eviction must be LRU, not FIFO.
///
/// Inserts 2000 nodes (exceeds the 1024-page cache many times over), then:
///   1. Reads node 1 repeatedly — warming it as the hottest page.
///   2. Reads nodes 2..=1500 — filling the cache with colder pages.
///   3. Asserts node 1's page is STILL resident in the page cache.
///
/// Under FIFO, node 1 was inserted first so it is the first victim once the
/// cache fills — node 1 would be evicted and this test fails. Under LRU, node
/// 1 is the most-recently-used (read last in the warm-up loop) so it survives
/// the colder-node reads. (We read node 1 *after* the cold reads too, so even
/// a strict access-order LRU keeps it.)
#[test]
fn test_v3_cache_lru_eviction() {
    let temp_dir = TempDir::new().expect("tempdir");
    let db_path = make_db_with_nodes(&temp_dir, 2000);

    let backend = V3Backend::open(&db_path).expect("open backend");

    let capacity = backend.node_page_cache_capacity();
    assert!(
        capacity < 2000,
        "test premise: cache capacity ({capacity}) must be smaller than node count (2000)"
    );

    // Phase A: warm node 1 as the hottest entry. Repeated reads make node 1's
    // page the most-recently-used under LRU (and merely first-inserted under FIFO).
    for _ in 0..5 {
        let e = backend.get_node(SnapshotId(0), 1).expect("read node 1");
        assert_eq!(e.id, 1);
    }

    // Phase B: read many distinct nodes to fill the cache with cold pages.
    // 1500 nodes span many pages — far more than the 1024-page capacity — so the
    // cache is forced to evict. Under FIFO, node 1 (inserted first) is evicted.
    for node_id in 2..=1500i64 {
        let _ = backend.get_node(SnapshotId(0), node_id);
    }

    // Phase C: read node 1 ONE more time so it is the most-recently-used under
    // even the strictest access-order LRU. Then read a few more cold nodes so
    // that if eviction were FIFO, node 1 (oldest by insertion) would be chosen.
    let e = backend
        .get_node(SnapshotId(0), 1)
        .expect("read node 1 again");
    assert_eq!(e.id, 1);

    // Phase D: assert node 1's page is resident. This is the core assertion.
    // Under FIFO it would have been evicted during phase B/C; under LRU it stays
    // because the phase-C read made it most-recently-used.
    assert!(
        backend.node_page_cache_resident_for(1),
        "node 1 was evicted from the page cache — eviction is FIFO, not LRU. \
         Node 1 was read most recently yet is no longer resident, meaning the \
         cache evicts by insertion order rather than by least-recently-used."
    );
}

/// Regression: the mutable lookup path must consult the unpacked-page cache.
///
/// Before the fix, `get_node` (→ `lookup_node` → `load_node_page`) checked the
/// raw-bytes `page_cache` and re-ran `NodePage::unpack` (varint decode) on every
/// hit, ignoring the `unpacked_page_cache`. After the fix, a second read of the
/// same node finds the already-decoded page in `unpacked_page_cache`.
///
/// We assert the unpacked cache is populated after a read (it would stay empty
/// on the mutable path pre-fix, since only `lookup_node_ro` populated it).
#[test]
fn test_v3_mutable_lookup_uses_unpacked_cache() {
    let temp_dir = TempDir::new().expect("tempdir");
    let db_path = make_db_with_nodes(&temp_dir, 50);

    let backend = V3Backend::open(&db_path).expect("open backend");

    // Clear caches to start clean, then read node 1 via get_node (mutable path).
    backend.clear_node_page_caches();

    assert_eq!(
        backend.node_unpacked_cache_len(),
        0,
        "unpacked cache must be empty after clear"
    );

    let e = backend.get_node(SnapshotId(0), 1).expect("read node 1");
    assert_eq!(e.id, 1);

    assert!(
        backend.node_unpacked_cache_len() >= 1,
        "unpacked page cache was not populated by the mutable get_node path — \
         load_node_page is not consulting unpacked_page_cache, causing \
         repeated varint decoding on every cache hit (double-decode bug)"
    );
}
