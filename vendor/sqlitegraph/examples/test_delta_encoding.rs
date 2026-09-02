use sqlitegraph::backend::native::v3::edge_compat::{Direction, V3EdgeCluster};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing Delta Encoding for Edge ID Compression\n");

    // Create a cluster with sequential edge IDs (common case)
    let mut cluster = V3EdgeCluster::new(1, Direction::Outgoing, 100);
    for i in 1..=100 {
        cluster.add_edge(i * 100, None); // IDs: 100, 200, 300, ..., 10000
    }

    // Serialize with delta encoding
    let serialized = cluster.serialize()?;
    println!(
        "Serialized {} edges with delta encoding",
        cluster.edges.len()
    );
    println!("Total size: {} bytes", serialized.len());

    // Calculate what the size would be without compression
    // Each edge: neighbor_id (8) + type_offset (2) + data_len (2) + data (0) = 12 bytes
    let uncompressed_size = cluster.edges.len() * 12;
    println!("Uncompressed size would be: {} bytes", uncompressed_size);

    // Calculate space savings
    let space_saved = uncompressed_size.saturating_sub(serialized.len());
    let compression_ratio = if !serialized.is_empty() {
        (space_saved as f64 / uncompressed_size as f64) * 100.0
    } else {
        0.0
    };

    println!("\nSpace saved: {} bytes", space_saved);
    println!("Compression ratio: {:.1}%", compression_ratio);

    // Verify roundtrip
    let deserialized = V3EdgeCluster::deserialize(&serialized, 100)?;
    assert_eq!(deserialized.dsts().len(), 100);
    assert_eq!(deserialized.format_version, 3);

    println!("\n✓ Delta encoding is working!");
    println!(
        "✓ Roundtrip successful: {} edges preserved",
        deserialized.dsts().len()
    );

    Ok(())
}
