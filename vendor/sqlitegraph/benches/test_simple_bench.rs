//! Simple test benchmark to verify Criterion setup

use criterion::{Criterion, black_box, criterion_group, criterion_main};

pub fn bench_simple(c: &mut Criterion) {
    c.bench_function("simple_test", |b| {
        b.iter(|| {
            black_box(42 + 42);
        });
    });
}

criterion_group!(benches, bench_simple);
criterion_main!(benches);
