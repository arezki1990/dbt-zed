//! Allocator Startup Cost Test
//!
//! Measures PageAllocator::new() time before and after sparse bitmap optimization

use sqlitegraph::backend::native::v3::allocator::PageAllocator;
use sqlitegraph::backend::native::v3::header::PersistentHeaderV3;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== ALLOCATOR STARTUP COST TEST ===\n");

    // Test different database sizes
    let sizes = vec![1000, 10_000, 100_000];

    for total_pages in sizes {
        println!("Testing with {} pages...", total_pages);

        // Create header with specified total_pages
        let mut header = PersistentHeaderV3::new_v3();
        header.total_pages = total_pages as u64;

        // Measure allocator creation time
        let start = Instant::now();
        let _allocator = PageAllocator::new(&header);
        let elapsed = start.elapsed();

        println!(
            "  PageAllocator::new(): {:.2} µs",
            elapsed.as_secs_f64() * 1_000_000.0
        );

        // Verify bitmap is sparse (only 2 entries for pages 0 and 1)
        let (allocated, _free, _total) = _allocator.stats();
        println!("  Allocated in bitmap: {}", allocated);
        println!("  Expected: 2 (pages 0 and 1 only)");
        assert_eq!(
            allocated, 2,
            "Sparse bitmap should only track pages 0 and 1"
        );
        println!("  ✓ Sparse bitmap verified\n");
    }

    println!("=== ALLOCATOR STARTUP COST TEST PASSED ===");
    Ok(())
}
