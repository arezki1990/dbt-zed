use super::*;
use tempfile::TempDir;

#[test]
fn test_persist_and_restore_roundtrip() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let kind_index = KindIndex::new();
    kind_index.insert("Function".to_string(), 1);
    kind_index.insert("Function".to_string(), 2);
    kind_index.insert("Function".to_string(), 3);
    kind_index.insert("Class".to_string(), 4);
    kind_index.insert("Class".to_string(), 5);

    let name_index = NameIndex::new();
    name_index.insert("func_a".to_string(), 1);
    name_index.insert("func_b".to_string(), 2);
    name_index.insert("func_b".to_string(), 3);
    name_index.insert("class_a".to_string(), 4);

    persist_indexes(&db_path, &kind_index, &name_index, 5).unwrap();

    let (restored_kind, restored_name) = restore_indexes(&db_path, 5).unwrap();

    assert_eq!(restored_kind.get("Function"), vec![1, 2, 3]);
    assert_eq!(restored_kind.get("Class"), vec![4, 5]);
    assert_eq!(restored_name.get_exact("func_a"), vec![1]);
    assert_eq!(restored_name.get_exact("func_b"), vec![2, 3]);
    assert_eq!(restored_name.get_exact("class_a"), vec![4]);
}

#[test]
fn test_restore_missing_file_fails() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("nonexistent.db");

    let result = restore_indexes(&db_path, 100);
    assert!(result.is_err());
}

#[test]
fn test_index_path_generation() {
    let cases = vec![
        ("test.db", "test.v3index"),
        ("mydb.sqlite", "mydb.v3index"),
        ("path/to/data.db", "path/to/data.v3index"),
    ];

    for (db, expected) in cases {
        let path = std::path::Path::new(db);
        let index_path = index_path_for_db(path);
        assert_eq!(index_path, std::path::Path::new(expected));
    }
}

#[test]
fn test_stale_index_detected() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let kind_index = KindIndex::new();
    kind_index.insert("Function".to_string(), 1);
    kind_index.insert("Class".to_string(), 2);

    let name_index = NameIndex::new();
    name_index.insert("func_a".to_string(), 1);

    persist_indexes(&db_path, &kind_index, &name_index, 5).unwrap();

    let result = restore_indexes(&db_path, 10);
    assert!(result.is_err());
}
