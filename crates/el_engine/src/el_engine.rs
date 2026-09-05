//! zdbt's embedded EL engine: extract from databases and files, cast with
//! polars, load into Snowflake. Pure data + engine — no gpui anywhere, so
//! everything here unit-tests like a normal library.
//!
//! P1 surface: spec parsing/validation/writing, env templating, the
//! polars⇄Snowflake type table, the cast plan, the files connector, and
//! the bounded preview the IDE calls. Database connectors and the ADBC
//! loader arrive in later phases.

pub mod cast;
pub mod connectors;
pub mod env;
pub mod explore;
pub mod load;
pub mod preview;
pub mod progress;
pub mod run;
pub mod spec;
pub mod state;
pub mod types;

/// Re-exported so downstream binaries (the connector worker) never pin a
/// second polars.
pub use polars;

pub use cast::{CastOutcome, CastPlan, ColumnFailures};
pub use preview::{PreviewColumn, PreviewResult, preview_stream};
pub use progress::{CancelFlag, Phase, ProgressEvent};
pub use spec::{
    ColumnSpec, Connection, Connections, Mode, Pipeline, SourceObject, SpecIssue, StreamSpec,
    load_connections, load_pipeline, write_pipeline,
};
pub use types::SnowflakeType;

#[cfg(feature = "adbc")]
pub mod adbc_check {
    //! Spike proof that the Go-built Snowflake driver dylib dlopens in a
    //! plain (non-GPUI) process and exposes a live ADBC function table.

    use adbc_core::Driver as _;
    use adbc_core::options::AdbcVersion;
    use adbc_driver_manager::ManagedDriver;

    pub fn handshake(driver_path: &std::path::Path) -> anyhow::Result<String> {
        let mut driver =
            ManagedDriver::load_dynamic_from_filename(driver_path, None, AdbcVersion::V110)
                .map_err(|error| anyhow::anyhow!("dlopen/init failed: {error:?}"))?;
        let _database = driver
            .new_database()
            .map_err(|error| anyhow::anyhow!("new_database failed: {error:?}"))?;
        Ok(format!(
            "ADBC driver loaded and database handle constructed from {}",
            driver_path.display()
        ))
    }

    /// Run manually:
    /// `EL_ADBC_DRIVER_PATH=… cargo test -p el_engine --features adbc -- --ignored adbc`
    #[cfg(test)]
    mod tests {
        #[test]
        #[ignore]
        fn adbc_driver_handshake() {
            let path =
                std::env::var("EL_ADBC_DRIVER_PATH").expect("set EL_ADBC_DRIVER_PATH");
            let message = super::handshake(std::path::Path::new(&path)).unwrap();
            println!("{message}");
        }
    }
}
