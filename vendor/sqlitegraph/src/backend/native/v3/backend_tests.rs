use super::*;

#[path = "backend_tests/basic.rs"]
mod basic;

#[path = "backend_tests/hnsw.rs"]
mod hnsw;

#[path = "backend_csr_neighbor_tests.rs"]
mod backend_csr_neighbor_tests;

#[path = "backend_csr_traversal_tests.rs"]
mod backend_csr_traversal_tests;

#[path = "backend_tests/query_index.rs"]
mod query_index;

#[path = "backend_tests/snapshots.rs"]
mod snapshots;
