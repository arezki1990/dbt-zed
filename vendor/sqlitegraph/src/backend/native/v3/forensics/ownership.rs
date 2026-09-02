use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Page type enumeration for ownership tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageType {
    Node = 1,
    BTree = 2,
    Edge = 3,
    Header = 4,
    Unknown = 5,
}

impl PageType {
    pub fn name(&self) -> &'static str {
        match self {
            PageType::Node => "NODE",
            PageType::BTree => "BTREE",
            PageType::Edge => "EDGE",
            PageType::Header => "HEADER",
            PageType::Unknown => "UNKNOWN",
        }
    }

    pub fn detect_from_bytes(bytes: &[u8]) -> PageType {
        if bytes.len() < 32 {
            return PageType::Unknown;
        }

        let page_id = u64::from_be_bytes(bytes[0..8].try_into().unwrap_or([0u8; 8]));
        if page_id == 0 {
            return PageType::Header;
        }

        let is_leaf_or_reserved = bytes[8];
        let is_root_flag = bytes[9];
        if is_leaf_or_reserved <= 1 && is_root_flag <= 1 {
            return PageType::BTree;
        }

        let used_bytes = u16::from_be_bytes([bytes[18], bytes[19]]);
        if used_bytes < 4000 {
            return PageType::Node;
        }

        PageType::Unknown
    }
}

/// Subsystem enumeration for ownership tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subsystem {
    NodeStore = 1,
    BTreeManager = 2,
    EdgeStore = 3,
    Allocator = 4,
    Unknown = 5,
}

impl Subsystem {
    pub fn name(&self) -> &'static str {
        match self {
            Subsystem::NodeStore => "NodeStore",
            Subsystem::BTreeManager => "BTreeManager",
            Subsystem::EdgeStore => "EdgeStore",
            Subsystem::Allocator => "Allocator",
            Subsystem::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PageOwnershipRecord {
    pub page_id: u64,
    pub first_owner: Subsystem,
    pub first_page_type: PageType,
    pub write_sequence: u64,
    pub writes: Vec<PageWriteRecord>,
    pub has_conflict: bool,
    pub first_conflict: Option<PageConflict>,
}

#[derive(Debug, Clone)]
pub struct PageWriteRecord {
    pub sequence: u64,
    pub subsystem: Subsystem,
    pub page_type: PageType,
    pub file_offset: u64,
    pub function: String,
    pub is_conflict: bool,
}

#[derive(Debug, Clone)]
pub struct PageConflict {
    pub first_write: PageWriteRecord,
    pub conflicting_write: PageWriteRecord,
    pub conflict_type: ConflictType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictType {
    SubsystemMismatch,
    PageTypeMismatch,
    BothMismatch,
}

impl PageOwnershipRecord {
    pub fn new(page_id: u64, subsystem: Subsystem, page_type: PageType) -> Self {
        Self {
            page_id,
            first_owner: subsystem,
            first_page_type: page_type,
            write_sequence: 0,
            writes: Vec::new(),
            has_conflict: false,
            first_conflict: None,
        }
    }

    pub fn record_write(
        &mut self,
        subsystem: Subsystem,
        page_type: PageType,
        file_offset: u64,
        function: String,
        sequence: u64,
    ) -> bool {
        let is_conflict = subsystem != self.first_owner || page_type != self.first_page_type;

        let write_record = PageWriteRecord {
            sequence,
            subsystem,
            page_type,
            file_offset,
            function,
            is_conflict,
        };

        if is_conflict && !self.has_conflict {
            self.has_conflict = true;
            self.first_conflict = Some(PageConflict {
                first_write: self.writes.first().cloned().unwrap_or(write_record.clone()),
                conflicting_write: write_record.clone(),
                conflict_type: if subsystem != self.first_owner && page_type != self.first_page_type
                {
                    ConflictType::BothMismatch
                } else if subsystem != self.first_owner {
                    ConflictType::SubsystemMismatch
                } else {
                    ConflictType::PageTypeMismatch
                },
            });
        }

        self.writes.push(write_record);
        self.write_sequence = sequence;
        is_conflict
    }
}

pub static PAGE_OWNERSHIP: OnceLock<Mutex<PageOwnershipRegistry>> = OnceLock::new();

pub fn get_page_registry() -> &'static Mutex<PageOwnershipRegistry> {
    PAGE_OWNERSHIP.get_or_init(|| Mutex::new(PageOwnershipRegistry::new()))
}

#[derive(Debug)]
pub struct PageOwnershipRegistry {
    pages: HashMap<u64, PageOwnershipRecord>,
    global_sequence: u64,
    total_conflicts: u64,
    first_conflict_page: Option<u64>,
}

impl PageOwnershipRegistry {
    pub fn new() -> Self {
        Self {
            pages: HashMap::new(),
            global_sequence: 0,
            total_conflicts: 0,
            first_conflict_page: None,
        }
    }

    pub fn register_allocation(&mut self, page_id: u64, subsystem: Subsystem, page_type: PageType) {
        self.pages
            .entry(page_id)
            .or_insert_with(|| PageOwnershipRecord::new(page_id, subsystem, page_type));
    }

    pub fn register_write(
        &mut self,
        page_id: u64,
        subsystem: Subsystem,
        page_type: PageType,
        file_offset: u64,
        function: String,
    ) -> bool {
        self.global_sequence += 1;

        if !self.pages.contains_key(&page_id) {
            self.register_allocation(page_id, subsystem, page_type);
        }

        let record = self
            .pages
            .get_mut(&page_id)
            .expect("invariant: page just registered");
        let is_conflict = record.record_write(
            subsystem,
            page_type,
            file_offset,
            function,
            self.global_sequence,
        );

        if is_conflict && self.first_conflict_page.is_none() {
            self.first_conflict_page = Some(page_id);
        }

        if is_conflict {
            self.total_conflicts += 1;
        }

        is_conflict
    }

    pub fn get(&self, page_id: u64) -> Option<&PageOwnershipRecord> {
        self.pages.get(&page_id)
    }

    pub fn get_conflicts(&self) -> Vec<&PageOwnershipRecord> {
        self.pages.values().filter(|r| r.has_conflict).collect()
    }

    pub fn total_conflicts(&self) -> u64 {
        self.total_conflicts
    }

    pub fn first_conflict_page(&self) -> Option<u64> {
        self.first_conflict_page
    }

    pub fn print_conflict_report(&self) {
        println!("\n═══════════════════════════════════════════════════════════");
        println!("              PAGE OWNERSHIP CONFLICT REPORT                 ");
        println!("═══════════════════════════════════════════════════════════\n");

        println!("Total conflicts detected: {}", self.total_conflicts);
        println!("First conflicting page: {:?}\n", self.first_conflict_page);

        let conflicts = self.get_conflicts();
        if conflicts.is_empty() {
            println!("No conflicts detected - all pages have consistent ownership.\n");
        } else {
            println!("Conflicting pages (showing first 10):\n");
            for (idx, record) in conflicts.iter().take(10).enumerate() {
                println!("{}. Page {}:", idx + 1, record.page_id);
                println!(
                    "   First owner: {} ({})",
                    record.first_owner.name(),
                    record.first_page_type.name()
                );

                if let Some(conflict) = &record.first_conflict {
                    println!("   FIRST CONFLICT:");
                    println!(
                        "     Original: {} wrote {} at offset {}",
                        conflict.first_write.subsystem.name(),
                        conflict.first_write.page_type.name(),
                        conflict.first_write.file_offset
                    );
                    println!(
                        "     Conflict: {} wrote {} at offset {}",
                        conflict.conflicting_write.subsystem.name(),
                        conflict.conflicting_write.page_type.name(),
                        conflict.conflicting_write.file_offset
                    );
                    println!("     Type: {:?}\n", conflict.conflict_type);
                }

                println!("   Total writes to this page: {}", record.writes.len());
                println!("   Write history:");
                for write in record.writes.iter().take(5) {
                    println!(
                        "     [seq={}] {} wrote {} at {} ({})",
                        write.sequence,
                        write.subsystem.name(),
                        write.page_type.name(),
                        write.file_offset,
                        write.function
                    );
                }
                if record.writes.len() > 5 {
                    println!("     ... and {} more", record.writes.len() - 5);
                }
                println!();
            }

            if conflicts.len() > 10 {
                println!("... and {} more conflicting pages", conflicts.len() - 10);
            }
        }

        println!("═══════════════════════════════════════════════════════════\n");
    }

    pub fn print_ownership_map(&self) {
        println!("\n═══════════════════════════════════════════════════════════");
        println!("                 PAGE OWNERSHIP MAP                         ");
        println!("═══════════════════════════════════════════════════════════\n");

        println!("Total pages tracked: {}\n", self.pages.len());

        let mut by_owner: HashMap<(&str, &str), Vec<u64>> = HashMap::new();
        for (page_id, record) in &self.pages {
            let key = (record.first_owner.name(), record.first_page_type.name());
            by_owner.entry(key).or_default().push(*page_id);
        }

        for ((owner, page_type), page_ids) in &by_owner {
            println!("{} ({}): {} pages", owner, page_type, page_ids.len());
            if page_ids.len() <= 10 {
                println!("  Page IDs: {:?}", page_ids);
            } else {
                println!(
                    "  Page IDs: {:?} ... and {} more",
                    &page_ids[..10],
                    page_ids.len() - 10
                );
            }
        }

        println!("\n═══════════════════════════════════════════════════════════\n");
    }

    pub fn reset(&mut self) {
        self.pages.clear();
        self.global_sequence = 0;
        self.total_conflicts = 0;
        self.first_conflict_page = None;
    }
}

impl Default for PageOwnershipRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[macro_export]
macro_rules! track_page_alloc {
    ($page_id:expr, $subsystem:expr, $page_type:expr) => {
        #[cfg(feature = "v3-forensics")]
        {
            let mut registry = $crate::backend::native::v3::forensics::get_page_registry().lock();
            registry.register_allocation($page_id, $subsystem, $page_type);
        }
    };
}

#[macro_export]
macro_rules! track_page_write {
    ($page_id:expr, $subsystem:expr, $page_type:expr, $offset:expr, $function:expr) => {
        #[cfg(feature = "v3-forensics")]
        {
            let mut registry = $crate::backend::native::v3::forensics::get_page_registry().lock();
            let has_conflict = registry.register_write(
                $page_id,
                $subsystem,
                $page_type,
                $offset,
                $function.to_string(),
            );
            if has_conflict {
                eprintln!(
                    "⚠️  PAGE OWNERSHIP CONFLICT: page_id={}, subsystem={:?}, page_type={:?}, offset={}",
                    $page_id, $subsystem, $page_type, $offset
                );
            }
        }
    };
}

#[macro_export]
macro_rules! track_page_write_auto {
    ($page_id:expr, $subsystem:expr, $bytes:expr, $offset:expr, $function:expr) => {
        #[cfg(feature = "v3-forensics")]
        {
            let page_type =
                $crate::backend::native::v3::forensics::PageType::detect_from_bytes($bytes);
            $crate::track_page_write!($page_id, $subsystem, page_type, $offset, $function);
        }

        #[cfg(not(feature = "v3-forensics"))]
        {};
    };
}

pub fn print_page_ownership_report() {
    let registry = get_page_registry().lock();
    registry.print_conflict_report();
}

pub fn print_page_ownership_map() {
    let registry = get_page_registry().lock();
    registry.print_ownership_map();
}

pub fn has_page_conflicts() -> bool {
    let registry = get_page_registry().lock();
    registry.total_conflicts() > 0
}

pub fn get_first_conflict_page() -> Option<u64> {
    let registry = get_page_registry().lock();
    registry.first_conflict_page()
}

pub fn reset_page_ownership() {
    let mut registry = get_page_registry().lock();
    registry.reset();
}

pub fn scan_database_pages(db_path: &std::path::Path) -> Result<PageScanReport, std::io::Error> {
    use std::fs::File;
    use std::io::{Read, Seek};

    let mut file = File::open(db_path)?;
    let metadata = file.metadata()?;
    let file_size = metadata.len();

    const PAGE_SIZE: u64 = 4096;
    const V3_HEADER_SIZE: u64 = 112;

    let mut report = PageScanReport {
        total_pages: 0,
        node_pages: 0,
        btree_pages: 0,
        edge_pages: 0,
        unknown_pages: 0,
        corrupted_pages: Vec::new(),
    };

    let mut page_buffer = vec![0u8; PAGE_SIZE as usize];
    let mut offset = V3_HEADER_SIZE;
    let mut page_id = 1u64;

    while offset + PAGE_SIZE <= file_size {
        file.seek(std::io::SeekFrom::Start(offset))?;
        file.read_exact(&mut page_buffer)?;

        let page_type = PageType::detect_from_bytes(&page_buffer);
        match page_type {
            PageType::Node => report.node_pages += 1,
            PageType::BTree => report.btree_pages += 1,
            PageType::Edge => report.edge_pages += 1,
            PageType::Header => {}
            PageType::Unknown => {
                report.unknown_pages += 1;
                if page_buffer.len() >= 32 {
                    let used_bytes = u16::from_be_bytes([page_buffer[18], page_buffer[19]]);
                    if used_bytes > 4000 {
                        report.corrupted_pages.push(PageCorruption {
                            page_id,
                            offset,
                            detected_type: page_type,
                            used_bytes,
                            first_bytes: page_buffer[..32].to_vec(),
                        });
                    }
                }
            }
        }

        report.total_pages += 1;
        offset += PAGE_SIZE;
        page_id += 1;
    }

    Ok(report)
}

#[derive(Debug)]
pub struct PageScanReport {
    pub total_pages: usize,
    pub node_pages: usize,
    pub btree_pages: usize,
    pub edge_pages: usize,
    pub unknown_pages: usize,
    pub corrupted_pages: Vec<PageCorruption>,
}

impl PageScanReport {
    pub fn print(&self) {
        println!("\n═══════════════════════════════════════════════════════════");
        println!("                 DATABASE PAGE SCAN REPORT                 ");
        println!("═══════════════════════════════════════════════════════════\n");

        println!("Total pages scanned: {}", self.total_pages);
        println!("  Node pages: {}", self.node_pages);
        println!("  B+Tree pages: {}", self.btree_pages);
        println!("  Edge pages: {}", self.edge_pages);
        println!("  Unknown pages: {}", self.unknown_pages);

        if !self.corrupted_pages.is_empty() {
            println!(
                "\n⚠️  CORRUPTED PAGES DETECTED: {}",
                self.corrupted_pages.len()
            );
            for corruption in self.corrupted_pages.iter().take(10) {
                println!(
                    "  Page {} at offset {}:",
                    corruption.page_id, corruption.offset
                );
                println!("    Detected type: {:?}", corruption.detected_type);
                println!(
                    "    used_bytes: {} (0x{:04x})",
                    corruption.used_bytes, corruption.used_bytes
                );
                println!("    First 32 bytes: {:?}", &corruption.first_bytes[..]);
            }
            if self.corrupted_pages.len() > 10 {
                println!("  ... and {} more", self.corrupted_pages.len() - 10);
            }
        } else {
            println!("\n✓ No corruption detected in page headers");
        }

        println!("\n═══════════════════════════════════════════════════════════\n");
    }
}

#[derive(Debug)]
pub struct PageCorruption {
    pub page_id: u64,
    pub offset: u64,
    pub detected_type: PageType,
    pub used_bytes: u16,
    pub first_bytes: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_type_detection_node_page() {
        let mut node_page_bytes = vec![0u8; 4096];
        node_page_bytes[0..8].copy_from_slice(&1u64.to_be_bytes());
        node_page_bytes[8..16].copy_from_slice(&0u64.to_be_bytes());
        node_page_bytes[16..18].copy_from_slice(&10u16.to_be_bytes());
        node_page_bytes[18..20].copy_from_slice(&512u16.to_be_bytes());

        let detected = PageType::detect_from_bytes(&node_page_bytes);
        assert_eq!(detected, PageType::Node);
    }

    #[test]
    fn test_page_type_detection_btree_leaf() {
        let mut btree_bytes = vec![0u8; 4096];
        btree_bytes[0..8].copy_from_slice(&2u64.to_be_bytes());
        btree_bytes[8] = 1;
        btree_bytes[9] = 0;

        let detected = PageType::detect_from_bytes(&btree_bytes);
        assert_eq!(detected, PageType::BTree);
    }

    #[test]
    fn test_page_ownership_registry() {
        let mut registry = PageOwnershipRegistry::new();

        registry.register_allocation(1, Subsystem::NodeStore, PageType::Node);

        let conflict1 = registry.register_write(
            1,
            Subsystem::NodeStore,
            PageType::Node,
            4208,
            "write_node_page".to_string(),
        );
        assert!(!conflict1);

        let conflict2 = registry.register_write(
            1,
            Subsystem::BTreeManager,
            PageType::BTree,
            4208,
            "write_page".to_string(),
        );
        assert!(conflict2);

        assert_eq!(registry.total_conflicts(), 1);
        assert_eq!(registry.first_conflict_page(), Some(1));

        let record = registry.get(1).expect("page ownership record exists");
        assert!(record.has_conflict);
        assert_eq!(record.writes.len(), 2);
    }
}
