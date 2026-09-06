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
        Some("install-remote") => install_remote(&args[1..]),
        _ => {
            eprintln!(
                "usage: zdbt el run <pipeline.yml | name> [--project <root>] [--chunk-rows <n>]\n       \
                 zdbt el ls [--project <root>]\n       \
                 zdbt el serve [--listen <addr:port>] [--project <root>] \
                 [--tls-cert <pem> --tls-key <pem>] [--insecure-http] [--track-checkout]\n       \
                 zdbt el install-remote <user@host> --remote-project <dir> [--name <n>] \
                 [--url <https://host:7431>] [--listen <addr:port>] [--branch <b>] [--project <root>]"
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
    config.track_checkout = args.iter().any(|arg| arg == "--track-checkout");
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
        profile_override: flag(args, "--profile"),
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


/// `zdbt el install-remote user@host`: installs the daemon on a server
/// over the user's own ssh, with the token generated HERE and shipped on
/// stdin (never argv), then declares the server locally — remotes.yml
/// entry + token in .env — so the Remote tab connects right away.
fn install_remote(args: &[String]) -> i32 {
    let Some(host_spec) = args.first().filter(|arg| !arg.starts_with("--")).cloned() else {
        eprintln!("usage: zdbt el install-remote <user@host> --remote-project <dir> …");
        return 2;
    };
    let Some(root) = project_root(args) else {
        eprintln!("no local project found — run inside an EL project or pass --project");
        return 2;
    };
    let remote_project = flag(args, "--remote-project").unwrap_or_else(|| "/srv/el-project".into());
    let host = host_spec.rsplit('@').next().unwrap_or(&host_spec).to_owned();
    let name = flag(args, "--name").unwrap_or_else(|| {
        host.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    });
    let url = flag(args, "--url").unwrap_or_else(|| format!("https://{host}:7431"));
    let listen = flag(args, "--listen").unwrap_or_else(|| "0.0.0.0:7431".into());
    let branch = flag(args, "--branch").unwrap_or_else(|| "el-spike".into());
    let profile = flag(args, "--profile").unwrap_or_else(|| "prod".into());
    let installer = format!(
        "https://raw.githubusercontent.com/arezki1990/dbt-zed/{branch}/deploy/el-serve/install.sh"
    );

    // A fresh token, from the OS CSPRNG.
    let token = {
        let mut bytes = [0u8; 24];
        match std::fs::File::open("/dev/urandom")
            .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes))
        {
            Ok(()) => bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
            Err(error) => {
                eprintln!("could not generate a token: {error}");
                return 1;
            }
        }
    };

    println!("==> {host_spec}: checking sudo (you may be asked for your password)");
    if !run_ssh(&host_spec, &["-t"], "sudo -v", None) {
        eprintln!("sudo check failed on {host_spec}");
        return 1;
    }
    println!("==> {host_spec}: placing the token (over stdin)");
    if !run_ssh(
        &host_spec,
        &[],
        "sudo mkdir -p /etc/zdbt-el-serve && sudo tee /etc/zdbt-el-serve/token >/dev/null \
         && sudo chmod 600 /etc/zdbt-el-serve/token",
        Some(&token),
    ) {
        eprintln!("could not place the token on {host_spec}");
        return 1;
    }
    println!("==> {host_spec}: installing (builds from source — this takes a while)");
    let install = format!(
        "curl -fsSL {installer} | sudo bash -s -- --project '{remote_project}' --listen '{listen}' \
         --branch '{branch}' --profile '{profile}'"
    );
    if !run_ssh(&host_spec, &["-t"], &install, None) {
        eprintln!("the installer failed on {host_spec} — see its output above");
        return 1;
    }

    // Declare it locally: token in .env (never in YAML), server in remotes.yml.
    let var = format!(
        "ZDBT_EL_TOKEN_{}",
        name.to_ascii_uppercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>()
    );
    let env_path = root.join(".env");
    {
        use std::io::Write as _;
        let mut file = match std::fs::OpenOptions::new().create(true).append(true).open(&env_path) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("could not write {}: {error}", env_path.display());
                return 1;
            }
        };
        let _ = writeln!(file, "{var}={token}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o600));
        }
    }
    let remotes_path = root.join("el").join("remotes.yml");
    let mut remotes = match el_engine::spec::load_remotes(&remotes_path) {
        Ok(remotes) => remotes,
        Err(_) => el_engine::spec::Remotes {
            version: 1,
            remotes: Default::default(),
            extra: Default::default(),
        },
    };
    remotes.remotes.insert(
        name.clone(),
        el_engine::spec::RemoteSpec {
            url: url.clone(),
            token: Some(format!("${{{var}}}")),
            extra: Default::default(),
        },
    );
    if let Err(error) = std::fs::write(
        &remotes_path,
        el_engine::spec::to_canonical_remotes_yaml(&remotes),
    ) {
        eprintln!("could not write {}: {error}", remotes_path.display());
        return 1;
    }
    println!(
        "\nDeclared {name} → {url} in el/remotes.yml; token stored as {var} in .env.\n\
         On the server: copy el/connections.yml under {remote_project}/el/, fill \
         /etc/zdbt-el-serve/env (profile + database URLs), add TLS to the unit \
         (or keep --insecure-http behind your TLS proxy), then\n  \
         systemctl enable --now zdbt-el-serve\n\
         Back in the IDE, open the Remote tab and deploy a pipeline from its canvas."
    );
    0
}

/// Runs a remote shell command via the user's ssh; `stdin` is piped
/// verbatim when given (how the token travels — never on a command line).
fn run_ssh(host: &str, ssh_flags: &[&str], command: &str, stdin: Option<&str>) -> bool {
    use std::io::Write as _;
    let mut cmd = std::process::Command::new("ssh");
    cmd.args(ssh_flags).arg(host).arg(command);
    if stdin.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("could not run ssh: {error}");
            return false;
        }
    };
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(input.as_bytes());
        let _ = pipe.write_all(b"\n");
    }
    child.wait().map(|status| status.success()).unwrap_or(false)
}
