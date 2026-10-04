//! The local runner: the EL daemon image run in this machine's own Docker,
//! serving the open project at `http://127.0.0.1:7431` as the `localhost`
//! remote.
//!
//! An image already in the local Docker runs as it is: Docker would not ask
//! the registry before running it either, and an expired sign-in must not
//! stop a runner that needs no network. Only a machine without the image
//! asks the registry, and there reading it is the licence: the provider
//! grants that read per customer, so the check is whether Docker can read
//! the image's manifest, with no layers pulled to find out.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// The name the runner is registered under in `el/remotes.yml`.
pub const REMOTE_NAME: &str = "localhost";
/// The daemon's port, published on loopback only.
pub const PORT: u16 = 7431;
/// The image offered when the project does not name one.
pub const DEFAULT_IMAGE: &str =
    "europe-west4-docker.pkg.dev/jules-terraform/docker/zdbt-el-serve:el-v0.3.0-42-g2f180fe";
/// Environment or `.env` variable overriding the image: another tag, or a
/// provider's other registry.
pub const IMAGE_VAR: &str = "EL_RUNNER_IMAGE";
/// `.env` variable holding the token the container and the remote share, when
/// the project does not already declare a server at the runner's address.
pub const TOKEN_VAR: &str = "EL_RUNNER_TOKEN";
/// Environment or `.env` variable naming a Linux Oracle Instant Client on this
/// machine. Oracle 10g / 11g sources need it, and it can't ship in the image.
pub const ORACLE_CLIENT_VAR: &str = "EL_RUNNER_ORACLE_CLIENT_DIR";

/// What the registry says about running the image on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// Docker cannot be run: not installed, or its daemon is down.
    NoDocker(String),
    /// The registry served the manifest: this machine holds a licence.
    Licensed,
    /// The registry refused these credentials: no licence for this identity.
    Unlicensed(String),
    /// Docker has no usable registry credentials to ask with — signed out,
    /// or its credential helper is missing.
    SignIn(String),
    /// Anything else: an unpublished tag, the network.
    Unavailable(String),
}

/// The runner container for a project, as `docker ps` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Container {
    Missing,
    Running,
    /// Created but not running; Docker's own status line ("Exited (1) 2
    /// minutes ago").
    Stopped(String),
}

/// The image this project runs: `EL_RUNNER_IMAGE` from the environment or
/// `.env`, else the default.
pub fn image(project_root: &Path) -> String {
    el_engine::env::EnvMap::load(project_root, None)
        .get(IMAGE_VAR)
        .map(|image| image.trim().to_owned())
        .filter(|image| !image.is_empty())
        .unwrap_or_else(|| DEFAULT_IMAGE.to_owned())
}

/// One container per project, named after its directory.
pub fn container_name(project_root: &Path) -> String {
    let slug: String = project_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("el-runner-{}", slug.trim_matches('-'))
}

/// Whether this machine may run the image: it is already in the local
/// Docker, or the registry lets Docker's own credentials read its manifest.
pub async fn check_access(image: &str) -> Access {
    if image_is_local(image).await {
        return Access::Licensed;
    }
    let mut command = docker_command();
    command
        .args(["manifest", "inspect", image])
        .kill_on_drop(true);
    match command.output().await {
        Ok(output) => classify(
            output.status.success(),
            &String::from_utf8_lossy(&output.stderr),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Access::NoDocker("docker is not installed".to_owned())
        }
        Err(error) => Access::Unavailable(error.to_string()),
    }
}

/// Whether Docker already holds the image under this exact reference. Any
/// failure — absent, or Docker not usable — sends the check on to the
/// registry, whose answer says which it was.
async fn image_is_local(image: &str) -> bool {
    docker_command()
        .args(["image", "inspect", "--format", "{{.Id}}", image])
        .kill_on_drop(true)
        .output()
        .await
        .is_ok_and(|output| output.status.success())
}

/// Sorts `docker manifest inspect`'s answer. Sign-in problems are told apart
/// from refusals: a registry answering "unauthenticated" has not looked at a
/// licence yet.
pub fn classify(success: bool, stderr: &str) -> Access {
    if success {
        return Access::Licensed;
    }
    let detail = first_line(stderr);
    let lower = stderr.to_lowercase();
    if lower.contains("cannot connect to the docker daemon")
        || lower.contains("is the docker daemon running")
    {
        Access::NoDocker(detail)
    } else if lower.contains("error getting credentials")
        || lower.contains("unauthenticated request")
        || lower.contains("authentication required")
        || lower.contains("no basic auth credentials")
    {
        Access::SignIn(detail)
    } else if lower.contains("denied")
        || lower.contains("forbidden")
        || lower.contains("unauthorized")
    {
        Access::Unlicensed(detail)
    } else {
        Access::Unavailable(detail)
    }
}

/// What the project's runner container is doing.
pub async fn container(project_root: &Path) -> Result<Container> {
    let name = container_name(project_root);
    let output = docker_command()
        .args([
            "ps",
            "--all",
            "--filter",
            &format!("name={name}"),
            "--format",
            "{{.Names}}\t{{.State}}\t{{.Status}}",
        ])
        .output()
        .await
        .context("running docker")?;
    if !output.status.success() {
        bail!("{}", first_line(&String::from_utf8_lossy(&output.stderr)));
    }
    Ok(parse_container(
        &String::from_utf8_lossy(&output.stdout),
        &name,
    ))
}

/// The `name` filter matches substrings, so the exact name is picked out of
/// `docker ps`'s lines here.
pub fn parse_container(ps_output: &str, name: &str) -> Container {
    ps_output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            Some((fields.next()?, fields.next()?, fields.next().unwrap_or("")))
        })
        .find(|(names, _, _)| names.split(',').any(|candidate| candidate == name))
        .map_or(Container::Missing, |(_, state, status)| {
            if state == "running" {
                Container::Running
            } else {
                Container::Stopped(status.trim().to_owned())
            }
        })
}

/// Starts (or restarts) the project's runner with the project mounted and the
/// port published on loopback only. The token and the server entry are worked
/// out first but written only once the container runs, so a start that fails
/// (a taken port, a missing image) leaves the project as it was.
pub async fn start(project_root: &Path, image: &str) -> Result<()> {
    let remote = runner_remote(project_root)?;
    let existing = el_engine::env::EnvMap::load(project_root, None)
        .get(&remote.token_var)
        .filter(|token| !token.trim().is_empty());
    // A tracked .env is refused before anything runs, not after.
    let exclude = match existing {
        Some(_) => None,
        None => dotenv_exclusion(project_root).await?,
    };
    let token = existing.clone().unwrap_or_else(new_token);
    let oracle = oracle_client_args(project_root)?;

    let name = container_name(project_root);
    // A previous container for this project (stopped, or on an old image)
    // is replaced, not reused.
    docker_command()
        .args(["rm", "--force", &name])
        .output()
        .await
        .context("running docker")?;

    let publish = format!("127.0.0.1:{PORT}:{PORT}");
    let mount = format!(
        "type=bind,source={},target=/project",
        project_root.display()
    );
    let mut run = docker_command();
    run.args([
        "run",
        "--detach",
        "--name",
        &name,
        "--restart",
        "unless-stopped",
        "--publish",
        &publish,
        "--mount",
        &mount,
        // Passed by name: the value comes from this process's environment,
        // so the token never appears in `ps`.
        "--env",
        "ELLODE_TOKEN",
    ])
    .args(&oracle)
    .arg(image)
    .env("ELLODE_TOKEN", &token);
    let output = run.output().await.context("running docker")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // `docker run` leaves a created, never-started container behind when
        // it can't start one; don't leave that for the panel to report.
        docker_command()
            .args(["rm", "--force", &name])
            .output()
            .await
            .ok();
        if stderr.contains("port is already allocated") {
            bail!(
                "port {PORT} is already in use — another project's local runner \
                 probably holds it; stop that one first"
            );
        }
        bail!("{}", first_line(&stderr));
    }

    if existing.is_none() {
        if let Some(exclude) = exclude {
            append_line(&exclude, ".env").context("excluding .env from git")?;
        }
        append_dotenv(project_root, &remote.token_var, &token)?;
    }
    declare_remote(project_root, &remote)
}

/// Removes the project's runner container. Already gone is fine.
pub async fn stop(project_root: &Path) -> Result<()> {
    let output = docker_command()
        .args(["rm", "--force", &container_name(project_root)])
        .output()
        .await
        .context("running docker")?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() && !stderr.contains("No such container") {
        bail!("{}", first_line(&stderr));
    }
    Ok(())
}

/// The server the runner is reached through in `el/remotes.yml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerRemote {
    /// A server already declared at the runner's address keeps its name;
    /// otherwise `localhost`.
    pub name: String,
    /// The variable holding the token the container is started with.
    pub token_var: String,
    change: RemoteChange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RemoteChange {
    /// Declared already, with a `${VAR}` token.
    None,
    /// Declared at the address without a token: it gets one.
    Token,
    /// Not declared: `localhost` is added.
    Entry,
}

/// Whether a declared URL is the runner's own address: plain http on
/// loopback, at the runner's port.
pub fn is_runner_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw.trim()) else {
        return false;
    };
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    url.scheme() == "http"
        && loopback
        && url.port() == Some(PORT)
        && url.path().trim_matches('/').is_empty()
}

/// How the runner is declared, worked out without writing anything. A server
/// already at the runner's address is adopted, name and token variable both,
/// so a project keeps one entry per daemon; otherwise it becomes `localhost`.
pub fn runner_remote(project_root: &Path) -> Result<RunnerRemote> {
    let remotes = load_remotes(project_root)?;
    // With several servers at the address, `localhost` is the runner's own
    // name and the one connections were pointed at; otherwise the first wins.
    let mut at_address = remotes
        .remotes
        .iter()
        .filter(|(_, remote)| is_runner_url(&remote.url));
    let adopted = at_address
        .clone()
        .find(|(name, _)| name.as_str() == REMOTE_NAME)
        .or_else(|| at_address.next());
    Ok(match adopted {
        Some((name, remote)) => match remote.token.as_deref() {
            Some(template) => {
                let mut refs = Vec::new();
                el_engine::env::collect_var_refs(template, &mut refs);
                let Some(token_var) = refs.into_iter().next() else {
                    bail!("the {name:?} server's token must be a ${{VAR}} reference");
                };
                RunnerRemote {
                    name: name.clone(),
                    token_var,
                    change: RemoteChange::None,
                }
            }
            None => RunnerRemote {
                name: name.clone(),
                token_var: TOKEN_VAR.to_owned(),
                change: RemoteChange::Token,
            },
        },
        None => RunnerRemote {
            name: REMOTE_NAME.to_owned(),
            token_var: TOKEN_VAR.to_owned(),
            change: RemoteChange::Entry,
        },
    })
}

fn load_remotes(project_root: &Path) -> Result<el_engine::spec::Remotes> {
    match el_engine::spec::load_remotes(&super::el_dir(project_root).join("remotes.yml")) {
        Ok(remotes) => Ok(remotes),
        Err(el_engine::spec::SpecError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(el_engine::spec::Remotes {
                version: 1,
                remotes: Default::default(),
                extra: Default::default(),
            })
        }
        // A broken file is not an empty one: rewriting it would lose the
        // servers it declares.
        Err(error) => bail!("remotes.yml could not be read: {error} — fix the file first"),
    }
}

/// Writes what `runner_remote` found missing, so deploys, the console and
/// connections' `workspace:` reach the runner like any other server.
fn declare_remote(project_root: &Path, remote: &RunnerRemote) -> Result<()> {
    let template = format!("${{{}}}", remote.token_var);
    let mut remotes = load_remotes(project_root)?;
    match remote.change {
        RemoteChange::None => return Ok(()),
        RemoteChange::Token => {
            if let Some(entry) = remotes.remotes.get_mut(&remote.name) {
                entry.token = Some(template);
            }
        }
        RemoteChange::Entry => {
            remotes.remotes.insert(
                remote.name.clone(),
                el_engine::spec::RemoteSpec {
                    url: format!("http://127.0.0.1:{PORT}"),
                    token: Some(template),
                    live: true,
                    extra: Default::default(),
                },
            );
        }
    }
    let path = super::el_dir(project_root).join("remotes.yml");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating el/")?;
    }
    std::fs::write(&path, el_engine::spec::to_canonical_remotes_yaml(&remotes))
        .context("writing el/remotes.yml")
}

fn new_token() -> String {
    let bytes: [u8; 32] = rand::random();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Oracle 10g / 11g sources go through the thick driver, which needs a Linux
/// Instant Client beside the worker. When `EL_RUNNER_ORACLE_CLIENT_DIR` names
/// one, it is mounted with a sqlnet.ora allowing the old logon protocol 10g
/// still speaks (kept in the IDE's data directory, never in the project).
fn oracle_client_args(project_root: &Path) -> Result<Vec<String>> {
    let Some(dir) = el_engine::env::EnvMap::load(project_root, None)
        .get(ORACLE_CLIENT_VAR)
        .map(|dir| dir.trim().to_owned())
        .filter(|dir| !dir.is_empty())
    else {
        return Ok(Vec::new());
    };
    let client = PathBuf::from(&dir);
    if !client.join("libclntsh.so").exists() {
        bail!(
            "{ORACLE_CLIENT_VAR} is {dir:?}, which holds no Linux Instant Client \
             (libclntsh.so) — point it at the unzipped instantclient_19_* directory"
        );
    }
    let admin = paths::data_dir().join("el-runner").join("network-admin");
    std::fs::create_dir_all(&admin).context("creating the runner's sqlnet.ora directory")?;
    std::fs::write(
        admin.join("sqlnet.ora"),
        "SQLNET.ALLOWED_LOGON_VERSION_CLIENT=8\nSQLNET.ALLOWED_LOGON_VERSION_SERVER=8\n",
    )
    .context("writing the runner's sqlnet.ora")?;
    Ok(vec![
        "--mount".to_owned(),
        format!(
            "type=bind,source={},target=/opt/oracle/instantclient,readonly",
            client.display()
        ),
        "--mount".to_owned(),
        format!(
            "type=bind,source={},target=/opt/oracle/network/admin,readonly",
            admin.display()
        ),
        "--env".to_owned(),
        "ELLODE_ORACLE_CLIENT_DIR=/opt/oracle/instantclient".to_owned(),
        "--env".to_owned(),
        "LD_LIBRARY_PATH=/opt/oracle/instantclient".to_owned(),
        "--env".to_owned(),
        "TNS_ADMIN=/opt/oracle/network/admin".to_owned(),
    ])
}

/// Checks `.env` can safely hold a token (the runner's, or a console
/// remote's access token), before one is written. A tracked `.env` is
/// refused. An unignored one is to be added to `.git/info/exclude` — local
/// to this clone rather than the team's `.gitignore` — and that path is
/// returned for the caller to write once the token is. Outside a git
/// checkout there is nothing to protect against.
pub(super) async fn dotenv_exclusion(project_root: &Path) -> Result<Option<PathBuf>> {
    let git = |args: &[&str]| {
        let mut command = util::command::new_command("git");
        command.arg("-C").arg(project_root).args(args);
        command
    };
    let Ok(exclude) = git(&["rev-parse", "--git-path", "info/exclude"])
        .output()
        .await
    else {
        return Ok(None);
    };
    if !exclude.status.success() {
        return Ok(None);
    }
    let tracked = git(&["ls-files", "--error-unmatch", ".env"])
        .output()
        .await
        .is_ok_and(|output| output.status.success());
    if tracked {
        bail!(
            ".env is tracked by git, so a token can't be written there — set it in \
             your environment instead"
        );
    }
    let ignored = git(&["check-ignore", "--quiet", ".env"])
        .output()
        .await
        .is_ok_and(|output| output.status.success());
    Ok((!ignored).then(|| project_root.join(String::from_utf8_lossy(&exclude.stdout).trim())))
}

fn append_dotenv(project_root: &Path, key: &str, value: &str) -> Result<()> {
    let path = project_root.join(".env");
    append_line(
        &path,
        "# The local EL runner's token, written by the IDE — never commit it.",
    )?;
    append_line(&path, &format!("{key}={value}"))
}

/// Appends a line, first ending the file's last line if it has no newline.
pub(super) fn append_line(path: &Path, line: &str) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    write!(file, "{separator}{line}\n").with_context(|| format!("writing {}", path.display()))
}

/// The docker CLI: first on PATH (Zed loads the login shell's PATH at
/// startup), then where Docker Desktop and Homebrew install it.
fn docker() -> PathBuf {
    let exe = if cfg!(windows) {
        "docker.exe"
    } else {
        "docker"
    };
    let on_path: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(exe))
                .collect()
        })
        .unwrap_or_default();
    let fallbacks = [
        "/usr/local/bin/docker",
        "/opt/homebrew/bin/docker",
        "/Applications/Docker.app/Contents/Resources/bin/docker",
    ]
    .map(PathBuf::from);
    on_path
        .into_iter()
        .chain(fallbacks)
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(exe))
}

/// A docker invocation whose credential helpers (`docker-credential-desktop`,
/// `-gcloud`…) resolve: Docker finds them on PATH, so its own directory goes
/// first.
fn docker_command() -> util::command::Command {
    let docker = docker();
    let mut command = util::command::new_command(&docker);
    if let Some(dir) = docker.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        let mut paths = vec![dir.to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            command.env("PATH", joined);
        }
    }
    command
}

/// Docker's first meaningful stderr line, capped for a tooltip.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("docker failed without saying why");
    line.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real answers from `docker manifest inspect` against Artifact Registry
    /// and a stopped Docker Desktop.
    #[test]
    fn registry_answers_sort_into_licence_states() {
        assert_eq!(classify(true, ""), Access::Licensed);
        assert!(matches!(
            classify(
                false,
                "denied: Permission \"artifactregistry.repositories.downloadArtifacts\" denied on resource \"projects/jules-terraform/locations/europe-west4/repositories/docker\" (or it may not exist)"
            ),
            Access::Unlicensed(_)
        ));
        // "denied" appears here too, but the registry never got to the
        // licence: the user has to sign in first.
        assert!(matches!(
            classify(
                false,
                "denied: Unauthenticated request. Unauthenticated requests do not have permission \"artifactregistry.repositories.downloadArtifacts\""
            ),
            Access::SignIn(_)
        ));
        assert!(matches!(
            classify(
                false,
                "error getting credentials - err: exec: \"docker-credential-gcloud\": executable file not found in $PATH, out: ``"
            ),
            Access::SignIn(_)
        ));
        assert!(matches!(
            classify(
                false,
                "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?"
            ),
            Access::NoDocker(_)
        ));
        assert_eq!(
            classify(
                false,
                "\n  no such manifest: europe-west4-docker.pkg.dev/x/y:z\n"
            ),
            Access::Unavailable("no such manifest: europe-west4-docker.pkg.dev/x/y:z".to_owned())
        );
    }

    #[test]
    fn container_state_is_read_for_the_exact_name() {
        let ps = "el-runner-shop-archive\trunning\tUp 3 hours\n\
                  el-runner-shop\texited\tExited (1) 2 minutes ago\n";
        assert_eq!(
            parse_container(ps, "el-runner-shop"),
            Container::Stopped("Exited (1) 2 minutes ago".to_owned())
        );
        assert_eq!(
            parse_container(ps, "el-runner-shop-archive"),
            Container::Running
        );
        assert_eq!(parse_container(ps, "el-runner-other"), Container::Missing);
        assert_eq!(parse_container("", "el-runner-shop"), Container::Missing);
    }

    #[test]
    fn container_names_are_valid_for_any_directory() {
        assert_eq!(
            container_name(Path::new("/Users/me/projects/dbt-fixture")),
            "el-runner-dbt-fixture"
        );
        assert_eq!(
            container_name(Path::new("/Users/me/Dev & Code/Shop Data")),
            "el-runner-shop-data"
        );
    }

    fn remotes_file(root: &Path, yaml: &str) -> PathBuf {
        let path = super::super::el_dir(root).join("remotes.yml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, yaml).unwrap();
        path
    }

    #[test]
    fn the_runner_address_is_recognised_however_it_is_spelled() {
        assert!(is_runner_url("http://127.0.0.1:7431"));
        assert!(is_runner_url("http://localhost:7431/"));
        assert!(is_runner_url("  HTTP://LOCALHOST:7431 "));
        assert!(is_runner_url("http://[::1]:7431"));
        assert!(!is_runner_url("http://127.0.0.1:7432"));
        assert!(!is_runner_url("https://127.0.0.1:7431"));
        assert!(!is_runner_url("http://10.0.0.5:7431"));
        assert!(!is_runner_url("http://127.0.0.1:7431/api"));
    }

    #[test]
    fn a_new_project_gets_a_localhost_server_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let remote = runner_remote(root).unwrap();
        assert_eq!(
            (remote.name.as_str(), remote.token_var.as_str()),
            (REMOTE_NAME, TOKEN_VAR)
        );
        declare_remote(root, &remote).unwrap();

        let path = super::super::el_dir(root).join("remotes.yml");
        let written = std::fs::read_to_string(&path).unwrap();
        let parsed = el_engine::spec::load_remotes(&path).unwrap();
        let local = &parsed.remotes[REMOTE_NAME];
        assert_eq!(local.url, "http://127.0.0.1:7431");
        assert_eq!(local.token.as_deref(), Some("${EL_RUNNER_TOKEN}"));
        // The loopback URL is one the remote client accepts over plain http.
        el_engine::server::check_remote_url(&local.url).unwrap();

        // Declared now, so it is adopted and nothing more is written.
        let again = runner_remote(root).unwrap();
        assert_eq!(again.change, RemoteChange::None);
        declare_remote(root, &again).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
    }

    /// The user's own daemon entry at the runner's address is the runner: no
    /// second name for the same port, and its token variable is the one the
    /// container gets.
    #[test]
    fn a_server_already_at_the_runner_address_is_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = "version: 1\nremotes:\n  local_daemon:\n    url: http://127.0.0.1:7431\n    token: ${ELLODE_TOKEN}\n  docker_test:\n    url: http://127.0.0.1:7432\n    token: ${ELLODE_TOKEN_DOCKER_TEST}\n";
        let path = remotes_file(dir.path(), yaml);

        let remote = runner_remote(dir.path()).unwrap();
        assert_eq!(remote.name, "local_daemon");
        assert_eq!(remote.token_var, "ELLODE_TOKEN");
        declare_remote(dir.path(), &remote).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), yaml);
    }

    #[test]
    fn localhost_wins_when_several_servers_share_the_address() {
        let dir = tempfile::tempdir().unwrap();
        remotes_file(
            dir.path(),
            "version: 1\nremotes:\n  local_daemon:\n    url: http://127.0.0.1:7431\n    token: ${ELLODE_TOKEN}\n  localhost:\n    url: http://127.0.0.1:7431\n    token: ${EL_RUNNER_TOKEN}\n",
        );
        let remote = runner_remote(dir.path()).unwrap();
        assert_eq!(
            (remote.name.as_str(), remote.token_var.as_str()),
            (REMOTE_NAME, TOKEN_VAR)
        );
    }

    #[test]
    fn a_server_at_the_address_without_a_token_is_given_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = remotes_file(
            dir.path(),
            "version: 1\nremotes:\n  mine:\n    url: http://localhost:7431\n",
        );
        let remote = runner_remote(dir.path()).unwrap();
        assert_eq!(
            (remote.name.as_str(), remote.token_var.as_str()),
            ("mine", TOKEN_VAR)
        );
        declare_remote(dir.path(), &remote).unwrap();
        let parsed = el_engine::spec::load_remotes(&path).unwrap();
        assert_eq!(parsed.remotes.len(), 1);
        assert_eq!(
            parsed.remotes["mine"].token.as_deref(),
            Some("${EL_RUNNER_TOKEN}")
        );
    }

    #[test]
    fn a_broken_remotes_file_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = remotes_file(dir.path(), "remotes: [this is: not valid");
        assert!(runner_remote(dir.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "remotes: [this is: not valid"
        );
    }

    #[test]
    fn an_oracle_client_is_mounted_only_when_named_and_real() {
        let dir = tempfile::tempdir().unwrap();
        assert!(oracle_client_args(dir.path()).unwrap().is_empty());

        // Named, but not an Instant Client: said plainly rather than mounted.
        let not_a_client = dir.path().join("empty");
        std::fs::create_dir_all(&not_a_client).unwrap();
        std::fs::write(
            dir.path().join(".env"),
            format!("{ORACLE_CLIENT_VAR}={}\n", not_a_client.display()),
        )
        .unwrap();
        let error = oracle_client_args(dir.path()).unwrap_err();
        assert!(format!("{error:#}").contains("libclntsh.so"), "{error:#}");
    }

    #[test]
    fn the_token_is_appended_after_a_last_line_without_newline() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "PG_URL=postgres://x").unwrap();
        append_dotenv(dir.path(), TOKEN_VAR, "abc123").unwrap();
        let written = std::fs::read_to_string(dir.path().join(".env")).unwrap();
        assert!(written.starts_with("PG_URL=postgres://x\n# "), "{written}");
        assert!(written.ends_with("\nEL_RUNNER_TOKEN=abc123\n"), "{written}");
        assert_eq!(
            el_engine::env::EnvMap::load(dir.path(), None).get(TOKEN_VAR),
            Some("abc123".to_owned())
        );
    }

    /// Live, gated: needs Docker and registry credentials that can read the
    /// image. Runs the whole chain in a throwaway project — licence check,
    /// start, the daemon answering through the `localhost` remote with the
    /// token from `.env`, stop.
    ///
    /// `cargo test -p dbt_ui -- --ignored local_runner_live --nocapture`
    #[test]
    #[ignore]
    fn local_runner_live() {
        smol::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let el = super::super::el_dir(root);
            std::fs::create_dir_all(&el).unwrap();
            std::fs::write(el.join("connections.yml"), "version: 1\nconnections: {}\n").unwrap();

            let image = image(root);
            // With the image already pulled this holds without any registry
            // sign-in: the local copy is checked first.
            assert_eq!(check_access(&image).await, Access::Licensed, "{image}");
            assert_eq!(container(root).await.unwrap(), Container::Missing);

            let started = std::time::Instant::now();
            start(root, &image).await.unwrap();
            eprintln!("started in {:?}", started.elapsed());
            let result = async {
                assert_eq!(container(root).await?, Container::Running);

                // A second project can't have the port: it gets told why, and
                // no dead container is left behind.
                let other = tempfile::tempdir()?;
                std::fs::create_dir_all(super::super::el_dir(other.path()))?;
                let refused = start(other.path(), &image).await.unwrap_err();
                assert!(
                    format!("{refused:#}").contains("already in use"),
                    "{refused:#}"
                );
                assert_eq!(container(other.path()).await?, Container::Missing);
                // Nothing is written for a runner that never ran.
                assert!(!other.path().join(".env").exists());
                assert!(
                    !super::super::el_dir(other.path())
                        .join("remotes.yml")
                        .exists()
                );

                let client = el_engine::server::RemoteClient::connect(root, REMOTE_NAME)?;
                let mut health = client.health();
                for _ in 0..60 {
                    if health.is_ok() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    health = client.health();
                }
                eprintln!("health: {}", health?);
                // Authenticated: proves the token in .env reached the container.
                let pipelines = client.pipelines()?;
                eprintln!("pipelines: {}", pipelines.len());
                anyhow::Ok(())
            }
            .await;
            stop(root).await.unwrap();
            result.unwrap();
            assert_eq!(container(root).await.unwrap(), Container::Missing);
        });
    }
}
