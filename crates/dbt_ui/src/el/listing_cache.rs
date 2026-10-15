//! A Redis cache in front of the explorer's catalog listings, so a large
//! account is listed once rather than on every unfold, Refresh or restart.
//! It is opt-in: without `ELLODE_REDIS_URL` every listing goes to the
//! database as before, and a Redis that is down only costs the connect
//! timeout — a listing never fails because of its cache.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result};
use redis::Commands as _;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest as _, Sha256};

/// Read from the environment or the project's `.env`, like the
/// connections' own variables; the URL may carry a password, so it is
/// never shown.
pub const REDIS_URL_VAR: &str = "ELLODE_REDIS_URL";

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// Housekeeping, not freshness: editing a connection changes its key, and
/// the listing stored under the old one is only ever dropped by expiry.
const TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// One listing the explorer makes.
pub enum Level<'a> {
    /// Every (schema, table) of a connection, in one listing.
    Tables,
    Databases,
    Schemas {
        database: &'a str,
    },
    SchemaTables {
        database: &'a str,
        schema: &'a str,
    },
}

/// What a refresh drops: a whole project, a connection, or one level of a
/// connection's tree together with everything listed under it.
#[derive(Clone, Debug)]
pub enum Scope {
    Project,
    Connection(String),
    Database(String, String),
    Schema(String, String, String),
}

/// Answers `level` from the cache when it holds it; otherwise runs `list`
/// and stores its answer. Failures are never stored.
pub fn read_through<T: Serialize + DeserializeOwned>(
    project_root: &Path,
    connection_name: &str,
    connection: &el_engine::spec::Connection,
    level: Level<'_>,
    list: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let Some(mut redis) = connect(project_root) else {
        return list();
    };
    let key = format!(
        "{}{}",
        connection_prefix(project_root, connection_name, connection),
        level_suffix(&level)
    );
    match redis.get::<_, Option<String>>(&key) {
        Ok(Some(cached)) => match serde_json::from_str(&cached) {
            Ok(value) => return Ok(value),
            // Written by an older shape of the listing: list it again.
            Err(error) => log::warn!("el: ignoring an unreadable cached listing: {error}"),
        },
        Ok(None) => {}
        Err(error) => log::warn!("el: listing cache read failed: {error}"),
    }
    let value = list()?;
    match serde_json::to_string(&value) {
        Ok(json) => {
            if let Err(error) = redis.set_ex::<_, _, ()>(&key, json, TTL_SECS) {
                log::warn!("el: listing cache write failed: {error}");
            }
        }
        Err(error) => log::warn!("el: could not encode a listing for the cache: {error}"),
    }
    Ok(value)
}

/// Drops every cached listing in `scope`. Without a configured Redis there
/// is nothing to drop.
pub fn invalidate(project_root: &Path, scope: &Scope) -> Result<()> {
    let Some(mut redis) = connect(project_root) else {
        return Ok(());
    };
    let pattern = match scope {
        Scope::Project => format!("{}*", glob_escape(&project_prefix(project_root))),
        Scope::Connection(connection) => format!(
            "{}@*",
            glob_escape(&format!(
                "{}{}",
                project_prefix(project_root),
                segment(connection)
            ))
        ),
        Scope::Database(connection, database) => format!(
            "{}@*/{}*",
            glob_escape(&format!(
                "{}{}",
                project_prefix(project_root),
                segment(connection)
            )),
            glob_escape(&database_prefix(database)),
        ),
        Scope::Schema(connection, database, schema) => format!(
            "{}@*/{}",
            glob_escape(&format!(
                "{}{}",
                project_prefix(project_root),
                segment(connection)
            )),
            glob_escape(&level_suffix(&Level::SchemaTables { database, schema })),
        ),
    };
    let keys = redis
        .scan_match::<_, String>(&pattern)
        .context("listing cached listings")?
        .collect::<Result<Vec<String>, _>>()
        .context("listing cached listings")?;
    if !keys.is_empty() {
        redis
            .del::<_, ()>(keys)
            .context("dropping cached listings")?;
    }
    Ok(())
}

fn connect(project_root: &Path) -> Option<redis::Connection> {
    let url = el_engine::env::EnvMap::load(project_root, None).get(REDIS_URL_VAR)?;
    if url.trim().is_empty() {
        return None;
    }
    // The URL is not in these messages: it may carry a password.
    let client = match redis::Client::open(url.trim()) {
        Ok(client) => client,
        Err(error) => {
            log::warn!("el: {REDIS_URL_VAR} is not a usable Redis URL: {error}");
            return None;
        }
    };
    let connection = match client.get_connection_with_timeout(CONNECT_TIMEOUT) {
        Ok(connection) => connection,
        Err(error) => {
            log::warn!("el: listing cache unavailable, listing live: {error}");
            return None;
        }
    };
    if let Err(error) = connection
        .set_read_timeout(Some(IO_TIMEOUT))
        .and_then(|()| connection.set_write_timeout(Some(IO_TIMEOUT)))
    {
        log::warn!("el: listing cache unavailable, listing live: {error}");
        return None;
    }
    Some(connection)
}

/// `ellode:explore:<project>/` — the checkout, hashed so its path is not
/// spelled out in a shared Redis.
fn project_prefix(project_root: &Path) -> String {
    let root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    format!(
        "ellode:explore:{}/",
        short_hash(root.to_string_lossy().as_bytes())
    )
}

/// `<project>/<connection>@<definition>/`. The definition's hash means an
/// edited connection (or a profile that repoints it) misses the cache
/// instead of answering with another database's tables.
fn connection_prefix(
    project_root: &Path,
    connection_name: &str,
    connection: &el_engine::spec::Connection,
) -> String {
    let definition = serde_json::to_vec(connection)
        .map(|json| short_hash(&json))
        .unwrap_or_default();
    format!(
        "{}{}@{definition}/",
        project_prefix(project_root),
        segment(connection_name)
    )
}

fn database_prefix(database: &str) -> String {
    format!("db/{}/", segment(database))
}

fn level_suffix(level: &Level<'_>) -> String {
    match level {
        Level::Tables => "tables".to_owned(),
        Level::Databases => "databases".to_owned(),
        Level::Schemas { database } => format!("{}schemas", database_prefix(database)),
        Level::SchemaTables { database, schema } => format!(
            "{}schema/{}/tables",
            database_prefix(database),
            segment(schema)
        ),
    }
}

/// A name as one key segment: quoted identifiers can hold any character,
/// and an unescaped `/` or `@` would make two names share a key.
fn segment(name: &str) -> String {
    name.replace('%', "%25")
        .replace('/', "%2F")
        .replace('@', "%40")
}

/// Escapes what SCAN's MATCH reads as a pattern.
fn glob_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(character, '*' | '?' | '[' | ']' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn short_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cannot_collide_across_segments() {
        let a = level_suffix(&Level::SchemaTables {
            database: "A/schema/B",
            schema: "C",
        });
        let b = level_suffix(&Level::SchemaTables {
            database: "A",
            schema: "B/schema/C",
        });
        assert_ne!(a, b);
        assert_ne!(segment("a@b"), "a@b");
    }

    #[test]
    fn patterns_match_names_literally() {
        assert_eq!(glob_escape("a*b?[c]\\"), "a\\*b\\?\\[c\\]\\\\");
    }

    #[test]
    fn without_redis_every_listing_is_live() {
        let project = tempfile::tempdir().unwrap();
        let connection = duckdb("a.duckdb");
        let mut calls = 0;
        for _ in 0..2 {
            read_through(project.path(), "src", &connection, Level::Tables, || {
                calls += 1;
                Ok(vec![("main".to_owned(), "orders".to_owned())])
            })
            .unwrap();
        }
        assert_eq!(calls, 2);
        invalidate(project.path(), &Scope::Project).unwrap();
    }

    /// Needs a Redis: `ELLODE_TEST_REDIS_URL=redis://127.0.0.1:6379`.
    #[test]
    #[ignore]
    fn caches_until_its_scope_is_refreshed() {
        let url = std::env::var("ELLODE_TEST_REDIS_URL").expect("set ELLODE_TEST_REDIS_URL");
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".env"),
            format!("{REDIS_URL_VAR}={url}\n"),
        )
        .unwrap();
        let root = project.path();
        let connection = duckdb("a.duckdb");
        let calls = std::cell::Cell::new(0);
        let list = |level: Level<'_>| {
            read_through(root, "s/rc@1", &connection, level, || {
                calls.set(calls.get() + 1);
                Ok(vec!["T".to_owned()])
            })
            .unwrap()
        };
        let schema = || Level::SchemaTables {
            database: "D*",
            schema: "S",
        };
        let other_schema = || Level::SchemaTables {
            database: "D*",
            schema: "S2",
        };

        list(schema());
        list(schema());
        assert_eq!(calls.get(), 1, "the second listing is answered by Redis");

        list(other_schema());
        invalidate(
            root,
            &Scope::Schema("s/rc@1".into(), "D*".into(), "S".into()),
        )
        .unwrap();
        list(schema());
        list(other_schema());
        assert_eq!(
            calls.get(),
            3,
            "a schema refresh leaves its siblings cached"
        );

        list(Level::Databases);
        invalidate(root, &Scope::Database("s/rc@1".into(), "D*".into())).unwrap();
        list(schema());
        list(other_schema());
        list(Level::Databases);
        assert_eq!(calls.get(), 6, "a database refresh drops its schemas only");

        read_through(
            root,
            "s/rc@1",
            &duckdb("b.duckdb"),
            Level::Databases,
            || {
                calls.set(calls.get() + 1);
                Ok(vec!["T".to_owned()])
            },
        )
        .unwrap();
        assert_eq!(calls.get(), 7, "an edited connection misses the cache");

        invalidate(root, &Scope::Connection("s/rc@1".into())).unwrap();
        list(Level::Databases);
        assert_eq!(calls.get(), 8, "a connection refresh drops every level");

        invalidate(root, &Scope::Project).unwrap();
    }

    fn duckdb(path: &str) -> el_engine::spec::Connection {
        serde_json::from_value(serde_json::json!({ "type": "duckdb", "path": path })).unwrap()
    }
}
