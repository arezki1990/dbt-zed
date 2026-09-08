//! Oracle dialect SQL for the warehouse loader: staging DDL from the same
//! `SnowflakeType` vocabulary every other target uses, the full-refresh
//! swap, the incremental MERGE, and the watermark read-back. Driver-free
//! and golden-tested — no database is needed to know what we send.
//!
//! Three dialect facts shape everything here:
//!
//! - Oracle before 23ai has no `IF EXISTS`, so every drop and the
//!   create-target-once statement runs inside a PL/SQL block that swallows
//!   exactly one error code (`-942` table does not exist, `-955` name
//!   already used) and re-raises anything else.
//! - Oracle has no `CREATE OR REPLACE TABLE` and no `CLONE`, so a full
//!   refresh publishes by RENAME: the live table steps aside as
//!   `…__ZDBT_OLD`, staging takes its name, and the old copy is dropped.
//!   That is O(1) and keeps OUR DDL, at the cost of a sub-second window
//!   where a concurrent reader sees ORA-00942, and of grants, synonyms and
//!   indexes that were attached to the old table. The transactional
//!   alternative (`DELETE` + `INSERT … SELECT` in one transaction) is
//!   atomic for readers but rewrites every row through undo; we chose the
//!   swap, as the other targets also republish rather than rewrite.
//! - Oracle has no `QUALIFY`, so the MERGE dedupes staging with an inline
//!   `ROW_NUMBER()` subquery (latest `update_key` wins), and its `ON`
//!   clause must be parenthesised.
//!
//! Identifiers follow the source connector's rule: written unquoted in the
//! spec they fold to upper case exactly as Oracle folds them; quote them in
//! the spec to keep a case-sensitive name. A name carrying a double quote
//! is rejected, never escaped. `StreamPlan::database` is ignored — an
//! Oracle target is addressed as `"SCHEMA"."TABLE"`.

use anyhow::{Result, bail};

use crate::oracle_types::{OracleDialect, normalize_ident, quote_ident};
use crate::types::{SfBase, SnowflakeType};

/// Suffix of the per-stream staging table.
pub const STAGING_SUFFIX: &str = "__ZDBT_STAGING";
/// Suffix the live table wears for the moment between the two renames.
pub const OLD_SUFFIX: &str = "__ZDBT_OLD";
/// The dedupe helper column inside the MERGE subquery.
const ROW_NUMBER_COLUMN: &str = "ZDBT_RN";
/// Oracle's identifier limit since 12.2 (30 bytes before that).
const MAX_IDENT: usize = 128;
/// ORA-00942: table or view does not exist.
const ORA_TABLE_MISSING: i32 = -942;
/// ORA-00955: name is already used by an existing object.
const ORA_NAME_IN_USE: i32 = -955;

/// The three table names one stream uses, already folded and checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Names {
    schema: String,
    target: String,
    staging: String,
    old: String,
}

impl Names {
    fn quoted(&self, table: &str) -> String {
        format!("\"{}\".\"{}\"", self.schema, table)
    }

    /// `"SCHEMA"."TABLE"`.
    pub fn target_fqn(&self) -> String {
        self.quoted(&self.target)
    }

    /// `"SCHEMA"."TABLE__ZDBT_STAGING"`.
    pub fn staging_fqn(&self) -> String {
        self.quoted(&self.staging)
    }

    fn old_fqn(&self) -> String {
        self.quoted(&self.old)
    }
}

/// Folds and checks a stream's target names. Oracle's identifier limit
/// applies to the staging name too, so a target within the limit whose
/// staging name is not is rejected here rather than at load time.
pub fn names(schema: &str, target_table: &str) -> Result<Names> {
    let schema = normalize_ident(schema)?;
    let target = normalize_ident(target_table)?;
    if target.len() + STAGING_SUFFIX.len() > MAX_IDENT {
        bail!(
            "oracle target table {target:?} is too long: with the {STAGING_SUFFIX} \
             suffix it passes Oracle's {MAX_IDENT}-character identifier limit"
        );
    }
    Ok(Names {
        staging: format!("{target}{STAGING_SUFFIX}"),
        old: format!("{target}{OLD_SUFFIX}"),
        schema,
        target,
    })
}

/// Wraps one statement in a block that swallows a single Oracle error and
/// re-raises everything else — Oracle's stand-in for `IF EXISTS`.
fn guarded(sql: &str, ignore_code: i32) -> String {
    format!(
        "BEGIN EXECUTE IMMEDIATE '{}'; \
         EXCEPTION WHEN OTHERS THEN IF SQLCODE != {ignore_code} THEN RAISE; END IF; END;",
        sql.replace('\'', "''")
    )
}

fn column_ddl(columns: &[(String, SnowflakeType)], dialect: &OracleDialect) -> Result<String> {
    let mut parts = Vec::with_capacity(columns.len());
    for (name, sf_type) in columns {
        parts.push(format!("{} {}", quote_ident(name)?, dialect.ddl_type(sf_type)));
    }
    Ok(parts.join(", "))
}

fn quoted_names(columns: &[(String, SnowflakeType)]) -> Result<Vec<String>> {
    columns.iter().map(|(name, _)| quote_ident(name)).collect()
}

/// Drops staging if it is there. `PURGE` keeps the recycle bin (and the
/// table explorer) clean.
pub fn drop_staging(schema: &str, target_table: &str) -> Result<String> {
    let names = names(schema, target_table)?;
    Ok(guarded(
        &format!("DROP TABLE {} PURGE", names.staging_fqn()),
        ORA_TABLE_MISSING,
    ))
}

/// Creates the staging table with OUR DDL. Two statements: a leftover from
/// a killed run is dropped first, because Oracle cannot replace a table.
pub fn create_staging(
    schema: &str,
    target_table: &str,
    columns: &[(String, SnowflakeType)],
    dialect: &OracleDialect,
) -> Result<Vec<String>> {
    let names = names(schema, target_table)?;
    Ok(vec![
        guarded(
            &format!("DROP TABLE {} PURGE", names.staging_fqn()),
            ORA_TABLE_MISSING,
        ),
        format!(
            "CREATE TABLE {} ({})",
            names.staging_fqn(),
            column_ddl(columns, dialect)?
        ),
    ])
}

/// First incremental run: the target must exist before MERGE.
pub fn create_target_if_not_exists(
    schema: &str,
    target_table: &str,
    columns: &[(String, SnowflakeType)],
    dialect: &OracleDialect,
) -> Result<String> {
    let names = names(schema, target_table)?;
    Ok(guarded(
        &format!(
            "CREATE TABLE {} ({})",
            names.target_fqn(),
            column_ddl(columns, dialect)?
        ),
        ORA_NAME_IN_USE,
    ))
}

/// The array-bound INSERT one staged chunk is loaded with. Values are
/// positional binds — no literal ever reaches the statement text.
pub fn insert_staging(
    schema: &str,
    target_table: &str,
    columns: &[(String, SnowflakeType)],
) -> Result<String> {
    if columns.is_empty() {
        bail!("cannot load a stream with no columns");
    }
    let names = names(schema, target_table)?;
    let binds = (1..=columns.len())
        .map(|position| format!(":{position}"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "INSERT INTO {} ({}) VALUES ({binds})",
        names.staging_fqn(),
        quoted_names(columns)?.join(", ")
    ))
}

/// Full-refresh commit: publish staging by renaming it over the target.
/// The steps must run in this order; each is a separate statement because
/// Oracle DDL commits as it goes.
pub fn full_refresh_swap(schema: &str, target_table: &str) -> Result<Vec<String>> {
    let names = names(schema, target_table)?;
    let drop_old = guarded(
        &format!("DROP TABLE {} PURGE", names.old_fqn()),
        ORA_TABLE_MISSING,
    );
    Ok(vec![
        // A previous run killed mid-swap could have left one behind.
        drop_old.clone(),
        // First run: there is no target yet, which is not an error.
        guarded(
            &format!(
                "ALTER TABLE {} RENAME TO \"{}\"",
                names.target_fqn(),
                names.old
            ),
            ORA_TABLE_MISSING,
        ),
        format!(
            "ALTER TABLE {} RENAME TO \"{}\"",
            names.staging_fqn(),
            names.target
        ),
        drop_old,
    ])
}

/// Incremental commit: staging deduped per key (latest cursor wins) is
/// merged into the target. Oracle has no `QUALIFY`, so the dedupe is an
/// inline `ROW_NUMBER()` subquery.
pub fn merge(
    schema: &str,
    target_table: &str,
    columns: &[(String, SnowflakeType)],
    primary_key: &[String],
    update_key: &str,
    dialect: &OracleDialect,
) -> Result<String> {
    if primary_key.is_empty() {
        bail!("an incremental oracle load needs a primary key to merge on");
    }
    let names = names(schema, target_table)?;
    // A LOB cannot be compared, so it can be neither the key we match on
    // nor the cursor we order by — catch it here, with the column named.
    for key in primary_key
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(update_key))
    {
        reject_lob_key(columns, key, dialect)?;
    }
    let pk_list = primary_key
        .iter()
        .map(|key| quote_ident(key))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let on = primary_key
        .iter()
        .map(|key| quote_ident(key).map(|key| format!("t.{key} = s.{key}")))
        .collect::<Result<Vec<_>>>()?
        .join(" AND ");
    let all_columns = quoted_names(columns)?;
    let insert_values = all_columns
        .iter()
        .map(|column| format!("s.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let non_pk: Vec<&String> = {
        let keys = primary_key
            .iter()
            .map(|key| normalize_ident(key))
            .collect::<Result<Vec<_>>>()?;
        let mut kept = Vec::new();
        for (index, (name, _)) in columns.iter().enumerate() {
            if !keys.contains(&normalize_ident(name)?) {
                kept.push(&all_columns[index]);
            }
        }
        kept
    };
    let dedupe = format!(
        "SELECT {columns} FROM (SELECT s.*, \
         ROW_NUMBER() OVER (PARTITION BY {pk_list} ORDER BY {uk} DESC) AS \"{ROW_NUMBER_COLUMN}\" \
         FROM {staging} s) WHERE \"{ROW_NUMBER_COLUMN}\" = 1",
        columns = all_columns.join(", "),
        uk = quote_ident(update_key)?,
        staging = names.staging_fqn(),
    );
    // A stream whose columns are all key columns has nothing to update;
    // an empty SET list is a syntax error, so that MERGE only inserts.
    let matched = if non_pk.is_empty() {
        String::new()
    } else {
        format!(
            " WHEN MATCHED THEN UPDATE SET {}",
            non_pk
                .iter()
                .map(|column| format!("t.{column} = s.{column}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Ok(format!(
        "MERGE INTO {target} t USING ({dedupe}) s ON ({on}){matched} \
         WHEN NOT MATCHED THEN INSERT ({columns}) VALUES ({insert_values})",
        target = names.target_fqn(),
        columns = all_columns.join(", "),
    ))
}

/// A key column must be comparable: CLOB and BLOB are not.
fn reject_lob_key(
    columns: &[(String, SnowflakeType)],
    key: &str,
    dialect: &OracleDialect,
) -> Result<()> {
    let key_name = normalize_ident(key)?;
    for (name, sf_type) in columns {
        if normalize_ident(name)? != key_name {
            continue;
        }
        let ddl = dialect.ddl_type(sf_type);
        if ddl == "CLOB" || ddl == "BLOB" {
            bail!(
                "column {name:?} loads as {ddl} in Oracle and cannot be a primary key \
                 or update key — cast it to a shorter type in the stream"
            );
        }
    }
    Ok(())
}

/// The cursor read back from the TARGET after commit, printed in the
/// format `state::WatermarkValue::parse_scalar` expects. Timestamps carry
/// no zone once stored, so a zoned column is normalised to UTC first.
pub fn max_scalar(
    schema: &str,
    target_table: &str,
    column: &str,
    sf_type: &SnowflakeType,
) -> Result<String> {
    let names = names(schema, target_table)?;
    let column = quote_ident(column)?;
    let value = match sf_type.base {
        SfBase::Date => format!("TO_CHAR(MAX({column}), 'YYYY-MM-DD')"),
        SfBase::TimestampNtz => format!("TO_CHAR(MAX({column}), 'YYYY-MM-DD HH24:MI:SS.FF6')"),
        SfBase::TimestampTz => format!(
            "TO_CHAR(SYS_EXTRACT_UTC(MAX({column})), 'YYYY-MM-DD HH24:MI:SS.FF6')"
        ),
        // 'TM9' is the shortest exact decimal form; the NLS override keeps
        // the separator a dot whatever the server's default is.
        SfBase::Number | SfBase::Float => format!(
            "TO_CHAR(MAX({column}), 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,''')"
        ),
        _ => format!("MAX({column})"),
    };
    Ok(format!("SELECT {value} FROM {}", names.target_fqn()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<(String, SnowflakeType)> {
        vec![
            ("id".to_owned(), "NUMBER(38,0)".parse().unwrap()),
            ("amount".to_owned(), "NUMBER(18,2)".parse().unwrap()),
            ("note".to_owned(), "VARCHAR".parse().unwrap()),
            ("updated_at".to_owned(), "TIMESTAMP_NTZ".parse().unwrap()),
        ]
    }

    #[test]
    fn staging_ddl_is_oracle_shaped() {
        let dialect = OracleDialect::default();
        assert_eq!(
            create_staging("landing", "orders", &columns(), &dialect).unwrap(),
            vec![
                "BEGIN EXECUTE IMMEDIATE 'DROP TABLE \"LANDING\".\"ORDERS__ZDBT_STAGING\" PURGE'; \
                 EXCEPTION WHEN OTHERS THEN IF SQLCODE != -942 THEN RAISE; END IF; END;"
                    .to_owned(),
                "CREATE TABLE \"LANDING\".\"ORDERS__ZDBT_STAGING\" (\"ID\" NUMBER(38,0), \
                 \"AMOUNT\" NUMBER(18,2), \"NOTE\" VARCHAR2(4000), \"UPDATED_AT\" TIMESTAMP(6))"
                    .to_owned(),
            ]
        );
        assert_eq!(
            create_target_if_not_exists("landing", "orders", &columns(), &dialect).unwrap(),
            "BEGIN EXECUTE IMMEDIATE 'CREATE TABLE \"LANDING\".\"ORDERS\" \
             (\"ID\" NUMBER(38,0), \"AMOUNT\" NUMBER(18,2), \"NOTE\" VARCHAR2(4000), \
             \"UPDATED_AT\" TIMESTAMP(6))'; EXCEPTION WHEN OTHERS THEN \
             IF SQLCODE != -955 THEN RAISE; END IF; END;"
        );
        assert_eq!(
            drop_staging("landing", "orders").unwrap(),
            "BEGIN EXECUTE IMMEDIATE 'DROP TABLE \"LANDING\".\"ORDERS__ZDBT_STAGING\" PURGE'; \
             EXCEPTION WHEN OTHERS THEN IF SQLCODE != -942 THEN RAISE; END IF; END;"
        );
    }

    #[test]
    fn insert_binds_every_value() {
        assert_eq!(
            insert_staging("landing", "orders", &columns()).unwrap(),
            "INSERT INTO \"LANDING\".\"ORDERS__ZDBT_STAGING\" \
             (\"ID\", \"AMOUNT\", \"NOTE\", \"UPDATED_AT\") VALUES (:1, :2, :3, :4)"
        );
        assert!(insert_staging("landing", "orders", &[]).is_err());
    }

    #[test]
    fn full_refresh_swaps_by_rename() {
        assert_eq!(
            full_refresh_swap("landing", "orders").unwrap(),
            vec![
                "BEGIN EXECUTE IMMEDIATE 'DROP TABLE \"LANDING\".\"ORDERS__ZDBT_OLD\" PURGE'; \
                 EXCEPTION WHEN OTHERS THEN IF SQLCODE != -942 THEN RAISE; END IF; END;"
                    .to_owned(),
                "BEGIN EXECUTE IMMEDIATE 'ALTER TABLE \"LANDING\".\"ORDERS\" \
                 RENAME TO \"ORDERS__ZDBT_OLD\"'; EXCEPTION WHEN OTHERS THEN \
                 IF SQLCODE != -942 THEN RAISE; END IF; END;"
                    .to_owned(),
                "ALTER TABLE \"LANDING\".\"ORDERS__ZDBT_STAGING\" RENAME TO \"ORDERS\"".to_owned(),
                "BEGIN EXECUTE IMMEDIATE 'DROP TABLE \"LANDING\".\"ORDERS__ZDBT_OLD\" PURGE'; \
                 EXCEPTION WHEN OTHERS THEN IF SQLCODE != -942 THEN RAISE; END IF; END;"
                    .to_owned(),
            ]
        );
    }

    #[test]
    fn merge_dedupes_with_row_number() {
        let dialect = OracleDialect::default();
        assert_eq!(
            merge(
                "landing",
                "orders",
                &columns(),
                &["id".to_owned()],
                "updated_at",
                &dialect
            )
            .unwrap(),
            "MERGE INTO \"LANDING\".\"ORDERS\" t USING (SELECT \"ID\", \"AMOUNT\", \"NOTE\", \
             \"UPDATED_AT\" FROM (SELECT s.*, ROW_NUMBER() OVER (PARTITION BY \"ID\" \
             ORDER BY \"UPDATED_AT\" DESC) AS \"ZDBT_RN\" FROM \
             \"LANDING\".\"ORDERS__ZDBT_STAGING\" s) WHERE \"ZDBT_RN\" = 1) s \
             ON (t.\"ID\" = s.\"ID\") WHEN MATCHED THEN UPDATE SET t.\"AMOUNT\" = s.\"AMOUNT\", \
             t.\"NOTE\" = s.\"NOTE\", t.\"UPDATED_AT\" = s.\"UPDATED_AT\" \
             WHEN NOT MATCHED THEN INSERT (\"ID\", \"AMOUNT\", \"NOTE\", \"UPDATED_AT\") \
             VALUES (s.\"ID\", s.\"AMOUNT\", s.\"NOTE\", s.\"UPDATED_AT\")"
        );
    }

    /// Every column a key: the MERGE has nothing to update and must not
    /// emit an empty SET list.
    #[test]
    fn merge_without_updatable_columns_only_inserts() {
        let dialect = OracleDialect::default();
        let columns = vec![
            ("id".to_owned(), "NUMBER(38,0)".parse().unwrap()),
            ("seen_at".to_owned(), "TIMESTAMP_NTZ".parse().unwrap()),
        ];
        let sql = merge(
            "landing",
            "hits",
            &columns,
            &["id".to_owned(), "seen_at".to_owned()],
            "seen_at",
            &dialect,
        )
        .unwrap();
        assert!(!sql.contains("WHEN MATCHED"), "{sql}");
        assert!(sql.contains("WHEN NOT MATCHED THEN INSERT"), "{sql}");
    }

    #[test]
    fn merge_rejects_lob_and_keyless_streams() {
        let dialect = OracleDialect::default();
        let columns = vec![
            ("id".to_owned(), "NUMBER(38,0)".parse().unwrap()),
            ("body".to_owned(), "VARCHAR(100000)".parse().unwrap()),
        ];
        let error = merge(
            "landing",
            "docs",
            &columns,
            &["body".to_owned()],
            "id",
            &dialect,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("CLOB"), "{error}");
        assert!(merge("landing", "docs", &columns, &[], "id", &dialect).is_err());
    }

    /// The read-back text must be exactly what the state store parses.
    #[test]
    fn max_scalar_matches_the_watermark_parser() {
        use crate::state::WatermarkValue;

        let timestamp: SnowflakeType = "TIMESTAMP_NTZ".parse().unwrap();
        assert_eq!(
            max_scalar("landing", "orders", "updated_at", &timestamp).unwrap(),
            "SELECT TO_CHAR(MAX(\"UPDATED_AT\"), 'YYYY-MM-DD HH24:MI:SS.FF6') \
             FROM \"LANDING\".\"ORDERS\""
        );
        assert_eq!(
            max_scalar(
                "landing",
                "orders",
                "seen_at",
                &"TIMESTAMP_TZ".parse().unwrap()
            )
            .unwrap(),
            "SELECT TO_CHAR(SYS_EXTRACT_UTC(MAX(\"SEEN_AT\")), 'YYYY-MM-DD HH24:MI:SS.FF6') \
             FROM \"LANDING\".\"ORDERS\""
        );
        assert_eq!(
            max_scalar("landing", "orders", "id", &"NUMBER(38,0)".parse().unwrap()).unwrap(),
            "SELECT TO_CHAR(MAX(\"ID\"), 'TM9', 'NLS_NUMERIC_CHARACTERS = ''.,''') \
             FROM \"LANDING\".\"ORDERS\""
        );
        assert_eq!(
            max_scalar("landing", "orders", "day", &"DATE".parse().unwrap()).unwrap(),
            "SELECT TO_CHAR(MAX(\"DAY\"), 'YYYY-MM-DD') FROM \"LANDING\".\"ORDERS\""
        );

        // The shapes those formats produce round-trip into watermarks.
        let parsed = WatermarkValue::parse_scalar("2026-01-07 09:30:00.123456", &timestamp)
            .expect("timestamp parses");
        assert!(matches!(parsed, WatermarkValue::Timestamp(_)));
        assert_eq!(parsed.to_string(), "2026-01-07 09:30:00.123456");
        assert_eq!(
            WatermarkValue::parse_scalar("41", &"NUMBER(38,0)".parse().unwrap()),
            Some(WatermarkValue::Int(41))
        );
        assert_eq!(
            WatermarkValue::parse_scalar("2026-01-07", &"DATE".parse().unwrap())
                .expect("date parses")
                .to_string(),
            "2026-01-07"
        );
    }

    #[test]
    fn identifiers_fold_and_hostile_names_are_rejected() {
        let mixed = names("landing", "\"MixedCase\"").unwrap();
        assert_eq!(mixed.target_fqn(), "\"LANDING\".\"MixedCase\"");
        assert_eq!(
            mixed.staging_fqn(),
            "\"LANDING\".\"MixedCase__ZDBT_STAGING\""
        );
        assert!(names("landing", "or\"ders").is_err());
        assert!(names("landing", &"x".repeat(120)).is_err());
    }
}
