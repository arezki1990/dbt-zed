//! Managed dbt Fusion distribution.
//!
//! When no dbt binary is configured or on PATH, zdbt downloads the official
//! dbt Fusion CLI from dbt Labs' CDN into the Zed data directory and runs
//! commands with it. Controlled by the `dbt.auto_install` and
//! `dbt.fusion_version` settings; the same managed binary is picked up by the
//! dbt language server adapter.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use async_compression::futures::bufread::GzipDecoder;
use async_tar::Archive;
use futures::{AsyncReadExt as _, io::BufReader};
use http_client::{AsyncBody, HttpClient};

use crate::dbt_settings::DbtSettings;

const VERSIONS_URL: &str = "https://public.cdn.getdbt.com/fs/versions.json";

pub fn managed_dir() -> PathBuf {
    paths::data_dir().join("dbt-fusion")
}

pub fn managed_binary_path() -> PathBuf {
    managed_dir().join(format!("dbt{}", std::env::consts::EXE_SUFFIX))
}

/// (CDN target triple, archive extension) for this platform.
fn cdn_target() -> Option<(&'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some(("aarch64-apple-darwin", "tar.gz")),
        ("macos", "x86_64") => Some(("x86_64-apple-darwin", "tar.gz")),
        ("linux", "x86_64") => Some(("x86_64-unknown-linux-gnu", "tar.gz")),
        ("linux", "aarch64") => Some(("aarch64-unknown-linux-gnu", "tar.gz")),
        ("windows", "x86_64") => Some(("x86_64-pc-windows-msvc", "zip")),
        _ => None,
    }
}

fn path_has_binary(name: &str) -> bool {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(&file).is_file()))
        .unwrap_or(false)
}

/// Resolves the dbt binary to run: an explicit `dbt.binary` setting wins, then
/// anything on PATH, then the managed install — downloading it first when
/// `dbt.auto_install` is enabled.
pub async fn ensure_binary(
    settings: &DbtSettings,
    http: Option<Arc<dyn HttpClient>>,
) -> Result<String> {
    if settings.binary != "dbt" {
        return Ok(settings.binary.clone());
    }
    if path_has_binary("dbt") {
        return Ok("dbt".to_owned());
    }
    let managed = managed_binary_path();
    if smol::fs::metadata(&managed).await.is_ok() {
        return Ok(managed.to_string_lossy().into_owned());
    }
    anyhow::ensure!(
        settings.auto_install,
        "dbt is not on PATH. Install dbt Fusion (https://docs.getdbt.com), set the \
         `dbt.binary` setting, or enable `dbt.auto_install` to let zdbt download it"
    );
    let http = http.context("no HTTP client available to download dbt Fusion")?;
    install(http.as_ref(), &settings.fusion_version).await?;
    Ok(managed_binary_path().to_string_lossy().into_owned())
}

/// A channel name ("latest", "dev", "canary") resolves through versions.json;
/// anything starting with a digit is used as an explicit version.
async fn resolve_version(http: &dyn HttpClient, requested: &str) -> Result<String> {
    if requested.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return Ok(requested.to_owned());
    }
    let mut response = http
        .get(VERSIONS_URL, AsyncBody::default(), true)
        .await
        .context("fetching dbt Fusion versions.json")?;
    let mut body = String::new();
    response.body_mut().read_to_string(&mut body).await?;
    let json: serde_json::Value =
        serde_json::from_str(&body).context("parsing dbt Fusion versions.json")?;
    let channel = if requested.is_empty() { "latest" } else { requested };
    let tag = json
        .get(channel)
        .and_then(|entry| entry.get("tag"))
        .and_then(|tag| tag.as_str())
        .with_context(|| format!("no '{channel}' channel in dbt Fusion versions.json"))?;
    Ok(tag.trim_start_matches('v').to_owned())
}

async fn install(http: &dyn HttpClient, requested_version: &str) -> Result<()> {
    let (triple, ext) =
        cdn_target().context("dbt Fusion has no prebuilt binary for this platform")?;
    let version = resolve_version(http, requested_version).await?;
    let url = format!("https://public.cdn.getdbt.com/fs/cli/fs-v{version}-{triple}.{ext}");
    log::info!("dbt: downloading dbt Fusion {version} from {url}");

    let dir = managed_dir();
    smol::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("creating {dir:?}"))?;
    let mut response = http
        .get(&url, AsyncBody::default(), true)
        .await
        .with_context(|| format!("downloading {url}"))?;
    anyhow::ensure!(
        response.status().is_success(),
        "downloading dbt Fusion failed: HTTP {} for {url}",
        response.status()
    );
    if ext == "zip" {
        util::archive::extract_zip(&dir, response.body_mut()).await?;
    } else {
        let decompressed = GzipDecoder::new(BufReader::new(response.body_mut()));
        Archive::new(decompressed)
            .unpack(&dir)
            .await
            .context("extracting dbt Fusion archive")?;
    }
    let binary = managed_binary_path();
    anyhow::ensure!(
        smol::fs::metadata(&binary).await.is_ok(),
        "the downloaded dbt Fusion archive did not contain a dbt binary"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        smol::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).await?;
    }
    log::info!("dbt: installed dbt Fusion {version} at {binary:?}");
    Ok(())
}
