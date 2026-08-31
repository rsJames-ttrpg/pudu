# Pudu S1 — Lockfile Parser & Instance Graph Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `pnpm-lock.yaml` v9 into a typed, validated instance graph — one node per snapshot key — exposed through a hidden `pudu debug print-graph`.

**Architecture:** A new `src/lock/` module with four files: `types.rs` (serde structs, no logic), `snapshot_key.rs` (the recursive peer grammar plus a port of pnpm's `depPathToFilename`), `graph.rs` (node/edge construction, the npm-alias rule, cycle detection), and `mod.rs` (the parse entry point, version gate, and feature gate). `src/cli/debug.rs` renders the graph as JSON.

**Tech Stack:** Rust 2024 · `serde` + `serde_norway` (YAML) · `serde_json` · `sha2` (new — required by pnpm's naming algorithm) · `thiserror` + `miette` · `insta` for snapshots.

**Spec:** `docs/superpowers/specs/2026-08-31-pudu-s1-lockfile-design.md` — the authority. Where this plan and the spec disagree, the spec wins; report the conflict rather than guessing.

**Evidence:** `docs/superpowers/research/2026-08-31-pnpm-lock-v9-field-survey.md` — every non-obvious rule here was derived from real lockfiles and is justified there.

## Global Constraints

- **MSRV is 1.88.** `cargo check --all-targets` must pass under `rustup run 1.88`. Let-chains are available; nothing newer is.
- **`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` must be clean** at every commit.
- **Determinism is an invariant.** `BTreeMap`/`BTreeSet` only — **no `HashMap` or `HashSet` anywhere in `src/lock/`**, including internal scratch structures whose iteration order could reach output.
- **Peers are never sorted.** pnpm hashes the lockfile's own peer order; sorting breaks byte-compatibility with the real virtual store. This reverses an earlier draft — do not "fix" it back.
- **Errors name the specific thing that failed** — the snapshot key, and the link name where one applies. Never a bare line number.
- **New typed errors are registered in `src/error.rs`'s `typed_errors!` macro**, which is the single registration point. Malformed lockfiles are `ExitCode::InputInvalid` (3).
- **No network, no filesystem writes** anywhere in S1. Reading the lockfile and `pudu.toml` is the only I/O.
- **Unknown fields on lockfile structs are tolerated** (no `deny_unknown_fields`) — pnpm adds fields between releases. Unknown *top-level* keys warn.
- Existing code style: `//!` module docs, `///` on public items, comments explain *why*. Match `src/config.rs` and `src/platform.rs`.

---

## File Structure

| file | responsibility |
|---|---|
| `src/lock/mod.rs` | `parse_lockfile()`; version gate; feature gate; re-exports |
| `src/lock/types.rs` | serde structs only — no logic |
| `src/lock/snapshot_key.rs` | peer grammar; `target_name()` (pnpm port) |
| `src/lock/graph.rs` | `Graph::build()`; alias rule; cycle detection |
| `src/cli/debug.rs` | `print-graph` |
| `src/error.rs` | *modify* — add `LockError`, `LockWarning`, register |
| `src/cli/mod.rs` | *modify* — real `Debug` subcommand |
| `src/lib.rs` | *modify* — `pub mod lock;` |

Task order follows the dependency chain: types → key grammar → naming → graph → cycles → CLI → differential test.

---

### Task 1: Lockfile types, version gate, and feature gate

**Files:**
- Create: `src/lock/types.rs`, `src/lock/mod.rs`
- Modify: `src/lib.rs` (add `pub mod lock;`), `src/error.rs` (add + register `LockError`, `LockWarning`)
- Test: unit tests in `src/lock/mod.rs` and `src/lock/types.rs`

**Interfaces:**
- Produces: `pudu::lock::{Lockfile, Settings, Importer, ImporterDep, PackageMeta, PeerMeta, Resolution, SnapshotEntry, parse_lockfile}`; `pudu::error::{LockError, LockWarning}`.
- `pub fn parse_lockfile(text: &str, path: &Path) -> Result<(Lockfile, Vec<LockWarning>), LockError>`

**Context:** `src/error.rs` already anticipates this — its `typed_errors!` doc comment says "Adding `LockfileError` in S1 is one line here." Follow `ConfigError` in that file for the `thiserror` + `miette` derive style, including `#[diagnostic(code(...), help(...))]`.

- [ ] **Step 1: Write the failing tests**

In `src/lock/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn parse(text: &str) -> Result<(Lockfile, Vec<LockWarning>), LockError> {
        parse_lockfile(text, Path::new("/repo/pnpm-lock.yaml"))
    }

    const MINIMAL: &str = "lockfileVersion: '9.0'\nimporters:\n  .:\n    dependencies: {}\n";

    #[test]
    fn accepts_quoted_and_unquoted_version() {
        assert!(parse(MINIMAL).is_ok());
        assert!(parse("lockfileVersion: 9.0\nimporters: {}\n").is_ok());
    }

    #[test]
    fn rejects_v6_naming_version_and_upgrade_path() {
        let err = parse("lockfileVersion: '6.0'\nimporters: {}\n").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("6.0"), "must name the version found: {msg}");
        assert!(msg.contains('9'), "must name the supported version: {msg}");
    }

    #[test]
    fn rejects_absent_version_as_absent() {
        let err = parse("importers: {}\n").unwrap_err();
        assert!(format!("{err}").contains("absent"), "{err}");
    }

    #[test]
    fn patched_dependencies_is_an_error() {
        let text = format!("{MINIMAL}patchedDependencies:\n  foo@1.0.0:\n    hash: abc\n    path: patches/foo.patch\n");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, LockError::PatchedDependencies));
    }

    #[test]
    fn excluded_links_is_an_error() {
        let text = format!("{MINIMAL}settings:\n  excludeLinksFromLockfile: true\n");
        let err = parse(&text).unwrap_err();
        assert!(matches!(err, LockError::ExcludedLinks));
    }

    #[test]
    fn catalogs_and_overrides_are_tolerated_silently() {
        let text = format!("{MINIMAL}catalogs:\n  default:\n    react:\n      specifier: ^18\n      version: 18.3.1\noverrides:\n  foo: 1.0.0\n");
        let (_, warnings) = parse(&text).expect("must parse");
        assert!(warnings.is_empty(), "must not warn: {warnings:?}");
    }

    #[test]
    fn unknown_top_level_key_warns_and_continues() {
        let text = format!("{MINIMAL}someFutureKey: 1\n");
        let (_, warnings) = parse(&text).expect("must still parse");
        assert!(
            warnings.iter().any(|w| matches!(w, LockWarning::UnknownTopLevelKey { key } if key == "someFutureKey")),
            "{warnings:?}"
        );
    }

    #[test]
    fn deprecated_package_warns_naming_the_key() {
        let text = format!(
            "{MINIMAL}packages:\n  glob@10.4.5:\n    resolution: {{integrity: sha512-x}}\n    deprecated: no longer supported\n"
        );
        let (_, warnings) = parse(&text).unwrap();
        assert!(
            warnings.iter().any(|w| matches!(w, LockWarning::DeprecatedPackage { key, .. } if key == "glob@10.4.5")),
            "{warnings:?}"
        );
    }

    #[test]
    fn settings_default_when_absent() {
        let (lf, _) = parse(MINIMAL).unwrap();
        assert!(lf.settings.auto_install_peers, "defaults to true");
        assert!(!lf.settings.exclude_links_from_lockfile);
    }
}
```

In `src/lock/types.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_platform_tokens_parse_without_error() {
        // The survey found `cpu: [wasm32]` in the wild. Unknown tokens must
        // parse and later match no platform, never fail the parse.
        let m: PackageMeta = serde_norway::from_str(
            "resolution: {integrity: sha512-x}\ncpu: [wasm32]\nos: [freebsd]\nlibc: [musl]\n",
        )
        .expect("unknown tokens must parse");
        assert_eq!(m.cpu.as_deref(), Some(&["wasm32".to_string()][..]));
        assert_eq!(m.libc.as_deref(), Some(&["musl".to_string()][..]));
    }

    #[test]
    fn each_resolution_variant_deserializes() {
        let i: PackageMeta = serde_norway::from_str("resolution: {integrity: sha512-abc}\n").unwrap();
        assert!(matches!(i.resolution, Resolution::Integrity { .. }));
        let t: PackageMeta = serde_norway::from_str("resolution: {tarball: https://x/y.tgz}\n").unwrap();
        assert!(matches!(t.resolution, Resolution::Tarball { .. }));
        let d: PackageMeta = serde_norway::from_str("resolution: {directory: ../lib, type: directory}\n").unwrap();
        assert!(matches!(d.resolution, Resolution::Directory { .. }));
    }

    #[test]
    fn unrecognised_resolution_is_an_error() {
        let e = serde_norway::from_str::<PackageMeta>("resolution: {mystery: 1}\n");
        assert!(e.is_err(), "an unknown resolution shape must not parse");
    }

    #[test]
    fn bundled_dependencies_parse_and_are_inert() {
        // The survey established pnpm already omits bundled names from the
        // snapshot graph, so pudu only has to tolerate the field.
        let m: PackageMeta = serde_norway::from_str(
            "resolution: {integrity: sha512-x}\nbundledDependencies:\n  - tslib\n",
        )
        .unwrap();
        assert_eq!(m.bundled_dependencies, vec!["tslib".to_string()]);
    }

    #[test]
    fn unknown_package_fields_are_tolerated() {
        let m: PackageMeta =
            serde_norway::from_str("resolution: {integrity: sha512-x}\nfutureField: 7\n").unwrap();
        assert!(matches!(m.resolution, Resolution::Integrity { .. }));
    }

    #[test]
    fn has_bin_defaults_false_and_reads_true() {
        let a: PackageMeta = serde_norway::from_str("resolution: {integrity: sha512-x}\n").unwrap();
        assert!(!a.has_bin);
        let b: PackageMeta =
            serde_norway::from_str("resolution: {integrity: sha512-x}\nhasBin: true\n").unwrap();
        assert!(b.has_bin);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib lock::`
Expected: FAIL — `src/lock/` does not exist.

- [ ] **Step 3: Add the `sha2` dependency**

Task 3 needs it; adding it now keeps `Cargo.toml` churn in one place.

```bash
cargo add sha2@0.10
```

- [ ] **Step 4: Write `src/lock/types.rs`**

```rust
//! Typed `pnpm-lock.yaml` v9 structures.
//!
//! Deserialization only — no logic lives here. Field choices are justified by
//! the v9 field survey in `docs/superpowers/research/`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A parsed lockfile.
///
/// `BTreeMap` throughout: determinism is an invariant (design §5), and it
/// should come from the data structure rather than a sort at the emit site.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Lockfile {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub importers: BTreeMap<String, Importer>,
    #[serde(default)]
    pub packages: BTreeMap<String, PackageMeta>,
    #[serde(default)]
    pub snapshots: BTreeMap<String, SnapshotEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// pnpm's own default is true.
    #[serde(default = "default_true")]
    pub auto_install_peers: bool,
    /// When true the lockfile omits `link:` dependencies entirely, which
    /// pudu refuses — see `LockError::ExcludedLinks`.
    #[serde(default)]
    pub exclude_links_from_lockfile: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { auto_install_peers: true, exclude_links_from_lockfile: false }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Importer {
    #[serde(default)]
    pub dependencies: BTreeMap<String, ImporterDep>,
    #[serde(default)]
    pub dev_dependencies: BTreeMap<String, ImporterDep>,
    #[serde(default)]
    pub optional_dependencies: BTreeMap<String, ImporterDep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImporterDep {
    /// The range as written: `^4.19.2`, `workspace:*`, `catalog:`.
    pub specifier: String,
    /// Encoded exactly like a snapshot edge value — see `graph::resolve_edge`.
    pub version: String,
}

/// Version-level metadata, from the `packages:` table.
///
/// Survey §1: `packages:` keys are never peer-suffixed, so this is looked up
/// by a snapshot key's `base()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageMeta {
    pub resolution: Resolution,
    #[serde(default)]
    pub engines: BTreeMap<String, String>,
    /// Raw npm strings, negation (`!win32`) intact. Kept as `String` because
    /// the survey found `cpu: [wasm32]`: an unknown token must parse and match
    /// no configured platform, never fail. S2 interprets these.
    #[serde(default)]
    pub os: Option<Vec<String>>,
    #[serde(default)]
    pub cpu: Option<Vec<String>>,
    #[serde(default)]
    pub libc: Option<Vec<String>>,
    /// Survived into v9 as a bare boolean. Not a bin map — S3 uses it only to
    /// cross-check the vendor pass.
    #[serde(default)]
    pub has_bin: bool,
    #[serde(default)]
    pub deprecated: Option<String>,
    #[serde(default)]
    pub peer_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub peer_dependencies_meta: BTreeMap<String, PeerMeta>,
    /// Parsed, never acted on: pnpm already omits bundled names from the
    /// snapshot graph, so they never become edges (survey Q2).
    #[serde(default)]
    pub bundled_dependencies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PeerMeta {
    #[serde(default)]
    pub optional: bool,
}

/// How a package's bytes are obtained. The variant is chosen by which key is
/// present; an unrecognised shape fails to deserialize.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", untagged, deny_unknown_fields)]
pub enum Resolution {
    Integrity { integrity: String },
    Tarball { tarball: String },
    Git { repo: String, commit: String },
    Directory {
        directory: String,
        #[serde(rename = "type")]
        kind: String,
    },
}

/// Resolved dependency edges, from the `snapshots:` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotEntry {
    /// Link name -> edge value. The value is *not* always a bare version;
    /// see the alias rule in `graph::resolve_edge`.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub transitive_peer_dependencies: Vec<String>,
}
```

- [ ] **Step 5: Add `LockError` and `LockWarning` to `src/error.rs`**

Place them beside `ConfigError`, matching its derive style, and register in the macro.

```rust
/// Lockfile parse and graph-construction failures.
///
/// A malformed lockfile is an *input* error, like a malformed `pudu.toml` —
/// both are files the user hands pudu, so both exit 3.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum LockError {
    #[error("unsupported lockfileVersion: {}", .found.as_deref().unwrap_or("absent"))]
    #[diagnostic(
        code(pudu::lock::unsupported_version),
        help("pudu supports lockfileVersion 9.0. Run `pnpm install` with pnpm 9 or newer to upgrade this lockfile.")
    )]
    UnsupportedVersion { found: Option<String> },

    #[error("could not parse {path}")]
    #[diagnostic(code(pudu::lock::yaml))]
    Yaml {
        path: std::path::PathBuf,
        #[source]
        source: serde_norway::Error,
    },

    #[error("invalid snapshot key `{key}` at byte {offset}: {reason}")]
    #[diagnostic(code(pudu::lock::key_parse))]
    KeyParse { key: String, offset: usize, reason: String },

    #[error("snapshot `{snapshot}` has no entry under `packages:` for `{base}`")]
    #[diagnostic(
        code(pudu::lock::missing_package_meta),
        help("The lockfile is inconsistent. Re-run `pnpm install` to regenerate it.")
    )]
    MissingPackageMeta { snapshot: String, base: String },

    #[error("`{from}` depends on `{link_name}`, which resolves to `{resolved}` — absent from `snapshots:`")]
    #[diagnostic(
        code(pudu::lock::unresolved_edge),
        help("The lockfile is inconsistent. Re-run `pnpm install` to regenerate it.")
    )]
    UnresolvedEdge { from: String, link_name: String, resolved: String },

    #[error("`{a}` and `{b}` both map to the Buck target name `{target}`")]
    #[diagnostic(code(pudu::lock::target_name_collision))]
    TargetNameCollision { a: String, b: String, target: String },

    #[error("this lockfile uses patchedDependencies, which pudu cannot reproduce")]
    #[diagnostic(
        code(pudu::lock::patched_dependencies),
        help("A patch changes a package's contents, so ignoring it would emit a build that silently does not match your source. Remove the patch, or wait for pudu to support it.")
    )]
    PatchedDependencies,

    #[error("this lockfile was written with excludeLinksFromLockfile: true")]
    #[diagnostic(
        code(pudu::lock::excluded_links),
        help("`link:` dependencies are omitted from the lockfile, so the dependency graph would be silently incomplete. Set excludeLinksFromLockfile=false in .npmrc and re-run `pnpm install`.")
    )]
    ExcludedLinks,
}

/// Non-fatal lockfile observations.
#[derive(Debug, Clone, PartialEq, thiserror::Error, miette::Diagnostic)]
pub enum LockWarning {
    #[error("unrecognised top-level key `{key}` in the lockfile")]
    #[diagnostic(
        code(pudu::lock::unknown_top_level_key),
        help("pudu does not read this key. If it changes how dependencies resolve, the generated build may be wrong.")
    )]
    UnknownTopLevelKey { key: String },

    #[error("`{key}` is deprecated: {message}")]
    #[diagnostic(code(pudu::lock::deprecated_package))]
    DeprecatedPackage { key: String, message: String },
}
```

Register it — one line, as the macro's doc comment promised:

```rust
typed_errors! {
    CliError => CliError::exit_code,
    ConfigError => |_| ExitCode::InputInvalid,
    DeriveError => |_| ExitCode::InputInvalid,
    LockError => |_| ExitCode::InputInvalid,
}
```

If a test asserts `REGISTERED_ERRORS` coverage against a sample set, add a `LockError` sample to it.

- [ ] **Step 6: Write `src/lock/mod.rs`**

```rust
//! Reading `pnpm-lock.yaml` v9.

pub mod graph;
pub mod snapshot_key;
pub mod types;

use std::path::Path;

use serde::Deserialize;

use crate::error::{LockError, LockWarning};
pub use graph::{Edge, EdgeKind, Graph, Node, Root, RootKind};
pub use snapshot_key::SnapshotKey;
pub use types::*;

/// The only lockfile version pudu reads.
pub const SUPPORTED_VERSION: &str = "9.0";

/// Top-level keys pudu knows about. Anything else warns (§8.2).
///
/// `catalogs`/`overrides` are listed as known-and-ignored: both are already
/// resolved to concrete versions by the time they reach the lockfile, so
/// ignoring them is safe. `patchedDependencies` is known-and-rejected.
const KNOWN_TOP_LEVEL: &[&str] = &[
    "lockfileVersion",
    "settings",
    "importers",
    "packages",
    "snapshots",
    "catalogs",
    "catalog",
    "overrides",
    "patchedDependencies",
    "time",
    "packageExtensionsChecksum",
    "pnpmfileChecksum",
    "ignoredOptionalDependencies",
];

/// `lockfileVersion` may be a YAML string (`'9.0'`) or a bare number (`9.0`).
/// Both occur, and both are legal YAML, so accept either and normalize.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawVersion {
    Str(String),
    Num(f64),
}

impl RawVersion {
    fn normalize(&self) -> String {
        match self {
            Self::Str(s) => s.clone(),
            // `9.0` parses as a float; render it back to the lockfile's own
            // one-decimal spelling rather than "9".
            Self::Num(n) => format!("{n:.1}"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Probe {
    #[serde(rename = "lockfileVersion")]
    lockfile_version: Option<RawVersion>,
    #[serde(rename = "patchedDependencies", default)]
    patched_dependencies: Option<serde_norway::Value>,
}

/// Parse and validate a lockfile.
///
/// Returns the lockfile with any non-fatal warnings. Errors are returned, not
/// printed: the CLI boundary owns rendering.
pub fn parse_lockfile(
    text: &str,
    path: &Path,
) -> Result<(Lockfile, Vec<LockWarning>), LockError> {
    // Gate the version before deserializing the body: a v6 lockfile would
    // otherwise fail with a confusing shape error instead of a clear
    // "upgrade your lockfile".
    let probe: Probe = serde_norway::from_str(text).map_err(|source| LockError::Yaml {
        path: path.to_path_buf(),
        source,
    })?;

    let found = probe.lockfile_version.as_ref().map(RawVersion::normalize);
    if found.as_deref() != Some(SUPPORTED_VERSION) {
        return Err(LockError::UnsupportedVersion { found });
    }

    if probe
        .patched_dependencies
        .as_ref()
        .is_some_and(|v| !matches!(v, serde_norway::Value::Null) && !is_empty_mapping(v))
    {
        return Err(LockError::PatchedDependencies);
    }

    let lockfile: Lockfile = serde_norway::from_str(text).map_err(|source| LockError::Yaml {
        path: path.to_path_buf(),
        source,
    })?;

    if lockfile.settings.exclude_links_from_lockfile {
        return Err(LockError::ExcludedLinks);
    }

    let mut warnings = Vec::new();
    warnings.extend(unknown_top_level_warnings(text));
    for (key, meta) in &lockfile.packages {
        if let Some(message) = &meta.deprecated {
            warnings.push(LockWarning::DeprecatedPackage {
                key: key.clone(),
                message: message.trim().to_string(),
            });
        }
    }

    Ok((lockfile, warnings))
}

fn is_empty_mapping(v: &serde_norway::Value) -> bool {
    matches!(v, serde_norway::Value::Mapping(m) if m.is_empty())
}

/// Warn once per unrecognised top-level key, in document order.
fn unknown_top_level_warnings(text: &str) -> Vec<LockWarning> {
    let Ok(serde_norway::Value::Mapping(map)) = serde_norway::from_str(text) else {
        return Vec::new();
    };
    map.keys()
        .filter_map(|k| k.as_str())
        .filter(|k| !KNOWN_TOP_LEVEL.contains(k))
        .map(|k| LockWarning::UnknownTopLevelKey { key: k.to_string() })
        .collect()
}
```

Add `pub mod lock;` to `src/lib.rs`, keeping the module list alphabetical.

- [ ] **Step 7: Run the tests**

Run: `cargo test --lib lock::`
Expected: PASS.

Then: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(lock): typed v9 lockfile structures with version and feature gates"
```

---

### Task 2: The snapshot-key grammar

**Files:**
- Create: `src/lock/snapshot_key.rs`
- Test: unit tests in the same file

**Interfaces:**
- Consumes: nothing from Task 1 (deliberately standalone — pure string work).
- Produces: `SnapshotKey { name, version, peers }` with `parse`, `base`, `canonical`. `target_name` arrives in Task 3.

**Context:** This is the highest-risk parsing in the stage. Two shortcuts are wrong, both proven against real lockfiles: splitting on the first `(` (nested peers), and splitting the head on the first `@` (scoped names). The survey documents both.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim from a real lockfile — the corpus's longest key at 272 chars.
    /// Nested three deep. If this parses, the grammar is right.
    const LONG_REAL_KEY: &str = "@sveltejs/kit@2.50.1(@sveltejs/vite-plugin-svelte@6.2.4(svelte@5.49.1)(vite@7.3.1(@types/node@22.19.7)(jiti@2.6.1)(lightningcss@1.30.2)(terser@5.46.0)))(svelte@5.49.1)(vite@7.3.1(@types/node@22.19.7)(jiti@2.6.1)(lightningcss@1.30.2)(terser@5.46.0))";

    #[test]
    fn parses_bare_name() {
        let k = SnapshotKey::parse("svelte@5.49.1").unwrap();
        assert_eq!(k.name, "svelte");
        assert_eq!(k.version, "5.49.1");
        assert!(k.peers.is_empty());
    }

    #[test]
    fn parses_scoped_name_without_splitting_on_the_leading_at() {
        let k = SnapshotKey::parse("@babel/core@7.28.6").unwrap();
        assert_eq!(k.name, "@babel/core");
        assert_eq!(k.version, "7.28.6");
    }

    #[test]
    fn parses_single_peer() {
        let k = SnapshotKey::parse("react-dom@18.3.1(react@18.3.1)").unwrap();
        assert_eq!(k.name, "react-dom");
        assert_eq!(k.peers.len(), 1);
        assert_eq!(k.peers[0].name, "react");
        assert_eq!(k.peers[0].version, "18.3.1");
    }

    #[test]
    fn parses_nested_peers() {
        // The shortcut of splitting on the first '(' produces garbage here.
        let k = SnapshotKey::parse(
            "eslint-plugin-svelte@3.14.0(eslint@9.39.2(jiti@2.6.1))(svelte@5.49.1)",
        )
        .unwrap();
        assert_eq!(k.name, "eslint-plugin-svelte");
        assert_eq!(k.peers.len(), 2, "two top-level peers, not three");
        assert_eq!(k.peers[0].name, "eslint");
        assert_eq!(k.peers[0].peers.len(), 1, "eslint carries its own peer");
        assert_eq!(k.peers[0].peers[0].name, "jiti");
        assert_eq!(k.peers[1].name, "svelte");
    }

    #[test]
    fn round_trips_the_long_real_key() {
        let k = SnapshotKey::parse(LONG_REAL_KEY).unwrap();
        assert_eq!(k.name, "@sveltejs/kit");
        assert_eq!(k.version, "2.50.1");
        assert_eq!(k.peers.len(), 3);
        assert_eq!(
            k.canonical(),
            LONG_REAL_KEY,
            "canonical() must reproduce the key exactly, peer order included"
        );
    }

    #[test]
    fn base_strips_the_peer_suffix() {
        let k = SnapshotKey::parse(LONG_REAL_KEY).unwrap();
        assert_eq!(k.base(), "@sveltejs/kit@2.50.1");
        let plain = SnapshotKey::parse("svelte@5.49.1").unwrap();
        assert_eq!(plain.base(), "svelte@5.49.1");
    }

    #[test]
    fn peer_order_is_preserved_not_sorted() {
        // pnpm's naming hashes the lockfile's own order; sorting would make
        // every hashed target name diverge from the real virtual store.
        let k = SnapshotKey::parse("x@1.0.0(b@2.0.0)(a@1.0.0)").unwrap();
        assert_eq!(k.peers[0].name, "b", "must NOT be sorted");
        assert_eq!(k.peers[1].name, "a");
        assert_eq!(k.canonical(), "x@1.0.0(b@2.0.0)(a@1.0.0)");
    }

    #[test]
    fn parses_prerelease_and_build_metadata_versions() {
        assert_eq!(SnapshotKey::parse("x@1.0.0-rc.1").unwrap().version, "1.0.0-rc.1");
        assert_eq!(SnapshotKey::parse("x@1.0.0+build.5").unwrap().version, "1.0.0+build.5");
    }

    #[test]
    fn rejects_malformed_keys() {
        for bad in [
            "x@1.0.0(a@1",       // unbalanced open
            "x@1.0.0)",          // stray close
            "@1.0.0",            // empty name
            "x@",                // empty version
            "noatsign",          // no '@'
            "@scope/name",       // scoped, no version
            "",                  // empty
        ] {
            assert!(
                SnapshotKey::parse(bad).is_err(),
                "must reject {bad:?}"
            );
        }
    }

    #[test]
    fn error_names_the_key() {
        let e = SnapshotKey::parse("x@1.0.0(a@1").unwrap_err();
        assert!(format!("{e}").len() > 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib snapshot_key`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Write `src/lock/snapshot_key.rs`**

```rust
//! The pnpm snapshot-key grammar.
//!
//! ```text
//! key     := name "@" version peers?
//! name    := ("@" scope "/")? ident
//! peers   := "(" key ")" peers?
//! ```
//!
//! A peer is itself a full key, so the grammar is recursive and the parens
//! balance to arbitrary depth — real lockfiles nest three levels and reach
//! 422 characters. Two shortcuts are wrong and both are guarded by tests:
//! splitting on the first `(` ignores nesting, and splitting the head on the
//! first `@` breaks scoped names.

use std::fmt;

/// A parsed snapshot key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotKey {
    pub name: String,
    pub version: String,
    /// In lockfile order. **Never sorted** — pnpm's directory naming hashes
    /// this order, so re-sorting would diverge from the real virtual store.
    pub peers: Vec<SnapshotKey>,
}

/// Why a key failed to parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyParseError {
    pub key: String,
    pub offset: usize,
    pub reason: &'static str,
}

impl fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid snapshot key `{}` at byte {}: {}", self.key, self.offset, self.reason)
    }
}

impl std::error::Error for KeyParseError {}

impl SnapshotKey {
    /// Parse a snapshot key.
    pub fn parse(s: &str) -> Result<Self, KeyParseError> {
        Self::parse_inner(s, s)
    }

    fn parse_inner(s: &str, whole: &str) -> Result<Self, KeyParseError> {
        let err = |offset: usize, reason: &'static str| KeyParseError {
            key: whole.to_string(),
            offset,
            reason,
        };

        let suffix_start = top_level_paren(s, whole)?;
        let (head, suffix) = match suffix_start {
            Some(i) => (&s[..i], &s[i..]),
            None => (s, ""),
        };

        // The split is the LAST '@' at index > 0. Index 0 is excluded because
        // a scoped name starts with '@'.
        let at = head
            .char_indices()
            .filter(|(i, c)| *c == '@' && *i > 0)
            .map(|(i, _)| i)
            .next_back()
            .ok_or_else(|| err(0, "expected `name@version`"))?;

        let (name, version) = (&head[..at], &head[at + 1..]);
        if name.is_empty() {
            return Err(err(0, "empty package name"));
        }
        if version.is_empty() {
            return Err(err(at + 1, "empty version"));
        }

        Ok(Self {
            name: name.to_string(),
            version: version.to_string(),
            peers: parse_peers(suffix, whole)?,
        })
    }

    /// The key with its peer suffix removed — the `packages:` table's key.
    pub fn base(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    /// Render back to lockfile form, peer order preserved.
    pub fn canonical(&self) -> String {
        let mut out = self.base();
        for p in &self.peers {
            out.push('(');
            out.push_str(&p.canonical());
            out.push(')');
        }
        out
    }
}

impl fmt::Display for SnapshotKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// Index of the first `(` at depth 0, or `None`. Errors on unbalanced parens.
fn top_level_paren(s: &str, whole: &str) -> Result<Option<usize>, KeyParseError> {
    let mut depth = 0usize;
    let mut first = None;
    for (i, c) in s.char_indices() {
        match c {
            '(' => {
                if depth == 0 && first.is_none() {
                    first = Some(i);
                }
                depth += 1;
            }
            ')' => {
                depth = depth.checked_sub(1).ok_or(KeyParseError {
                    key: whole.to_string(),
                    offset: i,
                    reason: "unbalanced `)`",
                })?;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(KeyParseError {
            key: whole.to_string(),
            offset: s.len(),
            reason: "unbalanced `(`",
        });
    }
    Ok(first)
}

/// Split `(a)(b)` into its depth-0 groups and parse each recursively.
fn parse_peers(suffix: &str, whole: &str) -> Result<Vec<SnapshotKey>, KeyParseError> {
    let mut peers = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (i, c) in suffix.char_indices() {
        match c {
            '(' => {
                if depth == 0 {
                    start = Some(i + 1);
                }
                depth += 1;
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let from = start.take().expect("a close at depth 0 follows an open");
                    peers.push(SnapshotKey::parse_inner(&suffix[from..i], whole)?);
                }
            }
            _ => {}
        }
    }
    Ok(peers)
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib snapshot_key`
Expected: PASS.

Then: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(lock): recursive snapshot-key grammar with nested peer suffixes"
```

---

### Task 3: Target-name mangling — port pnpm's `depPathToFilename`

**Files:**
- Modify: `src/lock/snapshot_key.rs` (add `target_name`)
- Test: unit tests in the same file

**Interfaces:**
- Consumes: `SnapshotKey` from Task 2.
- Produces: `pub fn target_name(dep_path: &str) -> String` and `SnapshotKey::target_name(&self)`.

**Context — read this before writing code.** This is a **port, not a design**. The reference is `@pnpm/dependency-path` v1001.1.10:

```js
function depPathToFilename(depPath, maxLengthWithoutHash) {
    let filename = depPathToFilenameUnescaped(depPath).replace(/[\\/:*?"<>|#]/g, '+');
    if (filename.includes('(')) {
        filename = filename.replace(/\)$/, '').replace(/\)\(|\(|\)/g, '_');
    }
    if (filename.length > maxLengthWithoutHash ||
        filename !== filename.toLowerCase() && !filename.startsWith('file+')) {
        return `${filename.substring(0, maxLengthWithoutHash - 33)}_${createShortHash(filename)}`;
    }
    return filename;
}
// createShortHash(input) = sha256(input).hex().slice(0, 32)
```

`maxLengthWithoutHash` is pnpm's `virtual-store-dir-max-length`, default **120**. Pudu hardcodes 120.

Four details a from-scratch implementation gets wrong, all verified against real virtual stores:
1. The escape set is `\ / : * ? " < > | #` → `+`, not `/` alone.
2. Peers flatten *readably* to `_` when short; hashing is the fallback.
3. **Uppercase forces the hash path at any length** — the case-insensitive-filesystem guard.
4. **Peers are not sorted.**

Operate on **bytes/chars, not a `SnapshotKey`** — the input is the raw key string, because the transformation is textual.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod target_name_tests {
    use super::*;

    #[test]
    fn plain_name_is_unchanged() {
        assert_eq!(target_name("svelte@5.49.1"), "svelte@5.49.1");
    }

    #[test]
    fn scope_slash_becomes_plus() {
        assert_eq!(target_name("@babel/core@7.28.6"), "@babel+core@7.28.6");
    }

    #[test]
    fn escapes_every_illegal_path_character() {
        // The full set, not just '/'.
        for (raw, want) in [
            ("a/b@1.0.0", "a+b@1.0.0"),
            ("a:b@1.0.0", "a+b@1.0.0"),
            ("a*b@1.0.0", "a+b@1.0.0"),
            ("a?b@1.0.0", "a+b@1.0.0"),
            ("a<b@1.0.0", "a+b@1.0.0"),
            ("a>b@1.0.0", "a+b@1.0.0"),
            ("a|b@1.0.0", "a+b@1.0.0"),
            ("a#b@1.0.0", "a+b@1.0.0"),
        ] {
            assert_eq!(target_name(raw), want, "escaping {raw}");
        }
    }

    #[test]
    fn short_peer_sets_flatten_readably_without_hashing() {
        assert_eq!(
            target_name("vite@7.3.1(@types/node@22.19.7)(terser@5.46.0)"),
            "vite@7.3.1_@types+node@22.19.7_terser@5.46.0",
            "short peers must stay readable, not be hashed"
        );
        assert_eq!(
            target_name("react-dom@18.3.1(react@18.3.1)"),
            "react-dom@18.3.1_react@18.3.1"
        );
    }

    #[test]
    fn uppercase_forces_the_hash_path_even_when_short() {
        let got = target_name("MyPkg@1.0.0");
        assert!(got.starts_with("MyPkg@1.0.0_"), "keeps the stem: {got}");
        assert_eq!(got.len(), "MyPkg@1.0.0".len() + 33, "stem + '_' + 32 hex: {got}");
        assert_ne!(got, "MyPkg@1.0.0", "must not pass through unhashed");
    }

    #[test]
    fn long_names_are_truncated_to_exactly_120() {
        let long = "@sveltejs/kit@2.50.1(@sveltejs/vite-plugin-svelte@6.2.4(svelte@5.49.1)(vite@7.3.1(@types/node@22.19.7)(jiti@2.6.1)(lightningcss@1.30.2)(terser@5.46.0)))(svelte@5.49.1)(vite@7.3.1(@types/node@22.19.7)(jiti@2.6.1)(lightningcss@1.30.2)(terser@5.46.0))";
        let got = target_name(long);
        assert_eq!(got.len(), 120, "pnpm's max length: {got}");
        assert!(got.starts_with("@sveltejs+kit@2.50.1_"), "readable stem survives: {got}");
        let hash = &got[got.len() - 32..];
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "hex tail: {hash}");
        assert_eq!(got.as_bytes()[got.len() - 33], b'_');
    }

    #[test]
    fn hash_is_sha256_of_the_flattened_name_truncated_to_32() {
        use sha2::{Digest, Sha256};
        let flat = "MyPkg@1.0.0";
        let want = format!("{:x}", Sha256::digest(flat.as_bytes()));
        assert_eq!(target_name(flat), format!("{flat}_{}", &want[..32]));
    }

    #[test]
    fn peer_order_changes_the_name() {
        assert_ne!(
            target_name("x@1.0.0(a@1.0.0)(b@2.0.0)"),
            target_name("x@1.0.0(b@2.0.0)(a@1.0.0)"),
            "peers are not sorted, so order is significant"
        );
    }

    #[test]
    fn snapshot_key_method_agrees_with_the_free_function() {
        let raw = "eslint-plugin-svelte@3.14.0(eslint@9.39.2(jiti@2.6.1))(svelte@5.49.1)";
        let k = SnapshotKey::parse(raw).unwrap();
        assert_eq!(k.target_name(), target_name(raw));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib target_name`
Expected: FAIL — `target_name` not found.

- [ ] **Step 3: Implement in `src/lock/snapshot_key.rs`**

```rust
use sha2::{Digest, Sha256};

/// pnpm's `virtual-store-dir-max-length` default.
///
/// Hardcoded for v0.1.0. A project that has changed the pnpm setting gets
/// names that do not match its own store — cosmetic only, since the names
/// stay internally consistent.
pub const MAX_LEN_WITHOUT_HASH: usize = 120;

/// Characters pnpm escapes to `+`: the Windows-illegal set plus `#`.
const ILLEGAL: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|', '#'];

/// The Buck target name for a snapshot key.
///
/// A direct port of `depPathToFilename` from `@pnpm/dependency-path`
/// v1001.1.10, so generated names are byte-identical to the directory names
/// in a real `node_modules/.pnpm/`. Verified against 1363 real directories;
/// see the field survey. **Do not "improve" this** — any deviation breaks
/// greppability against the real store, which is the whole point.
pub fn target_name(dep_path: &str) -> String {
    // pnpm strips a leading '/' (a legacy v5 dep-path form) before escaping.
    let s = dep_path.strip_prefix('/').unwrap_or(dep_path);
    let mut filename: String = s
        .chars()
        .map(|c| if ILLEGAL.contains(&c) { '+' } else { c })
        .collect();

    if filename.contains('(') {
        // Order matters: drop the trailing ')' first, so `)(` -> `_` does not
        // have to account for it.
        if let Some(stripped) = filename.strip_suffix(')') {
            filename = stripped.to_string();
        }
        filename = filename.replace(")(", "_").replace('(', "_").replace(')', "_");
    }

    // Uppercase forces the hash regardless of length: a case-insensitive
    // filesystem would otherwise collapse two distinct packages.
    let needs_hash = filename.len() > MAX_LEN_WITHOUT_HASH
        || (filename != filename.to_lowercase() && !filename.starts_with("file+"));

    if needs_hash {
        let digest = format!("{:x}", Sha256::digest(filename.as_bytes()));
        let keep = MAX_LEN_WITHOUT_HASH - 33;
        // Truncate on a char boundary. Package names are ASCII in practice,
        // but a non-ASCII name must not panic.
        let stem = if filename.len() > keep {
            let mut end = keep;
            while !filename.is_char_boundary(end) {
                end -= 1;
            }
            &filename[..end]
        } else {
            &filename[..]
        };
        return format!("{stem}_{}", &digest[..32]);
    }

    filename
}

impl SnapshotKey {
    /// The Buck target name for this key. See [`target_name`].
    pub fn target_name(&self) -> String {
        target_name(&self.canonical())
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib target_name`
Expected: PASS.

Then: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(lock): port pnpm's depPathToFilename for Buck target names"
```

---

### Task 4: Instance graph construction and the npm-alias rule

**Files:**
- Create: `src/lock/graph.rs`
- Test: unit tests in the same file

**Interfaces:**
- Consumes: `Lockfile`, `PackageMeta`, `SnapshotEntry` (Task 1); `SnapshotKey`, `target_name` (Tasks 2–3).
- Produces: `Graph { nodes: BTreeMap<String, Node>, roots: Vec<Root>, cycles: Vec<Vec<String>> }`, `Node`, `Edge`, `EdgeKind`, `Root`, `RootKind`, and `Graph::build(&Lockfile) -> Result<Graph, LockError>`.
- `cycles` is populated in Task 5 — build it as an empty `Vec` here so the type is stable.

**Context — the alias rule is the thing to get right.** A snapshot edge is `link_name: value`. The value is **not always a bare version**:

```yaml
'@isaacs/cliui@8.0.2':
  dependencies:
    string-width: 5.1.2                    # bare version -> string-width@5.1.2
    string-width-cjs: string-width@4.2.3   # ALIAS       -> string-width@4.2.3
```

`link_name` is the directory name under `node_modules/`; the value may name a *different package*. Rule: strip any peer suffix from the value; if what remains still contains `@` beyond position 0, the value is already a complete key — use it verbatim. Otherwise the key is `link_name + "@" + value`.

**`link_name` must be kept on the edge even when it differs from the package name** — the virtual store symlinks content in under the alias, so S4 needs it and cannot reconstruct it.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::parse_lockfile;
    use std::path::Path;

    fn build(yaml: &str) -> Graph {
        let (lf, _) = parse_lockfile(yaml, Path::new("/x/pnpm-lock.yaml")).expect("parses");
        Graph::build(&lf).expect("builds")
    }

    const ALIAS_LOCK: &str = r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      '@isaacs/cliui':
        specifier: ^8.0.2
        version: 8.0.2
packages:
  '@isaacs/cliui@8.0.2':
    resolution: {integrity: sha512-a}
  string-width@5.1.2:
    resolution: {integrity: sha512-b}
  string-width@4.2.3:
    resolution: {integrity: sha512-c}
snapshots:
  '@isaacs/cliui@8.0.2':
    dependencies:
      string-width: 5.1.2
      string-width-cjs: string-width@4.2.3
  string-width@5.1.2: {}
  string-width@4.2.3: {}
"#;

    #[test]
    fn alias_edge_resolves_to_the_aliased_package_and_keeps_the_link_name() {
        let g = build(ALIAS_LOCK);
        let cliui = &g.nodes["@isaacs/cliui@8.0.2"];
        let aliased = cliui
            .edges
            .iter()
            .find(|e| e.link_name == "string-width-cjs")
            .expect("the alias edge must survive under its link name");
        // BOTH halves matter: resolving alone would pass with link_name lost.
        assert_eq!(aliased.target, "string-width@4.2.3", "resolves to the aliased package");
        assert_eq!(aliased.link_name, "string-width-cjs", "link name is retained");

        let plain = cliui.edges.iter().find(|e| e.link_name == "string-width").unwrap();
        assert_eq!(plain.target, "string-width@5.1.2");
    }

    #[test]
    fn peer_suffixed_edge_value_resolves_to_the_suffixed_key() {
        let g = build(r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0:
    resolution: {integrity: sha512-a}
  eslint@9.39.2:
    resolution: {integrity: sha512-b}
  jiti@2.6.1:
    resolution: {integrity: sha512-c}
snapshots:
  a@1.0.0:
    dependencies:
      eslint: 9.39.2(jiti@2.6.1)
  'eslint@9.39.2(jiti@2.6.1)':
    dependencies:
      jiti: 2.6.1
  jiti@2.6.1: {}
"#);
        assert_eq!(g.nodes["a@1.0.0"].edges[0].target, "eslint@9.39.2(jiti@2.6.1)");
    }

    #[test]
    fn peer_instances_are_separate_nodes_sharing_one_packages_entry() {
        let g = build(r#"
lockfileVersion: '9.0'
importers: {}
packages:
  dom@1.0.0:
    resolution: {integrity: sha512-a}
  react@17.0.0:
    resolution: {integrity: sha512-b}
  react@18.0.0:
    resolution: {integrity: sha512-c}
snapshots:
  'dom@1.0.0(react@17.0.0)':
    dependencies: {react: 17.0.0}
  'dom@1.0.0(react@18.0.0)':
    dependencies: {react: 18.0.0}
  react@17.0.0: {}
  react@18.0.0: {}
"#);
        assert!(g.nodes.contains_key("dom@1.0.0(react@17.0.0)"));
        assert!(g.nodes.contains_key("dom@1.0.0(react@18.0.0)"));
        assert_ne!(
            g.nodes["dom@1.0.0(react@17.0.0)"].target_name,
            g.nodes["dom@1.0.0(react@18.0.0)"].target_name,
            "distinct instances need distinct Buck targets"
        );
    }

    #[test]
    fn roots_carry_their_importer_and_kind() {
        let g = build(r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      a: {specifier: ^1, version: 1.0.0}
  packages/app:
    devDependencies:
      a: {specifier: ^1, version: 1.0.0}
    optionalDependencies:
      b: {specifier: ^1, version: 1.0.0}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  b@1.0.0: {resolution: {integrity: sha512-b}}
snapshots:
  a@1.0.0: {}
  b@1.0.0: {}
"#);
        assert_eq!(g.roots.len(), 3);
        assert!(g.roots.iter().any(|r| r.importer == "." && r.kind == RootKind::Prod));
        assert!(g.roots.iter().any(|r| r.importer == "packages/app" && r.kind == RootKind::Dev));
        assert!(g.roots.iter().any(|r| r.kind == RootKind::Optional));
    }

    #[test]
    fn workspace_specifier_root_is_recorded_but_not_resolved() {
        let g = build(r#"
lockfileVersion: '9.0'
importers:
  packages/app:
    dependencies:
      '@fixture/lib': {specifier: 'workspace:*', version: link:../lib}
packages: {}
snapshots: {}
"#);
        let r = &g.roots[0];
        assert_eq!(r.link_name, "@fixture/lib");
        assert!(r.target.is_none(), "a link: root resolves to no node at S1");
        assert_eq!(r.specifier, "workspace:*");
    }

    #[test]
    fn optional_edges_are_tagged() {
        let g = build(r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  b@1.0.0: {resolution: {integrity: sha512-b}}
snapshots:
  a@1.0.0:
    optionalDependencies: {b: 1.0.0}
  b@1.0.0: {}
"#);
        assert_eq!(g.nodes["a@1.0.0"].edges[0].kind, EdgeKind::Optional);
    }

    #[test]
    fn missing_package_metadata_names_snapshot_and_base() {
        let (lf, _) = parse_lockfile(
            "lockfileVersion: '9.0'\nimporters: {}\npackages: {}\nsnapshots:\n  a@1.0.0: {}\n",
            Path::new("/x"),
        )
        .unwrap();
        let e = Graph::build(&lf).unwrap_err();
        let m = format!("{e}");
        assert!(m.contains("a@1.0.0"), "{m}");
    }

    #[test]
    fn unresolved_edge_names_source_link_and_target() {
        let (lf, _) = parse_lockfile(r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
snapshots:
  a@1.0.0:
    dependencies: {ghost: 9.9.9}
"#, Path::new("/x")).unwrap();
        let e = Graph::build(&lf).unwrap_err();
        let m = format!("{e}");
        assert!(m.contains("a@1.0.0") && m.contains("ghost") && m.contains("ghost@9.9.9"), "{m}");
    }

    #[test]
    fn target_name_collision_is_an_error() {
        // Two distinct keys, same mangled name. Constructed directly because
        // a natural collision needs a 48-bit-plus coincidence.
        let a = "x@1.0.0(a@1.0.0)";
        let b = "x@1.0.0_a@1.0.0"; // mangles to the same string
        assert_eq!(crate::lock::snapshot_key::target_name(a), crate::lock::snapshot_key::target_name(b));
    }

    #[test]
    fn edges_are_sorted_by_link_name() {
        let g = build(r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  m@1.0.0: {resolution: {integrity: sha512-m}}
  z@1.0.0: {resolution: {integrity: sha512-z}}
snapshots:
  a@1.0.0:
    dependencies: {z: 1.0.0, m: 1.0.0}
  m@1.0.0: {}
  z@1.0.0: {}
"#);
        let names: Vec<_> = g.nodes["a@1.0.0"].edges.iter().map(|e| e.link_name.as_str()).collect();
        assert_eq!(names, vec!["m", "z"], "deterministic order");
    }
}
```

> **Note on `target_name_collision_is_an_error`:** the test above only asserts
> that the two strings collide. Extend it to drive `Graph::build` through a
> lockfile containing both keys and assert `LockError::TargetNameCollision`.
> If those two inputs do not in fact collide under the ported algorithm, find
> a pair that does (or construct the collision by calling the builder's
> internal insert directly) — do not delete the test.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib graph`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Write `src/lock/graph.rs`**

```rust
//! The instance graph: one node per snapshot key.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::LockError;
use crate::lock::snapshot_key::SnapshotKey;
use crate::lock::types::{Lockfile, PackageMeta};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Graph {
    /// Keyed by canonical snapshot key.
    pub nodes: BTreeMap<String, Node>,
    pub roots: Vec<Root>,
    /// Populated by cycle detection. Cycles are normal — see the S1 spec §7.
    pub cycles: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    pub name: String,
    pub version: String,
    pub peers: Vec<String>,
    pub target_name: String,
    pub optional: bool,
    pub meta: PackageMeta,
    /// Sorted by `link_name` for determinism.
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Edge {
    /// The directory name under `node_modules/`. May differ from the target
    /// package's own name when the dependency is an npm alias.
    pub link_name: String,
    /// Canonical snapshot key of the dependency.
    pub target: String,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    Prod,
    Optional,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Root {
    pub importer: String,
    pub link_name: String,
    /// `None` for `link:`/`file:`/`workspace:` roots, which resolve to another
    /// importer rather than a package. S5 makes those real.
    pub target: Option<String>,
    pub specifier: String,
    pub kind: RootKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RootKind {
    Prod,
    Dev,
    Optional,
}

/// Resolve one dependency edge to a snapshot key.
///
/// The value is not always a bare version — npm aliases encode a complete
/// `name@version`, in which case `link_name` is only a directory name:
///
/// ```text
/// string-width:     5.1.2                -> string-width@5.1.2
/// string-width-cjs: string-width@4.2.3   -> string-width@4.2.3
/// eslint:           9.39.2(jiti@2.6.1)   -> eslint@9.39.2(jiti@2.6.1)
/// ```
pub fn resolve_edge(link_name: &str, value: &str) -> String {
    let head = strip_peer_suffix(value);
    // An '@' beyond position 0 in the head means the value already names a
    // package. Position 0 is excluded so a scoped alias target still works.
    if head.char_indices().any(|(i, c)| c == '@' && i > 0) {
        value.to_string()
    } else {
        format!("{link_name}@{value}")
    }
}

fn strip_peer_suffix(s: &str) -> &str {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => {
                if depth == 0 {
                    return &s[..i];
                }
                depth += 1;
            }
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    s
}

/// True for a specifier that points at another importer rather than a package.
fn is_link_specifier(specifier: &str, version: &str) -> bool {
    specifier.starts_with("workspace:")
        || specifier.starts_with("link:")
        || specifier.starts_with("file:")
        || version.starts_with("link:")
        || version.starts_with("file:")
}

impl Graph {
    pub fn build(lockfile: &Lockfile) -> Result<Self, LockError> {
        let mut nodes = BTreeMap::new();
        let mut by_target: BTreeMap<String, String> = BTreeMap::new();

        for (raw_key, entry) in &lockfile.snapshots {
            let key = SnapshotKey::parse(raw_key).map_err(|e| LockError::KeyParse {
                key: e.key,
                offset: e.offset,
                reason: e.reason.to_string(),
            })?;
            let base = key.base();
            let meta = lockfile.packages.get(&base).cloned().ok_or_else(|| {
                LockError::MissingPackageMeta { snapshot: raw_key.clone(), base: base.clone() }
            })?;

            let target_name = key.target_name();
            if let Some(other) = by_target.get(&target_name) {
                return Err(LockError::TargetNameCollision {
                    a: other.clone(),
                    b: raw_key.clone(),
                    target: target_name,
                });
            }
            by_target.insert(target_name.clone(), raw_key.clone());

            let mut edges: Vec<Edge> = entry
                .dependencies
                .iter()
                .map(|(n, v)| (n, v, EdgeKind::Prod))
                .chain(
                    entry
                        .optional_dependencies
                        .iter()
                        .map(|(n, v)| (n, v, EdgeKind::Optional)),
                )
                .map(|(link_name, value, kind)| Edge {
                    link_name: link_name.clone(),
                    target: resolve_edge(link_name, value),
                    kind,
                })
                .collect();
            edges.sort_by(|a, b| a.link_name.cmp(&b.link_name));

            nodes.insert(
                raw_key.clone(),
                Node {
                    name: key.name.clone(),
                    version: key.version.clone(),
                    peers: key.peers.iter().map(SnapshotKey::canonical).collect(),
                    target_name,
                    optional: entry.optional,
                    meta,
                    edges,
                },
            );
        }

        // Validate every edge target after all nodes exist, so a forward
        // reference is not mistaken for a dangling one.
        for (from, node) in &nodes {
            for edge in &node.edges {
                if !nodes.contains_key(&edge.target) {
                    return Err(LockError::UnresolvedEdge {
                        from: from.clone(),
                        link_name: edge.link_name.clone(),
                        resolved: edge.target.clone(),
                    });
                }
            }
        }

        let mut roots = Vec::new();
        for (importer, imp) in &lockfile.importers {
            let groups = [
                (&imp.dependencies, RootKind::Prod),
                (&imp.dev_dependencies, RootKind::Dev),
                (&imp.optional_dependencies, RootKind::Optional),
            ];
            for (deps, kind) in groups {
                for (link_name, dep) in deps {
                    let target = if is_link_specifier(&dep.specifier, &dep.version) {
                        None
                    } else {
                        Some(resolve_edge(link_name, &dep.version))
                    };
                    if let Some(t) = &target
                        && !nodes.contains_key(t)
                    {
                        return Err(LockError::UnresolvedEdge {
                            from: format!("importer {importer}"),
                            link_name: link_name.clone(),
                            resolved: t.clone(),
                        });
                    }
                    roots.push(Root {
                        importer: importer.clone(),
                        link_name: link_name.clone(),
                        target,
                        specifier: dep.specifier.clone(),
                        kind,
                    });
                }
            }
        }

        Ok(Self { nodes, roots, cycles: Vec::new() })
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib graph`
Expected: PASS.

Then: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(lock): instance graph with npm-alias edge resolution"
```

---

### Task 5: Cycle detection

**Files:**
- Modify: `src/lock/graph.rs`
- Test: unit tests in the same file

**Interfaces:**
- Consumes: `Graph` from Task 4.
- Produces: `Graph.cycles` populated by `Graph::build`.

**Context:** Cycles are **normal and must not error**. The survey found them in every real lockfile — `@babel/core` ↔ `@babel/helper-module-transforms`, `eslint` ↔ `@eslint-community/eslint-utils`, `browserslist` ↔ `update-browserslist-db`. Rejecting them would reject nearly every real project. They are also **not** a warning: warning on every run is noise.

**Use an iterative DFS.** A recursive one risks stack overflow on an 800-node graph with deep chains, and the test below proves it.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod cycle_tests {
    use super::*;
    use crate::lock::parse_lockfile;
    use std::path::Path;

    fn build(yaml: &str) -> Graph {
        let (lf, _) = parse_lockfile(yaml, Path::new("/x")).unwrap();
        Graph::build(&lf).unwrap()
    }

    #[test]
    fn two_node_cycle_is_recorded_and_does_not_error() {
        let g = build(r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  b@1.0.0: {resolution: {integrity: sha512-b}}
snapshots:
  a@1.0.0:
    dependencies: {b: 1.0.0}
  b@1.0.0:
    dependencies: {a: 1.0.0}
"#);
        assert_eq!(g.cycles.len(), 1, "one cycle: {:?}", g.cycles);
        let c = &g.cycles[0];
        assert!(c.contains(&"a@1.0.0".to_string()) && c.contains(&"b@1.0.0".to_string()));
    }

    #[test]
    fn self_edge_is_a_cycle_of_length_one() {
        let g = build(r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
snapshots:
  a@1.0.0:
    dependencies: {a: 1.0.0}
"#);
        assert_eq!(g.cycles.len(), 1, "{:?}", g.cycles);
    }

    #[test]
    fn an_acyclic_graph_reports_no_cycles() {
        let g = build(r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  b@1.0.0: {resolution: {integrity: sha512-b}}
snapshots:
  a@1.0.0:
    dependencies: {b: 1.0.0}
  b@1.0.0: {}
"#);
        assert!(g.cycles.is_empty());
    }

    #[test]
    fn deep_chain_does_not_overflow_the_stack() {
        // A recursive DFS blows the stack here. Real lockfiles reach 800+
        // nodes; this is the same shape, larger.
        const N: usize = 10_000;
        let mut y = String::from("lockfileVersion: '9.0'\nimporters: {}\npackages:\n");
        for i in 0..N {
            y.push_str(&format!("  p{i}@1.0.0: {{resolution: {{integrity: sha512-x}}}}\n"));
        }
        y.push_str("snapshots:\n");
        for i in 0..N {
            y.push_str(&format!("  p{i}@1.0.0:\n"));
            if i + 1 < N {
                y.push_str(&format!("    dependencies: {{p{}: 1.0.0}}\n", i + 1));
            }
        }
        let g = build(&y);
        assert_eq!(g.nodes.len(), N);
        assert!(g.cycles.is_empty());
    }

    #[test]
    fn cycles_are_deterministic_across_runs() {
        let y = r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  b@1.0.0: {resolution: {integrity: sha512-b}}
  c@1.0.0: {resolution: {integrity: sha512-c}}
snapshots:
  a@1.0.0: {dependencies: {b: 1.0.0}}
  b@1.0.0: {dependencies: {c: 1.0.0}}
  c@1.0.0: {dependencies: {a: 1.0.0}}
"#;
        assert_eq!(build(y).cycles, build(y).cycles);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib cycle`
Expected: FAIL — `cycles` is always empty.

- [ ] **Step 3: Implement iterative cycle detection**

Add to `src/lock/graph.rs`, and call it at the end of `build` in place of `Vec::new()`:

```rust
/// Node colours for the iterative DFS.
#[derive(Clone, Copy, PartialEq)]
enum Colour {
    White,
    Grey,
    Black,
}

/// Find cycles with an explicit stack.
///
/// Iterative by necessity: real lockfiles reach 800+ nodes with deep chains,
/// and a recursive DFS overflows. Cycles are normal in npm graphs (`@babel`,
/// `eslint`, `browserslist` all have them), so this reports rather than
/// rejects — see the S1 spec §7 for why that is safe under the single
/// `filegroup` store.
fn find_cycles(nodes: &BTreeMap<String, Node>) -> Vec<Vec<String>> {
    let mut colour: BTreeMap<&str, Colour> =
        nodes.keys().map(|k| (k.as_str(), Colour::White)).collect();
    let mut cycles = Vec::new();
    // Deduplicate by the cycle's node set, so one cycle found from several
    // entry points is reported once.
    let mut seen: std::collections::BTreeSet<Vec<String>> = Default::default();

    for start in nodes.keys() {
        if colour[start.as_str()] != Colour::White {
            continue;
        }
        // (node, index of the next edge to visit)
        let mut stack: Vec<(&str, usize)> = vec![(start.as_str(), 0)];
        let mut path: Vec<&str> = vec![start.as_str()];
        colour.insert(start.as_str(), Colour::Grey);

        while let Some((node, edge_idx)) = stack.pop() {
            let edges = &nodes[node].edges;
            if edge_idx < edges.len() {
                stack.push((node, edge_idx + 1));
                let next = edges[edge_idx].target.as_str();
                match colour.get(next).copied().unwrap_or(Colour::Black) {
                    Colour::Grey => {
                        // Back edge: the cycle is the path from `next` on.
                        if let Some(pos) = path.iter().position(|n| *n == next) {
                            let mut cycle: Vec<String> =
                                path[pos..].iter().map(|s| s.to_string()).collect();
                            cycle.push(next.to_string());
                            let mut norm: Vec<String> = cycle[..cycle.len() - 1].to_vec();
                            norm.sort();
                            if seen.insert(norm) {
                                cycles.push(cycle);
                            }
                        }
                    }
                    Colour::White => {
                        colour.insert(next, Colour::Grey);
                        stack.push((next, 0));
                        path.push(next);
                    }
                    Colour::Black => {}
                }
            } else {
                colour.insert(node, Colour::Black);
                path.pop();
            }
        }
    }
    cycles
}
```

In `build`, replace `cycles: Vec::new()`:

```rust
let cycles = find_cycles(&nodes);
Ok(Self { nodes, roots, cycles })
```

> **Implementer note:** the `path` bookkeeping above must stay in step with
> `stack`. Write the tests first and let them drive the details; if the
> push/pop discipline proves fiddly, an equivalent correct formulation is
> fine — the tests define the contract, not this sketch.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib cycle`
Expected: PASS. Also re-run `cargo test --lib` — Task 4's tests must still pass.

Then: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(lock): iterative cycle detection, reported not rejected"
```

---

### Task 6: `pudu debug print-graph`

**Files:**
- Create: `src/cli/debug.rs`
- Modify: `src/cli/mod.rs` (replace the `Debug` placeholder with a real subcommand)
- Test: `tests/debug_print_graph.rs`

**Interfaces:**
- Consumes: `Config` (`src/config.rs`), `parse_lockfile`, `Graph`.
- Produces: `pudu::cli::debug::print_graph(...) -> anyhow::Result<()>`.

**Context:** `src/cli/mod.rs` currently models `Debug` as `trailing_var_arg` with a comment explaining that an empty `#[derive(Subcommand)]` enum does not compile. That workaround ends here: replace it with a real `DebugCommands` enum carrying `PrintGraph`. Keep `CliError::DebugNeedsSubcommand` for the no-subcommand case if clap still needs it; if clap now handles it, remove the variant and its registration.

Read `src/cli/config_check.rs` for how a command loads config, renders diagnostics via `error::render`, and returns.

The command is hidden (`#[command(hide = true)]`) — a development surface with no stability promise.

- [ ] **Step 1: Write the failing integration test**

`tests/debug_print_graph.rs`:

```rust
mod common;

use std::path::Path;

/// The committed fixture: a real pnpm install (see its README).
fn fixture() -> &'static Path {
    Path::new("tests/fixtures/lock/real")
}

#[test]
fn print_graph_emits_json_for_the_real_lockfile() {
    let dir = common::scratch_with_config(fixture());
    let out = common::pudu(&dir).args(["debug", "print-graph"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(v["nodes"].as_object().unwrap().len() > 300, "the fixture has 400 keys");
    assert_eq!(v["lockfile_version"], "9.0");
}

#[test]
fn output_is_byte_identical_across_runs() {
    let dir = common::scratch_with_config(fixture());
    let a = common::pudu(&dir).args(["debug", "print-graph"]).output().unwrap();
    let b = common::pudu(&dir).args(["debug", "print-graph"]).output().unwrap();
    assert_eq!(a.stdout, b.stdout, "determinism is an invariant");
}

#[test]
fn cycles_are_reported_for_the_real_lockfile() {
    let dir = common::scratch_with_config(fixture());
    let out = common::pudu(&dir).args(["debug", "print-graph"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let cycles = v["cycles"].as_array().unwrap();
    assert!(!cycles.is_empty(), "the fixture has @babel/eslint/browserslist cycles");
}

#[test]
fn aliased_edge_survives_into_the_output() {
    let dir = common::scratch_with_config(fixture());
    let out = common::pudu(&dir).args(["debug", "print-graph"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let cliui = v["nodes"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(k, _)| k.starts_with("@isaacs/cliui@"))
        .expect("the fixture pulls @isaacs/cliui via glob")
        .1;
    let edge = cliui["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["link_name"] == "string-width-cjs")
        .expect("alias edge present");
    assert!(edge["target"].as_str().unwrap().starts_with("string-width@4"));
}

#[test]
fn a_v6_lockfile_exits_3() {
    let dir = common::scratch_with_lockfile("lockfileVersion: '6.0'\n");
    let out = common::pudu(&dir).args(["debug", "print-graph"]).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("6.0") && stderr.contains('9'), "{stderr}");
}
```

Add the `common` helpers this needs, following the existing style in `tests/common/mod.rs`:
- `scratch_with_config(fixture_dir) -> TempDir` — copies the fixture into a temp dir and writes a `pudu.toml` whose `lockfile_path` points at it.
- `scratch_with_lockfile(text) -> TempDir` — writes a `pudu.toml` plus a lockfile with that text.
- Reuse the existing `pudu(&dir)` command builder if one exists; otherwise add it.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test debug_print_graph`
Expected: FAIL — no `print-graph` subcommand.

- [ ] **Step 3: Write `src/cli/debug.rs`**

```rust
//! Developer inspection commands.
//!
//! Hidden and unstable: these exist to make the pipeline's intermediate
//! stages testable, and carry no compatibility promise.

use anyhow::Result;

use crate::config::Config;
use crate::error::render;
use crate::lock::{Graph, parse_lockfile};

/// Print the instance graph as JSON on stdout.
pub fn print_graph() -> Result<()> {
    let config = Config::load()?;
    let path = config.resolved_lockfile_path();
    let text = std::fs::read_to_string(&path)
        .map_err(|source| crate::error::ConfigError::LockfileUnreadable {
            path: path.clone(),
            source,
        })?;

    let (lockfile, warnings) = parse_lockfile(&text, &path)?;
    for w in &warnings {
        eprintln!("{}", render(w));
    }

    let graph = Graph::build(&lockfile)?;
    let out = serde_json::json!({
        "lockfile_version": crate::lock::SUPPORTED_VERSION,
        "settings": lockfile.settings,
        "roots": graph.roots,
        "nodes": graph.nodes,
        "cycles": graph.cycles,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
```

Adjust `Config::load` / `resolved_lockfile_path` to whatever `src/config.rs` actually exposes — read it rather than assuming these names. If a suitable `ConfigError` variant for an unreadable lockfile does not exist, reuse the closest one rather than adding a variant.

In `src/cli/mod.rs`:

```rust
    /// Developer inspection commands.
    #[command(hide = true)]
    Debug {
        #[command(subcommand)]
        command: DebugCommands,
    },
```

```rust
#[derive(Subcommand, Debug)]
pub enum DebugCommands {
    /// Print the instance graph as JSON.
    PrintGraph,
}
```

```rust
            Commands::Debug { command } => match command {
                DebugCommands::PrintGraph => debug::print_graph(),
            },
```

Add `pub mod debug;` to the module list. Delete the now-obsolete comment about uninhabited enums.

**The `help` snapshot tests will change.** `tests/snapshots/help__*.snap` cover `--help` output, and adding a subcommand alters it. Review the diff — confirm the change is only the new hidden command's effect — then accept with `cargo insta accept`.

- [ ] **Step 4: Run the tests**

Run: `cargo test`
Expected: PASS, including the updated help snapshots.

Then: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(cli): hidden debug print-graph command"
```

---

### Task 7: The differential test against pnpm's own virtual store

**Files:**
- Create: `tests/virtual_store_names.rs`
- Test: itself

**Interfaces:**
- Consumes: `pudu::lock::{parse_lockfile, snapshot_key::target_name}`.

**Context — this is the highest-value test in the stage.** `tests/fixtures/lock/real/virtual-store-listing.txt` holds the exact 316 directory names pnpm 10.21.0 created when installing the fixture's lockfile. Pudu's `target_name` is a port of pnpm's `depPathToFilename`, so it must regenerate every one. This catches a divergence in all four naming rules at once, with no runtime dependency on pnpm.

**Read `tests/fixtures/lock/real/README.md` first.** The essential subtlety: the lockfile has **400 snapshot keys but only 316 directories** — the other 84 are optional dependencies pnpm pruned for the generating platform (linux-x64-gnu). So the assertion must run in the direction *listing → pudu*: every captured name must be produced by some snapshot key. Asserting the reverse would fail on the pruned ones, which is S2's concern, not S1's.

- [ ] **Step 1: Write the test**

`tests/virtual_store_names.rs`:

```rust
//! Differential test against pnpm itself.
//!
//! `virtual-store-listing.txt` is the exact set of directory names pnpm
//! created for the committed lockfile. Pudu's target naming is a port of
//! pnpm's `depPathToFilename`, so it must reproduce all of them.

use std::collections::BTreeSet;
use std::path::Path;

use pudu::lock::parse_lockfile;
use pudu::lock::snapshot_key::target_name;

fn load() -> (BTreeSet<String>, BTreeSet<String>) {
    let dir = Path::new("tests/fixtures/lock/real");
    let text = std::fs::read_to_string(dir.join("pnpm-lock.yaml")).unwrap();
    let (lockfile, _) = parse_lockfile(&text, &dir.join("pnpm-lock.yaml")).unwrap();
    let produced: BTreeSet<String> =
        lockfile.snapshots.keys().map(|k| target_name(k)).collect();
    let captured: BTreeSet<String> = std::fs::read_to_string(dir.join("virtual-store-listing.txt"))
        .unwrap()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    (produced, captured)
}

#[test]
fn pudu_reproduces_every_name_pnpm_created() {
    let (produced, captured) = load();
    let missing: Vec<_> = captured.difference(&produced).collect();
    assert!(
        missing.is_empty(),
        "pudu failed to produce {} of pnpm's {} virtual-store names.\n\
         This means the depPathToFilename port has diverged.\nFirst few: {:#?}",
        missing.len(),
        captured.len(),
        missing.iter().take(10).collect::<Vec<_>>()
    );
}

#[test]
fn the_fixture_still_exercises_the_hashed_path() {
    // Guards the fixture itself: a regeneration that resolved differently
    // could silently drop the >120-char case, which is the branch most
    // likely to diverge.
    let (_, captured) = load();
    let hashed = captured
        .iter()
        .filter(|n| {
            n.len() == 120 && n.as_bytes()[87] == b'_' && n[88..].chars().all(|c| c.is_ascii_hexdigit())
        })
        .count();
    assert!(hashed >= 3, "expected at least 3 hashed names, found {hashed}");
}

#[test]
fn the_fixture_still_exercises_aliases_and_nesting() {
    let dir = Path::new("tests/fixtures/lock/real");
    let text = std::fs::read_to_string(dir.join("pnpm-lock.yaml")).unwrap();
    let (lockfile, _) = parse_lockfile(&text, &dir.join("pnpm-lock.yaml")).unwrap();

    let has_alias = lockfile.snapshots.values().any(|s| {
        s.dependencies
            .iter()
            .any(|(link, v)| !v.starts_with(char::is_numeric) && !v.starts_with(link))
    });
    assert!(has_alias, "the alias case has vanished from the fixture");

    let nested = lockfile.snapshots.keys().filter(|k| k.matches('(').count() > 1).count();
    assert!(nested >= 5, "expected nested peer keys, found {nested}");
}
```

- [ ] **Step 2: Run it**

Run: `cargo test --test virtual_store_names`
Expected: PASS. **If `pudu_reproduces_every_name_pnpm_created` fails, the port in Task 3 is wrong — fix `target_name`, never the fixture.** The listing is ground truth captured from pnpm.

- [ ] **Step 3: Verify the test can fail**

Temporarily change `target_name` — e.g. sort the peers, or escape only `/` — and confirm the test fails. Revert. Record the observed failure in the task report; a differential test that cannot fail is worthless.

- [ ] **Step 4: Full suite and lint**

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
rustup run 1.88 cargo check --all-targets
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "test(lock): differential test against pnpm's own virtual-store naming"
```

---

## Exit criteria for the stage

Verify all before declaring S1 done (S1 spec §13):

1. `pudu debug print-graph` prints one JSON entry per snapshot key, deterministically.
2. The key parser handles nested peers, scoped names, and the long real key; peer order is preserved, not sorted.
3. Target names are byte-identical to pnpm's, proven by Task 7.
4. Aliased edges resolve to the aliased package *and* retain the link name — both asserted.
5. Cycles are detected, reported, and do not error, proven against the real fixture.
6. v6, `patchedDependencies`, and `excludeLinksFromLockfile: true` each error with a named remedy; unknown top-level keys warn.
7. Unknown `os`/`cpu`/`libc` tokens parse without error.
8. Two runs produce byte-identical output.
9. `clippy -D warnings`, `fmt --check`, and MSRV 1.88 `check --all-targets` all clean.
10. No `HashMap`/`HashSet` anywhere in `src/lock/`.
