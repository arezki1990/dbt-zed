//! The standalone EL daemon: schedules, retries, failure hooks, and the
//! token-guarded JSON API — no IDE attached. This is the binary a
//! container or VM runs; the IDE connects to it via el/remotes.yml.

use std::path::PathBuf;

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|ix| args.get(ix + 1).cloned())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "zdbt-el-serve --project <root> [--listen <addr:port>] \
             [--tls-cert <pem> --tls-key <pem>] [--insecure-http] [--track-checkout] \
             [--worker <path>]\n\
             Token: ZDBT_EL_TOKEN env (required beyond loopback)."
        );
        return Ok(());
    }
    let project = flag(&args, "--project")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .filter(|root| root.join("el").is_dir())
        .ok_or_else(|| anyhow::anyhow!("--project must point at a directory holding el/"))?;
    let listen = flag(&args, "--listen")
        .unwrap_or_else(|| "127.0.0.1:7431".to_owned())
        .parse()
        .map_err(|_| anyhow::anyhow!("--listen must be addr:port"))?;

    let mut config = el_engine::server::ServerConfig::new(project, listen);
    config.allow_insecure_http = args.iter().any(|arg| arg == "--insecure-http");
    config.track_checkout = args.iter().any(|arg| arg == "--track-checkout");
    config.tls = match (flag(&args, "--tls-cert"), flag(&args, "--tls-key")) {
        (Some(cert), Some(key)) => Some((PathBuf::from(cert), PathBuf::from(key))),
        (None, None) => None,
        _ => anyhow::bail!("--tls-cert and --tls-key go together"),
    };
    config.worker = flag(&args, "--worker").map(PathBuf::from).or_else(|| {
        std::env::current_exe().ok().and_then(|exe| {
            let sibling = exe.parent()?.join("zdbt-el-worker");
            sibling.is_file().then_some(sibling)
        })
    });
    el_engine::server::serve(config)
}
