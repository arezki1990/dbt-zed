//! The loader sidecar protocol: newline-delimited JSON requests on the
//! worker's stdin, responses on stdout. Secrets never cross this channel —
//! the parent puts them in the child's environment (`ZDBT_EL_SF_*`) and
//! the sidecar merges them into the driver options itself.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Env var names the parent sets on the sidecar process. Values never
/// appear in requests, logs, or errors.
pub const ENV_PASSWORD: &str = "ZDBT_EL_SF_PASSWORD";
pub const ENV_PRIVATE_KEY_PATH: &str = "ZDBT_EL_SF_PRIVATE_KEY_PATH";
/// The Oracle target's password. Its user and connect string are
/// locations, not secrets, and travel in `OpenOracle`.
pub const ENV_ORACLE_PASSWORD: &str = "ZDBT_EL_ORACLE_PASSWORD";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Open {
        driver_path: PathBuf,
        account: String,
        user: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        warehouse: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        database: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<String>,
        auth: AuthMethod,
    },
    /// Opens a local DuckDB warehouse file instead of Snowflake.
    OpenDuckdb {
        path: PathBuf,
    },
    /// Opens an Oracle target. The password comes from
    /// [`ENV_ORACLE_PASSWORD`] in the sidecar's environment.
    OpenOracle {
        user: String,
        connect_string: String,
    },
    Exec {
        sql: String,
    },
    QueryScalar {
        sql: String,
    },
    /// Appends the Arrow IPC file at `ipc_path` into `table` (fully
    /// qualified, already quoted).
    Ingest {
        table: String,
        ipc_path: PathBuf,
    },
    /// Appends the Arrow IPC file at `ipc_path` through `insert_sql` — an
    /// array-bound `INSERT … VALUES (:1, :2, …)` the parent generated, so
    /// every statement Oracle runs still comes from `oracle_sql`.
    IngestOracle {
        insert_sql: String,
        ipc_path: PathBuf,
    },
    Shutdown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    /// Private key read from the file named by ZDBT_EL_SF_PRIVATE_KEY_PATH.
    KeyPair,
    /// Password from ZDBT_EL_SF_PASSWORD.
    Password,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows_affected: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scalar: Option<serde_json::Value>,
}

impl Response {
    pub fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            rows_affected: None,
            scalar: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            rows_affected: None,
            scalar: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        let requests = vec![
            Request::Open {
                driver_path: "/tmp/libadbc.dylib".into(),
                account: "org-acct".into(),
                user: "loader".into(),
                role: Some("LOADER".into()),
                warehouse: None,
                database: Some("RAW".into()),
                schema: None,
                auth: AuthMethod::KeyPair,
            },
            Request::Exec {
                sql: "SELECT 1".into(),
            },
            Request::Ingest {
                table: "\"RAW\".\"CRM\".\"T__ZDBT_STAGING\"".into(),
                ipc_path: "/tmp/chunk-000000.ipc".into(),
            },
            Request::OpenOracle {
                user: "loader".into(),
                connect_string: "db.example.com:1521/ORCLPDB1".into(),
            },
            Request::IngestOracle {
                insert_sql: "INSERT INTO \"RAW\".\"T__ZDBT_STAGING\" (\"ID\") VALUES (:1)"
                    .into(),
                ipc_path: "/tmp/chunk-000000.ipc".into(),
            },
            Request::Shutdown,
        ];
        for request in requests {
            let line = serde_json::to_string(&request).unwrap();
            let back: Request = serde_json::from_str(&line).unwrap();
            assert_eq!(
                serde_json::to_string(&back).unwrap(),
                line,
                "round trip of {line}"
            );
            // No secret env NAME should even appear in the wire format.
            assert!(!line.contains("PASSWORD"), "{line}");
        }
    }
}
