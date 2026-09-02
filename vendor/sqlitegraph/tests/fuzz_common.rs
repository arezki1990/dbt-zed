use rand::{SeedableRng, rngs::StdRng};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

pub fn fuzz_iterations() -> usize {
    std::env::var("SQLITEGRAPH_FUZZ_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256)
}

pub fn labeled_rng(label: &str) -> StdRng {
    let mut hasher = DefaultHasher::new();
    label.hash(&mut hasher);
    StdRng::seed_from_u64(hasher.finish())
}
