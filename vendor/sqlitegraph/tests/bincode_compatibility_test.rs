//! Bincode 1.3 compatibility tests
//!
//! This test verifies that serialization works correctly with bincode 1.3.

/// A simple test struct for serialization testing
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct TestStruct {
    id: u64,
    name: String,
    value: i32,
}

#[test]
fn test_bincode_encode() -> Result<(), Box<dyn std::error::Error>> {
    let data = TestStruct {
        id: 42,
        name: "test".to_string(),
        value: -100,
    };

    // Test that bincode 1.3 encode works with serde feature
    let encoded = bincode::serialize(&data)?;

    assert!(!encoded.is_empty());
    // Small struct with id (8 bytes), len (varint), string "test" (4 bytes + 1 byte len),
    // and value (4 bytes) should be at least a few bytes
    assert!(encoded.len() >= 5);

    Ok(())
}

#[test]
fn test_bincode_decode() -> Result<(), Box<dyn std::error::Error>> {
    let data = TestStruct {
        id: 42,
        name: "test".to_string(),
        value: -100,
    };

    let encoded = bincode::serialize(&data)?;

    // Test that bincode 1.3 decode works with serde
    let decoded: TestStruct = bincode::deserialize(&encoded)?;

    assert_eq!(data, decoded);

    Ok(())
}

#[test]
fn test_bincode_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let original = TestStruct {
        id: 123456789012345678,
        name: "Hello, bincode 1.3!".to_string(),
        value: 42,
    };

    // Encode
    let encoded = bincode::serialize(&original)?;

    // Decode
    let decoded: TestStruct = bincode::deserialize(&encoded)?;

    // Verify round-trip
    assert_eq!(original, decoded);
    assert_eq!(decoded.id, 123456789012345678);
    assert_eq!(decoded.name, "Hello, bincode 1.3!");
    assert_eq!(decoded.value, 42);

    Ok(())
}

#[test]
fn test_bincode_vec_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let original = vec![
        TestStruct {
            id: 1,
            name: "first".to_string(),
            value: 10,
        },
        TestStruct {
            id: 2,
            name: "second".to_string(),
            value: 20,
        },
        TestStruct {
            id: 3,
            name: "third".to_string(),
            value: 30,
        },
    ];

    let encoded = bincode::serialize(&original)?;

    let decoded: Vec<TestStruct> = bincode::deserialize(&encoded)?;

    assert_eq!(original, decoded);
    assert_eq!(decoded.len(), 3);

    Ok(())
}

#[test]
fn test_bincode_basic_types() -> Result<(), Box<dyn std::error::Error>> {
    // Test basic types round-trip
    let original_i32: i32 = -12345;
    let encoded_i32 = bincode::serialize(&original_i32)?;
    let decoded_i32: i32 = bincode::deserialize(&encoded_i32)?;
    assert_eq!(original_i32, decoded_i32);

    let original_u64: u64 = 987654321012345678;
    let encoded_u64 = bincode::serialize(&original_u64)?;
    let decoded_u64: u64 = bincode::deserialize(&encoded_u64)?;
    assert_eq!(original_u64, decoded_u64);

    let original_str: String = "Hello, bincode 1.3 with serde!".to_string();
    let encoded_str = bincode::serialize(&original_str)?;
    let decoded_str: String = bincode::deserialize(&encoded_str)?;
    assert_eq!(original_str, decoded_str);

    Ok(())
}

#[test]
fn test_bincode_format_size() -> Result<(), Box<dyn std::error::Error>> {
    // Verify that the serialization format is reasonable
    let data = TestStruct {
        id: 1,
        name: "a".to_string(),
        value: 100,
    };

    let encoded = bincode::serialize(&data)?;

    // Verify we get a reasonable encoding size
    // bincode should encode this efficiently, certainly less than 100 bytes
    assert!(
        encoded.len() < 100,
        "Encoded size {} is unexpectedly large",
        encoded.len()
    );

    // Should be non-zero
    assert!(!encoded.is_empty());

    Ok(())
}
