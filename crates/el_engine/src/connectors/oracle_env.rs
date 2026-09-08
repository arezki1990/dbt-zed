//! How resolved Oracle credentials reach the worker. Oracle connections
//! carry discrete fields (user / password / connect string) rather than
//! one URL, so they travel as three named environment variables on the
//! child process — never argv, never a log line. This module is
//! driver-free: the app side (which resolves `${VAR}`s and spawns) and
//! the worker side (which reads them back) share it whether or not the
//! `oracle` feature is on.

use std::process::Command;

use anyhow::{Context as _, Result};

use crate::env::{EnvMap, Secret};
use crate::spec::OracleConn;

pub const ENV_USER: &str = "ZDBT_EL_SRC_ORACLE_USER";
pub const ENV_PASSWORD: &str = "ZDBT_EL_SRC_ORACLE_PASSWORD";
pub const ENV_CONNECT: &str = "ZDBT_EL_SRC_ORACLE_CONNECT";
/// Oracle's own variable: the directory holding `tnsnames.ora`,
/// `sqlnet.ora` and (for Autonomous Database) the wallet. ODPI-C reads it
/// from the process environment, so we set it on the child directly.
pub const ENV_TNS_ADMIN: &str = "TNS_ADMIN";

/// A connection's credentials with every `${VAR}` resolved. Values are
/// `Secret`s: they print as «redacted» and only reach the child's
/// environment.
pub struct OracleCreds {
    pub user: Secret,
    pub password: Secret,
    pub connect: Secret,
    /// `tns_admin` when set, else `wallet_dir` — an Autonomous Database
    /// wallet directory is also where its `tnsnames.ora` lives.
    pub tns_admin: Option<Secret>,
}

impl OracleCreds {
    pub fn resolve(conn: &OracleConn, env: &EnvMap) -> Result<Self> {
        let resolve = |value: &str| -> Result<Secret> {
            crate::env::resolve_templates(value, env)
                .map_err(|missing| anyhow::anyhow!("{missing}"))
        };
        let tns_admin = conn.tns_admin.as_deref().or(conn.wallet_dir.as_deref());
        Ok(Self {
            user: resolve(&conn.user)?,
            password: resolve(&conn.password)?,
            connect: resolve(&conn.connect)?,
            tns_admin: tns_admin.map(resolve).transpose()?,
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
    }
}

/// The schema (owner) a stream reads from when it names none: the
/// connection's `schema`, else the user's own schema.
pub fn default_schema(conn: &OracleConn, env: &EnvMap) -> Result<String> {
    let resolved = crate::env::resolve_templates(conn.effective_schema(), env)
        .map_err(|missing| anyhow::anyhow!("{missing}"))?;
    Ok(resolved.expose().to_owned())
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
