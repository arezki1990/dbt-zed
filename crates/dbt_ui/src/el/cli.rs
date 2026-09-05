//! The headless EL CLI: `zdbt el run <pipeline>` — cron-able on any
//! machine, no GPUI involved. Progress streams as JSON lines on stdout;
//! the exit code is the run status.

use std::path::{Path, PathBuf};

/// Entry point for `zdbt el …`, called from main() before GPUI init.
/// Returns the process exit code.
pub fn main(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("ls") => list(&args[1..]),
        Some("serve") => serve(&args[1..]),
        _ => {
            eprintln!(
                "usage: zdbt el run <pipeline.yml | name> [--project <root>] [--chunk-rows <n>]\n       \
                 zdbt el ls [--project <root>]\n       \
                 zdbt el serve [--listen <addr:port>] [--project <root>] \
                 [--tls-cert <pem> --tls-key <pem>] [--insecure-http]"
            );
            2
        }
    }
}

/// `zdbt el serve`: the scheduling daemon with the JSON API. Token from
/// ZDBT_EL_TOKEN; non-loopback binds require the token AND TLS.
fn serve(args: &[String]) -> i32 {
    let Some(root) = project_root(args) else {
        eprintln!("no project found — run inside an EL project or pass --project");
        return 2;
    };
    let listen = flag(args, "--listen").unwrap_or_else(|| "127.0.0.1:7431".to_owned());
    let listen: std::net::SocketAddr = match listen.parse() {
        Ok(listen) => listen,
        Err(_) => {
            eprintln!("--listen must be addr:port, e.g. 127.0.0.1:7431");
            return 2;
        }
    };
    let mut config = el_engine::server::ServerConfig::new(root, listen);
    config.worker = super::find_worker();
    config.allow_insecure_http = args.iter().any(|arg| arg == "--insecure-http");
    config.tls = match (flag(args, "--tls-cert"), flag(args, "--tls-key")) {
        (Some(cert), Some(key)) => Some((PathBuf::from(cert), PathBuf::from(key))),
        (None, None) => None,
        _ => {
            eprintln!("--tls-cert and --tls-key go together");
            return 2;
        }
    };
    match el_engine::server::serve(config) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("el serve failed: {error:#}");
            1
        }
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|ix| args.get(ix + 1).cloned())
}

fn project_root(args: &[String]) -> Option<PathBuf> {
    if let Some(root) = flag(args, "--project") {
        return Some(PathBuf::from(root));
    }
    // Walk up from cwd to the nearest directory holding el/ or
    // dbt_project.yml.
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("el").is_dir() || dir.join("dbt_project.yml").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn resolve_pipeline(root: &Path, spec: &str) -> Option<PathBuf> {
    let as_path = Path::new(spec);
    if as_path.is_absolute() && as_path.is_file() {
        return Some(as_path.to_path_buf());
    }
    let relative = root.join(spec);
    if relative.is_file() {
        return Some(relative);
    }
    let named = root.join("el").join("pipelines").join(format!("{spec}.yml"));
    named.is_file().then_some(named)
}

fn list(args: &[String]) -> i32 {
    let Some(root) = project_root(args) else {
        eprintln!("no project found — run inside a dbt project or pass --project");
        return 1;
    };
    for path in el_engine::spec::list_pipelines(&root.join("el")) {
        println!("{}", path.display());
    }
    0
}

fn run(args: &[String]) -> i32 {
    let Some(root) = project_root(args) else {
        eprintln!("no project found — run inside a dbt project or pass --project");
        return 1;
    };
    let Some(spec_arg) = args.first().filter(|arg| !arg.starts_with("--")) else {
        eprintln!("usage: zdbt el run <pipeline.yml | name>");
        return 2;
    };
    let Some(spec_path) = resolve_pipeline(&root, spec_arg) else {
        eprintln!("pipeline {spec_arg:?} not found under {}", root.display());
        return 1;
    };
    let pipeline = match el_engine::spec::load_pipeline(&spec_path) {
        Ok(pipeline) => pipeline,
        Err(error) => {
            eprintln!("{error:#}");
            return 1;
        }
    };
    let chunk_rows = flag(args, "--chunk-rows")
        .and_then(|value| value.parse().ok())
        .unwrap_or(50_000);

    let request = el_engine::run::RunRequest {
        project_root: root,
        pipeline,
        worker: super::find_worker(),
        driver: None,
        chunk_rows,
    };
    let cancel = el_engine::CancelFlag::default();

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let printer = std::thread::spawn(move || {
        use futures::StreamExt as _;
        futures::executor::block_on(async move {
            while let Some(event) = rx.next().await {
                if let Ok(line) = serde_json::to_string(&event) {
                    println!("{line}");
                }
            }
        });
    });

    let result = el_engine::run::run_pipeline(&request, &tx, &cancel);
    drop(tx);
    let _ = printer.join();

    match result {
        Ok(report) if report.streams_failed == 0 => {
            eprintln!(
                "ok: {} stream(s), {} rows written",
                report.streams_ok, report.rows_written
            );
            0
        }
        Ok(report) => {
            eprintln!(
                "failed: {} ok, {} failed, {} rows written",
                report.streams_ok, report.streams_failed, report.rows_written
            );
            1
        }
        Err(error) => {
            eprintln!("{error:#}");
            1
        }
    }
}
