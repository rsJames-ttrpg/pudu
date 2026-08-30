# Pudu S0 — Scaffolding & Config Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up pudu's CLI dispatcher, `pudu.toml` parser, error machinery, `pudu init`, and `pudu config check`, so every later stage plugs into a working tool harness.

**Architecture:** A single Rust crate with a `lib.rs` / `main.rs` split. `clap` derive builds the command tree; `serde` + `toml` parse config into typed structs whose enums make invalid states unrepresentable; `thiserror` defines per-module error enums and `miette` renders them. `pudu init` detects a pnpm workspace, derives the platform matrix from `supportedArchitectures`, writes a project skeleton, and appends a marker-delimited Node toolchain block to the user's `toolchains/BUCK`.

**Tech Stack:** Rust 2024 edition, clap 4.6 (derive), serde 1.0, toml 1.1, serde_norway 0.9 (YAML), miette 7.6, thiserror 2.0, anyhow 1.0, url 2.5 (serde feature), insta 1.48, assert_cmd 2.2, predicates 3.1, tempfile 3.27.

## Global Constraints

- Rust edition 2024, `rust-version = "1.85"`. Do not raise either. In particular, **no `let`-chains** (`if let ... && ...`) — they need 1.88. Nothing in CI enforces the MSRV, so violations are silent.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` must pass. A pre-commit hook at `.claude/scripts/rust-precommit-gate.sh` enforces both and will block `git commit` on failure.
- No new dependencies. Everything needed is already in `Cargo.toml`. If a task seems to need one, stop and ask.
- **`BTreeMap` / `BTreeSet` everywhere, never `HashMap` / `HashSet`.** Deterministic iteration order is a precondition for the byte-stable output later stages require (design §5).
- Every error message names a field or a file by path, and carries line/column where the source position is known (spec §6). This is contract, asserted by tests.
- No business logic: no lockfile parsing, no tarball fetching, no BUCK emission. Those are S1–S4.
- Existing files `src/main.rs` and `src/lib.rs` are stubs from the scaffolding commit and are meant to be replaced.
- Spec reference: `docs/superpowers/specs/2026-08-30-pudu-s0-scaffolding-design.md`. Design reference: `docs/superpowers/specs/2026-08-30-pudu-design.md`.

## File Structure

| File | Responsibility |
|---|---|
| `src/error.rs` | `ConfigError` enum, `thiserror` + `miette` derives |
| `src/platform.rs` | `Os` / `Cpu` / `Libc` enums and their serde impls |
| `src/config.rs` | `pudu.toml` types, deserialization, validation |
| `src/cli/mod.rs` | clap derive structs, dispatcher |
| `src/cli/stub.rs` | UNIMPLEMENTED verb handling |
| `src/cli/config_check.rs` | `pudu config check` |
| `src/cli/init.rs` | detection, platform derivation, file writing |
| `src/cli/toolchain.rs` | the `toolchains/BUCK` append state machine |
| `src/lib.rs` | module declarations and re-exports |
| `src/main.rs` | entrypoint; renders errors through miette |
| `tests/common/mod.rs` | shared integration-test helpers |
| `tests/help.rs`, `tests/init.rs`, `tests/config_check.rs` | integration tests |

---

### Task 1: Error types

**Files:**
- Create: `src/error.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pudu::error::ConfigError` — an enum with the variants below; `pub type Result<T> = std::result::Result<T, ConfigError>;`

- [ ] **Step 1: Write the failing test**

Append to `src/error.rs` (create the file with this test plus the `use` lines it needs):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn missing_field_error_names_the_field_and_file() {
        let e = ConfigError::MissingField {
            path: PathBuf::from("/repo/pudu.toml"),
            field: "lockfile_path",
        };
        let msg = e.to_string();
        assert!(msg.contains("lockfile_path"), "message must name the field: {msg}");
        assert!(msg.contains("/repo/pudu.toml"), "message must name the file: {msg}");
    }

    #[test]
    fn parse_error_reports_line_and_column() {
        let bad = "lockfile_path = \n";
        let inner = toml::from_str::<toml::Value>(bad).unwrap_err();
        let e = ConfigError::Parse {
            path: PathBuf::from("/repo/pudu.toml"),
            source: inner,
        };
        let msg = format!("{}", e.source_message());
        assert!(msg.contains("line"), "parse errors must carry a line: {msg}");
    }

    #[test]
    fn libc_on_non_linux_names_the_platform() {
        let e = ConfigError::LibcOnNonLinux { platform: "darwin-arm64".into() };
        assert!(e.to_string().contains("darwin-arm64"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib error 2>&1 | tail -20`
Expected: FAIL — `cannot find type ConfigError`.

- [ ] **Step 3: Write minimal implementation**

Put this above the `mod tests` block in `src/error.rs`:

```rust
//! Error types for pudu.
//!
//! Contract: every message names a field or a file by path, and carries
//! line/column where the source position is known.

use std::path::PathBuf;

use miette::Diagnostic;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, ConfigError>;

#[derive(Debug, Error, Diagnostic)]
pub enum ConfigError {
    #[error("failed to parse {path}")]
    #[diagnostic(code(pudu::config::parse))]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("{path}: missing required field `{field}`")]
    #[diagnostic(code(pudu::config::missing_field))]
    MissingField { path: PathBuf, field: &'static str },

    #[error("platform `{platform}`: `libc` applies only to linux")]
    #[diagnostic(
        code(pudu::config::libc_on_non_linux),
        help("remove `libc`, or change `os` to \"linux\"")
    )]
    LibcOnNonLinux { platform: String },

    #[error("platform `{platform}`: windows is not supported in v1")]
    #[diagnostic(
        code(pudu::config::windows_unsupported),
        help("see the roadmap: Windows is a Phase 2 deliverable")
    )]
    WindowsUnsupported { platform: String },

    #[error("platforms `{first}` and `{second}` resolve to the same (os, cpu, libc)")]
    #[diagnostic(code(pudu::config::duplicate_platform))]
    DuplicatePlatform { first: String, second: String },

    #[error("platform `{platform}`: `{label}` is not a Buck target label")]
    #[diagnostic(
        code(pudu::config::bad_constraint_label),
        help("expected the form `cell//path:target`")
    )]
    BadConstraintLabel { platform: String, label: String },

    #[error("registry scope `{scope}` must start with `@`")]
    #[diagnostic(code(pudu::config::bad_registry_scope))]
    BadRegistryScope { scope: String },

    #[error("`[fixups].registry` value `{value}` is not recognized")]
    #[diagnostic(
        code(pudu::config::bad_fixup_registry),
        help("expected \"none\", \"file://<path>\", or \"github.com/<owner>/<repo>\"")
    )]
    BadFixupRegistry { value: String },

    #[error("`[scripts].allow` entry `{name}` is not a valid npm package name")]
    #[diagnostic(code(pudu::config::bad_package_name))]
    BadPackageName { name: String },

    #[error("`[buck].node_toolchain` value `{label}` is not a Buck target label")]
    #[diagnostic(code(pudu::config::bad_toolchain_label))]
    BadToolchainLabel { label: String },

    #[error("lockfile not found at {path}")]
    #[diagnostic(
        code(pudu::config::lockfile_not_found),
        help("edit `lockfile_path` in pudu.toml, then re-run `pudu config check`")
    )]
    LockfileNotFound { path: PathBuf },

    #[error("third-party directory {path} is not writable")]
    #[diagnostic(code(pudu::config::third_party_not_writable))]
    ThirdPartyDirNotWritable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("no platforms configured")]
    #[diagnostic(
        code(pudu::config::no_platforms),
        help("add at least one `[platforms.<name>]` table")
    )]
    NoPlatforms,

    #[error("{path}: {message}")]
    #[diagnostic(code(pudu::config::io))]
    Io { path: PathBuf, message: String },
}

impl ConfigError {
    /// The underlying source error's message, or this error's own if it has none.
    /// Used to surface `toml` parse positions, which live on the source.
    pub fn source_message(&self) -> String {
        use std::error::Error as _;
        match self.source() {
            Some(s) => s.to_string(),
            None => self.to_string(),
        }
    }
}
```

Replace `src/lib.rs` entirely with:

```rust
//! pudu — translate `pnpm-lock.yaml` into Buck2 build rules.
//!
//! No public API is committed in v1.

pub mod error;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib error`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/error.rs src/lib.rs
git commit -m "feat(s0): ConfigError types with miette diagnostics"
```

---

### Task 2: Platform enums

**Files:**
- Create: `src/platform.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pudu::platform::{Os, Cpu, Libc}` — `Copy` enums with serde impls. `Os::Win32` **exists deliberately**: npm packages declare `os: ["win32"]`, so S2 must be able to represent it when matching package fields, and having the variant lets config validation reject it with a helpful message instead of an opaque serde "unknown variant" error. `Os::as_npm(&self) -> &'static str` and `Cpu::as_npm(&self) -> &'static str` return the npm spelling.

- [ ] **Step 1: Write the failing test**

Create `src/platform.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Holder {
        os: Os,
        cpu: Cpu,
        libc: Option<Libc>,
    }

    #[test]
    fn deserializes_npm_spellings() {
        let h: Holder = toml::from_str(r#"os = "linux"
cpu = "x64"
libc = "musl"
"#)
        .unwrap();
        assert_eq!(h.os, Os::Linux);
        assert_eq!(h.cpu, Cpu::X64);
        assert_eq!(h.libc, Some(Libc::Musl));
    }

    #[test]
    fn win32_is_representable() {
        let h: Holder = toml::from_str("os = \"win32\"\ncpu = \"x64\"\n").unwrap();
        assert_eq!(h.os, Os::Win32);
        assert_eq!(h.libc, None);
    }

    #[test]
    fn unknown_os_lists_valid_values() {
        let err = toml::from_str::<Holder>("os = \"solaris\"\ncpu = \"x64\"\n").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("linux"), "should list valid values: {msg}");
    }

    #[test]
    fn npm_spellings_round_trip() {
        assert_eq!(Os::Darwin.as_npm(), "darwin");
        assert_eq!(Os::Win32.as_npm(), "win32");
        assert_eq!(Cpu::Arm64.as_npm(), "arm64");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib platform 2>&1 | tail -20`
Expected: FAIL — `cannot find type Os`.

- [ ] **Step 3: Write minimal implementation**

Put above `mod tests` in `src/platform.rs`:

```rust
//! Platform axes, spelled the way npm spells them.
//!
//! S0 defines the types; S2 adds npm-field matching and the mapping to Buck2
//! constraint labels.

use serde::{Deserialize, Serialize};

/// Operating system, using npm's `os` field vocabulary.
///
/// `Win32` is representable even though Windows is a v1 non-goal: npm packages
/// declare `os: ["win32"]`, so S2 must parse it, and config validation rejects
/// a win32 *platform* with a helpful message rather than a serde error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Linux,
    Darwin,
    Win32,
}

/// CPU architecture, using npm's `cpu` field vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cpu {
    X64,
    Arm64,
}

/// C standard library, using npm's `libc` field vocabulary. Linux only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Libc {
    Glibc,
    Musl,
}

impl Os {
    pub fn as_npm(&self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Darwin => "darwin",
            Os::Win32 => "win32",
        }
    }
}

impl Cpu {
    pub fn as_npm(&self) -> &'static str {
        match self {
            Cpu::X64 => "x64",
            Cpu::Arm64 => "arm64",
        }
    }
}

impl Libc {
    pub fn as_npm(&self) -> &'static str {
        match self {
            Libc::Glibc => "glibc",
            Libc::Musl => "musl",
        }
    }

    /// Short form used in generated platform names (`linux-x64-gnu`).
    pub fn short(&self) -> &'static str {
        match self {
            Libc::Glibc => "gnu",
            Libc::Musl => "musl",
        }
    }
}
```

Add `pub mod platform;` to `src/lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib platform`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/platform.rs src/lib.rs
git commit -m "feat(s0): Os/Cpu/Libc platform enums"
```

---

### Task 3: Config types and deserialization

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `pudu::platform::{Os, Cpu, Libc}`, `pudu::error::{ConfigError, Result}`.
- Produces: `Config`, `Platform`, `RegistryConfig`, `FixupsConfig`, `FixupRegistry`, `ScriptsConfig`, `BuckConfig`, and `Config::from_str(text: &str, path: &Path) -> Result<Config>`.

- [ ] **Step 1: Write the failing test**

Create `src/config.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const GOOD: &str = r#"
lockfile_path   = "pnpm-lock.yaml"
third_party_dir = "third-party/js"

[platforms.linux-x64-gnu]
os   = "linux"
cpu  = "x64"
libc = "glibc"

[platforms.darwin-arm64]
os  = "darwin"
cpu = "arm64"

[registry]
default  = "https://registry.npmjs.org"
"@myorg" = "https://npm.example.com"

[fixups]
registry = "none"

[scripts]
allow = ["sharp"]

[buck]
file_name = "BUCK"
"#;

    #[test]
    fn parses_a_full_config() {
        let c = Config::from_str(GOOD, Path::new("pudu.toml")).unwrap();
        assert_eq!(c.platforms.len(), 2);
        assert_eq!(c.platforms["linux-x64-gnu"].os, Os::Linux);
        assert_eq!(c.platforms["linux-x64-gnu"].libc, Some(Libc::Glibc));
        assert_eq!(c.platforms["darwin-arm64"].libc, None);
        assert_eq!(c.registry.scopes["@myorg"].as_str(), "https://npm.example.com/");
        assert_eq!(c.fixups.registry, FixupRegistry::None);
        assert!(c.scripts.allow.contains("sharp"));
    }

    #[test]
    fn applies_documented_defaults() {
        let c = Config::from_str(
            "lockfile_path = \"pnpm-lock.yaml\"\n[platforms.darwin-arm64]\nos = \"darwin\"\ncpu = \"arm64\"\n",
            Path::new("pudu.toml"),
        )
        .unwrap();
        assert_eq!(c.third_party_dir, PathBuf::from("third-party/js"));
        assert_eq!(c.buck.file_name, "BUCK");
        assert_eq!(c.buck.node_toolchain, "toolchains//:node");
        assert!(c.fixups.allow_local_overrides);
        assert_eq!(c.registry.default.as_str(), "https://registry.npmjs.org/");
    }

    #[test]
    fn rejects_unknown_top_level_key() {
        let err = Config::from_str(
            "lockfile_path = \"x\"\nwidgets = 3\n[platforms.p]\nos=\"linux\"\ncpu=\"x64\"\n",
            Path::new("pudu.toml"),
        )
        .unwrap_err();
        assert!(err.source_message().contains("widgets"), "{}", err.source_message());
    }

    #[test]
    fn parse_error_names_the_file() {
        let err = Config::from_str("lockfile_path = \n", Path::new("/repo/pudu.toml")).unwrap_err();
        assert!(err.to_string().contains("/repo/pudu.toml"));
    }

    #[test]
    fn fixup_registry_forms() {
        for (input, want) in [
            ("none", FixupRegistry::None),
            ("file:///tmp/reg", FixupRegistry::File(PathBuf::from("/tmp/reg"))),
            (
                "github.com/owner/repo",
                FixupRegistry::Github { owner: "owner".into(), repo: "repo".into() },
            ),
        ] {
            let text = format!(
                "lockfile_path=\"x\"\n[platforms.p]\nos=\"linux\"\ncpu=\"x64\"\n[fixups]\nregistry=\"{input}\"\n"
            );
            let c = Config::from_str(&text, Path::new("pudu.toml")).unwrap();
            assert_eq!(c.fixups.registry, want, "for input {input}");
        }
    }

    #[test]
    fn rejects_unrecognized_fixup_registry() {
        let text = "lockfile_path=\"x\"\n[platforms.p]\nos=\"linux\"\ncpu=\"x64\"\n[fixups]\nregistry=\"gitlab.com/a/b\"\n";
        let err = Config::from_str(text, Path::new("pudu.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::BadFixupRegistry { .. }));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib config 2>&1 | tail -20`
Expected: FAIL — `cannot find type Config`.

- [ ] **Step 3: Write minimal implementation**

Put above `mod tests` in `src/config.rs`:

```rust
//! `pudu.toml` parsing.
//!
//! Deserialization only. Validation that needs the filesystem lives in
//! [`Config::validate`] (Task 4).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use url::Url;

use crate::error::{ConfigError, Result};
use crate::platform::{Cpu, Libc, Os};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub lockfile_path: PathBuf,
    pub third_party_dir: PathBuf,
    pub platforms: BTreeMap<String, Platform>,
    pub registry: RegistryConfig,
    pub fixups: FixupsConfig,
    pub scripts: ScriptsConfig,
    pub buck: BuckConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Platform {
    pub os: Os,
    pub cpu: Cpu,
    #[serde(default)]
    pub libc: Option<Libc>,
    /// Escape hatch: replaces the generated Buck constraint labels (design §7).
    #[serde(default)]
    pub constraints: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryConfig {
    pub default: Url,
    pub scopes: BTreeMap<String, Url>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixupsConfig {
    pub registry: FixupRegistry,
    pub registry_rev: Option<String>,
    pub allow_local_overrides: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixupRegistry {
    None,
    File(PathBuf),
    Github { owner: String, repo: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScriptsConfig {
    pub allow: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuckConfig {
    pub file_name: String,
    pub node_toolchain: String,
}

// --- Raw (on-disk) shapes -------------------------------------------------

fn default_third_party_dir() -> PathBuf {
    PathBuf::from("third-party/js")
}
fn default_registry_url() -> Url {
    Url::parse("https://registry.npmjs.org").expect("valid literal URL")
}
fn default_file_name() -> String {
    "BUCK".to_string()
}
fn default_node_toolchain() -> String {
    "toolchains//:node".to_string()
}
fn default_true() -> bool {
    true
}
fn default_fixup_registry() -> String {
    "none".to_string()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    lockfile_path: PathBuf,
    #[serde(default = "default_third_party_dir")]
    third_party_dir: PathBuf,
    #[serde(default)]
    platforms: BTreeMap<String, Platform>,
    #[serde(default)]
    registry: RawRegistry,
    #[serde(default)]
    fixups: RawFixups,
    #[serde(default)]
    scripts: RawScripts,
    #[serde(default)]
    buck: RawBuck,
}

// `flatten` collects the scope keys, so `deny_unknown_fields` cannot be used here.
#[derive(Deserialize)]
struct RawRegistry {
    #[serde(default = "default_registry_url")]
    default: Url,
    #[serde(flatten)]
    scopes: BTreeMap<String, Url>,
}

impl Default for RawRegistry {
    fn default() -> Self {
        Self { default: default_registry_url(), scopes: BTreeMap::new() }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFixups {
    #[serde(default = "default_fixup_registry")]
    registry: String,
    #[serde(default)]
    registry_rev: Option<String>,
    #[serde(default = "default_true")]
    allow_local_overrides: bool,
}

impl Default for RawFixups {
    fn default() -> Self {
        Self {
            registry: default_fixup_registry(),
            registry_rev: None,
            allow_local_overrides: true,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawScripts {
    #[serde(default)]
    allow: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBuck {
    #[serde(default = "default_file_name")]
    file_name: String,
    #[serde(default = "default_node_toolchain")]
    node_toolchain: String,
}

impl Default for RawBuck {
    fn default() -> Self {
        Self { file_name: default_file_name(), node_toolchain: default_node_toolchain() }
    }
}

fn parse_fixup_registry(value: &str) -> Result<FixupRegistry> {
    if value == "none" {
        return Ok(FixupRegistry::None);
    }
    if let Some(rest) = value.strip_prefix("file://") {
        return Ok(FixupRegistry::File(PathBuf::from(rest)));
    }
    if let Some(rest) = value.strip_prefix("github.com/") {
        let mut parts = rest.split('/');
        // No `let`-chains here: they stabilized in Rust 1.88 and this crate
        // declares rust-version = "1.85".
        if let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) {
            if !owner.is_empty() && !repo.is_empty() {
                return Ok(FixupRegistry::Github { owner: owner.into(), repo: repo.into() });
            }
        }
    }
    Err(ConfigError::BadFixupRegistry { value: value.to_string() })
}

impl Config {
    /// Parse `pudu.toml` text. `path` is used only for error messages.
    pub fn from_str(text: &str, path: &Path) -> Result<Config> {
        let raw: RawConfig = toml::from_str(text)
            .map_err(|source| ConfigError::Parse { path: path.to_path_buf(), source })?;

        Ok(Config {
            lockfile_path: raw.lockfile_path,
            third_party_dir: raw.third_party_dir,
            platforms: raw.platforms,
            registry: RegistryConfig { default: raw.registry.default, scopes: raw.registry.scopes },
            fixups: FixupsConfig {
                registry: parse_fixup_registry(&raw.fixups.registry)?,
                registry_rev: raw.fixups.registry_rev,
                allow_local_overrides: raw.fixups.allow_local_overrides,
            },
            scripts: ScriptsConfig { allow: raw.scripts.allow },
            buck: BuckConfig {
                file_name: raw.buck.file_name,
                node_toolchain: raw.buck.node_toolchain,
            },
        })
    }
}
```

Add `pub mod config;` to `src/lib.rs`.

Note: `Config::from_str` is an inherent method, not the `FromStr` trait, because it takes a path for error context. Clippy may suggest implementing `FromStr`; if `clippy::should_implement_trait` fires, add `#[allow(clippy::should_implement_trait)]` on the `impl Config` block with a comment explaining why.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib config`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/config.rs src/lib.rs
git commit -m "feat(s0): pudu.toml types and deserialization"
```

---

### Task 4: Config validation

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: `Config` from Task 3.
- Produces: `Config::validate(&self, base_dir: &Path) -> (Vec<ConfigError>, Vec<String>)` returning `(errors, warnings)`. `base_dir` is the directory containing `pudu.toml`; relative paths resolve against it. Returning collected errors rather than short-circuiting lets `config check` report every problem in one pass.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `src/config.rs`:

```rust
    use std::fs;

    fn cfg(text: &str) -> Config {
        Config::from_str(text, Path::new("pudu.toml")).unwrap()
    }

    fn tempdir_with_lockfile() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        d
    }

    #[test]
    fn accepts_a_valid_config() {
        let d = tempdir_with_lockfile();
        let (errors, _) = cfg(GOOD).validate(d.path());
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn rejects_libc_on_darwin() {
        let d = tempdir_with_lockfile();
        let c = cfg("lockfile_path=\"pnpm-lock.yaml\"\n[platforms.darwin-arm64]\nos=\"darwin\"\ncpu=\"arm64\"\nlibc=\"glibc\"\n");
        let (errors, _) = c.validate(d.path());
        assert!(errors.iter().any(|e| matches!(e, ConfigError::LibcOnNonLinux { .. })), "{errors:?}");
    }

    #[test]
    fn rejects_windows_platform() {
        let d = tempdir_with_lockfile();
        let c = cfg("lockfile_path=\"pnpm-lock.yaml\"\n[platforms.win]\nos=\"win32\"\ncpu=\"x64\"\n");
        let (errors, _) = c.validate(d.path());
        assert!(errors.iter().any(|e| matches!(e, ConfigError::WindowsUnsupported { .. })), "{errors:?}");
    }

    #[test]
    fn rejects_duplicate_platform_triples() {
        let d = tempdir_with_lockfile();
        let c = cfg("lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"linux\"\ncpu=\"x64\"\nlibc=\"glibc\"\n[platforms.b]\nos=\"linux\"\ncpu=\"x64\"\nlibc=\"glibc\"\n");
        let (errors, _) = c.validate(d.path());
        assert!(errors.iter().any(|e| matches!(e, ConfigError::DuplicatePlatform { .. })), "{errors:?}");
    }

    #[test]
    fn rejects_bad_constraint_label() {
        let d = tempdir_with_lockfile();
        let c = cfg("lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"linux\"\ncpu=\"x64\"\nconstraints=[\"not-a-label\"]\n");
        let (errors, _) = c.validate(d.path());
        assert!(errors.iter().any(|e| matches!(e, ConfigError::BadConstraintLabel { .. })), "{errors:?}");
    }

    #[test]
    fn rejects_scope_without_at_sign() {
        let d = tempdir_with_lockfile();
        let c = cfg("lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"linux\"\ncpu=\"x64\"\n[registry]\nmyorg=\"https://x.example.com\"\n");
        let (errors, _) = c.validate(d.path());
        assert!(errors.iter().any(|e| matches!(e, ConfigError::BadRegistryScope { .. })), "{errors:?}");
    }

    #[test]
    fn rejects_bad_package_name_in_allow() {
        let d = tempdir_with_lockfile();
        let c = cfg("lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"linux\"\ncpu=\"x64\"\n[scripts]\nallow=[\"Not A Package\"]\n");
        let (errors, _) = c.validate(d.path());
        assert!(errors.iter().any(|e| matches!(e, ConfigError::BadPackageName { .. })), "{errors:?}");
    }

    #[test]
    fn rejects_missing_lockfile() {
        let d = tempfile::tempdir().unwrap();
        let (errors, _) = cfg(GOOD).validate(d.path());
        assert!(errors.iter().any(|e| matches!(e, ConfigError::LockfileNotFound { .. })), "{errors:?}");
    }

    #[test]
    fn rejects_empty_platforms() {
        let d = tempdir_with_lockfile();
        let c = cfg("lockfile_path=\"pnpm-lock.yaml\"\n");
        let (errors, _) = c.validate(d.path());
        assert!(errors.iter().any(|e| matches!(e, ConfigError::NoPlatforms)), "{errors:?}");
    }

    #[test]
    fn warns_on_single_platform_and_unpinned_registry() {
        let d = tempdir_with_lockfile();
        let c = cfg("lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"linux\"\ncpu=\"x64\"\n[fixups]\nregistry=\"github.com/o/r\"\n");
        let (errors, warnings) = c.validate(d.path());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib config 2>&1 | tail -20`
Expected: FAIL — no method `validate`.

- [ ] **Step 3: Write minimal implementation**

Add to `src/config.rs`, inside the existing `impl Config` block:

```rust
    /// Validate the config. Relative paths resolve against `base_dir`, the
    /// directory containing `pudu.toml`.
    ///
    /// Collects every problem rather than short-circuiting, so `pudu config
    /// check` can report them all in one pass.
    pub fn validate(&self, base_dir: &Path) -> (Vec<ConfigError>, Vec<String>) {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        let lockfile = base_dir.join(&self.lockfile_path);
        if !lockfile.is_file() {
            errors.push(ConfigError::LockfileNotFound { path: lockfile });
        }

        let tpd = base_dir.join(&self.third_party_dir);
        if let Err(source) = check_writable(&tpd) {
            errors.push(ConfigError::ThirdPartyDirNotWritable { path: tpd, source });
        }

        if self.platforms.is_empty() {
            errors.push(ConfigError::NoPlatforms);
        } else if self.platforms.len() == 1 {
            warnings.push(
                "only one platform is configured; generated rules will not vary by platform"
                    .to_string(),
            );
        }

        let mut seen: BTreeMap<(Os, Cpu, Option<Libc>), &str> = BTreeMap::new();
        for (name, p) in &self.platforms {
            if p.os == Os::Win32 {
                errors.push(ConfigError::WindowsUnsupported { platform: name.clone() });
            }
            if p.libc.is_some() && p.os != Os::Linux {
                errors.push(ConfigError::LibcOnNonLinux { platform: name.clone() });
            }
            for label in p.constraints.iter().flatten() {
                if !is_buck_label(label) {
                    errors.push(ConfigError::BadConstraintLabel {
                        platform: name.clone(),
                        label: label.clone(),
                    });
                }
            }
            let key = (p.os, p.cpu, p.libc);
            if let Some(first) = seen.get(&key) {
                errors.push(ConfigError::DuplicatePlatform {
                    first: (*first).to_string(),
                    second: name.clone(),
                });
            } else {
                seen.insert(key, name);
            }
        }

        for scope in self.registry.scopes.keys() {
            if !scope.starts_with('@') {
                errors.push(ConfigError::BadRegistryScope { scope: scope.clone() });
            }
        }

        for name in &self.scripts.allow {
            if !is_npm_package_name(name) {
                errors.push(ConfigError::BadPackageName { name: name.clone() });
            }
        }

        if !is_buck_label(&self.buck.node_toolchain) {
            errors.push(ConfigError::BadToolchainLabel {
                label: self.buck.node_toolchain.clone(),
            });
        }

        if self.fixups.registry != FixupRegistry::None && self.fixups.registry_rev.is_none() {
            warnings.push(
                "`[fixups].registry` is set but `registry_rev` is not pinned".to_string(),
            );
        }

        (errors, warnings)
    }
```

And these free functions at the bottom of `src/config.rs`, above `mod tests`:

```rust
/// Prove the directory is writable *without* creating it — `config check` is
/// specified as side-effect free (spec §4), so it must not materialize
/// `third_party_dir` merely by validating.
///
/// If the directory exists, probe it directly. If it does not, probe the
/// nearest existing ancestor instead: that is what determines whether `pudu
/// init` or `pudu buckify` could create it later.
fn check_writable(dir: &Path) -> std::io::Result<()> {
    let mut target = dir;
    while !target.exists() {
        match target.parent() {
            Some(p) => target = p,
            None => break,
        }
    }
    let probe = target.join(".pudu-write-probe");
    std::fs::write(&probe, b"")?;
    std::fs::remove_file(&probe)
}

/// A Buck target label: `cell//path:target` or `//path:target`.
fn is_buck_label(s: &str) -> bool {
    let Some((cell_and_path, target)) = s.rsplit_once(':') else {
        return false;
    };
    !target.is_empty() && cell_and_path.contains("//")
}

/// npm package naming rules, restricted to what pudu needs: lowercase, no
/// spaces, optional `@scope/` prefix, and the URL-safe character set.
fn is_npm_package_name(s: &str) -> bool {
    fn segment_ok(seg: &str) -> bool {
        !seg.is_empty()
            && seg.len() <= 214
            && !seg.starts_with('.')
            && !seg.starts_with('_')
            && seg
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-._~".contains(c))
    }
    match s.strip_prefix('@') {
        Some(rest) => match rest.split_once('/') {
            Some((scope, name)) => segment_ok(scope) && segment_ok(name),
            None => false,
        },
        None => segment_ok(s),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib config`
Expected: 16 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/config.rs
git commit -m "feat(s0): pudu.toml validation with collected errors and warnings"
```

---

### Task 5: CLI skeleton and stubbed verbs

**Files:**
- Create: `src/cli/mod.rs`, `src/cli/stub.rs`
- Modify: `src/lib.rs`, `src/main.rs`
- Test: `tests/help.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pudu::cli::Cli` (clap `Parser`), `pudu::cli::Cli::run(self) -> anyhow::Result<()>`, and `pudu::cli::stub::unimplemented(verb: &str, stage: &str) -> anyhow::Error`.

- [ ] **Step 1: Write the failing test**

Create `tests/help.rs`:

```rust
use assert_cmd::Command;

fn pudu() -> Command {
    Command::cargo_bin("pudu").expect("binary builds")
}

#[test]
fn help_lists_every_phase_one_verb() {
    let out = pudu().arg("--help").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    for verb in ["init", "vendor", "buckify", "fixups", "audit", "unused", "config", "debug"] {
        assert!(text.contains(verb), "--help must list `{verb}`:\n{text}");
    }
}

#[test]
fn version_prints_the_crate_version() {
    let out = pudu().arg("--version").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
}

#[test]
fn stubbed_verbs_report_their_stage_and_exit_two() {
    for (verb, stage) in [("vendor", "S3"), ("buckify", "S4"), ("audit", "Phase 2")] {
        let out = pudu().arg(verb).output().unwrap();
        let text = String::from_utf8(out.stderr).unwrap();
        assert_eq!(out.status.code(), Some(2), "`{verb}` must exit 2:\n{text}");
        assert!(text.contains("not implemented yet"), "{text}");
        assert!(text.contains(stage), "`{verb}` must name {stage}:\n{text}");
    }
}

#[test]
fn debug_without_subcommand_exits_two() {
    let out = pudu().arg("debug").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test help 2>&1 | tail -20`
Expected: FAIL — the binary prints the placeholder text and exits 0.

- [ ] **Step 3: Write minimal implementation**

Create `src/cli/stub.rs`:

```rust
//! Registration of verbs that are planned but not yet implemented.
//!
//! Keeping them in `--help` from day one makes the tool's trajectory legible
//! and lets the help snapshot lock verb names before they have behaviour.

/// The error every unimplemented verb returns. `main` maps it to exit code 2.
pub fn unimplemented(verb: &str, stage: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "pudu {verb} is not implemented yet (planned for {stage}); \
         see docs/superpowers/specs/"
    )
}
```

Create `src/cli/mod.rs`:

```rust
//! CLI surface and dispatch.

pub mod stub;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "pudu",
    version,
    about = "Translate pnpm-lock.yaml into Buck2 build rules"
)]
pub struct Cli {
    /// Change to this directory before running.
    #[arg(short = 'C', global = true, value_name = "PATH")]
    pub directory: Option<PathBuf>,

    /// Increase log verbosity (-v, -vv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Forbid all network access. (No effect until S3.)
    #[arg(long, global = true)]
    pub no_network: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Scaffold a pudu project.
    Init {
        /// Overwrite existing files.
        #[arg(long)]
        force: bool,
        /// Directory to scaffold (default: current directory).
        path: Option<PathBuf>,
    },
    /// Inspect pudu.toml.
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Fetch tarballs and write pudu.lock. [UNIMPLEMENTED — S3]
    Vendor {
        /// Exit non-zero if pudu.lock is stale.
        #[arg(long)]
        check: bool,
    },
    /// Emit BUCK, pudu.bzl, and config/BUCK. [UNIMPLEMENTED — S4]
    Buckify {
        /// Exit non-zero if generated files are stale.
        #[arg(long)]
        check: bool,
    },
    /// Manage the community fixup registry. [UNIMPLEMENTED — S7/S8]
    Fixups,
    /// Cross-check the lockfile against advisories. [UNIMPLEMENTED — Phase 2]
    Audit,
    /// Report unreferenced vendored tarballs. [UNIMPLEMENTED — Phase 2]
    Unused,
    /// Developer inspection commands.
    ///
    /// Has no subcommands at S0; S1 adds `print-graph` and S2 adds
    /// `platforms`. Modelled as trailing args rather than an empty
    /// `#[derive(Subcommand)]` enum, because deriving `Subcommand` on an
    /// uninhabited enum does not compile.
    Debug {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Validate pudu.toml.
    Check {
        /// Output format.
        #[arg(long, value_name = "FORMAT", default_value = "human")]
        format: String,
    },
}

impl Cli {
    pub fn run(self) -> anyhow::Result<()> {
        if let Some(dir) = &self.directory {
            std::env::set_current_dir(dir)
                .map_err(|e| anyhow::anyhow!("cannot change directory to {}: {e}", dir.display()))?;
        }

        match self.command {
            Commands::Init { .. } => Err(stub::unimplemented("init", "S0 Task 8")),
            Commands::Config { .. } => Err(stub::unimplemented("config check", "S0 Task 6")),
            Commands::Vendor { .. } => Err(stub::unimplemented("vendor", "S3")),
            Commands::Buckify { .. } => Err(stub::unimplemented("buckify", "S4")),
            Commands::Fixups => Err(stub::unimplemented("fixups", "S7/S8")),
            Commands::Audit => Err(stub::unimplemented("audit", "Phase 2")),
            Commands::Unused => Err(stub::unimplemented("unused", "Phase 2")),
            Commands::Debug { args } => Err(anyhow::anyhow!(
                "pudu debug requires a subcommand (none exist yet; S1 adds \
                 `print-graph`){}",
                if args.is_empty() { String::new() } else { format!(": unknown `{}`", args[0]) }
            )),
        }
    }
}
```

Replace `src/main.rs` entirely:

```rust
//! pudu CLI entrypoint.

use clap::Parser;

use pudu::cli::Cli;

fn main() {
    if let Err(e) = Cli::parse().run() {
        eprintln!("error: {e:#}");
        std::process::exit(2);
    }
}
```

Add `pub mod cli;` to `src/lib.rs`.

Note: `Init` and `Config` are wired to stubs here on purpose — Tasks 6 and 8 replace those two arms. Every other arm is final.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test help`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/cli/ src/main.rs src/lib.rs tests/help.rs
git commit -m "feat(s0): CLI skeleton with stubbed phase-1 verbs"
```

---

### Task 6: `pudu config check`

**Files:**
- Create: `src/cli/config_check.rs`, `tests/common/mod.rs`, `tests/config_check.rs`
- Modify: `src/cli/mod.rs`

**Interfaces:**
- Consumes: `Config::from_str`, `Config::validate` (Tasks 3–4).
- Produces: `pudu::cli::config_check::run(format: &str) -> anyhow::Result<()>`. Reads `pudu.toml` from the current directory (`-C` has already applied). Exits non-zero via `Err` on any validation error.

- [ ] **Step 1: Write the failing test**

Create `tests/common/mod.rs`:

```rust
use std::path::Path;

use assert_cmd::Command;

pub fn pudu(dir: &Path) -> Command {
    let mut c = Command::cargo_bin("pudu").expect("binary builds");
    c.current_dir(dir);
    c
}

pub const GOOD_CONFIG: &str = r#"
lockfile_path   = "pnpm-lock.yaml"
third_party_dir = "third-party/js"

[platforms.linux-x64-gnu]
os   = "linux"
cpu  = "x64"
libc = "glibc"

[platforms.darwin-arm64]
os  = "darwin"
cpu = "arm64"
"#;

/// A tempdir containing a lockfile and, optionally, a `pudu.toml`.
pub fn project(config: Option<&str>) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    if let Some(c) = config {
        std::fs::write(d.path().join("pudu.toml"), c).unwrap();
    }
    d
}
```

Create `tests/config_check.rs`:

```rust
mod common;

use common::{GOOD_CONFIG, project, pudu};

#[test]
fn accepts_a_good_config() {
    let d = project(Some(GOOD_CONFIG));
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("2 platforms"), "{stdout}");
}

#[test]
fn rejects_a_missing_lockfile() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("pudu.toml"), GOOD_CONFIG).unwrap();
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("pnpm-lock.yaml"), "{stderr}");
}

#[test]
fn reports_every_error_not_just_the_first() {
    let d = project(Some(
        "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"win32\"\ncpu=\"x64\"\nlibc=\"glibc\"\n",
    ));
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("windows"), "{stderr}");
    assert!(stderr.contains("libc"), "{stderr}");
}

#[test]
fn json_format_is_machine_readable() {
    let d = project(Some(GOOD_CONFIG));
    let out = pudu(d.path())
        .args(["config", "check", "--format", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("\"ok\""), "{stdout}");
    assert!(stdout.contains("true"), "{stdout}");
}

#[test]
fn missing_config_file_names_the_path() {
    let d = project(None);
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("pudu.toml"), "{stderr}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test config_check 2>&1 | tail -20`
Expected: FAIL — `config check` returns the stub error.

- [ ] **Step 3: Write minimal implementation**

Create `src/cli/config_check.rs`:

```rust
//! `pudu config check` — validate pudu.toml with no side effects.

use std::path::Path;

use crate::config::Config;

/// Validate `pudu.toml` in the current directory.
///
/// `format` is `"human"` or `"json"`. JSON goes to stdout in both the ok and
/// error cases so CI can parse it; human-readable errors go to stderr.
pub fn run(format: &str) -> anyhow::Result<()> {
    let path = Path::new("pudu.toml");
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;

    let config = Config::from_str(&text, path)?;
    let base = std::env::current_dir()?;
    let (errors, warnings) = config.validate(&base);

    let json = match format {
        "human" => false,
        "json" => true,
        other => anyhow::bail!("unknown --format `{other}` (expected \"human\" or \"json\")"),
    };

    if json {
        let obj = serde_json::json!({
            "ok": errors.is_empty(),
            "errors": errors.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
            "warnings": warnings,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        if errors.is_empty() {
            return Ok(());
        }
        anyhow::bail!("{} error(s) in pudu.toml", errors.len());
    }

    for w in &warnings {
        eprintln!("warning: {w}");
    }

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("error: {e}");
        }
        anyhow::bail!("{} error(s) in pudu.toml", errors.len());
    }

    let names: Vec<&str> = config.platforms.keys().map(String::as_str).collect();
    println!(
        "pudu.toml ok: {} platform{} ({})",
        names.len(),
        if names.len() == 1 { "" } else { "s" },
        names.join(", ")
    );
    Ok(())
}
```

In `src/cli/mod.rs`: add `pub mod config_check;` beside `pub mod stub;`, and replace the `Commands::Config` arm with:

```rust
            Commands::Config { command } => match command {
                ConfigCommands::Check { format } => config_check::run(&format),
            },
```

`serde_json` is already a dependency.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test config_check`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/cli/config_check.rs src/cli/mod.rs tests/common/mod.rs tests/config_check.rs
git commit -m "feat(s0): pudu config check with human and json output"
```

---

### Task 7: Workspace detection and platform derivation

**Files:**
- Create: `src/cli/init.rs`
- Modify: `src/lib.rs` is unchanged; `src/cli/mod.rs` gains `pub mod init;`

**Interfaces:**
- Consumes: `pudu::platform::{Os, Cpu, Libc}`.
- Produces:
  - `pub struct Detected { pub lockfile: PathBuf, pub workspace_yaml: Option<PathBuf> }`
  - `pub fn detect(start: &Path) -> Option<Detected>` — walks upward for `pnpm-lock.yaml`.
  - `pub struct DerivedPlatforms { pub platforms: BTreeMap<String, Platform>, pub warnings: Vec<String> }`
  - `pub fn derive_platforms(workspace_yaml: Option<&str>) -> Result<DerivedPlatforms, String>` — expands `supportedArchitectures`, or returns the default set.
  - `pub fn platform_name(os: Os, cpu: Cpu, libc: Option<Libc>) -> String`

- [ ] **Step 1: Write the failing test**

Create `src/cli/init.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lockfile_from_a_subdirectory() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        let nested = d.path().join("packages/server");
        std::fs::create_dir_all(&nested).unwrap();

        let found = detect(&nested).expect("walks upward to the lockfile");
        assert_eq!(found.lockfile, d.path().join("pnpm-lock.yaml"));
        assert!(found.workspace_yaml.is_none());
    }

    #[test]
    fn detect_returns_none_without_a_lockfile() {
        let d = tempfile::tempdir().unwrap();
        assert!(detect(d.path()).is_none());
    }

    #[test]
    fn default_platform_set_when_no_workspace_yaml() {
        let d = derive_platforms(None).unwrap();
        let names: Vec<&str> = d.platforms.keys().map(String::as_str).collect();
        assert_eq!(names, ["darwin-arm64", "linux-arm64-gnu", "linux-x64-gnu"]);
        assert!(d.warnings.is_empty());
    }

    #[test]
    fn expands_the_os_cpu_cross_product() {
        let yaml = "supportedArchitectures:\n  os: [linux, darwin]\n  cpu: [x64, arm64]\n";
        let d = derive_platforms(Some(yaml)).unwrap();
        let names: Vec<&str> = d.platforms.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            ["darwin-arm64", "darwin-x64", "linux-arm64-gnu", "linux-x64-gnu"]
        );
    }

    #[test]
    fn libc_applies_only_to_linux() {
        let yaml = "supportedArchitectures:\n  os: [linux, darwin]\n  cpu: [arm64]\n  libc: [musl]\n";
        let d = derive_platforms(Some(yaml)).unwrap();
        assert_eq!(d.platforms["darwin-arm64"].libc, None);
        assert_eq!(d.platforms["linux-arm64-musl"].libc, Some(Libc::Musl));
        assert_eq!(d.platforms.len(), 2, "no darwin-musl platform may be emitted");
    }

    #[test]
    fn win32_is_skipped_with_a_warning() {
        let yaml = "supportedArchitectures:\n  os: [linux, win32]\n  cpu: [x64]\n";
        let d = derive_platforms(Some(yaml)).unwrap();
        assert_eq!(d.platforms.len(), 1);
        assert!(d.platforms.contains_key("linux-x64-gnu"));
        assert_eq!(d.warnings.len(), 1);
        assert!(d.warnings[0].contains("win32"), "{:?}", d.warnings);
    }

    #[test]
    fn only_win32_is_an_error() {
        let yaml = "supportedArchitectures:\n  os: [win32]\n  cpu: [x64]\n";
        let err = derive_platforms(Some(yaml)).unwrap_err();
        assert!(err.contains("no supported platforms"), "{err}");
    }

    #[test]
    fn current_resolves_to_the_host() {
        let yaml = "supportedArchitectures:\n  os: [current]\n  cpu: [current]\n";
        let d = derive_platforms(Some(yaml)).unwrap();
        assert_eq!(d.platforms.len(), 1, "{:?}", d.platforms.keys().collect::<Vec<_>>());
    }

    #[test]
    fn unknown_keys_warn_rather_than_fail() {
        let yaml = "supportedArchitectures:\n  os: [linux]\n  cpu: [x64]\n  future: [thing]\n";
        let d = derive_platforms(Some(yaml)).unwrap();
        assert_eq!(d.platforms.len(), 1);
        assert!(d.warnings.iter().any(|w| w.contains("future")), "{:?}", d.warnings);
    }

    #[test]
    fn absent_supported_architectures_uses_defaults() {
        let d = derive_platforms(Some("packages:\n  - packages/*\n")).unwrap();
        assert_eq!(d.platforms.len(), 3);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib init 2>&1 | tail -20`
Expected: FAIL — `cannot find function detect`.

- [ ] **Step 3: Write minimal implementation**

Put above `mod tests` in `src/cli/init.rs`:

```rust
//! `pudu init` — detect a pnpm workspace and scaffold a pudu project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::Platform;
use crate::platform::{Cpu, Libc, Os};

/// What an upward walk from the invocation directory found.
pub struct Detected {
    pub lockfile: PathBuf,
    pub workspace_yaml: Option<PathBuf>,
}

/// Walk upward from `start` looking for `pnpm-lock.yaml`.
///
/// The lockfile is the anchor rather than `package.json`: it is pudu's actual
/// input, and a repo holds many manifests but one lockfile per workspace.
pub fn detect(start: &Path) -> Option<Detected> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let lockfile = d.join("pnpm-lock.yaml");
        if lockfile.is_file() {
            let ws = d.join("pnpm-workspace.yaml");
            return Some(Detected {
                lockfile,
                workspace_yaml: ws.is_file().then_some(ws),
            });
        }
        dir = d.parent();
    }
    None
}

pub struct DerivedPlatforms {
    pub platforms: BTreeMap<String, Platform>,
    pub warnings: Vec<String>,
}

#[derive(Deserialize)]
struct WorkspaceYaml {
    #[serde(rename = "supportedArchitectures")]
    supported_architectures: Option<serde_norway::Value>,
}

/// Generated platform name: `linux-x64-gnu`, `darwin-arm64`.
pub fn platform_name(os: Os, cpu: Cpu, libc: Option<Libc>) -> String {
    match libc {
        Some(l) => format!("{}-{}-{}", os.as_npm(), cpu.as_npm(), l.short()),
        None => format!("{}-{}", os.as_npm(), cpu.as_npm()),
    }
}

fn host_os() -> Os {
    if cfg!(target_os = "macos") { Os::Darwin } else { Os::Linux }
}

fn host_cpu() -> Cpu {
    if cfg!(target_arch = "aarch64") { Cpu::Arm64 } else { Cpu::X64 }
}

/// Read a `supportedArchitectures` axis into a list of strings.
fn axis(v: &serde_norway::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_sequence())
        .map(|s| s.iter().filter_map(|i| i.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Expand `supportedArchitectures` into a platform matrix, or return the
/// default set when it is absent.
///
/// Rules (spec §3.2): cross-product os × cpu; `libc` applies only to linux;
/// `win32` is skipped with a warning; `current` resolves to the host.
pub fn derive_platforms(workspace_yaml: Option<&str>) -> Result<DerivedPlatforms, String> {
    let mut warnings = Vec::new();

    let sa = match workspace_yaml {
        None => None,
        Some(text) => serde_norway::from_str::<WorkspaceYaml>(text)
            .map_err(|e| format!("cannot parse pnpm-workspace.yaml: {e}"))?
            .supported_architectures,
    };

    let Some(sa) = sa else {
        return Ok(DerivedPlatforms { platforms: default_platforms(), warnings });
    };

    if let Some(map) = sa.as_mapping() {
        for k in map.keys().filter_map(|k| k.as_str()) {
            if !matches!(k, "os" | "cpu" | "libc") {
                warnings.push(format!(
                    "pnpm-workspace.yaml: ignoring unrecognized supportedArchitectures key `{k}`"
                ));
            }
        }
    }

    let mut oses = Vec::new();
    for raw in axis(&sa, "os") {
        match raw.as_str() {
            "current" => oses.push(host_os()),
            "linux" => oses.push(Os::Linux),
            "darwin" => oses.push(Os::Darwin),
            "win32" => warnings.push(
                "pnpm-workspace.yaml: skipping `win32` — Windows is a Phase 2 deliverable, \
                 see docs/superpowers/specs/2026-08-30-pudu-roadmap.md"
                    .to_string(),
            ),
            other => warnings.push(format!("pnpm-workspace.yaml: ignoring unknown os `{other}`")),
        }
    }

    let mut cpus = Vec::new();
    for raw in axis(&sa, "cpu") {
        match raw.as_str() {
            "current" => cpus.push(host_cpu()),
            "x64" => cpus.push(Cpu::X64),
            "arm64" => cpus.push(Cpu::Arm64),
            other => warnings.push(format!("pnpm-workspace.yaml: ignoring unknown cpu `{other}`")),
        }
    }

    let mut libcs = Vec::new();
    for raw in axis(&sa, "libc") {
        match raw.as_str() {
            "current" | "glibc" => libcs.push(Libc::Glibc),
            "musl" => libcs.push(Libc::Musl),
            other => warnings.push(format!("pnpm-workspace.yaml: ignoring unknown libc `{other}`")),
        }
    }

    if oses.is_empty() {
        oses.push(host_os());
    }
    if cpus.is_empty() {
        cpus.push(host_cpu());
    }
    if libcs.is_empty() {
        libcs.push(Libc::Glibc);
    }

    oses.sort();
    oses.dedup();
    cpus.sort();
    cpus.dedup();
    libcs.sort();
    libcs.dedup();

    let mut platforms = BTreeMap::new();
    for os in &oses {
        for cpu in &cpus {
            // `libc` is meaningless off linux — a darwin-musl platform must
            // never be emitted.
            if *os == Os::Linux {
                for libc in &libcs {
                    platforms.insert(
                        platform_name(*os, *cpu, Some(*libc)),
                        Platform { os: *os, cpu: *cpu, libc: Some(*libc), constraints: None },
                    );
                }
            } else {
                platforms.insert(
                    platform_name(*os, *cpu, None),
                    Platform { os: *os, cpu: *cpu, libc: None, constraints: None },
                );
            }
        }
    }

    if platforms.is_empty() {
        return Err(
            "pnpm-workspace.yaml declares no supported platforms pudu can target \
             (after skipping win32); edit supportedArchitectures or remove it"
                .to_string(),
        );
    }

    Ok(DerivedPlatforms { platforms, warnings })
}

fn default_platforms() -> BTreeMap<String, Platform> {
    let mut m = BTreeMap::new();
    for (os, cpu, libc) in [
        (Os::Linux, Cpu::X64, Some(Libc::Glibc)),
        (Os::Linux, Cpu::Arm64, Some(Libc::Glibc)),
        (Os::Darwin, Cpu::Arm64, None),
    ] {
        m.insert(platform_name(os, cpu, libc), Platform { os, cpu, libc, constraints: None });
    }
    m
}
```

Add `pub mod init;` to `src/cli/mod.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib init`
Expected: 10 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/cli/init.rs src/cli/mod.rs
git commit -m "feat(s0): workspace detection and supportedArchitectures expansion"
```

---

### Task 8: Toolchain append state machine

**Files:**
- Create: `src/cli/toolchain.rs`
- Modify: `src/cli/mod.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `pub const BEGIN: &str` / `pub const END: &str` — the marker lines.
  - `pub fn managed_block(toolchain_bzl_label: &str) -> String` — the full marked block.
  - `pub enum AppendOutcome { Created, Appended, AlreadyManaged, Replaced, ExistingToolchain(String), Unparseable }`
  - `pub fn apply(existing: Option<&str>, block: &str, force: bool) -> (Option<String>, AppendOutcome)` — pure function over file contents; `None` in the returned tuple means "do not write".

The pure-function split matters: every row of the spec §3.3 table becomes a table-driven unit test with no filesystem involved.

- [ ] **Step 1: Write the failing test**

Create `src/cli/toolchain.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn block() -> String {
        managed_block("root//third-party/js:toolchains.bzl")
    }

    #[test]
    fn creates_the_file_when_absent() {
        let (written, outcome) = apply(None, &block(), false);
        assert!(matches!(outcome, AppendOutcome::Created));
        let text = written.unwrap();
        assert!(text.contains(BEGIN) && text.contains(END));
        assert!(text.contains("system_node_toolchain"));
    }

    #[test]
    fn appends_to_a_file_without_a_node_toolchain() {
        let existing = "system_python_toolchain(name = \"python\")\n";
        let (written, outcome) = apply(Some(existing), &block(), false);
        assert!(matches!(outcome, AppendOutcome::Appended));
        let text = written.unwrap();
        assert!(text.starts_with(existing), "existing content must be preserved verbatim");
        assert!(text.contains(BEGIN));
    }

    #[test]
    fn never_appends_over_an_existing_node_toolchain() {
        let existing = "system_node_toolchain(name = \"node\")\n";
        let (written, outcome) = apply(Some(existing), &block(), false);
        assert!(written.is_none(), "must not rewrite a user's own toolchain");
        match outcome {
            AppendOutcome::ExistingToolchain(t) => assert_eq!(t, "node"),
            other => panic!("expected ExistingToolchain, got {other:?}"),
        }
    }

    #[test]
    fn managed_block_present_is_left_alone_without_force() {
        let existing = format!("x = 1\n{}", block());
        let (written, outcome) = apply(Some(&existing), &block(), false);
        assert!(written.is_none());
        assert!(matches!(outcome, AppendOutcome::AlreadyManaged));
    }

    #[test]
    fn force_replaces_only_the_managed_span() {
        let existing = format!("before = 1\n{}after = 2\n", block());
        let stale = format!("{BEGIN}\nstale content\n{END}\n");
        let existing = existing.replace(&block(), &stale);

        let (written, outcome) = apply(Some(&existing), &block(), true);
        assert!(matches!(outcome, AppendOutcome::Replaced));
        let text = written.unwrap();
        assert!(text.contains("before = 1"), "content before the block must survive");
        assert!(text.contains("after = 2"), "content after the block must survive");
        assert!(!text.contains("stale content"));
        assert!(text.contains("system_node_toolchain"));
    }

    #[test]
    fn is_idempotent_across_three_runs() {
        let mut current: Option<String> = None;
        for _ in 0..3 {
            let (written, _) = apply(current.as_deref(), &block(), false);
            if let Some(t) = written {
                current = Some(t);
            }
        }
        let first = current.clone().unwrap();
        let (written, outcome) = apply(current.as_deref(), &block(), false);
        assert!(written.is_none());
        assert!(matches!(outcome, AppendOutcome::AlreadyManaged));
        assert_eq!(first, current.unwrap(), "content must be stable");
    }

    #[test]
    fn unbalanced_markers_are_treated_as_unparseable() {
        let existing = format!("{BEGIN}\nno end marker\n");
        let (written, outcome) = apply(Some(&existing), &block(), true);
        assert!(written.is_none());
        assert!(matches!(outcome, AppendOutcome::Unparseable));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib toolchain 2>&1 | tail -20`
Expected: FAIL — `cannot find function managed_block`.

- [ ] **Step 3: Write minimal implementation**

Put above `mod tests` in `src/cli/toolchain.rs`:

```rust
//! The `toolchains/BUCK` append state machine.
//!
//! The Buck2 prelude ships no Node toolchain, so pudu supplies one. Declining
//! to write it would force a manual step on every user, so pudu writes into a
//! user-owned file — which muntjac deliberately refused to do. The safety
//! machinery that requires lives here (spec §3.3):
//!
//! * nothing outside the markers is ever modified;
//! * an existing node toolchain is never overwritten;
//! * repeated runs converge (idempotent).
//!
//! [`apply`] is a pure function over file contents so every case is unit
//! testable without touching the filesystem.

pub const BEGIN: &str = "# --- begin pudu-managed (do not edit inside this block) ---";
pub const END: &str = "# --- end pudu-managed ---";

/// The marker-delimited block pudu owns inside `toolchains/BUCK`.
pub fn managed_block(toolchain_bzl_label: &str) -> String {
    format!(
        "{BEGIN}\nload(\"{toolchain_bzl_label}\", \"system_node_toolchain\")\n\
         system_node_toolchain(name = \"node\", visibility = [\"PUBLIC\"])\n{END}\n"
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum AppendOutcome {
    /// The file did not exist; it was created containing only the block.
    Created,
    /// The block was appended after existing content.
    Appended,
    /// A current managed block is already present; nothing to do.
    AlreadyManaged,
    /// `--force` replaced the contents of an existing managed block.
    Replaced,
    /// A node toolchain the user owns was found; pudu did not write.
    ExistingToolchain(String),
    /// Markers are unbalanced; pudu refuses to guess.
    Unparseable,
}

/// Does the file already declare a node toolchain outside pudu's block?
///
/// Deliberately conservative and textual: a false positive costs one printed
/// line of manual instruction, while a false negative produces a duplicate
/// target and a confusing Buck error.
fn existing_node_toolchain(text: &str) -> Option<String> {
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('#') {
            continue;
        }
        if l.starts_with("system_node_toolchain(") {
            return Some("node".to_string());
        }
    }
    None
}

/// Compute the new contents of `toolchains/BUCK`.
///
/// Returns `(None, outcome)` when nothing should be written.
pub fn apply(existing: Option<&str>, block: &str, force: bool) -> (Option<String>, AppendOutcome) {
    let Some(text) = existing else {
        return (Some(block.to_string()), AppendOutcome::Created);
    };

    let begin = text.find(BEGIN);
    let end = text.find(END);

    match (begin, end) {
        (Some(b), Some(e)) if e > b => {
            if !force {
                return (None, AppendOutcome::AlreadyManaged);
            }
            // Replace exactly the marked span, including END's trailing newline.
            let mut tail = e + END.len();
            if text[tail..].starts_with('\n') {
                tail += 1;
            }
            let mut out = String::with_capacity(text.len() + block.len());
            out.push_str(&text[..b]);
            out.push_str(block);
            out.push_str(&text[tail..]);
            (Some(out), AppendOutcome::Replaced)
        }
        (None, None) => {
            if let Some(name) = existing_node_toolchain(text) {
                return (None, AppendOutcome::ExistingToolchain(name));
            }
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
            out.push_str(block);
            (Some(out), AppendOutcome::Appended)
        }
        // Exactly one marker, or END before BEGIN: refuse to guess.
        _ => (None, AppendOutcome::Unparseable),
    }
}
```

Add `pub mod toolchain;` to `src/cli/mod.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib toolchain`
Expected: 7 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/cli/toolchain.rs src/cli/mod.rs
git commit -m "feat(s0): marker-delimited toolchains/BUCK append state machine"
```

---

### Task 9: `pudu init` file writing

**Files:**
- Modify: `src/cli/init.rs`, `src/cli/mod.rs`
- Test: `tests/init.rs`

**Interfaces:**
- Consumes: `detect`, `derive_platforms`, `platform_name` (Task 7); `toolchain::{apply, managed_block, AppendOutcome}` (Task 8).
- Produces: `pub fn run(force: bool, path: Option<PathBuf>) -> anyhow::Result<()>`.

- [ ] **Step 1: Write the failing test**

Create `tests/init.rs`:

```rust
mod common;

use std::fs;

use common::pudu;

fn workspace(with_lockfile: bool) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    if with_lockfile {
        fs::write(d.path().join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    }
    d
}

#[test]
fn writes_the_project_skeleton() {
    let d = workspace(true);
    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    for f in [
        "pudu.toml",
        "third-party/js/BUCK",
        "third-party/js/toolchains.bzl",
        "third-party/js/.gitignore",
        "third-party/js/fixups/.gitkeep",
        "toolchains/BUCK",
    ] {
        assert!(d.path().join(f).exists(), "missing {f}");
    }

    let cfg = fs::read_to_string(d.path().join("pudu.toml")).unwrap();
    assert!(cfg.contains("lockfile_path   = \"pnpm-lock.yaml\""), "{cfg}");
    assert!(cfg.contains("node_toolchain = \"toolchains//:node\""), "{cfg}");
}

#[test]
fn the_generated_config_passes_config_check() {
    let d = workspace(true);
    pudu(d.path()).arg("init").output().unwrap();
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn derives_platforms_from_supported_architectures() {
    let d = workspace(true);
    fs::write(
        d.path().join("pnpm-workspace.yaml"),
        "supportedArchitectures:\n  os: [linux]\n  cpu: [x64, arm64]\n",
    )
    .unwrap();
    pudu(d.path()).arg("init").output().unwrap();

    let cfg = fs::read_to_string(d.path().join("pudu.toml")).unwrap();
    assert!(cfg.contains("[platforms.linux-x64-gnu]"), "{cfg}");
    assert!(cfg.contains("[platforms.linux-arm64-gnu]"), "{cfg}");
    assert!(!cfg.contains("darwin"), "{cfg}");
}

#[test]
fn refuses_to_overwrite_without_force() {
    let d = workspace(true);
    pudu(d.path()).arg("init").output().unwrap();
    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("--force"), "{stderr}");
}

#[test]
fn force_overwrites() {
    let d = workspace(true);
    pudu(d.path()).arg("init").output().unwrap();
    let out = pudu(d.path()).args(["init", "--force"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn toolchain_append_is_idempotent() {
    let d = workspace(true);
    pudu(d.path()).arg("init").output().unwrap();
    let first = fs::read_to_string(d.path().join("toolchains/BUCK")).unwrap();
    pudu(d.path()).args(["init", "--force"]).output().unwrap();
    pudu(d.path()).args(["init", "--force"]).output().unwrap();
    let third = fs::read_to_string(d.path().join("toolchains/BUCK")).unwrap();
    assert_eq!(first, third, "toolchains/BUCK must be stable across runs");
}

#[test]
fn preserves_an_existing_toolchains_buck() {
    let d = workspace(true);
    fs::create_dir_all(d.path().join("toolchains")).unwrap();
    let original = "system_python_toolchain(name = \"python\")\n";
    fs::write(d.path().join("toolchains/BUCK"), original).unwrap();

    pudu(d.path()).arg("init").output().unwrap();

    let text = fs::read_to_string(d.path().join("toolchains/BUCK")).unwrap();
    assert!(text.starts_with(original), "existing content must be preserved: {text}");
    assert!(text.contains("system_node_toolchain"), "{text}");
}

#[test]
fn never_overwrites_a_user_node_toolchain() {
    let d = workspace(true);
    fs::create_dir_all(d.path().join("toolchains")).unwrap();
    let original = "system_node_toolchain(name = \"node\", node = \"/opt/node/bin/node\")\n";
    fs::write(d.path().join("toolchains/BUCK"), original).unwrap();

    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let text = fs::read_to_string(d.path().join("toolchains/BUCK")).unwrap();
    assert_eq!(text, original, "a user's own node toolchain must be untouched");
}

#[test]
fn undetected_project_writes_a_todo_template() {
    let d = workspace(false);
    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let cfg = fs::read_to_string(d.path().join("pudu.toml")).unwrap();
    assert!(cfg.contains("TODO"), "{cfg}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test init 2>&1 | tail -20`
Expected: FAIL — `init` returns the stub error.

- [ ] **Step 3: Write minimal implementation**

Append to `src/cli/init.rs` (above `mod tests`):

```rust
use crate::cli::toolchain::{self, AppendOutcome};

const TOOLCHAINS_BZL: &str = r#"##
## @generated by pudu init. Safe to edit.
##
## The Buck2 prelude ships no Node toolchain, so pudu defines one. Swap the
## `node` attribute for an absolute path, or replace this rule entirely, to
## use a hermetic Node instead of whatever is on PATH.
##

NodeToolchainInfo = provider(fields = {"node": provider_field(typing.Any, default = None)})

def _system_node_toolchain_impl(ctx):
    return [
        DefaultInfo(),
        NodeToolchainInfo(node = RunInfo(args = [ctx.attrs.node])),
    ]

system_node_toolchain = rule(
    impl = _system_node_toolchain_impl,
    attrs = {"node": attrs.string(default = "node")},
    is_toolchain_rule = True,
)
"#;

const THIRD_PARTY_GITIGNORE: &str = "# Populated by `pudu vendor` in vendor mode (v2).\nvendor/\n";

const PLACEHOLDER_BUCK: &str = "# Generated by pudu. Run: pudu buckify\n";

fn render_config(lockfile_path: &str, platforms: &BTreeMap<String, Platform>, detected: bool) -> String {
    let mut s = String::new();
    s.push_str("# Generated by `pudu init`. Edit freely.\n");
    s.push_str("# Full schema: docs/superpowers/specs/2026-08-30-pudu-design.md\n");
    if !detected {
        s.push_str("\n# TODO: pudu init could not find a pnpm-lock.yaml.\n");
        s.push_str("# Edit `lockfile_path`, then run `pudu config check`.\n");
    }
    s.push('\n');
    s.push_str(&format!("lockfile_path   = \"{lockfile_path}\"\n"));
    s.push_str("third_party_dir = \"third-party/js\"\n\n");

    for (name, p) in platforms {
        s.push_str(&format!("[platforms.{name}]\n"));
        s.push_str(&format!("os   = \"{}\"\n", p.os.as_npm()));
        s.push_str(&format!("cpu  = \"{}\"\n", p.cpu.as_npm()));
        if let Some(l) = p.libc {
            s.push_str(&format!("libc = \"{}\"\n", l.as_npm()));
        }
        s.push('\n');
    }

    s.push_str("[registry]\ndefault = \"https://registry.npmjs.org\"\n\n");
    s.push_str("[fixups]\n");
    s.push_str("# Community fixup registry. Leave as \"none\" until a v0.1.0+ release exists.\n");
    s.push_str("registry              = \"none\"\n");
    s.push_str("allow_local_overrides = true\n\n");
    s.push_str("[scripts]\n");
    s.push_str("# Packages whose lifecycle scripts are acknowledged (not run). See design §6.\n");
    s.push_str("allow = []\n\n");
    s.push_str("[buck]\n");
    s.push_str("file_name      = \"BUCK\"\n");
    s.push_str("node_toolchain = \"toolchains//:node\"\n");
    s
}

/// Scaffold a pudu project in `path` (default: the current directory).
pub fn run(force: bool, path: Option<PathBuf>) -> anyhow::Result<()> {
    let root = match path {
        Some(p) => p,
        None => std::env::current_dir()?,
    };

    let config_path = root.join("pudu.toml");
    if config_path.exists() && !force {
        anyhow::bail!(
            "{} already exists; pass --force to overwrite",
            config_path.display()
        );
    }

    let found = detect(&root);
    let lockfile_rel = match &found {
        Some(d) => pathdiff::diff_paths(&d.lockfile, &root)
            .unwrap_or_else(|| d.lockfile.clone())
            .to_string_lossy()
            .replace('\\', "/"),
        None => "TODO: path to your pnpm-lock.yaml".to_string(),
    };

    let ws_text = found
        .as_ref()
        .and_then(|d| d.workspace_yaml.as_ref())
        .map(std::fs::read_to_string)
        .transpose()?;

    let derived = derive_platforms(ws_text.as_deref()).map_err(|e| anyhow::anyhow!(e))?;
    for w in &derived.warnings {
        eprintln!("warning: {w}");
    }

    // pudu.toml
    std::fs::write(
        &config_path,
        render_config(&lockfile_rel, &derived.platforms, found.is_some()),
    )?;
    println!("wrote {}", config_path.display());

    // third-party/js skeleton — never overwrite existing contents.
    let tp = root.join("third-party/js");
    std::fs::create_dir_all(tp.join("fixups"))?;
    for (rel, contents) in [
        ("BUCK", PLACEHOLDER_BUCK),
        ("toolchains.bzl", TOOLCHAINS_BZL),
        (".gitignore", THIRD_PARTY_GITIGNORE),
        ("fixups/.gitkeep", ""),
    ] {
        let p = tp.join(rel);
        if p.exists() && !force {
            eprintln!("warning: {} exists; leaving it alone", p.display());
            continue;
        }
        std::fs::write(&p, contents)?;
    }
    println!("wrote {}", tp.display());

    // toolchains/BUCK — the one user-owned file pudu writes into.
    let tc_dir = root.join("toolchains");
    let tc_path = tc_dir.join("BUCK");
    let existing = match std::fs::read_to_string(&tc_path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let block = toolchain::managed_block("root//third-party/js:toolchains.bzl");
    let (written, outcome) = toolchain::apply(existing.as_deref(), &block, force);
    if let Some(text) = written {
        std::fs::create_dir_all(&tc_dir)?;
        std::fs::write(&tc_path, text)?;
    }
    match outcome {
        AppendOutcome::Created | AppendOutcome::Appended | AppendOutcome::Replaced => {
            println!("wrote {}", tc_path.display());
        }
        AppendOutcome::AlreadyManaged => {
            println!("{} already current", tc_path.display());
        }
        AppendOutcome::ExistingToolchain(name) => {
            println!(
                "{} already declares a node toolchain (`:{name}`); leaving it alone.\n\
                 If that is not the toolchain you want pudu to use, set \
                 `[buck] node_toolchain` in pudu.toml.",
                tc_path.display()
            );
        }
        AppendOutcome::Unparseable => {
            println!(
                "{} has unbalanced pudu markers; not modifying it. Add this manually:\n\n{block}",
                tc_path.display()
            );
        }
    }

    println!("\nNext: pudu vendor && pudu buckify");
    Ok(())
}
```

In `src/cli/mod.rs`, replace the `Commands::Init` arm with:

```rust
            Commands::Init { force, path } => init::run(force, path),
```

`pathdiff` is already a dependency.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test init`
Expected: 9 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/cli/init.rs src/cli/mod.rs tests/init.rs
git commit -m "feat(s0): pudu init writes project skeleton and node toolchain"
```

---

### Task 10: Exit-criteria sweep

**Files:**
- Modify: whatever the sweep turns up.

**Interfaces:**
- Consumes: everything.
- Produces: a green stage.

- [ ] **Step 1: Run the full suite**

Run: `cargo test --all-targets`
Expected: all tests pass. Investigate and fix any failure before continuing.

- [ ] **Step 2: Run the CI gates exactly as CI does**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo build --all-targets
```

Expected: all clean.

- [ ] **Step 3: Walk the spec's exit criteria by hand**

Work through §9 of `docs/superpowers/specs/2026-08-30-pudu-s0-scaffolding-design.md`, one numbered item at a time, in a scratch directory:

```bash
cd "$(mktemp -d)"
printf "lockfileVersion: '9.0'\n" > pnpm-lock.yaml
cargo run --manifest-path /home/jackm/repos/pudu/Cargo.toml -- init
cargo run --manifest-path /home/jackm/repos/pudu/Cargo.toml -- config check
cargo run --manifest-path /home/jackm/repos/pudu/Cargo.toml -- --help
cargo run --manifest-path /home/jackm/repos/pudu/Cargo.toml -- vendor; echo "exit=$?"
```

Confirm each of the ten criteria holds. Any that does not is a bug to fix, not a criterion to reword.

- [ ] **Step 4: Verify determinism**

Run `cargo test --all-targets` twice and confirm identical results, and that no test writes into the repo working tree:

```bash
cargo test --all-targets && cargo test --all-targets && git status --porcelain
```

Expected: `git status --porcelain` prints nothing.

- [ ] **Step 5: Update the roadmap and commit**

In `docs/superpowers/specs/2026-08-30-pudu-roadmap.md`, mark S0 shipped in the specs index: set its Plan cell to a link to this plan, and its Status cell to `✅ shipped`, adding the commit count and test count.

```bash
git add -A
git commit -m "chore(s0): mark S0 shipped in the roadmap"
```

---

## Notes for the implementer

**Read the spec.** This plan implements `docs/superpowers/specs/2026-08-30-pudu-s0-scaffolding-design.md`. Where the plan and the spec disagree, the spec wins — say so rather than silently following the plan.

**Do not use `let`-chains.** `if let ... && ...` stabilized in Rust 1.88, but this
crate declares `rust-version = "1.85"`. Your local toolchain will accept them
and CI (on stable) will too, so the breakage is silent — the declared MSRV would
simply be wrong. Use nested `if`s. The same applies to any other post-1.85
feature: if you want one, raise `rust-version` deliberately in its own commit
rather than letting it drift.

**Testing style.** Unit tests live in `#[cfg(test)] mod tests` inside the module they test; integration tests live in `tests/` and drive the real binary through `assert_cmd`. Test names state the behaviour, not the function name. Assertions carry the offending value in their message — every `assert!` above does this, and new ones should too.

**`--version` prints the crate version only.** Spec §9 criterion 6 says "version
plus a build identifier"; a git sha needs a `build.rs`, which is not worth adding
for cosmetics at S0. The spec has been amended to match, with the richer version
string folded into the S9 polish pass — the same stage where muntjac revisited
its own. Do not add a build script.

**Don't reach ahead.** No lockfile parsing, no tarball fetching, no BUCK emission. If a task seems to need one, it is a plan bug — stop and ask.
