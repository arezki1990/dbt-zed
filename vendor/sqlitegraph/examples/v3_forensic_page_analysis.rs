//! V3 Forensic Page Analysis
//!
//! Deep analysis of corrupted pages to identify exact corruption pattern.

use std::fs::File;
use std::io::Read;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = "/tmp/v3_corruption_test.db";

    // Read the entire file
    let mut file = File::open(db_path)?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;

    println!("=== V3 FILE FORENSIC ANALYSIS ===\n");
    println!(
        "File size: {} bytes ({} pages)\n",
        contents.len(),
        contents.len() / 4096
    );

    // Check V3 header
    println!("V3 Header (first 112 bytes):");
    println!("  Magic: {:?}", &contents[0..8]);
    println!(
        "  Page size: {}",
        u32::from_be_bytes(contents[100..104].try_into().unwrap())
    );
    println!(
        "  Node count: {}",
        u64::from_be_bytes(contents[72..80].try_into().unwrap())
    );
    println!(
        "  Edge count: {}",
        u64::from_be_bytes(contents[80..88].try_into().unwrap())
    );

    // Scan for corrupted pages
    println!("\n=== SCANNING FOR CORRUPTED PAGES ===\n");
    let page_count = contents.len() / 4096;
    let mut corrupted_pages = Vec::new();

    for page_idx in 1..page_count {
        // Skip page 0 (V3 header)
        let offset = page_idx * 4096;
        if offset + 32 <= contents.len() {
            let page_bytes = &contents[offset..];

            // Check used_bytes field (offset 18-19 within page)
            let used_bytes_bytes = &page_bytes[18..20];
            let used_bytes = u16::from_be_bytes([used_bytes_bytes[0], used_bytes_bytes[1]]);

            // Check for suspicious values (ASCII strings or too large)
            if used_bytes > 4064 || (used_bytes > 32 && used_bytes < 128) {
                corrupted_pages.push((page_idx, used_bytes, offset));
            }
        }
    }

    println!(
        "Found {} potentially corrupted pages:",
        corrupted_pages.len()
    );

    // Analyze first few corrupted pages in detail
    for (page_idx, used_bytes, offset) in corrupted_pages.iter().take(5) {
        println!("\n--- PAGE {} at offset {} ---", *page_idx, *offset);
        println!("used_bytes field: {} (0x{:04x})", *used_bytes, *used_bytes);

        let page_bytes = &contents[*offset..];

        // Show header bytes
        println!("Page header (first 32 bytes):");
        for i in (0..32).step_by(4) {
            let end = (i + 4).min(32);
            let bytes = &page_bytes[i..end];
            println!(
                "  [{}..{}]: {:?} | ASCII: {}",
                i,
                end,
                bytes,
                to_ascii_string(bytes)
            );
        }

        // Show data region start (offset 32-100)
        println!("\nData region start (offset 32-100):");
        let data_start = 32;
        let data_end = (data_start + 68).min(page_bytes.len());
        for i in (data_start..data_end).step_by(16) {
            let end = (i + 16).min(data_end);
            let bytes = &page_bytes[i..end];
            println!(
                "  [{}..{}]: {:?} | {}",
                i,
                end,
                bytes,
                to_ascii_string(bytes)
            );
        }

        // Look for JSON-like patterns
        if let Some(pos) = find_json_start(page_bytes) {
            println!("\nJSON-like content found at offset {}", pos);
            let snippet = &page_bytes[pos..(pos + 50).min(page_bytes.len())];
            println!("  Snippet: {:?}", to_ascii_string(snippet));
        }

        // Check if this could be a B+Tree page
        println!("\nPage type analysis:");
        println!(
            "  page_id (u64 BE): {}",
            u64::from_be_bytes(page_bytes[0..8].try_into().unwrap())
        );
        println!(
            "  next_page_id (u64 BE): {}",
            u64::from_be_bytes(page_bytes[8..16].try_into().unwrap())
        );
        println!(
            "  node_count (u16 BE): {}",
            u16::from_be_bytes(page_bytes[16..18].try_into().unwrap())
        );
        println!("  used_bytes (u16 BE): {}", *used_bytes);
        println!(
            "  base_id (i64 BE): {}",
            i64::from_be_bytes(page_bytes[20..28].try_into().unwrap())
        );
    }

    // Check specifically for pages with ASCII "kind:" strings (node kind data)
    println!("\n=== SEARCHING FOR KIND/NAME STRING DATA ===\n");
    let mut kind_pages = Vec::new();
    let search_string = b"kind";

    for page_idx in 1..page_count {
        let offset = page_idx * 4096;
        if offset + 100 <= contents.len() {
            let page_bytes = &contents[offset..offset + 4096];
            if page_bytes.windows(4).any(|w| w == *search_string) {
                kind_pages.push(page_idx);
            }
        }
    }

    println!("Found 'kind' string in {} pages", kind_pages.len());
    if !kind_pages.is_empty() {
        println!(
            "First 10 pages with 'kind' string: {:?}",
            kind_pages.iter().take(10).collect::<Vec<_>>()
        );
    }

    Ok(())
}

fn to_ascii_string(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '.'
            }
        })
        .collect()
}

fn find_json_start(data: &[u8]) -> Option<usize> {
    data.iter().position(|&b| b == b'{' || b == b'"')
}
