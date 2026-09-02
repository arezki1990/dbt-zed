//! SQL value helpers for V3 backend query surfaces.

use crate::SqliteGraphError;

/// SQL value representation for query results
#[derive(Debug, Clone)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl SqlValue {
    pub(crate) fn from_sqlite_value_ref(value_ref: &rusqlite::types::ValueRef) -> Self {
        match value_ref {
            rusqlite::types::ValueRef::Null => SqlValue::Null,
            rusqlite::types::ValueRef::Integer(i) => SqlValue::Integer(*i),
            rusqlite::types::ValueRef::Real(r) => SqlValue::Real(*r),
            rusqlite::types::ValueRef::Text(t) => {
                let text = std::str::from_utf8(t).unwrap_or("<invalid utf8>");
                SqlValue::Text(text.to_owned())
            }
            rusqlite::types::ValueRef::Blob(b) => SqlValue::Blob(b.to_vec()),
        }
    }

    pub fn as_i64(&self) -> Result<i64, SqliteGraphError> {
        match self {
            SqlValue::Integer(i) => Ok(*i),
            _ => Err(SqliteGraphError::validation("Expected integer value")),
        }
    }

    pub fn as_f64(&self) -> Result<f64, SqliteGraphError> {
        match self {
            SqlValue::Real(r) => Ok(*r),
            SqlValue::Integer(i) => Ok(*i as f64),
            _ => Err(SqliteGraphError::validation("Expected numeric value")),
        }
    }

    pub fn as_str(&self) -> Result<&str, SqliteGraphError> {
        match self {
            SqlValue::Text(s) => Ok(s),
            _ => Err(SqliteGraphError::validation("Expected text value")),
        }
    }

    pub fn as_blob(&self) -> Result<&[u8], SqliteGraphError> {
        match self {
            SqlValue::Blob(b) => Ok(b),
            _ => Err(SqliteGraphError::validation("Expected blob value")),
        }
    }
}

/// SQL query result row
pub type SqlRow = Vec<SqlValue>;
