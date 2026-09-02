use super::free_page_support::{FreePageHeader, xor_checksum};
use super::*;

#[test]
fn test_allocator_initialization() {
    let header = PersistentHeaderV3::new_v3();
    let allocator = PageAllocator::new(&header);

    let (allocated, free, total) = allocator.stats();
    assert_eq!(
        total, 0,
        "New allocator should have 0 total pages from header"
    );
    assert_eq!(
        allocated, 2,
        "Pages 0 (header) and 1 (data) should be reserved"
    );
    assert_eq!(free, 0, "Should have 0 free pages initially (all reserved)");
}

#[test]
fn test_page_offset_calculation() {
    assert_eq!(
        PageAllocator::page_offset(0).unwrap(),
        0,
        "Page 0 should be at offset 0"
    );

    let first_data = V3_HEADER_SIZE;
    assert_eq!(
        PageAllocator::page_offset(1).unwrap(),
        first_data,
        "Page 1 should start after header"
    );

    let second_data = V3_HEADER_SIZE + PAGE_SIZE;
    assert_eq!(
        PageAllocator::page_offset(2).unwrap(),
        second_data,
        "Page 2 should start after page 1"
    );
}

#[test]
fn test_allocate_new_pages() {
    let header = PersistentHeaderV3::new_v3();
    let mut allocator = PageAllocator::new(&header);

    let page1 = allocator.allocate().unwrap();
    assert_eq!(
        page1, 2,
        "First allocation should be page 2 (pages 0,1 reserved)"
    );

    let state1 = allocator.get_page_state(page1).unwrap();
    assert_eq!(state1, PageState::Allocated);

    let page2 = allocator.allocate().unwrap();
    assert_eq!(page2, 3, "Second allocation should be page 3");
}

#[test]
fn test_deallocate_pages() {
    let header = PersistentHeaderV3::new_v3();
    let mut allocator = PageAllocator::new(&header);

    let page1 = allocator.allocate().unwrap();
    assert_eq!(page1, 2, "First allocation returns page 2");

    let page2 = allocator.allocate().unwrap();
    assert_eq!(page2, 3, "Second allocation returns page 3");

    allocator.deallocate(page2).unwrap();

    let state = allocator.get_page_state(page2).unwrap();
    assert_eq!(state, PageState::Free);
}

#[test]
fn test_double_free_detection() {
    let header = PersistentHeaderV3::new_v3();
    let mut allocator = PageAllocator::new(&header);

    let page0 = allocator.allocate().unwrap();
    assert_eq!(page0, 2);

    let page1 = allocator.allocate().unwrap();
    assert_eq!(page1, 3);

    allocator.deallocate(page1).unwrap();

    let result = allocator.deallocate(page1);
    assert!(result.is_err(), "Double-free should return error");
}

#[test]
fn test_pin_page() {
    let header = PersistentHeaderV3::new_v3();
    let mut allocator = PageAllocator::new(&header);

    let page = allocator.allocate().unwrap();
    allocator.pin_page(page).unwrap();

    let state = allocator.get_page_state(page).unwrap();
    assert_eq!(state, PageState::Allocated);
}

#[test]
fn test_checksum_validation() {
    let data = b"test page data";
    let checksum = xor_checksum(data);

    assert!(PageAllocator::validate_checksum(data, checksum).is_ok());

    let result = PageAllocator::validate_checksum(data, checksum + 1);
    assert!(result.is_err(), "Invalid checksum should fail validation");
}

#[test]
fn test_free_page_header_serialization() {
    let header = FreePageHeader::new(42);
    let bytes = header.to_bytes();

    assert_eq!(bytes[0..8], 42u64.to_le_bytes());
    assert_eq!(bytes[8..16], 0u64.to_le_bytes());

    let deserialized = FreePageHeader::from_bytes(&bytes).unwrap();
    assert_eq!(deserialized.next_free, 42);
}

#[test]
fn test_free_list_chain_reuse() {
    let header = PersistentHeaderV3::new_v3();
    let mut allocator = PageAllocator::new(&header);

    let pages: Vec<u64> = (0..5).map(|_| allocator.allocate().unwrap()).collect();
    assert_eq!(pages, vec![2, 3, 4, 5, 6]);

    allocator.deallocate(3).unwrap();
    allocator.deallocate(4).unwrap();
    allocator.deallocate(5).unwrap();

    let reused1 = allocator.allocate().unwrap();
    assert_eq!(reused1, 5, "First reuse should be last freed (LIFO)");

    let reused2 = allocator.allocate().unwrap();
    assert_eq!(reused2, 4, "Second reuse should be middle freed");

    let reused3 = allocator.allocate().unwrap();
    assert_eq!(reused3, 3, "Third reuse should be first freed");

    let new_page = allocator.allocate().unwrap();
    assert_eq!(
        new_page, 7,
        "After exhausting free list, should allocate new page 7"
    );
}

#[test]
fn test_stats_accuracy_after_alloc_dealloc() {
    let header = PersistentHeaderV3::new_v3();
    let mut allocator = PageAllocator::new(&header);

    allocator.allocate().unwrap();
    allocator.allocate().unwrap();
    allocator.allocate().unwrap();

    let (allocated, free, total) = allocator.stats();
    assert_eq!(
        allocated, 5,
        "Should have 5 allocated pages (2 reserved + 3 new)"
    );
    assert_eq!(free, 0, "No free pages yet");
    assert_eq!(total, 5, "Total pages should be 5");

    allocator.deallocate(3).unwrap();
    let (allocated, free, total) = allocator.stats();
    assert_eq!(allocated, 4, "Should have 4 allocated after freeing one");
    assert_eq!(free, 1, "Should have 1 free page");
    assert_eq!(total, 5, "Total pages unchanged");

    allocator.deallocate(4).unwrap();
    let (allocated, free, _total) = allocator.stats();
    assert_eq!(allocated, 3, "Should have 3 allocated after freeing two");
    assert_eq!(free, 2, "Should have 2 free pages");
}

#[test]
fn test_double_free_beyond_bitmap() {
    let header = PersistentHeaderV3::new_v3();
    let mut allocator = PageAllocator::new(&header);

    let page = allocator.allocate().unwrap();
    assert_eq!(page, 2);

    allocator.total_pages = 5000;

    allocator.deallocate(page).unwrap();

    let result = allocator.deallocate(page);
    assert!(
        result.is_err(),
        "Double-free of freed page should be detected"
    );
}
