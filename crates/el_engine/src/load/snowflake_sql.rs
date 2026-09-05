//! Transport-independent Snowflake SQL generation: staging DDL, the
//! atomic CLONE swap, cleanup, and (next phase) MERGE. Identifiers come
//! only from spec-validated names and are always double-quoted with
//! embedded quotes doubled — no user SQL is ever interpolated.

use crate::types::{SfBase, SnowflakeType};

pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `"DB"."SCHEMA"."TABLE"` (db optional).
pub fn fqn(database: Option<&str>, schema: &str, table: &str) -> String {
    match database {
        Some(database) => format!(
            "{}.{}.{}",
            quote_ident(database),
            quote_ident(schema),
            quote_ident(table)
        ),
        None => format!("{}.{}", quote_ident(schema), quote_ident(table)),
    }
}

pub fn staging_table_name(target_table: &str) -> String {
    format!("{target_table}__ZDBT_STAGING")
}

/// The DDL type a column loads as. VARIANT stages as VARCHAR in v1 —
/// ADBC ingest binds text columns as VARCHAR; a PARSE_JSON transform is a
/// later phase. Honest limitation, documented in the release notes.
fn ddl_type(sf_type: &SnowflakeType) -> String {
    match sf_type.base {
        SfBase::Variant => "VARCHAR".to_owned(),
        _ => sf_type.to_string(),
    }
}

/// `CREATE OR REPLACE TRANSIENT TABLE <staging> (…)` with OUR types —
/// never the driver's inference.
pub fn create_staging(
    database: Option<&str>,
    schema: &str,
    target_table: &str,
    columns: &[(String, SnowflakeType)],
) -> String {
    let column_list = columns
        .iter()
        .map(|(name, sf_type)| format!("{} {}", quote_ident(name), ddl_type(sf_type)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "CREATE OR REPLACE TRANSIENT TABLE {} ({column_list})",
        fqn(database, schema, &staging_table_name(target_table))
    )
}

/// Atomic full-refresh commit: the target becomes a clone of staging.
pub fn clone_swap(database: Option<&str>, schema: &str, target_table: &str) -> String {
    format!(
        "CREATE OR REPLACE TABLE {} CLONE {}",
        fqn(database, schema, target_table),
        fqn(database, schema, &staging_table_name(target_table))
    )
}

pub fn drop_staging(database: Option<&str>, schema: &str, target_table: &str) -> String {
    format!(
        "DROP TABLE IF EXISTS {}",
        fqn(database, schema, &staging_table_name(target_table))
    )
}

/// First incremental run: the target must exist before MERGE.
pub fn create_target_if_not_exists(
    database: Option<&str>,
    schema: &str,
    target_table: &str,
    columns: &[(String, SnowflakeType)],
) -> String {
    let column_list = columns
        .iter()
        .map(|(name, sf_type)| format!("{} {}", quote_ident(name), ddl_type(sf_type)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "CREATE TABLE IF NOT EXISTS {} ({column_list})",
        fqn(database, schema, target_table)
    )
}

/// Incremental commit: staging deduped per key (latest cursor wins) is
/// merged into the target.
pub fn merge(
    database: Option<&str>,
    schema: &str,
    target_table: &str,
    columns: &[(String, SnowflakeType)],
    primary_key: &[String],
    update_key: &str,
) -> String {
    let staging = fqn(database, schema, &staging_table_name(target_table));
    let target = fqn(database, schema, target_table);
    let pk_list = primary_key
        .iter()
        .map(|key| quote_ident(key))
        .collect::<Vec<_>>()
        .join(", ");
    let on = primary_key
        .iter()
        .map(|key| format!("t.{k} = s.{k}", k = quote_ident(key)))
        .collect::<Vec<_>>()
        .join(" AND ");
    let non_pk: Vec<&(String, SnowflakeType)> = columns
        .iter()
        .filter(|(name, _)| !primary_key.iter().any(|key| key == name))
        .collect();
    let update_set = non_pk
        .iter()
        .map(|(name, _)| format!("{c} = s.{c}", c = quote_ident(name)))
        .collect::<Vec<_>>()
        .join(", ");
    let all_cols = columns
        .iter()
        .map(|(name, _)| quote_ident(name))
        .collect::<Vec<_>>()
        .join(", ");
    let insert_vals = columns
        .iter()
        .map(|(name, _)| format!("s.{}", quote_ident(name)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "MERGE INTO {target} t USING (         SELECT * FROM {staging}          QUALIFY ROW_NUMBER() OVER (PARTITION BY {pk_list} ORDER BY {uk} DESC) = 1         ) s ON {on}          WHEN MATCHED THEN UPDATE SET {update_set}          WHEN NOT MATCHED THEN INSERT ({all_cols}) VALUES ({insert_vals})",
        uk = quote_ident(update_key),
    )
}

/// The cursor read back from the TARGET after commit.
pub fn max_scalar(database: Option<&str>, schema: &str, table: &str, column: &str) -> String {
    format!(
        "SELECT MAX({}) FROM {}",
        quote_ident(column),
        fqn(database, schema, table)
    )
}

pub fn count_rows(database: Option<&str>, schema: &str, table: &str) -> String {
    format!("SELECT COUNT(*) FROM {}", fqn(database, schema, table))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<(String, SnowflakeType)> {
        vec![
            ("id".to_owned(), "NUMBER(38,0)".parse().unwrap()),
            ("AMOUNT_EUR".to_owned(), "NUMBER(18,2)".parse().unwrap()),
            ("note".to_owned(), "VARCHAR".parse().unwrap()),
            ("meta".to_owned(), "VARIANT".parse().unwrap()),
        ]
    }

    #[test]
    fn golden_sql() {
        assert_eq!(
            create_staging(Some("RAW"), "CRM", "ORDERS", &columns()),
            "CREATE OR REPLACE TRANSIENT TABLE \"RAW\".\"CRM\".\"ORDERS__ZDBT_STAGING\" \
             (\"id\" NUMBER(38,0), \"AMOUNT_EUR\" NUMBER(18,2), \"note\" VARCHAR, \"meta\" VARCHAR)"
        );
        assert_eq!(
            clone_swap(Some("RAW"), "CRM", "ORDERS"),
            "CREATE OR REPLACE TABLE \"RAW\".\"CRM\".\"ORDERS\" \
             CLONE \"RAW\".\"CRM\".\"ORDERS__ZDBT_STAGING\""
        );
        assert_eq!(
            drop_staging(None, "CRM", "ORDERS"),
            "DROP TABLE IF EXISTS \"CRM\".\"ORDERS__ZDBT_STAGING\""
        );
    }

    #[test]
    fn quoting_doubles_embedded_quotes() {
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }
}
