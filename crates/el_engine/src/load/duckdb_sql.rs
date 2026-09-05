//! DuckDB dialect SQL for the local-warehouse loader: staging DDL from
//! the same SnowflakeType vocabulary (one spec, two warehouses), swap and
//! cleanup. Ingest happens via `read_parquet` — DuckDB reads the chunk
//! files natively, so no driver-level binding is needed at all.

use crate::types::{SfBase, SnowflakeType};

pub use super::snowflake_sql::{quote_ident, staging_table_name};

/// `"SCHEMA"."TABLE"` — DuckDB targets ignore the `database` field.
pub fn fqn(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(table))
}

/// The DuckDB DDL type for a spec type.
fn ddl_type(sf_type: &SnowflakeType) -> String {
    match sf_type.base {
        SfBase::Number => match (sf_type.precision, sf_type.scale) {
            (Some(precision), Some(scale)) => format!("DECIMAL({precision},{scale})"),
            (Some(precision), None) => format!("DECIMAL({precision},0)"),
            _ => "BIGINT".to_owned(),
        },
        SfBase::Float => "DOUBLE".to_owned(),
        SfBase::Varchar => "VARCHAR".to_owned(),
        SfBase::Boolean => "BOOLEAN".to_owned(),
        SfBase::Date => "DATE".to_owned(),
        SfBase::Time => "TIME".to_owned(),
        SfBase::TimestampNtz => "TIMESTAMP".to_owned(),
        SfBase::TimestampTz => "TIMESTAMPTZ".to_owned(),
        SfBase::Binary => "BLOB".to_owned(),
        // Same v1 stance as Snowflake staging: nested data lands as text.
        SfBase::Variant => "VARCHAR".to_owned(),
    }
}

pub fn create_schema(schema: &str) -> String {
    format!("CREATE SCHEMA IF NOT EXISTS {}", quote_ident(schema))
}

pub fn create_staging(
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
        "CREATE OR REPLACE TABLE {} ({column_list})",
        fqn(schema, &staging_table_name(target_table))
    )
}

/// Ingest one parquet chunk. BY NAME tolerates column order differences;
/// the path is a temp file we created, single-quoted with escaping.
pub fn ingest_parquet(schema: &str, target_table: &str, parquet_path: &str) -> String {
    format!(
        "INSERT INTO {} BY NAME SELECT * FROM read_parquet('{}')",
        fqn(schema, &staging_table_name(target_table)),
        parquet_path.replace('\'', "''")
    )
}

pub fn swap(schema: &str, target_table: &str) -> String {
    format!(
        "CREATE OR REPLACE TABLE {} AS FROM {}",
        fqn(schema, target_table),
        fqn(schema, &staging_table_name(target_table))
    )
}

pub fn drop_staging(schema: &str, target_table: &str) -> String {
    format!(
        "DROP TABLE IF EXISTS {}",
        fqn(schema, &staging_table_name(target_table))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_sql() {
        let columns = vec![
            ("id".to_owned(), "NUMBER(38,0)".parse().unwrap()),
            ("amount".to_owned(), "NUMBER(18,2)".parse().unwrap()),
            ("at".to_owned(), "TIMESTAMP_NTZ".parse().unwrap()),
        ];
        assert_eq!(
            create_staging("LANDING", "ORDERS", &columns),
            "CREATE OR REPLACE TABLE \"LANDING\".\"ORDERS__ZDBT_STAGING\" \
             (\"id\" DECIMAL(38,0), \"amount\" DECIMAL(18,2), \"at\" TIMESTAMP)"
        );
        assert_eq!(
            ingest_parquet("LANDING", "ORDERS", "/tmp/it's.parquet"),
            "INSERT INTO \"LANDING\".\"ORDERS__ZDBT_STAGING\" BY NAME \
             SELECT * FROM read_parquet('/tmp/it''s.parquet')"
        );
        assert_eq!(
            swap("LANDING", "ORDERS"),
            "CREATE OR REPLACE TABLE \"LANDING\".\"ORDERS\" AS FROM \"LANDING\".\"ORDERS__ZDBT_STAGING\""
        );
    }
}
