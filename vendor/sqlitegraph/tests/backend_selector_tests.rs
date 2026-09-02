use serde_json::json;
use sqlitegraph::backend::{GraphBackend, NodeSpec};
use sqlitegraph::backend_selector::GraphBackendFactory;
use sqlitegraph::config::BackendKind;

#[test]
fn test_backend_kind_from_env_defaults_to_sqlite() {
    unsafe {
        std::env::remove_var("GRAPH_BACKEND");
    }
    assert_eq!(BackendKind::from_env(), BackendKind::SQLite);
}

#[test]
fn test_backend_kind_selects_sqlite_backend() {
    unsafe {
        std::env::set_var("GRAPH_BACKEND", "sqlite");
    }
    let backend = GraphBackendFactory::from_env().new_sqlite().unwrap();
    let node = backend
        .insert_node(NodeSpec {
            kind: "Node".into(),
            name: "A".into(),
            file_path: None,
            data: json!({}),
        })
        .unwrap();
    assert!(node > 0);
}

#[test]
fn test_backend_kind_from_env_accepts_native_aliases() {
    unsafe {
        std::env::set_var("GRAPH_BACKEND", "native");
    }
    assert_eq!(BackendKind::from_env(), BackendKind::Native);

    unsafe {
        std::env::set_var("GRAPH_BACKEND", "native-v3");
    }
    assert_eq!(BackendKind::from_env(), BackendKind::Native);
}

#[test]
fn test_backend_kind_from_env_accepts_combined() {
    unsafe {
        std::env::set_var("GRAPH_BACKEND", "combined");
    }
    assert_eq!(BackendKind::from_env(), BackendKind::Combined);
}
