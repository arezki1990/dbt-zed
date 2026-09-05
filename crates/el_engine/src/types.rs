//! The Snowflake type vocabulary and its mapping to polars dtypes — the
//! single source of truth used by both the cast plan and (later) target
//! DDL generation, so spec and DDL can never drift.

use std::fmt;
use std::str::FromStr;

use polars::prelude::{DataType, TimeUnit, TimeZone};
use serde::{Deserialize, Serialize};

/// A parsed Snowflake type spelling from a spec's `cast:` value, e.g.
/// `NUMBER(38,0)`, `VARCHAR(255)`, `TIMESTAMP_NTZ`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnowflakeType {
    pub base: SfBase,
    pub precision: Option<u8>,
    pub scale: Option<u8>,
    pub length: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SfBase {
    Number,
    Float,
    Varchar,
    Boolean,
    Date,
    Time,
    TimestampNtz,
    TimestampTz,
    Binary,
    Variant,
}

impl SnowflakeType {
    fn bare(base: SfBase) -> Self {
        Self {
            base,
            precision: None,
            scale: None,
            length: None,
        }
    }

    /// The polars dtype a cast to this Snowflake type targets.
    ///
    /// `Variant` returns None: nested/JSON data keeps its source dtype and
    /// the loader hands it to Snowflake as-is.
    pub fn polars_dtype(&self) -> Option<DataType> {
        Some(match self.base {
            SfBase::Number => match (self.precision, self.scale) {
                (Some(precision), Some(scale)) if scale > 0 => {
                    DataType::Decimal(precision as usize, scale as usize)
                }
                (Some(precision), _) if precision > 18 => {
                    DataType::Decimal(precision as usize, 0)
                }
                _ => DataType::Int64,
            },
            SfBase::Float => DataType::Float64,
            SfBase::Varchar => DataType::String,
            SfBase::Boolean => DataType::Boolean,
            SfBase::Date => DataType::Date,
            SfBase::Time => DataType::Time,
            SfBase::TimestampNtz => DataType::Datetime(TimeUnit::Microseconds, None),
            SfBase::TimestampTz => {
                DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::UTC))
            }
            SfBase::Binary => DataType::Binary,
            SfBase::Variant => return None,
        })
    }

    /// The Snowflake DDL type for a source column that carries no explicit
    /// `cast:` — derived from its polars dtype.
    pub fn from_polars(dtype: &DataType) -> Self {
        match dtype {
            DataType::Boolean => Self::bare(SfBase::Boolean),
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64 => Self {
                base: SfBase::Number,
                precision: Some(38),
                scale: Some(0),
                length: None,
            },
            DataType::Float32 | DataType::Float64 => Self::bare(SfBase::Float),
            DataType::Decimal(precision, scale) => Self {
                base: SfBase::Number,
                precision: Some((*precision).min(38) as u8),
                scale: Some((*scale).min(37) as u8),
                length: None,
            },
            DataType::Date => Self::bare(SfBase::Date),
            DataType::Time => Self::bare(SfBase::Time),
            DataType::Datetime(_, None) => Self::bare(SfBase::TimestampNtz),
            DataType::Datetime(_, Some(_)) => Self::bare(SfBase::TimestampTz),
            // Nanosecond count; documented in the plan's type table.
            DataType::Duration(_) => Self {
                base: SfBase::Number,
                precision: Some(38),
                scale: Some(0),
                length: None,
            },
            DataType::Binary => Self::bare(SfBase::Binary),
            DataType::List(_) | DataType::Struct(_) => Self::bare(SfBase::Variant),
            _ => Self::bare(SfBase::Varchar),
        }
    }
}

impl FromStr for SnowflakeType {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        let (name, args) = match input.find('(') {
            Some(open) => {
                let close = input
                    .rfind(')')
                    .ok_or_else(|| format!("unclosed '(' in type {input:?}"))?;
                (
                    input[..open].trim(),
                    Some(
                        input[open + 1..close]
                            .split(',')
                            .map(str::trim)
                            .collect::<Vec<_>>(),
                    ),
                )
            }
            None => (input, None),
        };
        let upper = name.to_ascii_uppercase();
        let base = match upper.as_str() {
            "NUMBER" | "NUMERIC" | "DECIMAL" | "INT" | "INTEGER" | "BIGINT" | "SMALLINT" => {
                SfBase::Number
            }
            "FLOAT" | "FLOAT4" | "FLOAT8" | "DOUBLE" | "REAL" => SfBase::Float,
            "VARCHAR" | "STRING" | "TEXT" | "CHAR" => SfBase::Varchar,
            "BOOLEAN" | "BOOL" => SfBase::Boolean,
            "DATE" => SfBase::Date,
            "TIME" => SfBase::Time,
            "TIMESTAMP_NTZ" | "TIMESTAMP" | "DATETIME" => SfBase::TimestampNtz,
            "TIMESTAMP_TZ" | "TIMESTAMP_LTZ" => SfBase::TimestampTz,
            "BINARY" | "VARBINARY" => SfBase::Binary,
            "VARIANT" | "OBJECT" | "ARRAY" => SfBase::Variant,
            other => return Err(format!("unknown Snowflake type {other:?}")),
        };
        let mut parsed = SnowflakeType::bare(base);
        if let Some(args) = args {
            let numbers: Vec<u32> = args
                .iter()
                .map(|arg| {
                    arg.parse::<u32>()
                        .map_err(|_| format!("bad type argument {arg:?} in {input:?}"))
                })
                .collect::<Result<_, _>>()?;
            match (base, numbers.as_slice()) {
                (SfBase::Number, [precision]) => parsed.precision = Some(*precision as u8),
                (SfBase::Number, [precision, scale]) => {
                    parsed.precision = Some(*precision as u8);
                    parsed.scale = Some(*scale as u8);
                }
                (SfBase::Varchar, [length]) => parsed.length = Some(*length),
                (SfBase::Time | SfBase::TimestampNtz | SfBase::TimestampTz, [_prec]) => {}
                _ => return Err(format!("unexpected arguments in type {input:?}")),
            }
        }
        Ok(parsed)
    }
}

impl fmt::Display for SnowflakeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.base {
            SfBase::Number => match (self.precision, self.scale) {
                (Some(p), Some(s)) => write!(f, "NUMBER({p},{s})"),
                (Some(p), None) => write!(f, "NUMBER({p})"),
                _ => write!(f, "NUMBER"),
            },
            SfBase::Float => write!(f, "FLOAT"),
            SfBase::Varchar => match self.length {
                Some(length) => write!(f, "VARCHAR({length})"),
                None => write!(f, "VARCHAR"),
            },
            SfBase::Boolean => write!(f, "BOOLEAN"),
            SfBase::Date => write!(f, "DATE"),
            SfBase::Time => write!(f, "TIME"),
            SfBase::TimestampNtz => write!(f, "TIMESTAMP_NTZ"),
            SfBase::TimestampTz => write!(f, "TIMESTAMP_TZ"),
            SfBase::Binary => write!(f, "BINARY"),
            SfBase::Variant => write!(f, "VARIANT"),
        }
    }
}

impl Serialize for SnowflakeType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SnowflakeType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl schemars::JsonSchema for SnowflakeType {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SnowflakeType".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "A Snowflake type spelling, e.g. NUMBER(38,0), VARCHAR, TIMESTAMP_NTZ, VARIANT.",
            "examples": ["NUMBER(38,0)", "VARCHAR", "BOOLEAN", "DATE", "TIMESTAMP_NTZ", "TIMESTAMP_TZ", "VARIANT"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_displays_round_trip() {
        for spelling in [
            "NUMBER(38,0)",
            "NUMBER(10)",
            "FLOAT",
            "VARCHAR(255)",
            "VARCHAR",
            "BOOLEAN",
            "DATE",
            "TIME",
            "TIMESTAMP_NTZ",
            "TIMESTAMP_TZ",
            "BINARY",
            "VARIANT",
        ] {
            let parsed: SnowflakeType = spelling.parse().unwrap();
            assert_eq!(parsed.to_string(), spelling, "round trip of {spelling}");
        }
    }

    #[test]
    fn aliases_normalize() {
        assert_eq!(
            "numeric(18,2)".parse::<SnowflakeType>().unwrap().to_string(),
            "NUMBER(18,2)"
        );
        assert_eq!(
            "string".parse::<SnowflakeType>().unwrap().to_string(),
            "VARCHAR"
        );
        assert!("FROBNICATE".parse::<SnowflakeType>().is_err());
    }

    #[test]
    fn polars_mapping_is_total_over_common_dtypes() {
        // Every dtype the connectors can produce maps to some DDL type.
        for dtype in [
            DataType::Boolean,
            DataType::Int64,
            DataType::UInt32,
            DataType::Float64,
            DataType::Decimal(18, 2),
            DataType::String,
            DataType::Date,
            DataType::Time,
            DataType::Datetime(TimeUnit::Microseconds, None),
            DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::UTC)),
            DataType::Duration(TimeUnit::Microseconds),
            DataType::Binary,
            DataType::List(Box::new(DataType::Int64)),
            DataType::Null,
        ] {
            let _ = SnowflakeType::from_polars(&dtype);
        }
        // Decimal-bearing NUMBER targets a decimal dtype; integer NUMBER an Int64.
        assert_eq!(
            "NUMBER(18,2)".parse::<SnowflakeType>().unwrap().polars_dtype(),
            Some(DataType::Decimal(18, 2))
        );
        assert_eq!(
            "NUMBER(38,0)".parse::<SnowflakeType>().unwrap().polars_dtype(),
            Some(DataType::Decimal(38, 0))
        );
        assert_eq!(
            "NUMBER(10,0)".parse::<SnowflakeType>().unwrap().polars_dtype(),
            Some(DataType::Int64)
        );
        assert_eq!(
            "VARIANT".parse::<SnowflakeType>().unwrap().polars_dtype(),
            None
        );
    }
}
