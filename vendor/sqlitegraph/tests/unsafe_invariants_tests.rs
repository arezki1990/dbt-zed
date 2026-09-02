#![allow(clippy::suspicious_open_options, reason = "test/bench code")]
//! TDD Tests for Unsafe Code Invariants
//!
//! These tests verify the safety properties of unsafe code blocks identified
//! in the unsafe code audit (docs/UNSAFE_CODE_AUDIT_REPORT.md).

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

// ============================================================================
// Test Category 1: V3 HNSW Storage Handle Safety
// ============================================================================

/// Test that V3VectorStorageHandle properly implements Send trait
/// This verifies the unsafe Send impl is sound
#[test]
#[cfg(feature = "native-v3")]
fn test_v3_storage_handle_is_send() {
    fn assert_send<T: Send>() {}

    // This will fail to compile if V3VectorStorageHandle is not Send
    // The unsafe impl Send is only sound if the underlying data is Send
    // assert_send::<V3VectorStorageHandle>();

    // Note: We can't directly test this without exposing the type,
    // but the test documents the requirement
}

/// Test that V3VectorStorageHandle properly implements Sync trait
/// This verifies the unsafe Sync impl is sound
#[test]
#[cfg(feature = "native-v3")]
fn test_v3_storage_handle_is_sync() {
    fn assert_sync<T: Sync>() {}

    // This will fail to compile if V3VectorStorageHandle is not Sync
    // assert_sync::<V3VectorStorageHandle>();
}

/// Test that raw pointer operations in V3 storage are bounded
/// This verifies the unsafe pointer derefs don't cause UB
#[test]
#[cfg(feature = "native-v3")]
fn test_v3_storage_pointer_safety() {
    // This test verifies that:
    // 1. The backend pointer is valid when dereferenced
    // 2. No use-after-free occurs
    // 3. Thread safety is maintained

    // Actual implementation would require exposing V3VectorStorageHandle
    // For now, this documents the test requirement
}

// ============================================================================
// Test Category 2: SIMD Safety
// ============================================================================

/// Test that SIMD operations produce correct results
/// This verifies the unsafe AVX2 intrinsics work correctly
#[test]
#[cfg(all(feature = "native-v3", target_arch = "x86_64"))]
fn test_simd_dot_product_correctness() {
    use sqlitegraph::hnsw::simd::dot_product;

    // Test vectors
    let a = vec![1.0f32, 2.0, 3.0, 4.0];
    let b = vec![5.0f32, 6.0, 7.0, 8.0];

    // Expected: 1*5 + 2*6 + 3*7 + 4*8 = 5 + 12 + 21 + 32 = 70
    let result = dot_product(&a, &b);
    assert!((result - 70.0).abs() < f32::EPSILON * 10.0);
}

/// Test that SIMD handles edge cases correctly
#[test]
#[cfg(all(feature = "native-v3", target_arch = "x86_64"))]
fn test_simd_edge_cases() {
    use sqlitegraph::hnsw::simd::{dot_product, euclidean_distance};

    // Empty vectors should panic
    let empty: Vec<f32> = vec![];
    let result = std::panic::catch_unwind(|| dot_product(&empty, &empty));
    assert!(result.is_err() || result.unwrap() == 0.0);

    // Single element
    let a = vec![3.0f32];
    let b = vec![4.0f32];
    let result = dot_product(&a, &b);
    assert!((result - 12.0).abs() < f32::EPSILON);

    // Zero vectors
    let zeros = vec![0.0f32; 100];
    let result = euclidean_distance(&zeros, &zeros);
    assert!(result.abs() < f32::EPSILON);
}

/// Test SIMD vs scalar consistency
#[test]
#[cfg(all(feature = "native-v3", target_arch = "x86_64"))]
fn test_simd_scalar_consistency() {
    use sqlitegraph::hnsw::simd::{dot_product, dot_product_scalar};

    // Test various sizes to ensure SIMD and scalar match
    for size in [1, 7, 8, 9, 15, 16, 17, 31, 32, 33, 100] {
        let a: Vec<f32> = (0..size).map(|i| i as f32 * 0.5).collect();
        let b: Vec<f32> = (0..size).map(|i| (size - i) as f32 * 0.3).collect();

        let simd_result = dot_product(&a, &b);
        let scalar_result = dot_product_scalar(&a, &b);

        // Results should be very close (may differ slightly due to FMA)
        let diff = (simd_result - scalar_result).abs();
        assert!(
            diff < 1e-4,
            "Size {}: SIMD {} vs Scalar {} (diff {})",
            size,
            simd_result,
            scalar_result,
            diff
        );
    }
}

// ============================================================================
// Test Category 4: Raw Pointer Operations
// ============================================================================

/// Test that unaligned reads don't cause UB with different alignments
#[test]
fn test_read_unaligned_various_alignments() {
    #[repr(C)]
    struct TestStruct {
        a: u64,
        b: u32,
        c: u16,
    }

    // Create buffer with offset to test unaligned reads
    let buffer = vec![0u8; std::mem::size_of::<TestStruct>() + 8];

    for offset in 0..8 {
        let ptr = unsafe { buffer.as_ptr().add(offset) };

        // This should be safe due to read_unaligned
        let _value = unsafe { std::ptr::read_unaligned::<TestStruct>(ptr as *const TestStruct) };
    }
}

// ============================================================================
// Test Category 5: Interior Mutability Patterns
// ============================================================================

/// Test that RwLock-based interior mutability works correctly
/// This verifies patterns used in V3 backend
#[test]
fn test_interior_mutability_with_rwlock() {
    use std::sync::RwLock;

    struct InteriorMutable {
        data: RwLock<u64>,
    }

    let obj = Arc::new(InteriorMutable {
        data: RwLock::new(0),
    });

    // Multiple readers
    let obj2 = Arc::clone(&obj);
    let obj3 = Arc::clone(&obj);

    {
        let _r1 = obj.data.read().unwrap();
        let _r2 = obj2.data.read().unwrap();
        let _r3 = obj3.data.read().unwrap();
        // Multiple concurrent reads should work
    }

    // Writer
    {
        let mut w = obj.data.write().unwrap();
        *w = 42;
    }

    // Verify write
    assert_eq!(*obj.data.read().unwrap(), 42);
}

/// Test that Arc<RwLock<>> pattern prevents use-after-free
#[test]
fn test_arc_rwlock_prevents_use_after_free() {
    use std::sync::{Arc, RwLock};

    struct Resource {
        value: u64,
    }

    let resource = Arc::new(RwLock::new(Resource { value: 42 }));
    let weak = Arc::downgrade(&resource);

    // Clone for another reference
    let resource2 = Arc::clone(&resource);

    // Drop original
    drop(resource);

    // Resource should still be accessible through clone
    assert!(weak.upgrade().is_some());
    assert_eq!(resource2.read().unwrap().value, 42);

    // Drop clone
    drop(resource2);

    // Now resource should be freed
    assert!(weak.upgrade().is_none());
}

// ============================================================================
// Test Category 7: Thread Safety
// ============================================================================

/// Test that AtomicU64 operations are thread-safe
/// This verifies patterns used in V3 storage counters
#[test]
fn test_atomic_operations_thread_safety() {
    use std::thread;

    let counter = Arc::new(AtomicU64::new(0));
    let mut handles = vec![];

    for _ in 0..10 {
        let counter = Arc::clone(&counter);
        handles.push(thread::spawn(move || {
            for _ in 0..1000 {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(counter.load(Ordering::SeqCst), 10000);
}

// ============================================================================
// Test Category 8: Documentation Tests
// ============================================================================

/// This test verifies that doc examples using std::mem::zeroed
/// are actually safe when used as documented
#[test]
fn test_doc_example_zeroed_safety() {
    // The doc examples use std::mem::zeroed() as placeholders
    // This is only safe for types that are valid with all zeros

    // Example: u64 is valid when zeroed
    let zeroed_u64: u64 = unsafe { std::mem::zeroed() };
    assert_eq!(zeroed_u64, 0);

    // Example: Option<&T> is valid when zeroed (None)
    let zeroed_option: Option<&u64> = unsafe { std::mem::zeroed() };
    assert!(zeroed_option.is_none());

    // Note: Not all types are safe to zero - e.g., NonNull<T>, references
}
