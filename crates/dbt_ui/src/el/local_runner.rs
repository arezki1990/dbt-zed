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
    "europe-west4-docker.pkg.dev/jules-terraform/docker/zdbt-el-serve:el-v0.3.0-10-gce77cc2";
/// Environment or `.env` variable overriding the image: another tag, or a
/// provider's other registry.
pub const IMAGE_VAR: &str = "EL_RUNNER_IMAGE";
/// `.env` variable holding the token the container and the remote share.
pub const TOKEN_VAR: &str = "EL_RUNNER_TOKEN";

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

/// Starts (or restarts) the project's runner: makes sure the token exists
/// and the `localhost` remote is declared, then runs the image with the
/// project mounted and the port published on loopback only.
pub async fn start(project_root: &Path, image: &str) -> Result<()> {
    let token = ensure_token(project_root).await?;
    ensure_remote(project_root)?;

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
    let output = docker_command()
        .args([
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
            // Passed by name: the value comes from this process's
            // environment, so the token never appears in `ps`.
            "--env",
            "ZDBT_EL_TOKEN",
            image,
        ])
        .env("ZDBT_EL_TOKEN", &token)
        .output()
        .await
        .context("running docker")?;
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
    Ok(())
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

/// Registers the runner in `el/remotes.yml` as `localhost`, so deploys, the
/// console and connections' `workspace: localhost` reach it like any other
/// server. The token is a `${VAR}` reference; its value lives in `.env`. An
/// existing `localhost` entry is the user's and is left alone.
pub fn ensure_remote(project_root: &Path) -> Result<()> {
    let path = super::el_dir(project_root).join("remotes.yml");
    let mut remotes = match el_engine::spec::load_remotes(&path) {
        Ok(remotes) => remotes,
        Err(el_engine::spec::SpecError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            el_engine::spec::Remotes {
                version: 1,
                remotes: Default::default(),
                extra: Default::default(),
            }
        }
        // A broken file is not an empty one: rewriting it would lose the
        // servers it declares.
        Err(error) => bail!("remotes.yml could not be read: {error} — fix the file first"),
    };
    if remotes.remotes.contains_key(REMOTE_NAME) {
        return Ok(());
    }
    remotes.remotes.insert(
        REMOTE_NAME.to_owned(),
        el_engine::spec::RemoteSpec {
            url: format!("http://127.0.0.1:{PORT}"),
            token: Some(format!("${{{TOKEN_VAR}}}")),
            extra: Default::default(),
        },
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating el/")?;
    }
    std::fs::write(&path, el_engine::spec::to_canonical_remotes_yaml(&remotes))
        .context("writing el/remotes.yml")
}

/// The runner token: the one already set in the environment or `.env`, or a
/// new one written to `.env` once the file is safe from a commit.
async fn ensure_token(project_root: &Path) -> Result<String> {
    if let Some(token) = el_engine::env::EnvMap::load(project_root, None)
        .get(TOKEN_VAR)
        .filter(|token| !token.trim().is_empty())
    {
        return Ok(token);
    }
    protect_dotenv(project_root).await?;
    let bytes: [u8; 32] = rand::random();
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    append_dotenv(project_root, TOKEN_VAR, &token)?;
    Ok(token)
}

/// Keeps `.env` out of git before a token goes into it. A tracked `.env` is
/// refused; an unignored one is added to `.git/info/exclude`, which stays
/// local to this clone rather than changing the team's `.gitignore`. Outside
/// a git checkout there is nothing to protect against.
async fn protect_dotenv(project_root: &Path) -> Result<()> {
    let git = |args: &[&str]| {
        let mut command = util::command::new_command("git");
        command.arg("-C").arg(project_root).args(args);
        command
    };
    let Ok(exclude) = git(&["rev-parse", "--git-path", "info/exclude"])
        .output()
        .await
    else {
        return Ok(());
    };
    if !exclude.status.success() {
        return Ok(());
    }
    let tracked = git(&["ls-files", "--error-unmatch", ".env"])
        .output()
        .await
        .is_ok_and(|output| output.status.success());
    if tracked {
        bail!(
            ".env is tracked by git, so the runner token can't be written there — \
             set {TOKEN_VAR} in your environment instead"
        );
    }
    let ignored = git(&["check-ignore", "--quiet", ".env"])
        .output()
        .await
        .is_ok_and(|output| output.status.success());
    if !ignored {
        let exclude = project_root.join(String::from_utf8_lossy(&exclude.stdout).trim());
        append_line(&exclude, ".env").context("excluding .env from git")?;
    }
    Ok(())
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
fn append_line(path: &Path, line: &str) -> Result<()> {
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

    #[test]
    fn the_localhost_remote_is_added_once_and_keeps_other_servers() {
        let dir = tempfile::tempdir().unwrap();
        let remotes_path = super::super::el_dir(dir.path()).join("remotes.yml");

        ensure_remote(dir.path()).unwrap();
        let first = std::fs::read_to_string(&remotes_path).unwrap();
        let parsed = el_engine::spec::load_remotes(&remotes_path).unwrap();
        let local = &parsed.remotes[REMOTE_NAME];
        assert_eq!(local.url, "http://127.0.0.1:7431");
        assert_eq!(local.token.as_deref(), Some("${EL_RUNNER_TOKEN}"));
        // The loopback URL is one the remote client accepts over plain http.
        el_engine::server::check_remote_url(&local.url).unwrap();

        ensure_remote(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&remotes_path).unwrap(), first);

        std::fs::write(
            &remotes_path,
            "version: 1\nremotes:\n  vm:\n    url: http://127.0.0.1:17431\n",
        )
        .unwrap();
        ensure_remote(dir.path()).unwrap();
        let parsed = el_engine::spec::load_remotes(&remotes_path).unwrap();
        assert!(parsed.remotes.contains_key("vm"));
        assert!(parsed.remotes.contains_key(REMOTE_NAME));
    }

    #[test]
    fn a_broken_remotes_file_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let remotes_path = super::super::el_dir(dir.path()).join("remotes.yml");
        std::fs::create_dir_all(remotes_path.parent().unwrap()).unwrap();
        std::fs::write(&remotes_path, "remotes: [this is: not valid").unwrap();
        assert!(ensure_remote(dir.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(&remotes_path).unwrap(),
            "remotes: [this is: not valid"
        );
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
