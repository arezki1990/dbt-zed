# P0 spike findings — el_engine (branch `el-spike`)

Gate for the EL feature plan. Verdict up front: **GO.** Every load-bearing
claim held; four research errors were caught and fixed before they could
cost anything.

## 1. Casting story — proven

`cargo test -p el_engine`: 3/3 pass.

- Lax cast NULLs the bad value and the engine counts exactly it
  (`("amount", 1)` on the fixture).
- Strict cast fails the chunk with a polars error — maps to "fail the
  stream" in the spec.
- **Finding:** plain `cast` String→Datetime is deprecated and parses
  ~nothing (all 4 values nulled, valid ones included). Temporal casts MUST
  go through `str().to_datetime(unit, tz, StrptimeOptions, ..)` — which is
  precisely the spec's `parse:` field, and `StrptimeOptions.strict` maps
  1:1 onto our strict/lax semantics. The engine helper now does this.
- **Polars 2.0** (RC announced, final "in the following weeks"): removes
  String→temporal casts outright — our code is already compliant — and
  makes the streaming engine the default, validating the engine choice.
  Its no-row-order caveat (join/group_by) doesn't affect EL: ordering
  lives in extraction SQL. Stay on `=0.55.2`; migrate deliberately later.

## 2. Build & size cost — acceptable

| Measurement | Result |
|---|---|
| Full polars dep tree, debug, clean (M-series) | **~48 s wall** |
| Release build of engine + probe binary | **~182 s wall** |
| Standalone probe binary (polars + engine) | 68 MB, **50 MB stripped** |
| Current shipped zed binary (for scale) | 402 MB |

The 50 MB is an upper bound (standalone static binary); linked into zed
the delta will be smaller and compresses further in the dmg. Roughly
+12% on the binary — acceptable for an embedded engine.

## 3. ADBC Snowflake driver — dlopen handshake PASSED

```
EL_ADBC_DRIVER_PATH=…/libadbc_driver_snowflake.dylib \
  cargo test -p el_engine --features adbc -- --ignored adbc
→ ADBC driver loaded and database handle constructed  (0.76 s)
```

Driver: `adbc-drivers/snowflake` release **go/v1.13.0** (2026-08-19),
`snowflake_macos_arm64_v1.13.0.tar.gz`
sha256 `53cc2dccbfce6f5b0b534c02c00639c33cf866c90eaafd7df200f9f42ac463de`,
dylib 51 MB arm64
sha256 `f03e36990b5c7da8b0d71b806e7937257864d2d4424c1d2c5c4d15bafbe6db0f`.
Release cadence monthly-ish; assets for linux amd64/arm64, macos arm64,
windows amd64 — including the Linux arm64 zdbt itself doesn't build.

Loaded in a plain (non-GPUI) test process — consistent with the sidecar
design; still run the real sidecar (`--el-loader` re-exec) before P3.

## 4. Research corrections found by compiling (why spikes exist)

1. polars facade feature is **`streaming`**, not `new_streaming` (that's an
   inner-crate flag).
2. `adbc_core 0.24` has **no cargo features**; the research's
   `features = ["driver_manager"]` doesn't exist.
3. The driver manager lives in a **separate crate `adbc_driver_manager`**
   (= 0.24.0) since the 0.22 restructure — `adbc_core` is traits only.
4. `LazyCsvReader::new` takes `PlRefPath` (from `&str`), not `PathBuf`;
   and polars' `PolarsContext` collides with `anyhow::Context` — the
   engine avoids `.context()` on polars results.

## Not yet verified (carried into P1-P3)

- Actual linked binary delta inside the zed bundle (measure at P1 when
  dbt_ui depends on el_engine).
- The real sidecar re-exec path (`--el-loader`) — the handshake ran
  in-process in a test binary, which is the same dlopen but not the same
  process shape.
- An authenticated `Ingest` into a real Snowflake account (needs creds;
  the plan's P3 exit criterion).
