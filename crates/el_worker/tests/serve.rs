//! The P6 proof: the `el serve` daemon end to end — token auth enforced,
//! a run triggered over the API, its events polled live to completion,
//! and the warehouse actually written. Local DuckDB through the real
//! worker binary; loopback HTTP (TLS is exercised by the config guard
//! tests in el_engine).

use std::net::TcpListener;

use el_engine::server::{RemoteClient, ServerConfig, serve};

fn worker() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_zdbt-el-worker"))
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_for_health(client: &RemoteClient) {
    for _ in 0..100 {
        if client.pipelines().is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("daemon never came up");
}

#[test]
fn serve_runs_pipelines_over_the_api() {
    let project = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    // SAFETY: test-scoped env; this integration test binary runs alone.
    unsafe { std::env::set_var("ZDBT_EL_STATE_DIR", state_dir.path()) };

    let el = project.path().join("el");
    std::fs::create_dir_all(el.join("pipelines")).unwrap();
    {
        let connection = duckdb::Connection::open(el.join("source.duckdb")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE main.orders (id BIGINT, customer VARCHAR);
                 INSERT INTO main.orders VALUES (1,'acme'), (2,'globex'), (3,'initech');",
            )
            .unwrap();
    }
    std::fs::write(
        el.join("connections.yml"),
        "version: 1\nconnections:\n  src: { type: duckdb, path: el/source.duckdb }\n  wh: { type: duckdb, path: el/warehouse.duckdb }\n",
    )
    .unwrap();
    let orders_yaml = "version: 1\npipeline: orders\nsource: src\ntarget: { connection: wh, schema: LANDING }\nstreams:\n- name: orders\n  source: { schema: main, table: orders }\n";
    std::fs::write(el.join("pipelines").join("orders.yml"), orders_yaml).unwrap();

    let port = free_port();
    let listen: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let mut config = ServerConfig::new(project.path().to_path_buf(), listen);
    config.token = Some("test-token".to_owned());
    config.worker = Some(worker());
    config.chunk_rows = 2;
    std::thread::spawn(move || serve(config).unwrap());

    let url = format!("http://127.0.0.1:{port}");
    let good = RemoteClient::direct(
        &url,
        Some(el_engine::env::Secret::new("test-token".to_owned())),
    )
    .unwrap();
    wait_for_health(&good);

    // Wrong (and missing) tokens are rejected.
    let bad = RemoteClient::direct(
        &url,
        Some(el_engine::env::Secret::new("wrong".to_owned())),
    )
    .unwrap();
    let error = format!("{:#}", bad.pipelines().unwrap_err());
    assert!(error.contains("401"), "expected 401, got: {error}");
    let anonymous = RemoteClient::direct(&url, None).unwrap();
    assert!(anonymous.pipelines().is_err());

    // NOTHING is live until the developer deploys: the checkout's
    // pipelines are invisible to the daemon.
    assert_eq!(good.pipelines().unwrap().len(), 0);
    let error = format!("{:#}", good.start_run("orders").unwrap_err());
    assert!(error.contains("deploy"), "got: {error}");

    // Deploy, then the pipeline list reflects the deployed set.
    let deployed = good
        .deploy(&[("orders".to_owned(), orders_yaml.to_owned())])
        .unwrap();
    assert_eq!(deployed, ["orders"]);
    // Broken or ill-named bundles are refused whole.
    assert!(good.deploy(&[("evil/../name".to_owned(), orders_yaml.to_owned())]).is_err());
    assert!(good.deploy(&[("orders".to_owned(), "not: [valid".to_owned())]).is_err());
    let pipelines = good.pipelines().unwrap();
    assert_eq!(pipelines.len(), 1);
    assert_eq!(pipelines[0].name, "orders");
    assert_eq!(pipelines[0].streams, 1);

    // Trigger a run and poll its events to completion.
    let run_id = good.start_run("orders").unwrap();
    let mut since = 0;
    let mut finished_rows = 0;
    for _ in 0..200 {
        let page = good.events(run_id, since).unwrap();
        for event in &page.events {
            if let el_engine::ProgressEvent::StreamFinished { rows_written, .. } = event {
                finished_rows = *rows_written;
            }
        }
        since = page.next;
        if page.done {
            assert!(page.error.is_none(), "run failed: {:?}", page.error);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(finished_rows, 3, "all three source rows must land");

    // History shows the finished run with its row count.
    let runs = good.runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "ok");
    assert_eq!(runs[0].rows_written, 3);

    // The warehouse really holds the rows.
    let connection = duckdb::Connection::open(el.join("warehouse.duckdb")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM LANDING.ORDERS", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 3);

    // Unknown pipeline is a clean API error, not a crash.
    assert!(good.start_run("nope").is_err());
}

/// Non-loopback binds must refuse to start without token or TLS — the
/// "secured protocol" contract for remote deployment.
#[test]
fn serve_refuses_insecure_remote_binds() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("el").join("pipelines")).unwrap();
    let listen: std::net::SocketAddr = "0.0.0.0:7431".parse().unwrap();

    let mut config = ServerConfig::new(project.path().to_path_buf(), listen);
    config.token = None;
    let error = format!("{:#}", serve(config).unwrap_err());
    assert!(error.contains("ZDBT_EL_TOKEN"), "got: {error}");

    let mut config = ServerConfig::new(project.path().to_path_buf(), listen);
    config.token = Some("t".to_owned());
    config.tls = None;
    let error = format!("{:#}", serve(config).unwrap_err());
    assert!(error.contains("--tls-cert"), "got: {error}");
}

/// The client refuses plaintext for anything that is not loopback — and
/// URL tricks that dress a remote host up as localhost.
#[test]
fn client_refuses_plaintext_remotes() {
    assert!(RemoteClient::direct("http://el.example.com:7431", None).is_err());
    assert!(RemoteClient::direct("http://127.0.0.1:7431", None).is_ok());
    assert!(RemoteClient::direct("http://localhost:7431", None).is_ok());
    assert!(RemoteClient::direct("https://el.example.com:7431", None).is_ok());
    // Userinfo bypasses: "localhost:6" is userinfo, the HOST is evil.com.
    assert!(RemoteClient::direct("http://localhost:6@evil.com", None).is_err());
    assert!(RemoteClient::direct("http://localhost:@evil.com/x", None).is_err());
    // Credentials never belong in the URL, even over https.
    assert!(RemoteClient::direct("https://user:pw@el.example.com", None).is_err());
    assert!(RemoteClient::direct("ftp://localhost:7431", None).is_err());
}
