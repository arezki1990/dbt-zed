use super::*;

impl ForensicCounters {
    /// Print a formatted report of all counters
    pub fn print_report(&self) {
        println!("\n═══════════════════════════════════════════════════════════");
        println!("                    V3 FORENSIC COUNTER REPORT                 ");
        println!("═══════════════════════════════════════════════════════════\n");

        println!("WRITE PATH:");
        println!(
            "  Logical insert_node calls:     {}",
            self.logical_insert_node_calls.load(Ordering::Relaxed)
        );
        println!(
            "  Logical insert_edge calls:     {}",
            self.logical_insert_edge_calls.load(Ordering::Relaxed)
        );
        println!(
            "  B+Tree insert calls:           {}",
            self.btree_insert_calls.load(Ordering::Relaxed)
        );
        println!(
            "  B+Tree split count:            {}",
            self.btree_split_count.load(Ordering::Relaxed)
        );
        println!(
            "  Page allocations:              {}",
            self.page_allocate_count.load(Ordering::Relaxed)
        );
        println!(
            "  Page writes (total):           {}",
            self.page_write_count.load(Ordering::Relaxed)
        );
        println!(
            "  WAL appends:                   {}",
            self.wal_append_count.load(Ordering::Relaxed)
        );
        println!(
            "  WAL flushes:                   {}",
            self.wal_flush_count.load(Ordering::Relaxed)
        );
        println!(
            "  Checkpoint count:              {}",
            self.checkpoint_count.load(Ordering::Relaxed)
        );
        println!(
            "  Node encodes:                  {}",
            self.node_encode_count.load(Ordering::Relaxed)
        );
        println!(
            "  Edge encodes:                  {}",
            self.edge_encode_count.load(Ordering::Relaxed)
        );
        println!(
            "  sync_data() calls:             {}",
            self.sync_data_count.load(Ordering::Relaxed)
        );
        println!(
            "  sync_all() calls:              {}",
            self.sync_all_count.load(Ordering::Relaxed)
        );

        println!("\nREAD PATH:");
        println!(
            "  Logical get_node calls:        {}",
            self.logical_get_node_calls.load(Ordering::Relaxed)
        );
        println!(
            "  Logical neighbors calls:       {}",
            self.logical_neighbors_calls.load(Ordering::Relaxed)
        );
        println!(
            "  B+Tree lookup calls:           {}",
            self.btree_lookup_calls.load(Ordering::Relaxed)
        );
        println!(
            "  Page reads (total):            {}",
            self.page_read_count.load(Ordering::Relaxed)
        );
        println!(
            "  Node decodes:                  {}",
            self.node_decode_count.load(Ordering::Relaxed)
        );
        println!(
            "  Edge decodes:                  {}",
            self.edge_decode_count.load(Ordering::Relaxed)
        );

        println!("\nCACHE PERFORMANCE:");
        let btree_hits = self.btree_cache_hit_count.load(Ordering::Relaxed);
        let btree_misses = self.btree_cache_miss_count.load(Ordering::Relaxed);
        let btree_total = btree_hits + btree_misses;
        let btree_hit_rate = if btree_total > 0 {
            (btree_hits as f64 / btree_total as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "  B+Tree cache hits:             {} (hit rate: {:.1}%)",
            btree_hits, btree_hit_rate
        );
        println!("  B+Tree cache misses:           {}", btree_misses);

        let node_hits = self.node_cache_hit_count.load(Ordering::Relaxed);
        let node_misses = self.node_cache_miss_count.load(Ordering::Relaxed);
        let node_total = node_hits + node_misses;
        let node_hit_rate = if node_total > 0 {
            (node_hits as f64 / node_total as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "  Node cache hits:               {} (hit rate: {:.1}%)",
            node_hits, node_hit_rate
        );
        println!("  Node cache misses:             {}", node_misses);

        println!("\nLOCK USAGE:");
        println!(
            "  B+Tree read lock count:        {}",
            self.btree_read_lock_count.load(Ordering::Relaxed)
        );
        println!(
            "  B+Tree write lock count:       {}",
            self.btree_write_lock_count.load(Ordering::Relaxed)
        );

        println!("\nPHASE 2: ENHANCED CACHE RESIDENCY:");
        println!(
            "  Dirty page hits:               {}",
            self.dirty_page_hit_count.load(Ordering::Relaxed)
        );
        println!(
            "  Node page cache hits:          {}",
            self.node_page_cache_hit_count.load(Ordering::Relaxed)
        );
        println!(
            "  Node page cache misses:        {}",
            self.node_page_cache_miss_count.load(Ordering::Relaxed)
        );
        println!(
            "  Redundant page reloads:        {}",
            self.redundant_page_reload_count.load(Ordering::Relaxed)
        );

        println!("\nPHASE 2: EDGE PATH VISIBILITY:");
        println!(
            "  Edge cache hits:               {}",
            self.edge_cache_hit_count.load(Ordering::Relaxed)
        );
        println!(
            "  Edge cache misses:             {}",
            self.edge_cache_miss_count.load(Ordering::Relaxed)
        );
        println!(
            "  Edge page reads:               {}",
            self.edge_page_read_count.load(Ordering::Relaxed)
        );

        println!("\nPHASE 3: NODE UNPACK COST BREAKDOWN:");
        println!(
            "  NodePage::unpack() calls:      {}",
            self.node_page_unpack_count.load(Ordering::Relaxed)
        );
        println!(
            "  Linear scan steps (total):     {}",
            self.node_linear_scan_steps.load(Ordering::Relaxed)
        );
        let avg_scan = if self.node_page_unpack_count.load(Ordering::Relaxed) > 0 {
            self.node_linear_scan_steps.load(Ordering::Relaxed) as f64
                / self.node_page_unpack_count.load(Ordering::Relaxed) as f64
        } else {
            0.0
        };
        println!("  Avg nodes scanned per unpack:  {:.1}", avg_scan);
        println!(
            "  B+Tree traversal depth total:  {}",
            self.btree_traversal_depth_total.load(Ordering::Relaxed)
        );
        let avg_depth = if self.btree_lookup_calls.load(Ordering::Relaxed) > 0 {
            self.btree_traversal_depth_total.load(Ordering::Relaxed) as f64
                / self.btree_lookup_calls.load(Ordering::Relaxed) as f64
        } else {
            0.0
        };
        println!("  Avg B+Tree depth per lookup:   {:.1}", avg_depth);

        println!("\n═══════════════════════════════════════════════════════════\n");
    }

    /// Get a snapshot of all counter values as a struct
    pub fn snapshot(&self) -> ForensicSnapshot {
        ForensicSnapshot {
            logical_insert_node_calls: self.logical_insert_node_calls.load(Ordering::Relaxed),
            logical_insert_edge_calls: self.logical_insert_edge_calls.load(Ordering::Relaxed),
            btree_insert_calls: self.btree_insert_calls.load(Ordering::Relaxed),
            btree_split_count: self.btree_split_count.load(Ordering::Relaxed),
            page_allocate_count: self.page_allocate_count.load(Ordering::Relaxed),
            page_write_count: self.page_write_count.load(Ordering::Relaxed),
            wal_append_count: self.wal_append_count.load(Ordering::Relaxed),
            wal_flush_count: self.wal_flush_count.load(Ordering::Relaxed),
            checkpoint_count: self.checkpoint_count.load(Ordering::Relaxed),
            node_encode_count: self.node_encode_count.load(Ordering::Relaxed),
            edge_encode_count: self.edge_encode_count.load(Ordering::Relaxed),
            sync_data_count: self.sync_data_count.load(Ordering::Relaxed),
            sync_all_count: self.sync_all_count.load(Ordering::Relaxed),
            logical_get_node_calls: self.logical_get_node_calls.load(Ordering::Relaxed),
            logical_neighbors_calls: self.logical_neighbors_calls.load(Ordering::Relaxed),
            btree_lookup_calls: self.btree_lookup_calls.load(Ordering::Relaxed),
            page_read_count: self.page_read_count.load(Ordering::Relaxed),
            node_decode_count: self.node_decode_count.load(Ordering::Relaxed),
            edge_decode_count: self.edge_decode_count.load(Ordering::Relaxed),
            btree_cache_hit_count: self.btree_cache_hit_count.load(Ordering::Relaxed),
            btree_cache_miss_count: self.btree_cache_miss_count.load(Ordering::Relaxed),
            node_cache_hit_count: self.node_cache_hit_count.load(Ordering::Relaxed),
            node_cache_miss_count: self.node_cache_miss_count.load(Ordering::Relaxed),
            btree_read_lock_count: self.btree_read_lock_count.load(Ordering::Relaxed),
            btree_write_lock_count: self.btree_write_lock_count.load(Ordering::Relaxed),
            dirty_page_hit_count: self.dirty_page_hit_count.load(Ordering::Relaxed),
            node_page_cache_hit_count: self.node_page_cache_hit_count.load(Ordering::Relaxed),
            node_page_cache_miss_count: self.node_page_cache_miss_count.load(Ordering::Relaxed),
            redundant_page_reload_count: self.redundant_page_reload_count.load(Ordering::Relaxed),
            edge_cache_hit_count: self.edge_cache_hit_count.load(Ordering::Relaxed),
            edge_cache_miss_count: self.edge_cache_miss_count.load(Ordering::Relaxed),
            edge_page_read_count: self.edge_page_read_count.load(Ordering::Relaxed),
            last_btree_pages_read: self.last_btree_pages_read.load(Ordering::Relaxed),
            last_node_pages_read: self.last_node_pages_read.load(Ordering::Relaxed),
            node_page_unpack_count: self.node_page_unpack_count.load(Ordering::Relaxed),
            node_linear_scan_steps: self.node_linear_scan_steps.load(Ordering::Relaxed),
            btree_traversal_depth_total: self.btree_traversal_depth_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ForensicSnapshot {
    pub logical_insert_node_calls: u64,
    pub logical_insert_edge_calls: u64,
    pub btree_insert_calls: u64,
    pub btree_split_count: u64,
    pub page_allocate_count: u64,
    pub page_write_count: u64,
    pub wal_append_count: u64,
    pub wal_flush_count: u64,
    pub checkpoint_count: u64,
    pub node_encode_count: u64,
    pub edge_encode_count: u64,
    pub sync_data_count: u64,
    pub sync_all_count: u64,
    pub logical_get_node_calls: u64,
    pub logical_neighbors_calls: u64,
    pub btree_lookup_calls: u64,
    pub page_read_count: u64,
    pub node_decode_count: u64,
    pub edge_decode_count: u64,
    pub btree_cache_hit_count: u64,
    pub btree_cache_miss_count: u64,
    pub node_cache_hit_count: u64,
    pub node_cache_miss_count: u64,
    pub btree_read_lock_count: u64,
    pub btree_write_lock_count: u64,
    pub dirty_page_hit_count: u64,
    pub node_page_cache_hit_count: u64,
    pub node_page_cache_miss_count: u64,
    pub redundant_page_reload_count: u64,
    pub edge_cache_hit_count: u64,
    pub edge_cache_miss_count: u64,
    pub edge_page_read_count: u64,
    pub last_btree_pages_read: u64,
    pub last_node_pages_read: u64,
    pub node_page_unpack_count: u64,
    pub node_linear_scan_steps: u64,
    pub btree_traversal_depth_total: u64,
}

impl ForensicSnapshot {
    pub fn diff(&self, after: &ForensicSnapshot) -> ForensicDelta {
        ForensicDelta {
            logical_insert_node_calls: after
                .logical_insert_node_calls
                .wrapping_sub(self.logical_insert_node_calls),
            logical_insert_edge_calls: after
                .logical_insert_edge_calls
                .wrapping_sub(self.logical_insert_edge_calls),
            btree_insert_calls: after
                .btree_insert_calls
                .wrapping_sub(self.btree_insert_calls),
            btree_split_count: after.btree_split_count.wrapping_sub(self.btree_split_count),
            page_allocate_count: after
                .page_allocate_count
                .wrapping_sub(self.page_allocate_count),
            page_write_count: after.page_write_count.wrapping_sub(self.page_write_count),
            wal_append_count: after.wal_append_count.wrapping_sub(self.wal_append_count),
            wal_flush_count: after.wal_flush_count.wrapping_sub(self.wal_flush_count),
            checkpoint_count: after.checkpoint_count.wrapping_sub(self.checkpoint_count),
            node_encode_count: after.node_encode_count.wrapping_sub(self.node_encode_count),
            edge_encode_count: after.edge_encode_count.wrapping_sub(self.edge_encode_count),
            sync_data_count: after.sync_data_count.wrapping_sub(self.sync_data_count),
            sync_all_count: after.sync_all_count.wrapping_sub(self.sync_all_count),
            logical_get_node_calls: after
                .logical_get_node_calls
                .wrapping_sub(self.logical_get_node_calls),
            logical_neighbors_calls: after
                .logical_neighbors_calls
                .wrapping_sub(self.logical_neighbors_calls),
            btree_lookup_calls: after
                .btree_lookup_calls
                .wrapping_sub(self.btree_lookup_calls),
            page_read_count: after.page_read_count.wrapping_sub(self.page_read_count),
            node_decode_count: after.node_decode_count.wrapping_sub(self.node_decode_count),
            edge_decode_count: after.edge_decode_count.wrapping_sub(self.edge_decode_count),
            btree_cache_hit_count: after
                .btree_cache_hit_count
                .wrapping_sub(self.btree_cache_hit_count),
            btree_cache_miss_count: after
                .btree_cache_miss_count
                .wrapping_sub(self.btree_cache_miss_count),
            node_cache_hit_count: after
                .node_cache_hit_count
                .wrapping_sub(self.node_cache_hit_count),
            node_cache_miss_count: after
                .node_cache_miss_count
                .wrapping_sub(self.node_cache_miss_count),
            btree_read_lock_count: after
                .btree_read_lock_count
                .wrapping_sub(self.btree_read_lock_count),
            btree_write_lock_count: after
                .btree_write_lock_count
                .wrapping_sub(self.btree_write_lock_count),
            dirty_page_hit_count: after
                .dirty_page_hit_count
                .wrapping_sub(self.dirty_page_hit_count),
            node_page_cache_hit_count: after
                .node_page_cache_hit_count
                .wrapping_sub(self.node_page_cache_hit_count),
            node_page_cache_miss_count: after
                .node_page_cache_miss_count
                .wrapping_sub(self.node_page_cache_miss_count),
            redundant_page_reload_count: after
                .redundant_page_reload_count
                .wrapping_sub(self.redundant_page_reload_count),
            edge_cache_hit_count: after
                .edge_cache_hit_count
                .wrapping_sub(self.edge_cache_hit_count),
            edge_cache_miss_count: after
                .edge_cache_miss_count
                .wrapping_sub(self.edge_cache_miss_count),
            edge_page_read_count: after
                .edge_page_read_count
                .wrapping_sub(self.edge_page_read_count),
            last_btree_pages_read: after
                .last_btree_pages_read
                .wrapping_sub(self.last_btree_pages_read),
            last_node_pages_read: after
                .last_node_pages_read
                .wrapping_sub(self.last_node_pages_read),
            node_page_unpack_count: after
                .node_page_unpack_count
                .wrapping_sub(self.node_page_unpack_count),
            node_linear_scan_steps: after
                .node_linear_scan_steps
                .wrapping_sub(self.node_linear_scan_steps),
            btree_traversal_depth_total: after
                .btree_traversal_depth_total
                .wrapping_sub(self.btree_traversal_depth_total),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ForensicDelta {
    pub logical_insert_node_calls: u64,
    pub logical_insert_edge_calls: u64,
    pub btree_insert_calls: u64,
    pub btree_split_count: u64,
    pub page_allocate_count: u64,
    pub page_write_count: u64,
    pub wal_append_count: u64,
    pub wal_flush_count: u64,
    pub checkpoint_count: u64,
    pub node_encode_count: u64,
    pub edge_encode_count: u64,
    pub sync_data_count: u64,
    pub sync_all_count: u64,
    pub logical_get_node_calls: u64,
    pub logical_neighbors_calls: u64,
    pub btree_lookup_calls: u64,
    pub page_read_count: u64,
    pub node_decode_count: u64,
    pub edge_decode_count: u64,
    pub btree_cache_hit_count: u64,
    pub btree_cache_miss_count: u64,
    pub node_cache_hit_count: u64,
    pub node_cache_miss_count: u64,
    pub btree_read_lock_count: u64,
    pub btree_write_lock_count: u64,
    pub dirty_page_hit_count: u64,
    pub node_page_cache_hit_count: u64,
    pub node_page_cache_miss_count: u64,
    pub redundant_page_reload_count: u64,
    pub edge_cache_hit_count: u64,
    pub edge_cache_miss_count: u64,
    pub edge_page_read_count: u64,
    pub last_btree_pages_read: u64,
    pub last_node_pages_read: u64,
    pub node_page_unpack_count: u64,
    pub node_linear_scan_steps: u64,
    pub btree_traversal_depth_total: u64,
}

impl ForensicDelta {
    pub fn print_report(&self) {
        println!("\n───────────────────────────────────────────────────────────────");
        println!("                    FORENSIC DELTA REPORT                       ");
        println!("───────────────────────────────────────────────────────────────\n");

        println!("WRITE PATH (per operation):");
        println!(
            "  B+Tree insert calls:           {}",
            self.btree_insert_calls
        );
        println!(
            "  B+Tree split count:            {}",
            self.btree_split_count
        );
        println!(
            "  Page allocations:              {}",
            self.page_allocate_count
        );
        println!("  Page writes (total):           {}", self.page_write_count);
        println!("  WAL appends:                   {}", self.wal_append_count);
        println!("  WAL flushes:                   {}", self.wal_flush_count);
        println!("  Checkpoint count:              {}", self.checkpoint_count);
        println!(
            "  Node encodes:                  {}",
            self.node_encode_count
        );
        println!(
            "  Edge encodes:                  {}",
            self.edge_encode_count
        );
        println!("  sync_data() calls:             {}", self.sync_data_count);
        println!("  sync_all() calls:              {}", self.sync_all_count);

        println!("\nREAD PATH (per operation):");
        println!(
            "  B+Tree lookup calls:           {}",
            self.btree_lookup_calls
        );
        println!("  Page reads (total):            {}", self.page_read_count);
        println!(
            "  Node decodes:                  {}",
            self.node_decode_count
        );
        println!(
            "  Edge decodes:                  {}",
            self.edge_decode_count
        );

        println!("\nCACHE PERFORMANCE (per operation):");
        let btree_total = self.btree_cache_hit_count + self.btree_cache_miss_count;
        let btree_hit_rate = if btree_total > 0 {
            (self.btree_cache_hit_count as f64 / btree_total as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "  B+Tree cache hits/misses:       {}/{} ({:.1}%)",
            self.btree_cache_hit_count, btree_total, btree_hit_rate
        );

        let node_total = self.node_cache_hit_count + self.node_cache_miss_count;
        let node_hit_rate = if node_total > 0 {
            (self.node_cache_hit_count as f64 / node_total as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "  Node cache hits/misses:        {}/{} ({:.1}%)",
            self.node_cache_hit_count, node_total, node_hit_rate
        );

        println!("\nPHASE 2: CACHE RESIDENCY (per operation):");
        println!(
            "  Dirty page hits:               {}",
            self.dirty_page_hit_count
        );
        println!(
            "  Node page cache hits:          {}",
            self.node_page_cache_hit_count
        );
        println!(
            "  Node page cache misses:        {}",
            self.node_page_cache_miss_count
        );
        println!(
            "  Redundant page reloads:        {}",
            self.redundant_page_reload_count
        );

        println!("\nPHASE 2: EDGE PATH (per operation):");
        println!(
            "  Edge cache hits/misses:        {}/{}",
            self.edge_cache_hit_count,
            self.edge_cache_hit_count + self.edge_cache_miss_count
        );
        println!(
            "  Edge page reads:               {}",
            self.edge_page_read_count
        );

        println!("\nPHASE 3: UNPACK COST BREAKDOWN (per operation):");
        println!(
            "  NodePage::unpack() calls:      {}",
            self.node_page_unpack_count
        );
        println!(
            "  Linear scan steps:             {}",
            self.node_linear_scan_steps
        );
        let avg_scan = if self.node_page_unpack_count > 0 {
            self.node_linear_scan_steps as f64 / self.node_page_unpack_count as f64
        } else {
            0.0
        };
        println!("  Avg nodes scanned per unpack:  {:.1}", avg_scan);
        println!(
            "  B+Tree traversal depth:        {}",
            self.btree_traversal_depth_total
        );
        let avg_depth = if self.btree_lookup_calls > 0 {
            self.btree_traversal_depth_total as f64 / self.btree_lookup_calls as f64
        } else {
            0.0
        };
        println!("  Avg B+Tree depth per lookup:   {:.1}", avg_depth);

        println!("\n───────────────────────────────────────────────────────────────\n");
    }
}
