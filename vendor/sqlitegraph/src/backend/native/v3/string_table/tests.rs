//! String Table Tests - TDD for V3 String Table
//!
//! These tests define the expected behavior of the V3 String Table.
//! Note: The string table is pre-populated with 16 common edge types.

use super::*;

const COMMON_TYPE_COUNT: usize = 16; // Number of pre-populated common edge types

#[test]
fn test_string_table_new_has_common_types() {
    let table = StringTable::new();
    // Table is pre-populated with common edge types
    assert!(!table.is_empty());
    assert_eq!(table.len(), COMMON_TYPE_COUNT);
}

#[test]
fn test_get_or_add_offset_basic() {
    let mut table = StringTable::new();

    // Adding a new string (not a common type)
    let offset1 = table.get_or_add_offset("Function").unwrap();
    // Offset should be after common types (cumulative byte offset)
    assert!(offset1 >= COMMON_TYPE_COUNT as u16);

    // Same string returns same offset
    let offset1_again = table.get_or_add_offset("Function").unwrap();
    assert_eq!(offset1_again, offset1);

    // Different string gets different offset
    let offset2 = table.get_or_add_offset("Class").unwrap();
    assert_ne!(offset2, offset1);
}

#[test]
fn test_get_string_by_offset() {
    let mut table = StringTable::new();

    let offset = table.get_or_add_offset("MyNode").unwrap();
    let retrieved = table.get_string(offset).unwrap();

    assert_eq!(retrieved, "MyNode");
}

#[test]
fn test_get_string_invalid_offset() {
    let table = StringTable::new();

    // Invalid offset should return error
    let result = table.get_string(999);
    assert!(result.is_err());
}

#[test]
fn test_string_deduplication() {
    let mut table = StringTable::new();
    let initial_len = table.len();

    // Add same string multiple times
    let offset1 = table.get_or_add_offset("duplicate").unwrap();
    let offset2 = table.get_or_add_offset("duplicate").unwrap();
    let offset3 = table.get_or_add_offset("duplicate").unwrap();

    // All should return the same offset
    assert_eq!(offset1, offset2);
    assert_eq!(offset2, offset3);

    // Table should only have 1 new unique string
    assert_eq!(table.len(), initial_len + 1);
}

#[test]
fn test_multiple_unique_strings() {
    let mut table = StringTable::new();
    let initial_len = table.len();

    let kinds = vec!["Function", "Class", "Variable", "Module", "Trait"];

    for kind in &kinds {
        table.get_or_add_offset(kind).unwrap();
    }

    assert_eq!(table.len(), initial_len + kinds.len());

    // Verify all can be retrieved
    for kind in &kinds {
        let offset = table.get_or_add_offset(kind).unwrap();
        let retrieved = table.get_string(offset).unwrap();
        assert_eq!(retrieved, *kind);
    }
}

#[test]
fn test_offset_u16_limit() {
    let mut table = StringTable::new();

    // Add many strings to approach u16 limit
    for i in 0..1000 {
        let s = format!("string_{}", i);
        let _offset = table.get_or_add_offset(&s).unwrap();
        // offset is u16, so it is always <= u16::MAX by definition
    }
}

#[test]
fn test_common_types_pre_populated() {
    let mut table = StringTable::new();

    // Common types should already have low offsets
    let calls_offset = table.get_or_add_offset("calls").unwrap();
    let imports_offset = table.get_or_add_offset("imports").unwrap();

    // Both should have small offsets (pre-populated)
    assert!(calls_offset < 100);
    assert!(imports_offset < 100);
    assert_ne!(calls_offset, imports_offset);
}

#[test]
fn test_serialize_deserialize() {
    let mut table = StringTable::new();

    // Add some strings
    table.get_or_add_offset("Function").unwrap();
    table.get_or_add_offset("Class").unwrap();
    table.get_or_add_offset("Variable").unwrap();

    let serialized = table.serialize();
    let deserialized = StringTable::deserialize(&serialized).unwrap();

    assert_eq!(deserialized.len(), table.len());

    // Verify strings can still be retrieved
    for s in &["Function", "Class", "Variable"] {
        let offset = table.get_or_add_offset(s).unwrap();
        let retrieved = deserialized.get_string(offset).unwrap();
        assert_eq!(retrieved, *s);
    }
}

#[test]
fn test_round_trip_preserve_offsets() {
    let mut table = StringTable::new();

    // Get offsets before serialization
    let offset_func = table.get_or_add_offset("Function").unwrap();
    let offset_class = table.get_or_add_offset("Class").unwrap();

    let serialized = table.serialize();
    let deserialized = StringTable::deserialize(&serialized).unwrap();

    // Offsets should be preserved
    assert_eq!(deserialized.get_string(offset_func).unwrap(), "Function");
    assert_eq!(deserialized.get_string(offset_class).unwrap(), "Class");
}

#[test]
fn test_serialize_includes_common_types() {
    let table = StringTable::new();
    let serialized = table.serialize();

    // Should have 4 bytes for count + entries for common types
    assert!(serialized.len() > 4);

    let deserialized = StringTable::deserialize(&serialized).unwrap();
    // After deserialization, common types should be restored
    assert_eq!(deserialized.len(), COMMON_TYPE_COUNT);
}

#[test]
fn test_large_string_handling() {
    let mut table = StringTable::new();

    // String at u16::MAX bytes should be truncated
    let large_string = "a".repeat(70000);
    let offset = table.get_or_add_offset(&large_string).unwrap();

    // Should still work, but string may be truncated
    let retrieved = table.get_string(offset).unwrap();
    assert!(!retrieved.is_empty());
}

#[test]
fn test_string_table_clear() {
    let mut table = StringTable::new();

    table.get_or_add_offset("test").unwrap();
    assert_eq!(table.len(), COMMON_TYPE_COUNT + 1);

    table.clear();
    // After clear, only common types remain
    assert_eq!(table.len(), COMMON_TYPE_COUNT);
}

#[test]
fn test_kind_and_name_offsets_independent() {
    let mut table = StringTable::new();

    // Simulate adding kinds and names
    let kind_offset = table.get_or_add_offset("Function").unwrap();
    let name_offset = table.get_or_add_offset("my_func").unwrap();

    // They should be different
    assert_ne!(kind_offset, name_offset);

    // Both retrievable
    assert_eq!(table.get_string(kind_offset).unwrap(), "Function");
    assert_eq!(table.get_string(name_offset).unwrap(), "my_func");
}

#[test]
fn test_serialized_size_calculation() {
    let mut table = StringTable::new();

    // Initial size includes common types
    let initial_size = table.serialized_size();
    assert!(initial_size > 4);

    // Add one string
    table.get_or_add_offset("test").unwrap();

    // Size should increase
    let new_size = table.serialized_size();
    assert!(new_size > initial_size);
}

#[test]
fn test_deserialize_corrupted_data() {
    // Too short
    let short_data = vec![0u8; 2];
    let result = StringTable::deserialize(&short_data);
    assert!(result.is_err());

    // Invalid count (too large)
    let mut bad_data = vec![0u8; 4];
    bad_data[0] = 0xFF;
    bad_data[1] = 0xFF;
    bad_data[2] = 0xFF;
    bad_data[3] = 0x7F; // Very large count
    let result = StringTable::deserialize(&bad_data);
    // Should error due to safety limit
    assert!(result.is_err());
}

#[test]
fn test_common_edge_types_accessible() {
    let mut table = StringTable::new();

    // All common edge types should be accessible
    let common_types = vec![
        "calls",
        "imports",
        "defines",
        "uses",
        "contains",
        "implements",
        "extends",
        "references",
        "declares",
        "exports",
        "inherits",
        "overrides",
        "depends_on",
        "relates_to",
        "includes",
        "aliases",
    ];

    for edge_type in &common_types {
        let offset = table.get_or_add_offset(edge_type).unwrap();
        let retrieved = table.get_string(offset).unwrap();
        assert_eq!(retrieved, *edge_type);
    }
}

#[test]
fn test_stats() {
    let mut table = StringTable::new();
    let stats = table.stats();

    assert_eq!(stats.unique_strings, COMMON_TYPE_COUNT);
    assert_eq!(stats.common_types_cached, COMMON_TYPE_COUNT);
    assert!(stats.total_bytes > 0);

    // Add more strings
    table.get_or_add_offset("test").unwrap();
    let stats2 = table.stats();

    assert_eq!(stats2.unique_strings, COMMON_TYPE_COUNT + 1);
}
