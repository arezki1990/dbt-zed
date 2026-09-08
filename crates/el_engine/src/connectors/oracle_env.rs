//! How resolved Oracle credentials reach the worker. Oracle connections
//! carry discrete fields (user / password / connect string) rather than
//! one URL, so they travel as three named environment variables on the
//! child process — never argv, never a log line. This module is
//! driver-free: the app side (which resolves `${VAR}`s and spawns) and
//! the worker side (which reads them back) share it whether or not the
//! `oracle` feature is on.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result};

use crate::env::{EnvMap, Secret};
use crate::spec::{OracleConn, OracleDriver};

pub const ENV_USER: &str = "ZDBT_EL_SRC_ORACLE_USER";
pub const ENV_PASSWORD: &str = "ZDBT_EL_SRC_ORACLE_PASSWORD";
pub const ENV_CONNECT: &str = "ZDBT_EL_SRC_ORACLE_CONNECT";
/// Oracle's own variable: the directory holding `tnsnames.ora`,
/// `sqlnet.ora` and (for Autonomous Database) the wallet. ODPI-C reads it
/// from the process environment, so we set it on the child directly.
pub const ENV_TNS_ADMIN: &str = "TNS_ADMIN";
/// Which driver the worker connects with: `auto`, `thin` or `thick`.
pub const ENV_DRIVER: &str = "ZDBT_EL_ORACLE_DRIVER";
/// The directory holding Oracle Instant Client, for the thick driver.
/// Handed to ODPI-C explicitly rather than trusting the loader's search
/// path (a hardened runtime strips `LD_*` / `DYLD_*` from a signed app's
/// children). Set in the process environment or the project's `.env`.
pub const ENV_CLIENT_DIR: &str = "ZDBT_EL_ORACLE_CLIENT_DIR";
/// Opt-in to the thick driver on an Apple Silicon Mac, where Oracle's
/// only Instant Client build crashes at connect (Oracle bug 36790189).
pub const ENV_TRY_MACOS_CLIENT: &str = "ZDBT_EL_ORACLE_TRY_MACOS_CLIENT";

/// A connection's credentials with every `${VAR}` resolved. Values are
/// `Secret`s: they print as «redacted» and only reach the child's
/// environment.
pub struct OracleCreds {
    pub user: Secret,
    pub password: Secret,
    pub connect: Secret,
    /// `tns_admin` when set, else `wallet_dir` — an Autonomous Database
    /// wallet directory is also where its `tnsnames.ora` lives. Relative
    /// paths were resolved against the project root.
    pub tns_admin: Option<Secret>,
    pub driver: OracleDriver,
    /// Thick-driver settings the project's `.env` (or the real
    /// environment) carries for the worker: [`ENV_CLIENT_DIR`],
    /// [`ENV_TRY_MACOS_CLIENT`]. Locations and flags, never secrets.
    pub worker_settings: Vec<(&'static str, String)>,
}

impl OracleCreds {
    pub fn resolve(conn: &OracleConn, env: &EnvMap, project_root: &Path) -> Result<Self> {
        let resolve = |value: &str| -> Result<Secret> {
            crate::env::resolve_templates(value, env)
                .map_err(|missing| anyhow::anyhow!("{missing}"))
        };
        let tns_admin = conn.tns_admin.as_deref().or(conn.wallet_dir.as_deref());
        // A wallet or tnsnames directory is a path like every other path
        // in a spec: relative to the project, as the connection form's
        // `el/wallet` placeholder suggests.
        let tns_admin = tns_admin
            .map(|dir| resolve(dir).map(|dir| Secret::new(anchor(project_root, dir.expose()))))
            .transpose()?;
        let worker_settings = [ENV_CLIENT_DIR, ENV_TRY_MACOS_CLIENT]
            .into_iter()
            .filter_map(|name| env.get(name).map(|value| (name, value)))
            .collect();
        Ok(Self {
            user: resolve(&conn.user)?,
            password: resolve(&conn.password)?,
            connect: resolve(&conn.connect)?,
            tns_admin,
            driver: conn.driver(),
            worker_settings,
        })
    }

    /// Puts the credentials in a worker command's environment.
    pub fn apply(&self, command: &mut Command) {
        command
            .env(ENV_USER, self.user.expose())
            .env(ENV_PASSWORD, self.password.expose())
            .env(ENV_CONNECT, self.connect.expose());
        if let Some(dir) = &self.tns_admin {
            command.env(ENV_TNS_ADMIN, dir.expose());
        }
        self.apply_driver_settings(command);
    }

    /// The driver choice and thick-driver settings only — what the loader
    /// sidecar needs besides the password it receives on its own channel.
    pub fn apply_driver_settings(&self, command: &mut Command) {
        command.env(ENV_DRIVER, self.driver.as_str());
        for (name, value) in &self.worker_settings {
            command.env(name, value);
        }
    }
}

fn anchor(project_root: &Path, dir: &str) -> String {
    let path = PathBuf::from(dir);
    if path.is_absolute() {
        dir.to_owned()
    } else {
        project_root.join(path).to_string_lossy().into_owned()
    }
}

/// The schema (owner) a stream reads from when it names none: the
/// connection's `schema`, else the user's own schema.
pub fn default_schema(conn: &OracleConn, env: &EnvMap) -> Result<String> {
    let resolved = crate::env::resolve_templates(conn.effective_schema(), env)
        .map_err(|missing| anyhow::anyhow!("{missing}"))?;
    Ok(resolved.expose().to_owned())
}

/// The driver the worker was told to use; `auto` when nothing said.
pub fn driver_from_env() -> OracleDriver {
    std::env::var(ENV_DRIVER)
        .ok()
        .and_then(|value| OracleDriver::parse(&value))
        .unwrap_or_default()
}

/// Where the worker was told the Instant Client lives, if anywhere.
pub fn client_dir_from_env() -> Option<String> {
    std::env::var(ENV_CLIENT_DIR)
        .ok()
        .filter(|dir| !dir.trim().is_empty())
}

/// The worker side of the same contract. Errors name the variable, never
/// a value.
pub struct WorkerCreds {
    pub user: String,
    pub password: String,
    pub connect: String,
}

pub fn creds_from_env() -> Result<WorkerCreds> {
    let read = |name: &str| {
        std::env::var(name)
            .with_context(|| format!("{name} is not set in the worker environment"))
    };
    Ok(WorkerCreds {
        user: read(ENV_USER)?,
        password: read(ENV_PASSWORD)?,
        connect: read(ENV_CONNECT)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(tns_admin: Option<&str>, wallet_dir: Option<&str>) -> OracleConn {
        OracleConn {
            user: "${U}".to_owned(),
            password: "${P}".to_owned(),
            connect: "db:1521/svc".to_owned(),
            schema: None,
            wallet_dir: wallet_dir.map(str::to_owned),
            tns_admin: tns_admin.map(str::to_owned),
            driver: None,
            extra: Default::default(),
        }
    }

    fn env() -> EnvMap {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "U=scott\nP=tiger\n").unwrap();
        EnvMap::load(dir.path(), None)
    }

    #[test]
    fn wallet_and_tns_dirs_anchor_on_the_project() {
        let root = Path::new("/proj");
        let creds = OracleCreds::resolve(&conn(None, Some("el/wallet")), &env(), root).unwrap();
        assert_eq!(creds.tns_admin.unwrap().expose(), "/proj/el/wallet");
        // tns_admin wins over wallet_dir; an absolute path is left alone.
        let creds =
            OracleCreds::resolve(&conn(Some("/etc/tns"), Some("el/wallet")), &env(), root)
                .unwrap();
        assert_eq!(creds.tns_admin.unwrap().expose(), "/etc/tns");
        assert!(
            OracleCreds::resolve(&conn(None, None), &env(), root)
                .unwrap()
                .tns_admin
                .is_none()
        );
    }

    #[test]
    fn credentials_travel_as_environment_only() {
        let creds = OracleCreds::resolve(&conn(None, None), &env(), Path::new("/proj")).unwrap();
        let mut command = Command::new("true");
        creds.apply(&mut command);
        let set: Vec<String> = command
            .get_envs()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        for name in [ENV_USER, ENV_PASSWORD, ENV_CONNECT, ENV_DRIVER] {
            assert!(set.iter().any(|n| n == name), "{name} not set: {set:?}");
        }
        assert!(!set.iter().any(|n| n == ENV_CLIENT_DIR), "unset setting forwarded: {set:?}");
        assert!(!set.iter().any(|n| n == ENV_TNS_ADMIN), "{set:?}");
        assert!(command.get_args().count() == 0, "nothing on argv");
    }
}
