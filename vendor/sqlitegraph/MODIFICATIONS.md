# Modifications (GPL-3.0 §5(a) notice)

This directory vendors the `sqlitegraph` crate, version 3.9.0, from crates.io
(https://crates.io/crates/sqlitegraph), licensed GPL-3.0-only — see LICENSE.

Changes made in this fork (2026-09-02):

- `Cargo.toml`: bumped `rusqlite` from `0.31` to `0.32` and `r2d2_sqlite`
  from `0.24` to `0.25`, so the crate shares the single `libsqlite3-sys`
  native link (0.30.x) used by the surrounding Zed workspace.

No source (.rs) files were modified.
