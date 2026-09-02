//! Configuration for backend selection and backend-specific options.

// Re-export public types
pub use combined::CombinedConfig;
pub use combined::CombinedReadMode;
pub use factory::open_graph;
pub use graph_config::GraphConfig;
pub use kinds::BackendKind;
pub use native::NativeConfig;
pub use sqlite::SqliteConfig;

// Private modules
mod combined;
mod factory;
mod graph_config;
mod kinds;
mod native;
mod sqlite;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::native::CpuProfile;
    use tempfile::tempdir;

    #[test]
    fn test_backend_kind_default() {
        assert_eq!(BackendKind::default(), BackendKind::SQLite);
    }

    #[test]
    fn test_graph_config_constructors() {
        let sqlite_cfg = GraphConfig::sqlite();
        assert_eq!(sqlite_cfg.backend, BackendKind::SQLite);

        let native_cfg = GraphConfig::native();
        assert_eq!(native_cfg.backend, BackendKind::Native);

        let combined_cfg = GraphConfig::combined();
        assert_eq!(combined_cfg.backend, BackendKind::Combined);
        assert!(!combined_cfg.sqlite.without_migrations);
        assert_eq!(
            combined_cfg.combined.read_mode,
            CombinedReadMode::SqliteOnly
        );
    }

    #[test]
    fn test_graph_config_default() {
        let cfg = GraphConfig::default();
        assert_eq!(cfg.backend, BackendKind::SQLite);
        assert!(!cfg.sqlite.without_migrations);
        assert!(cfg.native.create_if_missing);
        assert!(cfg.native.reserve_node_capacity.is_none());
        assert!(cfg.native.reserve_edge_capacity.is_none());
        assert!(cfg.native.cpu_profile.is_none());
        assert_eq!(cfg.native.effective_cpu_profile(), CpuProfile::Generic);
    }

    #[test]
    fn test_open_graph_sqlite() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let cfg = GraphConfig::sqlite();
        let result = open_graph(&db_path, &cfg);
        assert!(result.is_ok());
        assert!(db_path.exists());
    }

    #[test]
    fn test_open_graph_native() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_native.db");

        let cfg = GraphConfig::native();
        let result = open_graph(&db_path, &cfg);
        assert!(result.is_ok());
        assert!(db_path.exists());
    }

    #[test]
    fn test_open_graph_combined() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_combined.db");

        let cfg = GraphConfig::combined();
        let result = open_graph(&db_path, &cfg);
        assert!(result.is_ok());
        assert!(db_path.exists());
    }

    #[test]
    fn test_open_graph_combined_exposes_sqlite_graph_ref() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_combined_graph_ref.db");

        let cfg = GraphConfig::combined();
        let backend = open_graph(&db_path, &cfg).expect("combined open should succeed");
        assert!(backend.get_graph_ref().is_some());
    }

    #[test]
    fn test_graph_config_with_cpu_profile() {
        let config = GraphConfig::native().with_cpu_profile(CpuProfile::X86Zen4);
        assert_eq!(config.native.cpu_profile, Some(CpuProfile::X86Zen4));
        assert_eq!(config.native.effective_cpu_profile(), CpuProfile::X86Zen4);
        assert_eq!(config.backend, BackendKind::Native);
    }

    #[test]
    fn test_sqlite_config_builder() {
        let cfg = SqliteConfig::new().with_wal_mode().with_cache_size(1000);

        assert_eq!(
            cfg.pragma_settings.get("journal_mode"),
            Some(&"WAL".to_string())
        );
        assert_eq!(cfg.cache_size, Some(1000));
    }

    #[test]
    fn test_native_config_builder() {
        let config = NativeConfig::default().with_cpu_profile(CpuProfile::X86Avx2);

        assert_eq!(config.cpu_profile, Some(CpuProfile::X86Avx2));
        assert_eq!(config.effective_cpu_profile(), CpuProfile::X86Avx2);
    }

    #[test]
    fn test_combined_config_builder() {
        let config =
            GraphConfig::combined().with_combined_config(|cfg| cfg.with_materialized_reads());

        assert_eq!(
            config.combined.read_mode,
            CombinedReadMode::PreferMaterialized
        );
        assert_eq!(config.backend, BackendKind::Combined);
    }
}
