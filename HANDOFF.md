# zdbt — agent handoff

Context brief for an AI agent picking up work on this repo. Everything here was
verified against the working tree; `file:line` references are real. Where a
claim is a known-wrong assumption, it is called out as a **trap** — those are
the things that cost hours.

---

## 1. What this is

**zdbt** is a dbt IDE built as a **fork of Zed**, not an extension. An earlier
attempt at a Zed extension was abandoned: Zed extensions cannot add native
panels, custom GPUI rendering, or a lineage canvas.

| | |
|---|---|
| Repo | `github.com/arezki1990/dbt-zed` (public, GPL-3.0) |
| Branch | **`dbt`** — all work happens here; `main` tracks upstream |
| Base | Zed **v1.17.2** (`upstream` remote = `zed-industries/zed`) |
| Fork size | ~632 files changed vs `v1.17.2` (most of it vendored deps) |
| App id | `dev.zdbt.zdbt`, `CFBundleName` = `zdbt` |
| Landing page | `arezki1990.github.io/dbt-zed` — served from the **`gh-pages` branch**, single `index.html` (Tailwind v4 via CDN) |
| Releases | tags `zdbt-v*` (**not** `v*`, which would trigger upstream's `release.yml`) |

The whole dbt integration lives in **`crates/dbt_ui/`** plus small hooks in
`editor`, `languages`, `project`, `settings_ui`, `settings_content`, `zed`.

---

## 2. Build, run, install

Rust is pinned by `rust-toolchain.toml` to **1.97.1** (rustup honours it
automatically). `cmake` is required.

```bash
# dev build + run  — runtime_shaders avoids needing Xcode's Metal offline toolchain
cargo build -p zed --features gpui_platform/runtime_shaders
ZED_RELEASE_CHANNEL=dev ./target/debug/zed /path/to/dbt/project

# full local .app + .dmg (~12-25 min; two cargo passes)
ZED_MAC_EXTRA_FEATURES=gpui_platform/runtime_shaders ./script/bundle-mac aarch64-apple-darwin
# -> target/aarch64-apple-darwin/release/{bundle/osx/zdbt.app, Zed-aarch64.dmg}

cargo test -p dbt_ui
```

**Traps**

- **`ZED_MAC_EXTRA_FEATURES` is mandatory locally** without the full Xcode Metal
  toolchain. `script/bundle-mac:90,93` appends it, and `:98-100` sets
  `CARGO_BUNDLE_SKIP_BUILD=true` so cargo-bundle doesn't do a featureless
  rebuild. CI does *not* set it (the `macos-15` runner has Xcode).
- **`bundle-mac` runs cargo twice.** The log shows the crate graph climbed
  twice; the first `Finished` line is not the end.
- **macOS keeps the old executable in memory.** After replacing
  `/Applications/zdbt.app`, `open` re-focuses the *running* process. You must
  kill the PID and relaunch, or you'll test stale code and conclude your change
  didn't work.
- **Linux build ≠ `./script/linux` alone.** CI does three steps
  (`.github/workflows/zdbt-release.yml:56-77`): install **clang-18** from
  apt.llvm.org (Ubuntu 22.04's clang 14 is too old for `webrtc-sys`; sets
  `CC=clang-18`/`CXX=clang++-18`), then `./script/linux`, then
  **`./script/download-wasi-sdk`** (a separate script `script/linux` does not call).
- **cross-rs is not viable here** — its images are x86_64-host only, impractical
  for Zed on Apple Silicon. Linux/Windows binaries must come from CI.
- `script/licenses/zed-licenses.toml` must allow `GPL-3.0`/`GPL-3.0-only` or
  `cargo-about` fails the bundle (vendored `sqlitegraph` is GPL-3.0).

---

## 3. Architecture — `crates/dbt_ui/`

| File | Role |
|---|---|
| `results_panel.rs` | ~3.8k lines. The panel: toolbar, results grid, lineage canvas, tree sidebar, details sidebar, Connection tab, dbt subprocess orchestration, dotenv loading. |
| `lineage.rs` | Builds the graph from `target/manifest.json` (+ `catalog.json`) into a **sqlitegraph** DB at `target/zed-dbt-lineage.db`. Layout, BFS, ops extraction. |
| `lineage_sql.rs` | AST column lineage via `sqlparser` 0.62 (Snowflake dialect → Generic fallback). |
| `connection.rs` | The Connection tab's data: project, binary, profile/target, parameter **names**. |
| `mcp.rs` | Built-in MCP server (stdio JSON-RPC), 6 dbt tools. |
| `dbt_install.rs` | Binary resolution + auto-install (Fusion CDN / dbt Core venv). |
| `dbt_settings.rs` | The 15 resolved settings and their defaults. |

Hooks outside the crate:

- `crates/editor/src/hover_links.rs` — `find_dbt_model_link`, `find_dbt_cte_link`,
  references-fallback suppression.
- `crates/editor/src/navigation.rs` — same suppression in `go_to_definition`.
- `crates/project/src/context_server_store.rs:1155-1190` — auto-registers the MCP server.
- `crates/project/src/lsp_store.rs:621-627` — **skips `workspace/didChangeConfiguration` for dbt-lsp**.
- `crates/zed/src/main.rs:201-206` — intercepts `--dbt-mcp`.
- `crates/settings_ui/src/page_data.rs:8825+` — the dbt settings page, `PathPick` file/dir pickers.

---

## 4. How lineage actually works

Three layers, in precedence order:

1. **AST column lineage** (`lineage_sql.rs`) — the good path. Parses each model's
   `compiled_code` with `sqlparser`, resolving CTEs as real scopes, joins
   per-alias, derived tables and scalar subqueries (depth cap 24), positional
   UNION merging, and `*` / `alias.*` expansion. Stored as `col_lineage`.
   **Measured coverage: 1709/2216 catalog columns (77%)** on the reference
   project — the remaining 23% fall through to layer 3. Don't claim "every
   column"; the `#[ignore]`d corpus benchmark in `lineage_sql.rs` is how that
   number is obtained (it hardcodes an absolute project path in the test
   source, so it only runs on a machine that has one).
2. **Per-hop expressions** (`lineage.rs:787-830`) — every `select` in the
   compiled SQL scanned for `(column, expression)` pairs, merged **first-wins**
   so the deepest CTE definition survives, with one chaining pass so a bare
   rename resolves to the real expression. Truncated to 160 chars.
3. **Name/reference heuristic** (`results_panel.rs:34-74`) — fallback. Closes
   over identifier references transitively and links by *name*. **This can
   over-link** (an unrelated upstream column with the same name gets drawn).
   The AST path exists to avoid that, and wins *negatively* too — "no link from
   this parent" is respected.

**Honest limits.** `column_lineage` returns `None` (whole model → heuristic)
when the SQL parses under neither dialect, there is no `Query`, or the scope is
empty. Within a parsed model: an unaliased projection that is not a plain
column reference — bare or qualified — (`SELECT sum(amount)` with no `AS`) is
dropped entirely; `VALUES` returns `None`; table
functions / `UNNEST` / `PIVOT` are ignored. Expression node kinds outside the
visitor list contribute nothing (`lineage_sql.rs:423` `_ => {}`).

**Ops badges** (`⋈ Σ σ ƒ D ∪`) come from `extract_ops`, a **string scan**, not a
parser — its own doc comment says so. The union count over-counts past one
`UNION ALL`.

Rebuild triggers on **`manifest.json` *or* `catalog.json` mtime change**.

---

## 5. Settings — 15 keys under `"dbt"`

Defined `settings_content.rs:1248-1328`, resolved `dbt_settings.rs:41-79`,
defaults `assets/settings/default.json:130-164`.

`show_limit` 500 · `binary` `"dbt"` · `auto_install` true · `fusion_version`
`"latest"` · `distribution` `"fusion"` · `core_adapter` `""` · `target` `""` ·
`profiles_dir` `""` · `env` `{}` · `project_dir` `""` · `parse_on_load` true ·
`env_file` `""` · `lineage_depth` 4 · `lineage_tree_depth` 8 ·
`lineage_max_nodes` 500.

Empty string == unset for the string settings (the resolver filters empties
before applying fallbacks).

**Traps**

- **Project-scoped dbt settings do not work.** Every consumer calls
  `DbtSettings::get_global(cx)`, which merges **user/global settings only**.
  dbt keys in a worktree's `.zed/settings.json` are parsed but never read. The
  settings UI marks five of them `files: USER | PROJECT`
  (`page_data.rs:8970,8993,9016,9039,9058`) but `FileMask` is **UI metadata
  only** — it just decides which page tab shows the widget. *This is a real bug:
  the UI offers a Project tab that silently does nothing.*
- **`profiles_dir` does not expand `~`.** `results_panel.rs` treats a value as
  absolute only via `Path::is_absolute()`, so `"~/.dbt"` becomes
  `<project_root>/~/.dbt` — a literal `~` directory. Use an absolute path.
- **`lineage_max_nodes` is spent differently per surface**: one shared cap
  across both directions for the canvas, but a **per-direction** budget for the
  tree sidebar (so the tree can reach ~2×).
- **dbt task templates bypass `dbt.binary`** — `languages/src/dbt.rs:241` runs a
  bare `dbt` from PATH, ignoring the setting and the managed install.

---

## 6. Credentials & the Connection tab

**Current state** (`connection.rs`, rewritten — do not trust older descriptions
mentioning a `ParamValue` enum, `classify`, or `••••` masking of individual
values; that design was replaced):

`ConnectionInfo` has **no value field**. Only `param_keys: Vec<String>`. Values
are read just far enough to distinguish a mapping from a scalar, then dropped —
so the panel *cannot* leak a credential even if rendering changes. A credential
block (`keyfile_json`) is listed under its own name, never expanded
(`is_secret_key` gates descent only). The UI renders every value as `••••••••`.

Env handling: `.env`/`.env.local` loaded from the project root up to the git
repo root (max 5 levels, never past `.git`), outer-first so inner wins;
configured `env_file` last; a variable already in the real environment is never
overridden. Values go **only** into the spawned dbt process — the log records
just the count, and the UI gets names only.

Tests assert no value (`hunter2`, an account id, even `env_var` or the variable
name) can appear in collected output. There's an `#[ignore]`d smoke test driven
by `DBT_PROFILES_SMOKE` that prints names only.

`profiles.yml` discovery: explicit `dbt.profiles_dir` → project-local
`local_profiles/`, `profiles/`, `.dbt/` → (no flag passed) dbt's own
`DBT_PROFILES_DIR` / `~/.dbt`.

---

## 7. MCP server

Six tools: `dbt_list_models`, `dbt_model_info`, `dbt_lineage` (depth 3, clamp
1-12), `dbt_column_lineage`, `dbt_show` (limit 50, clamp 1-500), `dbt_compile`.
Every tool requires an absolute `project_root` containing `dbt_project.yml`.

Auto-registered by `context_server_store.rs` when a visible worktree has
`dbt_project.yml` at root or one level down (scans ≤64 entries); a user-defined
`"dbt"` entry always wins. `main()` intercepts `--dbt-mcp`, so the same binary
serves any MCP client.

**Caveats**: runs headless with `auto_install: false` and hardcoded
`show_limit` 100 (`mcp.rs:28`), using Fusion defaults rather than the user's Zed
settings. `dbt_show` hits the real warehouse.

**Trap**: `README.md:110,116` tells users to run
`/Applications/zdbt.app/Contents/MacOS/**zdbt** --dbt-mcp`. **That file does not
exist** — the executable is still `zed`. Any copy-pasted command fails.

---

## 8. Known bugs and rough edges

1. **`⌘⏎` is bound on macOS only.** `assets/keymaps/default-macos.json:653,660`
   binds `cmd-k d` and `cmd-enter` to `dbt::ShowModelData`;
   **`default-linux.json` and `default-windows.json` contain zero dbt
   bindings.** The headline interaction does nothing on two of the three
   platforms that are shipped and equally advertised. Linux/Windows users must
   run *dbt: show model data* from the command palette or bind it themselves.
   Fixing this is a two-line keymap change.
2. **README MCP path is broken** (§7) — `MacOS/zdbt` → `MacOS/zed`.
3. **Project-scoped dbt settings silently ignored** (§5).
4. **`SHA256SUMS.txt` is uploaded by hand** — no CI step; `grep -riE 'sha256sum|SHA256SUMS' .github/` finds nothing. It happens to be correct today.
5. **Landing page install text is wrong for macOS 15+.** Apple removed the
   Control-click→Open bypass; users need System Settings → Privacy & Security →
   **Open Anyway**. The page still says "right-click → Open", which loops.
6. **`shasum -c SHA256SUMS.txt` fails on a good download** — the file lists all
   three artifacts. Must show `--ignore-missing`.
7. **Linux tarball unpacks as `zed-dev.app`**, with `bin/zed`,
   `libexec/zed-editor`, `dev.zed.Zed-Dev.desktop`. Linux/Windows bundles are
   still Zed-Dev branded (only macOS got zdbt branding).
8. **zdbt shares Zed's settings and data directory** — `paths::APP_NAME` was
   never changed, so it reads the same `settings.json`/keymap as official Zed.
   Only sqlite state is separated (by `dev` channel scope). The macOS `.app`
   coexists with `Zed.app`; auto-update never polls (dev channel).
9. **No macOS Intel and no Linux arm64 build** — CI ships exactly three
   artifacts.
10. `xattr -d com.apple.quarantine` must be **`-dr`** (recursive) or nested
   helpers (`Contents/MacOS/cli`, `git`) stay quarantined.

---

## 9. Hard-won debugging lessons

- **dbt Fusion's LSP dies on `workspace/didChangeConfiguration`** — it treats it
  as a fatal routing error and exits. The fork patches `lsp_store` to skip it
  for dbt-lsp. Don't "fix" that skip.
- **dbt LSP gates `definition`/`references` behind a full project compile**
  (120 s+ measured). That's why `ref()`/CTE navigation uses **local resolvers**
  and why the references-fallback is suppressed in dbt SQL buffers — otherwise a
  cmd-click that resolves nothing triggers a minutes-long search.
- **Never read the `Workspace` entity during its own update.** `http_client()`
  once did and panicked with *"cannot read Workspace while it is already being
  updated"*. Use `client::Client::global(cx).http_client()`. Find these via the
  stderr backtrace (`gpui/src/app/entity_map.rs:164`).
- **GPUI scroll**: a scrollable strip needs an **explicit total pixel width** or
  cells compress to the viewport and `max_offset` stays 0. Use
  `restrict_scroll_to_axis` so the two axes don't fight, and render scrollbars as
  **sibling absolute overlays** reading the same handle — not on the scroll
  container. This took many failed attempts; see commits `f6bfbb4472`,
  `e262315230`, `4b4650927c`.
- **`uniform_list` assumes a fixed row height** — multi-line cells overlap.
  Cells are collapsed to one line and capped at 200 chars, with click-to-inspect
  for the full value.
- **Ordering bug pattern**: state-reset code running *after* a centring call
  stomped the pan and made "centre on model" work only on the second click.
  Centring must come last.
- Logs: `~/Library/Logs/Zed/Zed.log`, `ZED_LOG` env var.
- A stray non-ASCII character in a user's model SQL (`àwith_previous_hash`) once
  presented as "macros aren't loading". Read the actual dbt stderr first.

---

## 10. State as of this handoff

Branch `dbt`, HEAD `7231174d4c` *"fix: single-line clipped grid cells; add zebra
striping"*.

**Uncommitted** (the Connection tab, not yet committed or released):

```
 M Cargo.lock
 M crates/dbt_ui/Cargo.toml          # + serde_yaml_ng
 M crates/dbt_ui/src/dbt_install.rs  # + path_lookup, describe_binary
 M crates/dbt_ui/src/dbt_ui.rs       # + pub mod connection
 M crates/dbt_ui/src/results_panel.rs
?? crates/dbt_ui/src/connection.rs
```

`cargo test -p dbt_ui` → 12 pass, 2 ignored.

Released: `zdbt-v0.1.1` (latest) and `zdbt-v0.1.0`, each with all four assets;
all download links verified HTTP 200 and the published checksums verified
byte-for-byte against a real download.

**Open work**: the landing page still describes v0.1.0 and needs the corrected
install/verify text from §8; a `zdbt-v0.2.0` release covering everything since
v0.1.1 has not been cut.
