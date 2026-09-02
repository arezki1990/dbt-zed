#![cfg(test)]

use super::*;

#[test]
fn test_v3_wal_magic() {
    assert_eq!(V3_WAL_MAGIC, [b'V', b'3', b'W', b'A', b'L', 0, 0, 0]);
}

#[test]
fn test_lsn_utilities() {
    assert!(!lsn_is_valid(LSN_INVALID));
    assert!(lsn_is_valid(LSN_BEGIN));
    assert_eq!(lsn_next(LSN_BEGIN), LSN_BEGIN + 1);
}

#[test]
fn test_v3_wal_header_new() {
    let header = V3WALHeader::new();
    assert!(header.validate().is_ok());
    assert_eq!(header.magic, V3_WAL_MAGIC);
    assert_eq!(header.version, V3_WAL_VERSION);
    assert_eq!(header.page_size, 4096);
}

#[test]
fn test_v3_wal_header_serialization() {
    let original = V3WALHeader::new();
    let bytes = original.to_bytes();
    assert_eq!(bytes.len(), V3_WAL_HEADER_SIZE);

    let restored = V3WALHeader::from_bytes(&bytes).unwrap();
    assert_eq!(restored.magic, original.magic);
    assert_eq!(restored.version, original.version);
    assert_eq!(restored.page_size, original.page_size);
    assert_eq!(restored.current_lsn, original.current_lsn);
    assert_eq!(restored.committed_lsn, original.committed_lsn);
    assert_eq!(restored.checkpointed_lsn, original.checkpointed_lsn);
}

#[test]
fn test_v3_wal_header_invalid_magic() {
    let mut header = V3WALHeader::new();
    header.magic = [b'B', b'A', b'D', 0, 0, 0, 0, 0];
    assert!(header.validate().is_err());
}

#[test]
fn test_v3_wal_header_invalid_page_size() {
    let mut header = V3WALHeader::new();
    header.page_size = 12345;
    assert!(header.validate().is_err());
}

#[test]
fn test_record_type_from_u8() {
    assert_eq!(
        V3WALRecordType::try_from(1).unwrap(),
        V3WALRecordType::PageAllocate
    );
    assert_eq!(
        V3WALRecordType::try_from(5).unwrap(),
        V3WALRecordType::Checkpoint
    );
    assert!(V3WALRecordType::try_from(99).is_err());
}

#[test]
fn test_page_allocate_record() {
    let record = V3WALRecord::page_allocate(42, 100);
    assert!(matches!(record, V3WALRecord::PageAllocate { .. }));
    assert_eq!(record.lsn(), 100);
    assert!(record.is_data_modifying());
    assert!(!record.is_transaction_control());
    assert!(!record.is_checkpoint());
}

#[test]
fn test_page_free_record() {
    let record = V3WALRecord::page_free(42, 0x12345678, 100);
    assert!(matches!(record, V3WALRecord::PageFree { .. }));
    assert_eq!(record.lsn(), 100);
    assert!(record.is_data_modifying());
}

#[test]
fn test_page_write_record() {
    let data = vec![1, 2, 3, 4, 5];
    let record = V3WALRecord::page_write(42, 100, data, 100);
    assert!(matches!(record, V3WALRecord::PageWrite { .. }));
    assert_eq!(record.lsn(), 100);
    assert!(record.is_data_modifying());
}

#[test]
fn test_btree_split_record() {
    let record = V3WALRecord::btree_split(10, 20, 500, true, 100);
    assert!(matches!(record, V3WALRecord::BTreeSplit { .. }));
    assert_eq!(record.lsn(), 100);
    assert!(record.is_data_modifying());
}

#[test]
fn test_checkpoint_record() {
    let header = PersistentHeaderV3::new_v3();
    let record = V3WALRecord::checkpoint(5, 100, 3, 0, &header, 100);
    assert!(matches!(record, V3WALRecord::Checkpoint { .. }));
    assert_eq!(record.lsn(), 100);
    assert!(!record.is_data_modifying());
    assert!(record.is_checkpoint());
}

#[test]
fn test_transaction_control_records() {
    let begin = V3WALRecord::TransactionBegin {
        tx_id: 1,
        lsn: 100,
        timestamp: 0,
    };
    let commit = V3WALRecord::TransactionCommit {
        tx_id: 1,
        lsn: 101,
        timestamp: 0,
    };
    let rollback = V3WALRecord::TransactionRollback {
        tx_id: 1,
        lsn: 102,
        timestamp: 0,
    };

    assert!(!begin.is_data_modifying());
    assert!(begin.is_transaction_control());
    assert!(!begin.is_checkpoint());
    assert!(!commit.is_data_modifying());
    assert!(commit.is_transaction_control());
    assert!(!rollback.is_data_modifying());
    assert!(rollback.is_transaction_control());
}

#[test]
fn test_record_serialization_round_trip() {
    let records = vec![
        V3WALRecord::page_allocate(42, 100),
        V3WALRecord::page_free(43, 0x12345678, 101),
        V3WALRecord::page_write(44, 0, vec![1, 2, 3], 102),
        V3WALRecord::btree_split(10, 20, 500, true, 103),
    ];

    for original in records {
        let bytes = original.to_bytes().unwrap();
        let restored = V3WALRecord::from_bytes(&bytes).unwrap();
        assert_eq!(restored.record_type(), original.record_type());
        assert_eq!(restored.lsn(), original.lsn());
    }
}

#[test]
fn test_wal_paths() {
    let db_path = std::path::Path::new("/tmp/test.db");
    let wal_path = V3WALPaths::wal_file(db_path);
    assert_eq!(wal_path, std::path::Path::new("/tmp/test.v3wal"));

    let checkpoint_path = V3WALPaths::checkpoint_file(db_path);
    assert_eq!(
        checkpoint_path,
        std::path::Path::new("/tmp/test.v3checkpoint")
    );

    let temp_path = V3WALPaths::temp_checkpoint_file(db_path);
    assert!(temp_path.to_string_lossy().contains("v3checkpoint.tmp"));
}

#[test]
fn test_wal_recovery_new() {
    let wal_path = std::path::PathBuf::from("/tmp/test_recovery.v3wal");
    let recovery = WALRecovery::new(wal_path);
    assert_eq!(recovery.last_lsn(), LSN_INVALID);
    assert!(!recovery.stats().has_activity());
    assert_eq!(recovery.stats().records_processed, 0);
    assert!(recovery.checkpoint_header().is_none());
}

#[test]
fn test_wal_recovery_stats_default() {
    let stats = WALRecoveryStats::new();
    assert_eq!(stats.records_processed, 0);
    assert_eq!(stats.records_applied, 0);
    assert_eq!(stats.records_skipped, 0);
    assert_eq!(stats.page_allocations, 0);
    assert_eq!(stats.page_frees, 0);
    assert_eq!(stats.page_writes, 0);
    assert_eq!(stats.btree_splits, 0);
    assert_eq!(stats.checkpoints, 0);
}

#[test]
fn test_wal_recovery_stats_success_rate() {
    let mut stats = WALRecoveryStats::new();
    assert!((stats.success_rate() - 1.0).abs() < f64::EPSILON);
    stats.records_processed = 10;
    stats.records_applied = 8;
    stats.records_skipped = 2;
    assert!((stats.success_rate() - 0.8).abs() < f64::EPSILON);
}

#[test]
fn test_wal_recovery_apply_page_allocate() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut recovery = WALRecovery::new(wal_path);
    let record = V3WALRecord::page_allocate(42, 100);
    recovery.apply_record(&record).unwrap();
    assert!(recovery.page_cache().contains_key(&42));
    assert_eq!(recovery.stats().page_allocations, 1);
    assert_eq!(recovery.stats().records_applied, 0);
    assert_eq!(recovery.last_lsn(), 100);
}

#[test]
fn test_wal_recovery_apply_page_free() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut recovery = WALRecovery::new(wal_path);
    let alloc_record = V3WALRecord::page_allocate(42, 100);
    recovery.apply_record(&alloc_record).unwrap();
    let free_record = V3WALRecord::page_free(42, 0x12345678, 101);
    recovery.apply_record(&free_record).unwrap();
    assert!(!recovery.page_cache().contains_key(&42));
    assert_eq!(recovery.stats().page_allocations, 1);
    assert_eq!(recovery.stats().page_frees, 1);
    assert_eq!(recovery.last_lsn(), 101);
}

#[test]
fn test_wal_recovery_apply_page_write() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut recovery = WALRecovery::new(wal_path);
    let data = vec![1, 2, 3, 4, 5];
    let record = V3WALRecord::page_write(42, 0, data.clone(), 0x12345678);
    recovery.apply_record(&record).unwrap();
    assert!(recovery.page_cache().contains_key(&42));
    let page = recovery.page_cache().get(&42).unwrap();
    assert_eq!(page[0..5], data[..]);
    assert_eq!(recovery.stats().page_writes, 1);
}

#[test]
fn test_wal_recovery_apply_btree_split() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut recovery = WALRecovery::new(wal_path);
    let record = V3WALRecord::btree_split(10, 20, 500, true, 100);
    recovery.apply_record(&record).unwrap();
    assert!(recovery.page_cache().contains_key(&20));
    assert_eq!(recovery.stats().btree_splits, 1);
    assert_eq!(recovery.last_lsn(), 100);
}

#[test]
fn test_wal_recovery_apply_checkpoint() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut recovery = WALRecovery::new(wal_path);
    let header = PersistentHeaderV3::new_v3();
    let record = V3WALRecord::checkpoint(5, 100, 3, 0, &header, 100);
    recovery.apply_record(&record).unwrap();
    assert!(recovery.checkpoint_header().is_some());
    assert_eq!(recovery.stats().checkpoints, 1);
    assert_eq!(recovery.last_lsn(), 100);
}

#[test]
fn test_wal_recovery_apply_transaction_control() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut recovery = WALRecovery::new(wal_path);

    let begin = V3WALRecord::TransactionBegin {
        tx_id: 1,
        lsn: 100,
        timestamp: 0,
    };
    let commit = V3WALRecord::TransactionCommit {
        tx_id: 1,
        lsn: 101,
        timestamp: 0,
    };
    let rollback = V3WALRecord::TransactionRollback {
        tx_id: 2,
        lsn: 102,
        timestamp: 0,
    };

    recovery.apply_record(&begin).unwrap();
    recovery.apply_record(&commit).unwrap();
    recovery.apply_record(&rollback).unwrap();
    assert_eq!(recovery.last_lsn(), 102);
}

#[test]
fn test_wal_recovery_no_file() {
    let wal_path = std::path::PathBuf::from("/tmp/nonexistent_wal_file_xyz.v3wal");
    let mut recovery = WALRecovery::new(wal_path);
    let result = recovery.recover();
    assert!(result.is_ok());
    assert!(!recovery.stats().has_activity());
}

#[test]
fn test_wal_recovery_get_header_state() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut recovery = WALRecovery::new(wal_path);
    assert!(recovery.get_header_state().is_none());

    let header = PersistentHeaderV3::new_v3();
    let record = V3WALRecord::checkpoint(5, 100, 3, 0, &header, 100);
    recovery.apply_record(&record).unwrap();
    assert!(recovery.get_header_state().is_some());
}

#[test]
fn test_wal_writer_new() {
    let wal_path = std::path::PathBuf::from("/tmp/test_writer.v3wal");
    let writer = WALWriter::new(wal_path, LSN_BEGIN).unwrap();
    assert_eq!(writer.current_lsn(), LSN_BEGIN);
    assert_eq!(writer.committed_lsn(), LSN_INVALID);
}

#[test]
fn test_wal_writer_set_flush_threshold() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut writer = WALWriter::new(wal_path, LSN_BEGIN).unwrap();
    writer.set_flush_threshold(128 * 1024);
    assert_eq!(writer.flush_threshold, 128 * 1024);
}

#[test]
fn test_wal_writer_page_allocate_helper() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut writer = WALWriter::new(wal_path, LSN_BEGIN).unwrap();
    let lsn = writer.page_allocate(42).unwrap();
    assert_eq!(lsn, LSN_BEGIN);
    assert_eq!(writer.current_lsn(), LSN_BEGIN + 1);
}

#[test]
fn test_wal_writer_page_free_helper() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut writer = WALWriter::new(wal_path, LSN_BEGIN).unwrap();
    let lsn = writer.page_free(42, 0).unwrap();
    assert_eq!(lsn, LSN_BEGIN);
    assert_eq!(writer.current_lsn(), LSN_BEGIN + 1);
}

#[test]
fn test_wal_writer_page_write_helper() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut writer = WALWriter::new(wal_path, LSN_BEGIN).unwrap();
    let data = vec![1, 2, 3, 4, 5];
    let lsn = writer.page_write(42, 0, data).unwrap();
    assert_eq!(lsn, LSN_BEGIN);
    assert_eq!(writer.current_lsn(), LSN_BEGIN + 1);
}

#[test]
fn test_wal_writer_btree_split_helper() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut writer = WALWriter::new(wal_path, LSN_BEGIN).unwrap();
    let lsn = writer.btree_split(10, 20, 500, true).unwrap();
    assert_eq!(lsn, LSN_BEGIN);
    assert_eq!(writer.current_lsn(), LSN_BEGIN + 1);
}

#[test]
fn test_wal_writer_checkpoint_helper() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut writer = WALWriter::new(wal_path, LSN_BEGIN).unwrap();
    let header = PersistentHeaderV3::new_v3();
    let lsn = writer.checkpoint(5, 100, 3, 0, &header).unwrap();
    assert_eq!(lsn, LSN_BEGIN);
    assert_eq!(writer.current_lsn(), LSN_BEGIN + 1);
}

#[test]
fn test_wal_writer_transaction_helpers() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut writer = WALWriter::new(wal_path, LSN_BEGIN).unwrap();
    let begin_lsn = writer.transaction_begin(1).unwrap();
    assert_eq!(begin_lsn, LSN_BEGIN);
    let commit_lsn = writer.transaction_commit(1).unwrap();
    assert_eq!(commit_lsn, LSN_BEGIN + 1);
    let rollback_lsn = writer.transaction_rollback(2).unwrap();
    assert_eq!(rollback_lsn, LSN_BEGIN + 2);
}

#[test]
fn test_wal_writer_multiple_records() {
    let wal_path = std::path::PathBuf::from("/tmp/test.v3wal");
    let mut writer = WALWriter::new(wal_path, LSN_BEGIN).unwrap();
    let lsn1 = writer.page_allocate(42).unwrap();
    let lsn2 = writer.page_allocate(43).unwrap();
    let lsn3 = writer.page_allocate(44).unwrap();
    assert_eq!(lsn1, LSN_BEGIN);
    assert_eq!(lsn2, LSN_BEGIN + 1);
    assert_eq!(lsn3, LSN_BEGIN + 2);
    assert_eq!(writer.current_lsn(), LSN_BEGIN + 3);
}
