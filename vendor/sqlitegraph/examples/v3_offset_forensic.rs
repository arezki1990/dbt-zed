//! V3 Offset Forensic Analysis
//!
//! Reads pages at the CORRECT offsets (accounting for V3_HEADER_SIZE=112)

use std::fs::File;
use std::io::Read;

const V3_HEADER_SIZE: u64 = 112;
const PAGE_SIZE: u64 = 4096;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = "/tmp/v3_corruption_test.db";

    // Read the entire file
    let mut file = File::open(db_path)?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;

    println!("=== V3 CORRECT OFFSET FORENSIC ANALYSIS ===\n");
    println!("File size: {} bytes\n", contents.len());

    // Function to get page offset
    fn page_offset(page_id: u64) -> u64 {
        if page_id == 0 {
            return 0;
        }
        V3_HEADER_SIZE + (page_id - 1) * PAGE_SIZE
    }

    // Check first few pages at CORRECT offsets
    for page_id in 1u64..=10 {
        let offset = page_offset(page_id) as usize;
        println!("--- Page {} at offset {} ---", page_id, offset);

        if offset + 100 <= contents.len() {
            let page_bytes = &contents[offset..];

            // Show header
            println!("Header (first 32 bytes):");
            for i in (0..32).step_by(8) {
                let end = (i + 8).min(32);
                let bytes = &page_bytes[i..end];
                println!("  [{}..{}]: {:?}", i, end, bytes);
            }

            // Check used_bytes
            let used_bytes = u16::from_be_bytes([page_bytes[18], page_bytes[19]]);
            println!("used_bytes: {}", used_bytes);

            // Check for ASCII data in header (corruption indicator)
            let header_ascii: String = page_bytes[0..32]
                .iter()
                .map(|&b| {
                    if b.is_ascii_graphic() || b == b' ' {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            println!("Header as ASCII: {}", header_ascii);

            if used_bytes > 4064 {
                println!("*** CORRUPTION DETECTED: used_bytes exceeds page boundary ***");
            }

            // Show some data region
            println!("Data region (offset 32-96):");
            for i in (32..96).step_by(16) {
                let end = (i + 16).min(96);
                let bytes = &page_bytes[i..end];
                let ascii: String = bytes
                    .iter()
                    .map(|&b| {
                        if b.is_ascii_graphic() || b == b' ' {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                println!(
                    "  [{}..{}]: {:16} | {}",
                    i,
                    end,
                    format!("{:?}", bytes),
                    ascii
                );
            }
        }
        println!();
    }

    // Scan for pages with corrupted headers
    println!("\n=== SCANNING FOR CORRUPTION ===\n");
    let mut corrupted = Vec::new();

    // Estimate total pages
    let total_pages = (contents.len() - V3_HEADER_SIZE as usize) / PAGE_SIZE as usize;

    for page_id in 1..=total_pages as u64 {
        let offset = page_offset(page_id) as usize;
        if offset + 32 <= contents.len() {
            let page_bytes = &contents[offset..];
            let used_bytes = u16::from_be_bytes([page_bytes[18], page_bytes[19]]);

            if used_bytes > 4064 {
                corrupted.push((page_id, offset, used_bytes));
            }
        }
    }

    println!("Found {} corrupted pages:", corrupted.len());
    for (page_id, offset, used_bytes) in corrupted.iter().take(10) {
        println!(
            "  Page {} at offset {}: used_bytes = {} (0x{:04x})",
            page_id, offset, used_bytes, used_bytes
        );

        // Show what's at the used_bytes offset
        if *offset + 32 <= contents.len() {
            let page_bytes = &contents[*offset..];
            let ascii: String = page_bytes[0..32]
                .iter()
                .map(|&b| {
                    if b.is_ascii_graphic() || b == b' ' {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            println!("    First 32 bytes as ASCII: {}", ascii);
        }
    }

    Ok(())
}
