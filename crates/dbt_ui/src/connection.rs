//! The effective dbt configuration behind the panel's Connection tab: project,
//! executable, profile/target, and the shape of the active target's
//! connection.
//!
//! No value from profiles.yml ever reaches the UI. Only parameter *names* are
//! collected — the values are dropped while parsing and are never stored on
//! [`ConnectionInfo`], so the panel cannot leak a credential even if the
//! rendering code changes later.

use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

use crate::dbt_settings::DbtSettings;

/// Key fragments that mark a block as holding credentials. Matched as
/// substrings of the lowercased key, so `access_token` and `keyfile_json`
/// both hit.
const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "passphrase",
    "secret",
    "token",
    "credential",
    "api_key",
    "apikey",
    "access_key",
    "private_key",
    "keyfile",
    "signature",
    "connection_string",
];

/// Whether a key names a credential.
///
/// Used only to decide whether to descend into a nested block: a credential
/// block is listed under its own name rather than expanded, so the panel
/// never enumerates the innards of a service-account key. Keys that merely
/// *point at* a credential — `private_key_path`, BigQuery's `keyfile` — are
/// ordinary parameters.
fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    if key == "keyfile" || key.ends_with("_path") || key.ends_with("_dir") {
        return false;
    }
    SECRET_KEYS.iter().any(|needle| key.contains(needle))
}

/// Collects the parameter *names* of a target, descending into plain nested
/// blocks with dotted keys. Values are read only far enough to tell a mapping
/// from a scalar, and are never retained.
fn collect_keys(prefix: &str, value: &Value, depth: usize, out: &mut Vec<String>) {
    let Value::Mapping(mapping) = value else {
        return;
    };
    for (key, value) in mapping {
        let Some(key) = key.as_str() else { continue };
        let full = if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            // A credential block is listed whole rather than expanded.
            Value::Mapping(_) | Value::Sequence(_) if is_secret_key(key) => out.push(full),
            Value::Mapping(_) if depth > 0 => collect_keys(&full, value, depth - 1, out),
            _ => out.push(full),
        }
    }
}

/// Everything the Connection tab renders. Field order mirrors the display.
///
/// Note the absence of any parameter *value*: see the module docs.
pub struct ConnectionInfo {
    pub project_name: Option<String>,
    pub project_dir: PathBuf,
    pub binary: String,
    pub binary_source: &'static str,
    pub distribution: String,
    pub profile_name: Option<String>,
    pub profiles_path: Option<PathBuf>,
    pub profiles_source: &'static str,
    pub active_target: Option<String>,
    pub target_source: &'static str,
    pub targets: Vec<String>,
    /// Parameter names configured for the active target — never their values.
    pub param_keys: Vec<String>,
    /// Names only — dotenv values are never read into the UI.
    pub env_names: Vec<String>,
    pub notes: Vec<String>,
}

fn read_yaml(path: &Path) -> Option<Value> {
    serde_yaml_ng::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Locates profiles.yml the way a dbt run here would: the `dbt.profiles_dir`
/// setting or a project-local directory (both of which we pass explicitly as
/// `--profiles-dir`), else dbt's own `DBT_PROFILES_DIR` / `~/.dbt` default.
fn profiles_file(
    settings: &DbtSettings,
    resolved: Option<PathBuf>,
) -> (Option<PathBuf>, &'static str) {
    if let Some(dir) = resolved {
        let source = if settings.profiles_dir.is_some() {
            "dbt.profiles_dir setting"
        } else {
            "project directory"
        };
        let path = dir.join("profiles.yml");
        return (path.is_file().then_some(path), source);
    }
    if let Some(dir) = std::env::var_os("DBT_PROFILES_DIR") {
        let path = PathBuf::from(dir).join("profiles.yml");
        return (path.is_file().then_some(path), "DBT_PROFILES_DIR");
    }
    let path = paths::home_dir().join(".dbt").join("profiles.yml");
    (path.is_file().then_some(path), "~/.dbt (dbt default)")
}

/// Gathers the effective configuration. Pure filesystem reads — no dbt
/// subprocess — so this is cheap enough to refresh on demand.
pub fn collect(
    settings: &DbtSettings,
    root: &Path,
    resolved_profiles_dir: Option<PathBuf>,
    binary: (String, &'static str),
    env_names: Vec<String>,
) -> ConnectionInfo {
    let mut notes = Vec::new();
    let project = read_yaml(&root.join("dbt_project.yml"));
    let project_string = |key: &str| -> Option<String> {
        project.as_ref()?.get(key)?.as_str().map(str::to_owned)
    };

    let (profiles_path, profiles_source) = profiles_file(settings, resolved_profiles_dir);
    let profiles = profiles_path.as_deref().and_then(read_yaml);
    if profiles_path.is_none() {
        notes.push(
            "No profiles.yml found. Set `dbt.profiles_dir`, or add one to the project \
             or ~/.dbt."
                .to_owned(),
        );
    }

    // The profile named by dbt_project.yml; when that is absent a single
    // top-level profile is unambiguous, so use it.
    let profile_name = project_string("profile").or_else(|| {
        let Value::Mapping(mapping) = profiles.as_ref()? else {
            return None;
        };
        let mut keys = mapping.keys().filter_map(|key| key.as_str());
        let only = keys.next()?;
        keys.next().is_none().then(|| only.to_owned())
    });

    let profile = profile_name
        .as_deref()
        .and_then(|name| profiles.as_ref()?.get(name));
    if profiles.is_some() && profile.is_none() {
        if let Some(name) = &profile_name {
            notes.push(format!("Profile \"{name}\" is not in profiles.yml."));
        }
    }

    let outputs = profile.and_then(|profile| profile.get("outputs"));
    let targets: Vec<String> = match outputs {
        Some(Value::Mapping(mapping)) => mapping
            .keys()
            .filter_map(|key| key.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };

    // Precedence mirrors the run: our `--target` flag wins, then the
    // profile's own default, then dbt's DBT_TARGET.
    let (active_target, target_source) = if let Some(target) = settings.target.clone() {
        (Some(target), "dbt.target setting")
    } else if let Some(target) = profile
        .and_then(|profile| profile.get("target"))
        .and_then(|target| target.as_str())
    {
        (Some(target.to_owned()), "profiles.yml")
    } else if let Some(target) = std::env::var("DBT_TARGET").ok().filter(|t| !t.is_empty()) {
        (Some(target), "DBT_TARGET")
    } else {
        (None, "not set")
    };

    let mut param_keys = Vec::new();
    if let (Some(outputs), Some(target)) = (outputs, active_target.as_deref()) {
        match outputs.get(target) {
            Some(output) => collect_keys("", output, 1, &mut param_keys),
            None if !targets.is_empty() => notes.push(format!(
                "Target \"{target}\" is not defined under this profile's outputs."
            )),
            None => {}
        }
    }

    ConnectionInfo {
        project_name: project_string("name"),
        project_dir: root.to_path_buf(),
        binary: binary.0,
        binary_source: binary.1,
        distribution: settings.distribution.clone(),
        profile_name,
        profiles_path,
        profiles_source,
        active_target,
        target_source,
        targets,
        param_keys,
        env_names,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_of(yaml: &str) -> Vec<String> {
        let value: Value = serde_yaml_ng::from_str(yaml).unwrap();
        let mut out = Vec::new();
        collect_keys("", &value, 1, &mut out);
        out
    }

    #[test]
    fn collects_parameter_names_in_file_order() {
        let keys = keys_of(
            r#"
type: snowflake
account: ab12345.eu-west-1
user: analytics_svc
password: hunter2
role: TRANSFORMER
threads: 8
"#,
        );
        assert_eq!(
            keys,
            ["type", "account", "user", "password", "role", "threads"]
        );
    }

    #[test]
    fn no_value_is_ever_collected() {
        // The whole point of the tab: structure without secrets. Values must
        // not appear in the collected output in any form.
        let keys = keys_of(
            r#"
type: snowflake
account: ab12345.eu-west-1
user: analytics_svc
password: hunter2
private_key_passphrase: "{{ env_var('SNOW_PASSPHRASE') }}"
warehouse: WH_XS
"#,
        );
        let rendered = keys.join(" ");
        for value in [
            "hunter2",
            "ab12345",
            "analytics_svc",
            "WH_XS",
            "SNOW_PASSPHRASE",
            "env_var",
            "snowflake",
        ] {
            assert!(
                !rendered.contains(value),
                "value {value:?} leaked into {rendered:?}"
            );
        }
    }

    #[test]
    fn credential_blocks_are_not_expanded() {
        let keys = keys_of(
            r#"
type: bigquery
method: service-account-json
keyfile_json:
  private_key_id: abc
  private_key: "-----BEGIN PRIVATE KEY-----"
  client_email: a@b.iam.gserviceaccount.com
threads: 4
"#,
        );
        assert_eq!(keys, ["type", "method", "keyfile_json", "threads"]);
        // Nothing from inside the credential block is enumerated.
        assert!(keys.iter().all(|key| !key.starts_with("keyfile_json.")));
    }

    #[test]
    fn plain_nested_blocks_are_expanded_with_dotted_keys() {
        let keys = keys_of(
            r#"
type: snowflake
extra:
  retry: 3
  region: eu
"#,
        );
        assert_eq!(keys, ["type", "extra.retry", "extra.region"]);
    }

    #[test]
    fn pointers_to_credentials_are_ordinary_parameters() {
        // A path is a location, not a secret, and it is listed like any
        // other parameter — its value is masked in the panel regardless.
        let keys = keys_of(
            r#"
private_key_path: /home/a/key.p8
keyfile: /home/a/bq.json
"#,
        );
        assert_eq!(keys, ["private_key_path", "keyfile"]);
    }
}

#[cfg(test)]
mod real_project {
    use super::*;

    /// Smoke test against a real profiles.yml, pointed at by
    /// `DBT_PROFILES_SMOKE`. Asserts the file parses and that only parameter
    /// names are produced. Never prints values.
    #[test]
    #[ignore]
    fn collects_names_from_a_real_profile() {
        let path = std::env::var("DBT_PROFILES_SMOKE").expect("set DBT_PROFILES_SMOKE");
        let text = std::fs::read_to_string(&path).expect("read profiles.yml");
        let value: Value = serde_yaml_ng::from_str(&text).expect("parse profiles.yml");
        let Value::Mapping(profiles) = &value else {
            panic!("profiles.yml is not a mapping")
        };
        for (profile, body) in profiles {
            let Some(Value::Mapping(outputs)) = body.get("outputs") else {
                continue;
            };
            for (target, output) in outputs {
                let mut keys = Vec::new();
                collect_keys("", output, 1, &mut keys);
                println!(
                    "{}::{} -> {:?}",
                    profile.as_str().unwrap_or("?"),
                    target.as_str().unwrap_or("?"),
                    keys
                );
                assert!(!keys.is_empty(), "no parameters found for a target");
            }
        }
    }
}
