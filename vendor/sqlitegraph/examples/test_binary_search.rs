//! Quick sanity check for binary search in NodePage::find_node

use sqlitegraph::backend::native::types::NodeFlags;
use sqlitegraph::backend::native::v3::node::page::NodePage;

fn main() {
    println!("Testing NodePage::find_node binary search...\n");

    // Create a test page with nodes 1-100
    let mut page = NodePage::new(1);
    for i in 1..=100 {
        let node = sqlitegraph::backend::native::v3::node::record::NodeRecordV3::new_inline(
            i,
            NodeFlags::empty(),
            0,
            0,
            vec![],
            0,
            0,
            0,
            0,
        );
        page.add_node(node).expect("add_node");
    }

    // Test binary search finds all nodes
    let mut found = 0;
    let mut missing = 0;
    for i in 1..=100 {
        match page.find_node(i) {
            Some(node) => {
                if node.id() == i {
                    found += 1;
                } else {
                    eprintln!(
                        "ERROR: find_node({}) returned wrong node with id={}",
                        i,
                        node.id()
                    );
                }
            }
            None => {
                missing += 1;
                eprintln!("ERROR: find_node({}) returned None", i);
            }
        }
    }

    println!("Found: {}/100 nodes", found);
    println!("Missing: {}/100 nodes", missing);

    // Test that non-existent nodes return None
    let mut false_positives = 0;
    for i in 101..=200 {
        if page.find_node(i).is_some() {
            false_positives += 1;
            eprintln!("ERROR: find_node({}) found non-existent node", i);
        }
    }
    println!("False positives: {}/100", false_positives);

    // Test edge cases
    println!("\nEdge cases:");
    println!("  find_node(1): {:?}", page.find_node(1).map(|n| n.id()));
    println!("  find_node(50): {:?}", page.find_node(50).map(|n| n.id()));
    println!(
        "  find_node(100): {:?}",
        page.find_node(100).map(|n| n.id())
    );
    println!("  find_node(0): {:?}", page.find_node(0).map(|n| n.id()));
    println!(
        "  find_node(101): {:?}",
        page.find_node(101).map(|n| n.id())
    );

    if found == 100 && missing == 0 && false_positives == 0 {
        println!("\n✓ All tests passed!");
    } else {
        eprintln!("\n✗ Tests failed!");
        std::process::exit(1);
    }
}
