//! Environment handling: dotenv loading (relocated verbatim from the dbt
//! panel so both features share one behavior), `${VAR}` template
//! resolution, and the [`Secret`] newtype that keeps resolved credentials
//! out of every Debug/Display path.

use std::fmt;
use std::path::{Path, PathBuf};

/// A resolved credential. Printing it in any form yields `«redacted»`;
/// the only way to the value is the explicit [`Secret::expose`].
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("«redacted»")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("«redacted»")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingVar(pub String);

impl fmt::Display for MissingVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "environment variable {} is not set", self.0)
    }
}

impl std::error::Error for MissingVar {}

/// Real environment merged with project dotenv files; real env wins.
pub struct EnvMap {
    dotenv: Vec<(String, String)>,
}

impl EnvMap {
    pub fn load(project_root: &Path, env_file: Option<&str>) -> Self {
        Self {
            dotenv: load_dotenv(project_root, env_file),
        }
    }

    pub fn empty() -> Self {
        Self { dotenv: Vec::new() }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().or_else(|| {
            self.dotenv
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        })
    }

    /// Whether `key` resolves at all — for validation messages that must
    /// name the variable without touching its value.
    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
}

/// Resolves `${VAR}` placeholders in `input`. The result is a [`Secret`]
/// because any templated value may be (or embed) a credential.
pub fn resolve_templates(input: &str, env: &EnvMap) -> Result<Secret, MissingVar> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let var = &after[..end];
        match env.get(var) {
            Some(value) => out.push_str(&value),
            None => return Err(MissingVar(var.to_owned())),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(Secret::new(out))
}

/// Collects `${VAR}` names appearing in `input` into `refs` — names only.
pub fn collect_var_refs(input: &str, refs: &mut Vec<String>) {
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        let var = &after[..end];
        if !var.is_empty()
            && var
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            refs.push(var.to_owned());
        }
        rest = &after[end + 1..];
    }
}

/// Loads KEY=VALUE pairs from a dotenv-style file (`export` prefixes,
/// quoted values, `#` comments). Variables already exported in the real
/// environment are left untouched; values are never logged.
fn merge_env_file(path: &Path, vars: &mut Vec<(String, String)>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        if std::env::var_os(key).is_some() {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        vars.retain(|(existing, _)| existing != key);
        vars.push((key.to_owned(), value.to_owned()));
    }
}

/// Discovers and loads `.env` / `.env.local` from the project root and its
/// ancestors up to the enclosing git repo root (never past it, at most 5
/// levels); outer files load first so inner override. An explicitly
/// configured `env_file` loads last.
pub fn load_dotenv(root: &Path, env_file: Option<&str>) -> Vec<(String, String)> {
    let mut dirs = vec![root.to_path_buf()];
    let mut cursor = root.to_path_buf();
    let mut found_repo = root.join(".git").exists();
    for _ in 0..5 {
        if found_repo {
            break;
        }
        let Some(parent) = cursor.parent() else {
            break;
        };
        cursor = parent.to_path_buf();
        dirs.push(cursor.clone());
        found_repo = cursor.join(".git").exists();
    }
    if !found_repo {
        dirs.truncate(1);
    }
    dirs.reverse();

    let mut vars: Vec<(String, String)> = Vec::new();
    for dir in &dirs {
        merge_env_file(&dir.join(".env"), &mut vars);
        merge_env_file(&dir.join(".env.local"), &mut vars);
    }
    if let Some(env_file) = env_file {
        let path = if Path::new(env_file).is_absolute() {
            PathBuf::from(env_file)
        } else {
            root.join(env_file)
        };
        if path.is_file() {
            merge_env_file(&path, &mut vars);
        } else {
            log::warn!("el: configured env_file not found: {}", path.display());
        }
    }
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_never_prints_its_value() {
        let secret = Secret::new("hunter2".into());
        assert_eq!(format!("{secret:?}"), "«redacted»");
        assert_eq!(format!("{secret}"), "«redacted»");
        assert_eq!(secret.expose(), "hunter2");
    }

    #[test]
    fn templates_resolve_and_missing_vars_name_only_the_var() {
        let env = EnvMap { dotenv: vec![("MY_HOST".into(), "db.internal".into())] };
        let resolved = resolve_templates("postgres://u@${MY_HOST}/app", &env).unwrap();
        assert_eq!(resolved.expose(), "postgres://u@db.internal/app");

        let missing = resolve_templates("${NOT_SET_ANYWHERE_ZDBT}", &env).unwrap_err();
        assert_eq!(missing.0, "NOT_SET_ANYWHERE_ZDBT");
    }

    #[test]
    fn collect_var_refs_names_only() {
        let mut refs = Vec::new();
        collect_var_refs("url: ${A_URL}\npass: ${B_PASS}\nplain: x", &mut refs);
        assert_eq!(refs, ["A_URL", "B_PASS"]);
    }

    #[test]
    fn real_env_wins_over_dotenv() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "PATH=stomped\nZDBT_EL_T1=fromfile\n")
            .unwrap();
        // No .git anywhere above the temp dir within 5 levels is not
        // guaranteed, so just assert on the two keys we control.
        let vars = load_dotenv(dir.path(), None);
        assert!(vars.iter().all(|(key, _)| key != "PATH"), "real env must win");
        assert!(
            vars.iter()
                .any(|(key, value)| key == "ZDBT_EL_T1" && value == "fromfile")
        );
    }
}
