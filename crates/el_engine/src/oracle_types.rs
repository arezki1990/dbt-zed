//! The Oracle type vocabulary: ONE Oracle → polars mapping shared by the
//! source connector (column probing) and the table explorer, and ONE
//! polars / spec-type → Oracle DDL mapping for targets. Driver-free so it
//! compiles into the IDE and unit-tests without a database; the `oracle`
//! feature adds the conversion from the driver's own type descriptor.
//!
//! Mapping decisions (source side):
//! - `NUMBER(p,0)` with `p <= 18` fits an i64 and reads as `Int64`; wider
//!   integers and any scale > 0 keep exactness as `Decimal(p,s)`, the
//!   same convention `types.rs` uses for `NUMBER` casts.
//! - Unconstrained `NUMBER`, `FLOAT`, `BINARY_FLOAT` and `BINARY_DOUBLE`
//!   read as `Float64`; add a `cast: NUMBER(p,s)` in the stream when a
//!   column must stay exact.
//! - `DATE` carries a time of day in Oracle, so it is a `Datetime(us)`,
//!   not a polars `Date`. `TIMESTAMP WITH [LOCAL] TIME ZONE` lands as
//!   UTC micros (the connector pins the session time zone to UTC).
//! - Character, LOB, interval, rowid, JSON and XML types read as text;
//!   `RAW`/`LONG RAW`/`BLOB`/`BFILE` as bytes; 23ai `BOOLEAN` as bool.
//!
//! Target side (`OracleDialect::ddl_type`): `VARCHAR2(4000)` unless the
//! spec asks for more (then `CLOB`), `NUMBER(19,0)` for bare integers,
//! `BINARY_DOUBLE` for floats, `NUMBER(p,s)` for decimals, `TIMESTAMP(6)`
//! [WITH TIME ZONE], `DATE`, `BLOB`, and `NUMBER(1)` for booleans unless
//! the dialect opts into the 23ai native `BOOLEAN`. Oracle has no TIME
//! type: times land as `VARCHAR2(18)` text (`HH:MM:SS.ffffff`).

use std::fmt;
use std::str::FromStr;

use polars::prelude::{DataType, TimeUnit, TimeZone};

use crate::types::{SfBase, SnowflakeType};

/// A column type as Oracle reports it — from `ALL_TAB_COLUMNS`
/// (`DATA_TYPE`/`DATA_PRECISION`/`DATA_SCALE`) or the driver's column info.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OracleColumnType {
    /// `VARCHAR2(n)`; the length is informational only.
    Varchar2(Option<u32>),
    NVarchar2,
    Char,
    NChar,
    Clob,
    NClob,
    Long,
    /// `NUMBER(p,s)`. `precision: None` is an unconstrained `NUMBER`
    /// (Oracle reports NULL precision); `scale: None` means unspecified.
    /// A declared `INTEGER` arrives as `NUMBER` with NULL precision and
    /// scale 0.
    Number {
        precision: Option<u8>,
        scale: Option<i8>,
    },
    /// `FLOAT(b)` — binary precision, always approximate.
    Float,
    BinaryFloat,
    BinaryDouble,
    Date,
    Timestamp,
    TimestampTz,
    TimestampLtz,
    Raw,
    LongRaw,
    Blob,
    Bfile,
    /// Oracle 23ai native boolean.
    Boolean,
    IntervalDs,
    IntervalYm,
    Rowid,
    Json,
    Xml,
    /// Anything else (object types, nested tables, …) — read as text.
    Other(String),
}

impl OracleColumnType {
    /// From the dictionary columns of `ALL_TAB_COLUMNS`: `DATA_TYPE`
    /// spelled as Oracle stores it, plus `DATA_PRECISION` / `DATA_SCALE`
    /// (both nullable) for `NUMBER`.
    pub fn from_dictionary(data_type: &str, precision: Option<u8>, scale: Option<i8>) -> Self {
        match data_type.parse::<OracleColumnType>() {
            Ok(OracleColumnType::Number { .. }) => OracleColumnType::Number { precision, scale },
            Ok(other) => other,
            Err(_) => OracleColumnType::Other(data_type.to_owned()),
        }
    }

    /// The polars dtype a column of this type is read as.
    pub fn polars_dtype(&self) -> DataType {
        match self {
            OracleColumnType::Varchar2(_)
            | OracleColumnType::NVarchar2
            | OracleColumnType::Char
            | OracleColumnType::NChar
            | OracleColumnType::Clob
            | OracleColumnType::NClob
            | OracleColumnType::Long => DataType::String,
            OracleColumnType::Number { precision, scale } => match (precision, scale) {
                // Scale > 0 must stay exact.
                (Some(precision), Some(scale)) if *scale > 0 => {
                    DataType::Decimal(*precision as usize, *scale as usize)
                }
                // Integers: i64 while they fit, exact decimal beyond.
                (Some(precision), _) if *precision <= 18 => DataType::Int64,
                (Some(precision), _) => DataType::Decimal(*precision as usize, 0),
                // INTEGER: NULL precision with scale 0 is a 38-digit integer.
                (None, Some(0)) => DataType::Decimal(38, 0),
                // Unconstrained NUMBER — approximate, like FLOAT.
                (None, _) => DataType::Float64,
            },
            OracleColumnType::Float
            | OracleColumnType::BinaryFloat
            | OracleColumnType::BinaryDouble => DataType::Float64,
            OracleColumnType::Date | OracleColumnType::Timestamp => {
                DataType::Datetime(TimeUnit::Microseconds, None)
            }
            OracleColumnType::TimestampTz | OracleColumnType::TimestampLtz => {
                DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::UTC))
            }
            OracleColumnType::Raw
            | OracleColumnType::LongRaw
            | OracleColumnType::Blob
            | OracleColumnType::Bfile => DataType::Binary,
            OracleColumnType::Boolean => DataType::Boolean,
            OracleColumnType::IntervalDs
            | OracleColumnType::IntervalYm
            | OracleColumnType::Rowid
            | OracleColumnType::Json
            | OracleColumnType::Xml
            | OracleColumnType::Other(_) => DataType::String,
        }
    }
}

impl FromStr for OracleColumnType {
    type Err = String;

    /// Parses Oracle's own spellings: `VARCHAR2(100 CHAR)`, `NUMBER(10,2)`,
    /// `TIMESTAMP(6) WITH LOCAL TIME ZONE`, `INTERVAL DAY(2) TO SECOND(6)`,
    /// `LONG RAW`, and the ANSI aliases Oracle accepts in DDL.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err("empty Oracle type".to_owned());
        }
        let upper = trimmed.to_ascii_uppercase();
        // Split "NAME(args) TAIL" into (NAME, args, TAIL) — TAIL carries
        // "WITH TIME ZONE" and interval suffixes.
        let (name, args, tail) = match upper.find('(') {
            Some(open) => {
                let close = upper[open..]
                    .find(')')
                    .map(|offset| open + offset)
                    .ok_or_else(|| format!("unclosed '(' in Oracle type {input:?}"))?;
                (
                    upper[..open].trim().to_owned(),
                    Some(upper[open + 1..close].trim().to_owned()),
                    upper[close + 1..].trim().to_owned(),
                )
            }
            None => {
                let mut words = upper.split_whitespace();
                let first = words.next().unwrap_or_default().to_owned();
                let rest = words.collect::<Vec<_>>().join(" ");
                (first, None, rest)
            }
        };
        let numeric_arg = |text: &str| -> Result<u32, String> {
            // "100 CHAR" / "100 BYTE" length semantics.
            text.split_whitespace()
                .next()
                .unwrap_or_default()
                .parse::<u32>()
                .map_err(|_| format!("unexpected arguments in Oracle type {input:?}"))
        };
        let parsed = match name.as_str() {
            "VARCHAR2" | "VARCHAR" => OracleColumnType::Varchar2(
                args.as_deref().map(numeric_arg).transpose()?,
            ),
            "NVARCHAR2" => OracleColumnType::NVarchar2,
            "CHAR" | "CHARACTER" => OracleColumnType::Char,
            "NCHAR" => OracleColumnType::NChar,
            "CLOB" => OracleColumnType::Clob,
            "NCLOB" => OracleColumnType::NClob,
            "LONG" if tail == "RAW" => OracleColumnType::LongRaw,
            "LONG" => OracleColumnType::Long,
            "NUMBER" | "NUMERIC" | "DECIMAL" | "DEC" => {
                let (precision, scale) = match args.as_deref() {
                    None | Some("") => (None, None),
                    Some(args) => {
                        let mut parts = args.split(',').map(str::trim);
                        let precision = parts
                            .next()
                            .map(|p| p.parse::<u8>())
                            .transpose()
                            .map_err(|_| format!("unexpected precision in Oracle type {input:?}"))?;
                        let scale = parts
                            .next()
                            .map(|s| s.parse::<i8>())
                            .transpose()
                            .map_err(|_| format!("unexpected scale in Oracle type {input:?}"))?;
                        (precision, Some(scale.unwrap_or(0)))
                    }
                };
                OracleColumnType::Number { precision, scale }
            }
            "INTEGER" | "INT" | "SMALLINT" => OracleColumnType::Number {
                precision: None,
                scale: Some(0),
            },
            "FLOAT" | "REAL" | "DOUBLE" => OracleColumnType::Float,
            "BINARY_FLOAT" => OracleColumnType::BinaryFloat,
            "BINARY_DOUBLE" => OracleColumnType::BinaryDouble,
            "DATE" => OracleColumnType::Date,
            "TIMESTAMP" => match tail.as_str() {
                "" => OracleColumnType::Timestamp,
                "WITH TIME ZONE" => OracleColumnType::TimestampTz,
                "WITH LOCAL TIME ZONE" => OracleColumnType::TimestampLtz,
                _ => return Err(format!("unknown Oracle type {input:?}")),
            },
            "RAW" => OracleColumnType::Raw,
            "BLOB" => OracleColumnType::Blob,
            "BFILE" => OracleColumnType::Bfile,
            "BOOLEAN" | "BOOL" => OracleColumnType::Boolean,
            "INTERVAL" if tail.starts_with("DAY") || upper.contains("TO SECOND") => {
                OracleColumnType::IntervalDs
            }
            "INTERVAL" | "INTERVAL YEAR" => OracleColumnType::IntervalYm,
            "ROWID" | "UROWID" => OracleColumnType::Rowid,
            "JSON" => OracleColumnType::Json,
            "XMLTYPE" | "SYS.XMLTYPE" => OracleColumnType::Xml,
            _ if name.starts_with("INTERVAL DAY") => OracleColumnType::IntervalDs,
            _ if name.starts_with("INTERVAL YEAR") => OracleColumnType::IntervalYm,
            _ => return Err(format!("unknown Oracle type {input:?}")),
        };
        Ok(parsed)
    }
}

impl fmt::Display for OracleColumnType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OracleColumnType::Varchar2(Some(length)) => write!(f, "VARCHAR2({length})"),
            OracleColumnType::Varchar2(None) => write!(f, "VARCHAR2"),
            OracleColumnType::NVarchar2 => write!(f, "NVARCHAR2"),
            OracleColumnType::Char => write!(f, "CHAR"),
            OracleColumnType::NChar => write!(f, "NCHAR"),
            OracleColumnType::Clob => write!(f, "CLOB"),
            OracleColumnType::NClob => write!(f, "NCLOB"),
            OracleColumnType::Long => write!(f, "LONG"),
            OracleColumnType::Number { precision, scale } => match (precision, scale) {
                (Some(p), Some(s)) => write!(f, "NUMBER({p},{s})"),
                (Some(p), None) => write!(f, "NUMBER({p})"),
                (None, Some(0)) => write!(f, "INTEGER"),
                _ => write!(f, "NUMBER"),
            },
            OracleColumnType::Float => write!(f, "FLOAT"),
            OracleColumnType::BinaryFloat => write!(f, "BINARY_FLOAT"),
            OracleColumnType::BinaryDouble => write!(f, "BINARY_DOUBLE"),
            OracleColumnType::Date => write!(f, "DATE"),
            OracleColumnType::Timestamp => write!(f, "TIMESTAMP"),
            OracleColumnType::TimestampTz => write!(f, "TIMESTAMP WITH TIME ZONE"),
            OracleColumnType::TimestampLtz => write!(f, "TIMESTAMP WITH LOCAL TIME ZONE"),
            OracleColumnType::Raw => write!(f, "RAW"),
            OracleColumnType::LongRaw => write!(f, "LONG RAW"),
            OracleColumnType::Blob => write!(f, "BLOB"),
            OracleColumnType::Bfile => write!(f, "BFILE"),
            OracleColumnType::Boolean => write!(f, "BOOLEAN"),
            OracleColumnType::IntervalDs => write!(f, "INTERVAL DAY TO SECOND"),
            OracleColumnType::IntervalYm => write!(f, "INTERVAL YEAR TO MONTH"),
            OracleColumnType::Rowid => write!(f, "ROWID"),
            OracleColumnType::Json => write!(f, "JSON"),
            OracleColumnType::Xml => write!(f, "XMLTYPE"),
            OracleColumnType::Other(name) => write!(f, "{name}"),
        }
    }
}

/// The driver's column descriptor → our vocabulary. Only the worker
/// (feature `oracle`) links the driver; the IDE never sees this impl.
#[cfg(feature = "oracle")]
impl From<&oracle::sql_type::OracleType> for OracleColumnType {
    fn from(oracle_type: &oracle::sql_type::OracleType) -> Self {
        use oracle::sql_type::OracleType;
        match oracle_type {
            OracleType::Varchar2(size) => OracleColumnType::Varchar2(Some(*size)),
            OracleType::NVarchar2(_) => OracleColumnType::NVarchar2,
            OracleType::Char(_) => OracleColumnType::Char,
            OracleType::NChar(_) => OracleColumnType::NChar,
            OracleType::Rowid => OracleColumnType::Rowid,
            OracleType::Raw(_) => OracleColumnType::Raw,
            OracleType::BinaryFloat => OracleColumnType::BinaryFloat,
            OracleType::BinaryDouble => OracleColumnType::BinaryDouble,
            // ODPI-C reports precision 0 for an unconstrained NUMBER and
            // scale -127 for a FLOAT (already split off as Float below).
            OracleType::Number(0, _) => OracleColumnType::Number {
                precision: None,
                scale: None,
            },
            OracleType::Number(precision, scale) => OracleColumnType::Number {
                precision: Some(*precision),
                scale: Some(*scale),
            },
            OracleType::Float(_) => OracleColumnType::Float,
            OracleType::Date => OracleColumnType::Date,
            OracleType::Timestamp(_) => OracleColumnType::Timestamp,
            OracleType::TimestampTZ(_) => OracleColumnType::TimestampTz,
            OracleType::TimestampLTZ(_) => OracleColumnType::TimestampLtz,
            OracleType::IntervalDS(_, _) => OracleColumnType::IntervalDs,
            OracleType::IntervalYM(_) => OracleColumnType::IntervalYm,
            OracleType::CLOB => OracleColumnType::Clob,
            OracleType::NCLOB => OracleColumnType::NClob,
            OracleType::BLOB => OracleColumnType::Blob,
            OracleType::BFILE => OracleColumnType::Bfile,
            OracleType::Boolean => OracleColumnType::Boolean,
            OracleType::Long => OracleColumnType::Long,
            OracleType::LongRaw => OracleColumnType::LongRaw,
            OracleType::Json => OracleColumnType::Json,
            OracleType::Xml => OracleColumnType::Xml,
            // Native define types the driver uses for narrow integers.
            OracleType::Int64 => OracleColumnType::Number {
                precision: Some(18),
                scale: Some(0),
            },
            OracleType::UInt64 => OracleColumnType::Number {
                precision: Some(20),
                scale: Some(0),
            },
            // RefCursor, Object(_) and anything a newer driver adds
            // (`OracleType` is non_exhaustive): text.
            other => OracleColumnType::Other(other.to_string()),
        }
    }
}

/// Target-side DDL choices that depend on the server version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OracleDialect {
    /// Emit the 23ai native `BOOLEAN` column type instead of `NUMBER(1)`.
    pub native_boolean: bool,
}

impl OracleDialect {
    /// Widest `VARCHAR2` on a default (`MAX_STRING_SIZE=STANDARD`) server.
    pub const MAX_VARCHAR2: u32 = 4000;

    /// The Oracle DDL type for a spec type — the same `SnowflakeType`
    /// vocabulary every other target uses, so one `cast:` means one thing.
    /// Explicit precisions are honoured verbatim (clamped to Oracle's 38).
    pub fn ddl_type(&self, sf_type: &SnowflakeType) -> String {
        match sf_type.base {
            SfBase::Number => match (sf_type.precision, sf_type.scale) {
                (Some(precision), Some(scale)) => {
                    let precision = precision.min(38);
                    format!("NUMBER({precision},{})", scale.min(precision))
                }
                (Some(precision), None) => format!("NUMBER({},0)", precision.min(38)),
                _ => "NUMBER(19,0)".to_owned(),
            },
            SfBase::Float => "BINARY_DOUBLE".to_owned(),
            SfBase::Varchar => match sf_type.length {
                Some(length) if length > Self::MAX_VARCHAR2 => "CLOB".to_owned(),
                Some(length) => format!("VARCHAR2({length})"),
                None => format!("VARCHAR2({})", Self::MAX_VARCHAR2),
            },
            SfBase::Boolean if self.native_boolean => "BOOLEAN".to_owned(),
            SfBase::Boolean => "NUMBER(1)".to_owned(),
            SfBase::Date => "DATE".to_owned(),
            // No TIME type in Oracle: HH:MM:SS.ffffff as text.
            SfBase::Time => "VARCHAR2(18)".to_owned(),
            SfBase::TimestampNtz => "TIMESTAMP(6)".to_owned(),
            SfBase::TimestampTz => "TIMESTAMP(6) WITH TIME ZONE".to_owned(),
            SfBase::Binary => "BLOB".to_owned(),
            // Same v1 stance as the other targets: nested data lands as text.
            SfBase::Variant => "CLOB".to_owned(),
        }
    }

    /// The Oracle DDL type for a column with no explicit `cast:` — from
    /// its polars dtype. Integers get `NUMBER(19,0)` (an i64 needs 19
    /// digits, 38 would only waste the optimizer's estimates); everything
    /// else follows the spec-type table above.
    pub fn ddl_type_for_polars(&self, dtype: &DataType) -> String {
        match dtype {
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::Duration(_) => "NUMBER(19,0)".to_owned(),
            DataType::UInt64 => "NUMBER(20,0)".to_owned(),
            other => self.ddl_type(&SnowflakeType::from_polars(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(precision: Option<u8>, scale: Option<i8>) -> OracleColumnType {
        OracleColumnType::Number { precision, scale }
    }

    #[test]
    fn source_types_map_to_polars() {
        let utc = DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::UTC));
        let naive = DataType::Datetime(TimeUnit::Microseconds, None);
        for (spelling, expected) in [
            ("VARCHAR2(100)", DataType::String),
            ("VARCHAR2(100 CHAR)", DataType::String),
            ("NVARCHAR2(50)", DataType::String),
            ("CHAR(3)", DataType::String),
            ("NCHAR(3)", DataType::String),
            ("CLOB", DataType::String),
            ("NCLOB", DataType::String),
            ("LONG", DataType::String),
            ("NUMBER(10,0)", DataType::Int64),
            ("NUMBER(18)", DataType::Int64),
            ("NUMBER(19,0)", DataType::Decimal(19, 0)),
            ("NUMBER(38,0)", DataType::Decimal(38, 0)),
            ("NUMBER(18,2)", DataType::Decimal(18, 2)),
            ("NUMBER(5,-2)", DataType::Int64),
            ("NUMBER", DataType::Float64),
            ("INTEGER", DataType::Decimal(38, 0)),
            ("FLOAT", DataType::Float64),
            ("FLOAT(126)", DataType::Float64),
            ("BINARY_FLOAT", DataType::Float64),
            ("BINARY_DOUBLE", DataType::Float64),
            ("DATE", naive.clone()),
            ("TIMESTAMP", naive.clone()),
            ("TIMESTAMP(6)", naive.clone()),
            ("TIMESTAMP(9)", naive.clone()),
            ("TIMESTAMP(6) WITH TIME ZONE", utc.clone()),
            ("TIMESTAMP WITH TIME ZONE", utc.clone()),
            ("TIMESTAMP(6) WITH LOCAL TIME ZONE", utc.clone()),
            ("RAW(16)", DataType::Binary),
            ("LONG RAW", DataType::Binary),
            ("BLOB", DataType::Binary),
            ("BFILE", DataType::Binary),
            ("BOOLEAN", DataType::Boolean),
            ("INTERVAL DAY(2) TO SECOND(6)", DataType::String),
            ("INTERVAL YEAR(2) TO MONTH", DataType::String),
            ("ROWID", DataType::String),
            ("UROWID", DataType::String),
            ("JSON", DataType::String),
            ("XMLTYPE", DataType::String),
        ] {
            let parsed: OracleColumnType = spelling
                .parse()
                .unwrap_or_else(|error| panic!("{spelling}: {error}"));
            assert_eq!(parsed.polars_dtype(), expected, "dtype of {spelling}");
        }
        assert!("FROBNICATE".parse::<OracleColumnType>().is_err());
        assert!("NUMBER(x)".parse::<OracleColumnType>().is_err());
        assert_eq!(
            OracleColumnType::Other("SDO_GEOMETRY".to_owned()).polars_dtype(),
            DataType::String
        );
    }

    #[test]
    fn dictionary_rows_carry_precision_and_scale() {
        // ALL_TAB_COLUMNS spells NUMBER without arguments; precision and
        // scale arrive in their own columns (NULL when unconstrained).
        assert_eq!(
            OracleColumnType::from_dictionary("NUMBER", Some(10), Some(0)),
            number(Some(10), Some(0))
        );
        assert_eq!(
            OracleColumnType::from_dictionary("NUMBER", None, None).polars_dtype(),
            DataType::Float64
        );
        assert_eq!(
            OracleColumnType::from_dictionary("NUMBER", None, Some(0)).polars_dtype(),
            DataType::Decimal(38, 0)
        );
        assert_eq!(
            OracleColumnType::from_dictionary("TIMESTAMP(6) WITH TIME ZONE", None, None),
            OracleColumnType::TimestampTz
        );
        assert_eq!(
            OracleColumnType::from_dictionary("SDO_GEOMETRY", None, None),
            OracleColumnType::Other("SDO_GEOMETRY".to_owned())
        );
    }

    #[test]
    fn display_round_trips() {
        for spelling in [
            "VARCHAR2(100)",
            "VARCHAR2",
            "NUMBER(10,2)",
            "NUMBER",
            "INTEGER",
            "FLOAT",
            "DATE",
            "TIMESTAMP",
            "TIMESTAMP WITH TIME ZONE",
            "TIMESTAMP WITH LOCAL TIME ZONE",
            "RAW",
            "LONG RAW",
            "BLOB",
            "BOOLEAN",
            "INTERVAL DAY TO SECOND",
            "INTERVAL YEAR TO MONTH",
            "JSON",
        ] {
            let parsed: OracleColumnType = spelling.parse().unwrap();
            assert_eq!(parsed.to_string(), spelling, "round trip of {spelling}");
        }
        // NUMBER(p) is NUMBER(p,0) in Oracle; the normalised form is shown.
        assert_eq!(
            "NUMBER(10)".parse::<OracleColumnType>().unwrap().to_string(),
            "NUMBER(10,0)"
        );
    }

    #[test]
    fn ddl_from_spec_types() {
        let dialect = OracleDialect::default();
        for (spec, expected) in [
            ("NUMBER(38,0)", "NUMBER(38,0)"),
            ("NUMBER(18,2)", "NUMBER(18,2)"),
            ("NUMBER(10)", "NUMBER(10,0)"),
            ("NUMBER", "NUMBER(19,0)"),
            ("FLOAT", "BINARY_DOUBLE"),
            ("VARCHAR", "VARCHAR2(4000)"),
            ("VARCHAR(255)", "VARCHAR2(255)"),
            ("VARCHAR(4000)", "VARCHAR2(4000)"),
            ("VARCHAR(4001)", "CLOB"),
            ("VARCHAR(16777216)", "CLOB"),
            ("BOOLEAN", "NUMBER(1)"),
            ("DATE", "DATE"),
            ("TIME", "VARCHAR2(18)"),
            ("TIMESTAMP_NTZ", "TIMESTAMP(6)"),
            ("TIMESTAMP_TZ", "TIMESTAMP(6) WITH TIME ZONE"),
            ("BINARY", "BLOB"),
            ("VARIANT", "CLOB"),
        ] {
            let sf_type: SnowflakeType = spec.parse().unwrap();
            assert_eq!(dialect.ddl_type(&sf_type), expected, "ddl for {spec}");
        }
        let native = OracleDialect {
            native_boolean: true,
        };
        assert_eq!(native.ddl_type(&"BOOLEAN".parse().unwrap()), "BOOLEAN");
        // Oracle caps precision at 38 and scale at precision.
        let wide = SnowflakeType {
            base: SfBase::Number,
            precision: Some(40),
            scale: Some(39),
            length: None,
        };
        assert_eq!(dialect.ddl_type(&wide), "NUMBER(38,38)");
    }

    #[test]
    fn ddl_from_polars_dtypes() {
        let dialect = OracleDialect::default();
        for (dtype, expected) in [
            (DataType::Boolean, "NUMBER(1)"),
            (DataType::Int8, "NUMBER(19,0)"),
            (DataType::Int64, "NUMBER(19,0)"),
            (DataType::UInt32, "NUMBER(19,0)"),
            (DataType::UInt64, "NUMBER(20,0)"),
            (DataType::Float32, "BINARY_DOUBLE"),
            (DataType::Float64, "BINARY_DOUBLE"),
            (DataType::Decimal(18, 2), "NUMBER(18,2)"),
            (DataType::Decimal(38, 0), "NUMBER(38,0)"),
            (DataType::String, "VARCHAR2(4000)"),
            (DataType::Date, "DATE"),
            (DataType::Time, "VARCHAR2(18)"),
            (DataType::Datetime(TimeUnit::Microseconds, None), "TIMESTAMP(6)"),
            (
                DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::UTC)),
                "TIMESTAMP(6) WITH TIME ZONE",
            ),
            (DataType::Duration(TimeUnit::Microseconds), "NUMBER(19,0)"),
            (DataType::Binary, "BLOB"),
            (DataType::List(Box::new(DataType::Int64)), "CLOB"),
            (DataType::Null, "VARCHAR2(4000)"),
        ] {
            assert_eq!(dialect.ddl_type_for_polars(&dtype), expected, "ddl for {dtype:?}");
        }
    }

    /// Source → polars → DDL round trip: whatever we read from Oracle we
    /// can write back to Oracle without a cast in the spec.
    #[test]
    fn source_types_have_a_target_ddl() {
        let dialect = OracleDialect::default();
        for spelling in [
            "VARCHAR2(10)",
            "NUMBER(10,0)",
            "NUMBER(18,2)",
            "NUMBER(38,0)",
            "NUMBER",
            "DATE",
            "TIMESTAMP(6) WITH TIME ZONE",
            "BLOB",
            "BOOLEAN",
        ] {
            let parsed: OracleColumnType = spelling.parse().unwrap();
            let ddl = dialect.ddl_type_for_polars(&parsed.polars_dtype());
            assert!(!ddl.is_empty(), "no DDL for {spelling}");
        }
    }

    #[cfg(feature = "oracle")]
    #[test]
    fn driver_types_map_into_the_vocabulary() {
        use oracle::sql_type::OracleType;
        for (driver, expected) in [
            (OracleType::Varchar2(100), OracleColumnType::Varchar2(Some(100))),
            (OracleType::Number(0, -127), number(None, None)),
            (OracleType::Number(10, 0), number(Some(10), Some(0))),
            (OracleType::Number(18, 2), number(Some(18), Some(2))),
            (OracleType::Float(126), OracleColumnType::Float),
            (OracleType::Date, OracleColumnType::Date),
            (OracleType::Timestamp(6), OracleColumnType::Timestamp),
            (OracleType::TimestampTZ(6), OracleColumnType::TimestampTz),
            (OracleType::TimestampLTZ(6), OracleColumnType::TimestampLtz),
            (OracleType::Raw(16), OracleColumnType::Raw),
            (OracleType::BLOB, OracleColumnType::Blob),
            (OracleType::CLOB, OracleColumnType::Clob),
            (OracleType::Boolean, OracleColumnType::Boolean),
            (OracleType::Int64, number(Some(18), Some(0))),
        ] {
            assert_eq!(OracleColumnType::from(&driver), expected, "{driver}");
        }
        assert_eq!(
            OracleColumnType::from(&OracleType::RefCursor),
            OracleColumnType::Other("REF CURSOR".to_owned())
        );
    }
}
