# pudu S3 — vendor & the pudu.lock sidecar: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `pudu vendor` downloads every tarball the configured platforms need, verifies it against the lockfile's sha512, inspects it, and records the result in a deterministic committed `pudu.lock`; `pudu vendor --check` is an offline staleness gate.

**Architecture:** Six new modules drawn so exactly one of them touches the network. `registry.rs`, `tarball.rs`, and `sidecar.rs` are pure functions testable with neither a socket nor a temp directory; `cache.rs` touches only the filesystem; `fetch.rs` owns ureq and the worker pool; `cli/vendor.rs` orchestrates. Results are collected into `BTreeMap`s so byte-identical output holds under parallelism by construction rather than by luck.

**Tech Stack:** Rust 2024, MSRV 1.88. New deps `ureq` 3.4 (rustls), `tar` 0.4, `flate2` 1.1, `base64` 0.23, `dirs` 6.0; dev-only `httpmock` 0.8. `sha2` 0.10, `tempfile`, `toml`, `url`, `serde_json` already present.

**Spec:** [`docs/superpowers/specs/2026-08-31-pudu-s3-vendor-design.md`](../specs/2026-08-31-pudu-s3-vendor-design.md)
**Evidence:** [`docs/superpowers/research/2026-08-31-npm-tarball-vendor-survey.md`](../research/2026-08-31-npm-tarball-vendor-survey.md)

## Global Constraints

- **MSRV is 1.88.** `let`-chains are available and used in this codebase; nothing newer is.
- **`cargo clippy --all-targets -- -D warnings` and `cargo fmt --all -- --check` must pass.** CI sets `RUSTFLAGS: -D warnings`.
- **Determinism is an exit criterion.** Every collection that reaches output is a `BTreeMap` or `BTreeSet`. Never a `HashMap`.
- **A typed error must carry its own complete message.** `render_cli` shows the typed diagnostic found in the chain, so `.context(...)` over a typed error is silently dropped. Add a variant or a field instead.
- **Errors go in `typed_errors!`; warnings do not.** Warnings have no exit code (the S2 precedent).
- **stdout is machine-parseable.** `vendor` writes a file; all progress and diagnostics go to stderr.
- **sha256 is recorded only for bytes whose sha512 already verified.** That ordering is the entire trust chain.
- **`sha512` is stored as the lockfile's integrity string verbatim, prefix included; `sha256` is lowercase hex** (the form `http_archive` expects).
- **Every test must be able to fail.** After writing a test, break the code it covers and confirm that specific test reddens. Tasks name the mutation to apply.

## Spec deltas

Three error variants this plan adds that spec §8's table does not list. They are deliberate; a reviewer should treat them as part of the contract:

- `BadDerivedUrl { name, version, url }` — §3.3 derivation produces a string that `Url::parse` rejects. Exit 3.
- `MalformedIntegrity { key, integrity }` — an integrity string that is not `sha512-<base64>`. Exit 3.
- `CacheUnavailable` — no `PUDU_CACHE_DIR` and no OS cache directory. Exit 3.

## File structure

| File | Status | Responsibility |
|---|---|---|
| `Cargo.toml` | modify | five new deps, one dev-dep |
| `src/lib.rs` | modify | declare the five new modules |
| `src/error.rs` | modify | `ExitCode::Stale`, `VendorError`, `VendorWarning` |
| `src/registry.rs` | create | `(name, version, &RegistryConfig) → Url` |
| `src/tarball.rs` | create | verify sha512, hash sha256, walk the archive, resolve bins and install scripts |
| `src/sidecar.rs` | create | `pudu.lock` render, load, staleness diff |
| `src/cache.rs` | create | integrity-addressed store under `~/.cache/pudu` |
| `src/fetch.rs` | create | ureq agent, worker pool, retries, `--no-network` |
| `src/cli/context.rs` | create | shared `load_lenient()` / `load_validated()` |
| `src/cli/debug.rs` | modify | drop its private `load()`, use `context::load_lenient()` |
| `src/cli/vendor.rs` | create | orchestration, `--check`, reporting |
| `src/cli/mod.rs` | modify | `--jobs`, dispatch `Commands::Vendor` |
| `tests/vendor.rs` | create | httpmock integration coverage |
| `tests/vendor_oracle.rs` | create | `#[ignore]`d live-registry oracle |
| `tests/fixtures/lock/real/oracle/capture-manifests.mjs` | create | oracle capture script |
| `tests/fixtures/lock/real/oracle/manifests.json` | create | 400-row committed oracle |
| `.github/workflows/ci.yml` | modify | `vendor-oracle` job |

---

### Task 1: Dependencies, `ExitCode::Stale`, and the vendor diagnostics

Every later task needs these error types, so they land first and complete. Spec §8 gives the full list; nothing here is a stub.

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/error.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `ExitCode::Stale`; `VendorError` (variants below); `VendorWarning::{HasBinDisagreement, BinNameRejected, BinPathEscapes, BinNameCollision, NonStringBinValue}`.

- [ ] **Step 1: Add the dependencies**

```bash
cargo add ureq@3.4 tar@0.4 flate2@1.1 base64@0.23 dirs@6.0
cargo add --dev httpmock@0.8
```

Leave `sha2` at its existing `0.10` — 0.11 changes the `Digest` trait and this plan's code is written against 0.10.

- [ ] **Step 2: Write the failing test for the new exit code**

In `src/error.rs`'s `mod tests`, extend the existing `exit_codes_are_classified` table with vendor cases. Find the `let cases: [...]` (or equivalent) it iterates and add:

```rust
        (
            anyhow::Error::from(VendorError::Stale {
                differences: vec!["pudu.lock has no entry for `left-pad@1.3.0`".to_string()],
            }),
            ExitCode::Stale,
        ),
        (
            anyhow::Error::from(VendorError::NoPlatformsConfigured),
            ExitCode::InputInvalid,
        ),
        (
            anyhow::Error::from(VendorError::HttpStatus {
                key: "left-pad@1.3.0".to_string(),
                url: "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz".to_string(),
                status: 503,
            }),
            ExitCode::Internal,
        ),
```

- [ ] **Step 3: Run it to confirm it fails**

Run: `cargo test --lib error::tests::exit_codes_are_classified`
Expected: FAIL to compile — `VendorError` and `ExitCode::Stale` do not exist.

- [ ] **Step 4: Add `ExitCode::Stale`**

In the `ExitCode` enum, after `Unimplemented = 4`:

```rust
    /// `pudu vendor --check` found `pudu.lock` out of date. Distinct from
    /// `InputInvalid` so CI can tell "regenerate the sidecar" from "your
    /// config is wrong" without parsing the message.
    Stale = 5,
```

- [ ] **Step 5: Add `VendorError`**

Append to `src/error.rs`, after the platform section:

```rust
// --- Vendor (S3) ----------------------------------------------------------

/// Failures of `pudu vendor`.
#[derive(Debug, Error, Diagnostic)]
pub enum VendorError {
    #[error("pudu.lock is out of date ({} difference(s))", differences.len())]
    #[diagnostic(
        severity(Error),
        code(pudu::vendor::stale),
        help("run `pudu vendor` to regenerate it, and commit the result")
    )]
    Stale { differences: Vec<String> },

    #[error(
        "{} package(s) use a dependency source pudu cannot verify: {}",
        packages.len(),
        capped_list(packages, 10)
    )]
    #[diagnostic(
        code(pudu::vendor::unsupported_resolution),
        help(
            "pudu vendors registry tarballs, which carry an integrity hash it can check. git and URL dependencies carry none, so their bytes cannot be verified. See the roadmap for when they land."
        )
    )]
    UnsupportedResolution { packages: Vec<String> },

    #[error("{key}: tarball does not match the integrity recorded in pnpm-lock.yaml")]
    #[diagnostic(
        code(pudu::vendor::integrity_mismatch),
        help("the registry served different bytes than pnpm recorded. Do not ignore this: it means the tarball changed after your lockfile was written.")
    )]
    IntegrityMismatch {
        key: String,
        url: String,
        expected: String,
        actual: String,
    },

    #[error("{key}: {reason}")]
    #[diagnostic(
        code(pudu::vendor::malformed_tarball),
        help("npm tarballs nest every entry under `package/`; pudu emits `strip_prefix = \"package\"`, so an archive shaped otherwise would fail at build time instead")
    )]
    MalformedTarball { key: String, reason: String },

    #[error("{key}: tarball contains no package/package.json")]
    #[diagnostic(code(pudu::vendor::missing_package_json))]
    MissingPackageJson { key: String },

    #[error("{key}: cannot read integrity `{integrity}`")]
    #[diagnostic(
        code(pudu::vendor::malformed_integrity),
        help("pudu understands `sha512-<base64>`, which is what npm publishes")
    )]
    MalformedIntegrity { key: String, integrity: String },

    #[error("{key}: derived tarball URL is not a valid URL: {url}")]
    #[diagnostic(
        code(pudu::vendor::bad_derived_url),
        help("check the `[registry]` entry this package resolves to")
    )]
    BadDerivedUrl {
        key: String,
        name: String,
        url: String,
    },

    #[error("cannot read {path}")]
    #[diagnostic(
        code(pudu::vendor::sidecar_malformed),
        help("pudu.lock is generated; delete it and run `pudu vendor` to rebuild it")
    )]
    SidecarMalformed { path: PathBuf, reason: String },

    #[error("no platforms configured, so there is nothing to vendor")]
    #[diagnostic(
        code(pudu::vendor::no_platforms),
        help("add at least one `[platforms.<name>]` table to pudu.toml")
    )]
    NoPlatformsConfigured,

    #[error("{key}: not in the cache and --no-network was given")]
    #[diagnostic(
        code(pudu::vendor::network_disabled),
        help("run `pudu vendor` once without --no-network to warm the cache")
    )]
    NetworkDisabled { key: String, url: String },

    #[error("{key}: {url} returned HTTP {status}")]
    #[diagnostic(
        code(pudu::vendor::http_status),
        help("{}", http_help(*status))
    )]
    HttpStatus {
        key: String,
        url: String,
        status: u16,
    },

    #[error("{key}: cannot fetch {url}")]
    #[diagnostic(code(pudu::vendor::transport))]
    Transport {
        key: String,
        url: String,
        #[source]
        source: ureq::Error,
    },

    #[error("no cache directory available")]
    #[diagnostic(
        code(pudu::vendor::cache_unavailable),
        help("set PUDU_CACHE_DIR to a writable directory")
    )]
    CacheUnavailable,
}

/// Help text for an HTTP failure. 401 and 403 get their own wording because
/// a private registry is the most likely first encounter with either, and
/// "authentication is not implemented" is the answer the user needs.
fn http_help(status: u16) -> String {
    match status {
        401 | 403 => "pudu sends no credentials — authentication is not implemented yet. A registry requiring a token cannot be vendored from.".to_string(),
        404 => "the package may have been unpublished, or the `[registry]` host may be wrong for it".to_string(),
        _ => "the registry rejected the request; pudu already retried transient failures".to_string(),
    }
}

impl VendorError {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            VendorError::Stale { .. } => ExitCode::Stale,
            // A registry that is down or refusing is not the user's input
            // being invalid, so it stays unclassified rather than claiming
            // pudu.toml or the lockfile is at fault.
            VendorError::HttpStatus { .. } | VendorError::Transport { .. } => ExitCode::Internal,
            _ => ExitCode::InputInvalid,
        }
    }
}

/// Non-fatal findings from inspecting a tarball.
///
/// Every one of these is a property of somebody's published package rather
/// than of pudu's input being malformed, and none makes the rest of the
/// sidecar wrong.
#[derive(Debug, Clone, PartialEq, Eq, Error, Diagnostic)]
pub enum VendorWarning {
    #[error("{key}: pnpm-lock.yaml says hasBin: {lockfile}, but the tarball yields {found} command(s)")]
    #[diagnostic(
        severity(Warning),
        code(pudu::vendor::has_bin_disagreement),
        help("pudu records what the tarball contains; `hasBin` is a flag pnpm derives from registry metadata")
    )]
    HasBinDisagreement {
        key: String,
        lockfile: bool,
        found: usize,
    },

    #[error("{key}: dropping bin `{name}` — the name is not URL-safe")]
    #[diagnostic(severity(Warning), code(pudu::vendor::bin_name_rejected))]
    BinNameRejected { key: String, name: String },

    #[error("{key}: dropping bin `{name}` — its path `{path}` escapes the package")]
    #[diagnostic(severity(Warning), code(pudu::vendor::bin_path_escapes))]
    BinPathEscapes {
        key: String,
        name: String,
        path: String,
    },

    #[error("{key}: two bins named `{name}`; keeping the last")]
    #[diagnostic(severity(Warning), code(pudu::vendor::bin_name_collision))]
    BinNameCollision { key: String, name: String },

    #[error("{key}: dropping bin `{name}` — its value is not a string")]
    #[diagnostic(severity(Warning), code(pudu::vendor::non_string_bin_value))]
    NonStringBinValue { key: String, name: String },
}
```

- [ ] **Step 6: Register `VendorError`**

```rust
typed_errors! {
    CliError => CliError::exit_code,
    ConfigError => |_| ExitCode::InputInvalid,
    DeriveError => |_| ExitCode::InputInvalid,
    LockError => |_| ExitCode::InputInvalid,
    VendorError => VendorError::exit_code,
}
```

- [ ] **Step 7: Satisfy the registry-coverage test**

`every_registered_error_classifies_with_a_code_and_an_exit_code` asserts its sample set covers `REGISTERED_ERRORS`. Add a `VendorError` sample to that test's sample list (any variant; `NoPlatformsConfigured` needs no fields).

- [ ] **Step 8: Run the suite**

Run: `cargo test --lib error`
Expected: PASS.

- [ ] **Step 9: Verify the new test can fail**

Change `VendorError::Stale { .. } => ExitCode::Stale` to `ExitCode::InputInvalid`. Confirm `exit_codes_are_classified` FAILS. Revert.

- [ ] **Step 10: Commit**

```bash
git add Cargo.toml Cargo.lock src/error.rs
git commit -m "feat(vendor): add the S3 dependency set and vendor diagnostics"
```

---

### Task 2: `registry.rs` — tarball URL derivation

The survey verified this rule against 400 live registry manifests with zero mismatches. It is exact on the public registry; the reason `pudu.lock` records the resolved URL is private registries, not doubt about this rule.

**Files:**
- Create: `src/registry.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `VendorError::BadDerivedUrl` (Task 1); `crate::config::RegistryConfig { default: Url, scopes: BTreeMap<String, Url> }` (existing).
- Produces: `pub fn tarball_url(name: &str, version: &str, cfg: &RegistryConfig) -> Result<Url, VendorError>` and `pub fn registry_for<'a>(name: &str, cfg: &'a RegistryConfig) -> &'a Url`.

- [ ] **Step 1: Declare the module**

Add `pub mod registry;` to `src/lib.rs`, keeping the list alphabetical (`platform`, then `registry`).

- [ ] **Step 2: Write the failing tests**

Create `src/registry.rs` with only the tests, so they fail to compile first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(default: &str, scopes: &[(&str, &str)]) -> RegistryConfig {
        RegistryConfig {
            default: Url::parse(default).unwrap(),
            scopes: scopes
                .iter()
                .map(|(k, v)| (k.to_string(), Url::parse(v).unwrap()))
                .collect(),
        }
    }

    fn url(name: &str, version: &str, c: &RegistryConfig) -> String {
        tarball_url(name, version, c).unwrap().to_string()
    }

    #[test]
    fn an_unscoped_name_derives_the_npm_url() {
        let c = cfg("https://registry.npmjs.org", &[]);
        assert_eq!(
            url("left-pad", "1.3.0", &c),
            "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz"
        );
    }

    #[test]
    fn a_scoped_name_drops_the_scope_from_the_basename_only() {
        let c = cfg("https://registry.npmjs.org", &[]);
        assert_eq!(
            url("@babel/core", "7.28.6", &c),
            "https://registry.npmjs.org/@babel/core/-/core-7.28.6.tgz"
        );
    }

    #[test]
    fn a_scope_override_changes_the_host() {
        let c = cfg(
            "https://registry.npmjs.org",
            &[("@myorg", "https://npm.mycorp.example")],
        );
        assert_eq!(
            url("@myorg/thing", "1.0.0", &c),
            "https://npm.mycorp.example/@myorg/thing/-/thing-1.0.0.tgz"
        );
    }

    #[test]
    fn a_scope_with_no_override_falls_back_to_the_default() {
        let c = cfg(
            "https://registry.npmjs.org",
            &[("@myorg", "https://npm.mycorp.example")],
        );
        assert_eq!(
            url("@babel/core", "7.28.6", &c),
            "https://registry.npmjs.org/@babel/core/-/core-7.28.6.tgz"
        );
    }

    #[test]
    fn scope_matching_is_exact_not_prefix() {
        // `@myorgtools` must not match the `@myorg` override: npm has no
        // notion of nested or prefixed scopes.
        let c = cfg(
            "https://registry.npmjs.org",
            &[("@myorg", "https://npm.mycorp.example")],
        );
        assert_eq!(
            url("@myorgtools/thing", "1.0.0", &c),
            "https://registry.npmjs.org/@myorgtools/thing/-/thing-1.0.0.tgz"
        );
    }

    #[test]
    fn a_registry_path_prefix_is_preserved() {
        // `Url::join` would discard the `repo` segment here, which is why
        // derivation appends to the path by hand.
        let c = cfg("https://npm.example.com/artifactory/api/npm/repo", &[]);
        assert_eq!(
            url("left-pad", "1.3.0", &c),
            "https://npm.example.com/artifactory/api/npm/repo/left-pad/-/left-pad-1.3.0.tgz"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_registry_changes_nothing() {
        let with = cfg("https://npm.example.com/repo/", &[]);
        let without = cfg("https://npm.example.com/repo", &[]);
        assert_eq!(
            url("left-pad", "1.3.0", &with),
            url("left-pad", "1.3.0", &without)
        );
    }

    #[test]
    fn a_prerelease_version_survives_verbatim() {
        let c = cfg("https://registry.npmjs.org", &[]);
        assert_eq!(
            url("@babel/core", "7.0.0-beta.4", &c),
            "https://registry.npmjs.org/@babel/core/-/core-7.0.0-beta.4.tgz"
        );
    }

    #[test]
    fn registry_for_returns_the_override_itself() {
        let c = cfg(
            "https://registry.npmjs.org",
            &[("@myorg", "https://npm.mycorp.example")],
        );
        assert_eq!(
            registry_for("@myorg/thing", &c).as_str(),
            "https://npm.mycorp.example/"
        );
    }
}
```

- [ ] **Step 3: Run them to confirm they fail**

Run: `cargo test --lib registry`
Expected: FAIL to compile — `tarball_url` and `registry_for` do not exist.

- [ ] **Step 4: Implement**

Prepend to `src/registry.rs`:

```rust
//! Tarball URL derivation.
//!
//! `<registry>/<name>/-/<basename>-<version>.tgz`, where `basename` is the
//! package name after its scope. Verified exact against 400 live registry
//! manifests — see the tarball survey, §1. Private registries are why the
//! resolved URL is recorded in `pudu.lock` rather than re-derived later.

use url::Url;

use crate::config::RegistryConfig;
use crate::error::VendorError;

/// The registry serving `name`: an exact `@scope` override when one is
/// configured, else the default.
///
/// Matching is exact. npm has no nested scopes, so `@myorgtools` must not
/// pick up an `@myorg` override.
pub fn registry_for<'a>(name: &str, cfg: &'a RegistryConfig) -> &'a Url {
    if let Some(scope) = scope_of(name)
        && let Some(url) = cfg.scopes.get(scope)
    {
        return url;
    }
    &cfg.default
}

/// `@babel/core` → `Some("@babel")`; `left-pad` → `None`.
fn scope_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix('@')?;
    let slash = rest.find('/')?;
    // `rest` is one byte shorter than `name`, so the separator sits at
    // `slash + 1` in `name` and `..slash + 1` is the scope without it.
    Some(&name[..slash + 1])
}

/// The tarball URL for `name@version`.
pub fn tarball_url(
    name: &str,
    version: &str,
    cfg: &RegistryConfig,
) -> Result<Url, VendorError> {
    let base = registry_for(name, cfg);
    let basename = name.rsplit('/').next().unwrap_or(name);
    // Built by hand rather than with `Url::join`, which resolves relatively
    // and would discard the last path segment of a registry served under a
    // prefix like `/artifactory/api/npm/repo`.
    let raw = format!(
        "{}/{name}/-/{basename}-{version}.tgz",
        base.as_str().trim_end_matches('/')
    );
    Url::parse(&raw).map_err(|_| VendorError::BadDerivedUrl {
        key: format!("{name}@{version}"),
        name: name.to_string(),
        url: raw,
    })
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --lib registry`
Expected: PASS, 9 tests.

- [ ] **Step 6: Verify the tests can fail**

Apply each mutation, confirm the named test reddens, revert:

1. Change `name.rsplit('/')` to `name.split('/')` → `a_scoped_name_drops_the_scope_from_the_basename_only` FAILS.
2. Change `scope_of` to return `Some(&name[..slash + 2])` → `a_scope_override_changes_the_host` FAILS.
3. Delete `.trim_end_matches('/')` → `a_trailing_slash_on_the_registry_changes_nothing` FAILS.
4. Replace the `cfg.scopes.get(scope)` lookup with a prefix scan (`cfg.scopes.iter().find(|(k, _)| name.starts_with(k.as_str()))`) → `scope_matching_is_exact_not_prefix` FAILS.

- [ ] **Step 7: Commit**

```bash
git add src/registry.rs src/lib.rs
git commit -m "feat(vendor): derive tarball URLs with scope overrides"
```

---

### Task 3: `tarball.rs` — verification, hashing, and `has_install_script`

Bin resolution is Task 4; this task establishes the archive walk it will consume.

The install-script rule has **three independent triggers**, and two of them are properties of the file list rather than of `package.json` (survey §2). Design §4 called this "package.json inspection" and was wrong.

**Files:**
- Create: `src/tarball.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `VendorError::{IntegrityMismatch, MalformedTarball, MissingPackageJson, MalformedIntegrity}` (Task 1).
- Produces:
  - `pub struct Inspection { pub bin: BTreeMap<String, String>, pub has_install_script: bool }`
  - `pub struct Verified { pub sha256: String, pub size: u64, pub inspection: Inspection }`
  - `pub fn verify_and_inspect(key: &str, name: &str, url: &str, bytes: &[u8], integrity: &str) -> Result<(Verified, Vec<VendorWarning>), VendorError>`
  - `pub fn decode_integrity(key: &str, integrity: &str) -> Result<Vec<u8>, VendorError>`
  - `pub fn sha512_digest(bytes: &[u8]) -> Vec<u8>`
  - `pub fn hex(bytes: &[u8]) -> String`
  - `pub(crate) struct Manifest` and `pub(crate) struct Archive { manifest: Manifest, entries: Vec<String> }` for Task 4

`name` is passed separately from `key` because the string-`bin` rule names the command after the **package**, and the key carries a version too. `url` is passed in only so `IntegrityMismatch` can name it; inspection never fetches anything.

- [ ] **Step 1: Declare the module**

Add `pub mod tarball;` to `src/lib.rs`.

- [ ] **Step 2: Write the failing tests**

Create `src/tarball.rs` containing only this test module. `tarball()` builds archives in memory, so no fixture files and no network are needed anywhere in this task.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const URL: &str = "https://registry.example/p/-/p-1.0.0.tgz";

    /// Build a gzipped tar in memory. Paths are given relative to the
    /// package root and nested under `package/` here.
    pub(crate) fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
        rooted_tarball("package", files)
    }

    pub(crate) fn rooted_tarball(root: &str, files: &[(&str, &str)]) -> Vec<u8> {
        let mut ar = tar::Builder::new(Vec::new());
        for (path, body) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            ar.append_data(&mut h, format!("{root}/{path}"), body.as_bytes())
                .unwrap();
        }
        let tar_bytes = ar.into_inner().unwrap();
        let mut gz =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar_bytes).unwrap();
        gz.finish().unwrap()
    }

    pub(crate) fn integrity_of(bytes: &[u8]) -> String {
        use base64::Engine as _;
        format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(sha512_digest(bytes))
        )
    }

    fn inspect(files: &[(&str, &str)]) -> Verified {
        let bytes = tarball(files);
        let i = integrity_of(&bytes);
        verify_and_inspect("p@1.0.0", "p", URL, &bytes, &i).unwrap().0
    }

    #[test]
    fn a_matching_integrity_verifies() {
        let v = inspect(&[("package.json", r#"{"name":"p"}"#)]);
        assert_eq!(v.sha256.len(), 64, "sha256 is 32 bytes of lowercase hex");
        assert!(v.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    }

    #[test]
    fn size_is_the_compressed_byte_count() {
        let bytes = tarball(&[("package.json", r#"{"name":"p"}"#)]);
        let i = integrity_of(&bytes);
        let (v, _) = verify_and_inspect("p@1.0.0", "p", URL, &bytes, &i).unwrap();
        assert_eq!(v.size, bytes.len() as u64);
    }

    #[test]
    fn a_mismatched_integrity_names_both_hashes() {
        let bytes = tarball(&[("package.json", r#"{"name":"p"}"#)]);
        let wrong = integrity_of(b"different bytes entirely");
        let err = verify_and_inspect("p@1.0.0", "p", URL, &bytes, &wrong).unwrap_err();
        let VendorError::IntegrityMismatch { expected, actual, key, url } = &err else {
            panic!("wrong variant: {err:?}");
        };
        assert_eq!(key, "p@1.0.0");
        assert_eq!(url, URL, "the error must name the URL it fetched");
        assert_eq!(*expected, wrong);
        assert_eq!(*actual, integrity_of(&bytes));
        assert_ne!(expected, actual);
    }

    #[test]
    fn a_non_package_root_is_rejected() {
        // A codeload archive nests under `<repo>-<sha>/`. pudu emits
        // `strip_prefix = "package"`, so this must fail here rather than at
        // build time.
        let bytes = rooted_tarball("is-plain-obj-3b33a59", &[("package.json", "{}")]);
        let i = integrity_of(&bytes);
        let err = verify_and_inspect("p@1.0.0", "p", URL, &bytes, &i).unwrap_err();
        let VendorError::MalformedTarball { reason, .. } = &err else {
            panic!("wrong variant: {err:?}");
        };
        assert!(
            reason.contains("is-plain-obj-3b33a59"),
            "the reason must name the root it found: {reason}"
        );
    }

    #[test]
    fn a_tarball_without_a_manifest_is_rejected() {
        let bytes = tarball(&[("index.js", "module.exports = 1;\n")]);
        let i = integrity_of(&bytes);
        let err = verify_and_inspect("p@1.0.0", "p", URL, &bytes, &i).unwrap_err();
        assert!(matches!(err, VendorError::MissingPackageJson { .. }), "{err:?}");
    }

    #[test]
    fn unparseable_json_is_a_malformed_tarball_not_a_panic() {
        let bytes = tarball(&[("package.json", "{not json")]);
        let i = integrity_of(&bytes);
        let err = verify_and_inspect("p@1.0.0", "p", URL, &bytes, &i).unwrap_err();
        assert!(matches!(err, VendorError::MalformedTarball { .. }), "{err:?}");
    }

    #[test]
    fn an_integrity_that_is_not_sha512_is_rejected() {
        let bytes = tarball(&[("package.json", "{}")]);
        for bad in ["sha1-abc", "not-an-integrity", "sha512", "sha512-!!!!"] {
            let err = verify_and_inspect("p@1.0.0", "p", URL, &bytes, bad).unwrap_err();
            assert!(
                matches!(err, VendorError::MalformedIntegrity { .. }),
                "{bad} should be rejected, got {err:?}"
            );
        }
    }

    // --- has_install_script: each trigger in isolation --------------------

    #[test]
    fn no_scripts_and_no_marker_files_means_no_install_script() {
        let v = inspect(&[
            ("package.json", r#"{"name":"p","scripts":{"test":"mocha","build":"tsc"}}"#),
            ("index.js", ""),
        ]);
        assert!(!v.inspection.has_install_script);
    }

    #[test]
    fn each_install_script_key_triggers_on_its_own() {
        for key in ["preinstall", "install", "postinstall"] {
            let json = format!(r#"{{"name":"p","scripts":{{"{key}":"node x.js"}}}}"#);
            let v = inspect(&[("package.json", &json)]);
            assert!(v.inspection.has_install_script, "{key} must trigger");
        }
    }

    #[test]
    fn a_lifecycle_script_that_is_not_an_install_hook_does_not_trigger() {
        let v = inspect(&[(
            "package.json",
            r#"{"name":"p","scripts":{"prepare":"husky","prepublishOnly":"x"}}"#,
        )]);
        assert!(!v.inspection.has_install_script);
    }

    #[test]
    fn an_empty_install_script_does_not_trigger() {
        let v = inspect(&[("package.json", r#"{"name":"p","scripts":{"install":""}}"#)]);
        assert!(!v.inspection.has_install_script);
    }

    #[test]
    fn binding_gyp_triggers_with_no_scripts_at_all() {
        // fsevents@2.3.3 is the live instance of this in our own fixture.
        let v = inspect(&[("package.json", r#"{"name":"p"}"#), ("binding.gyp", "{}")]);
        assert!(v.inspection.has_install_script);
    }

    #[test]
    fn binding_gyp_below_the_root_does_not_trigger() {
        let v = inspect(&[
            ("package.json", r#"{"name":"p"}"#),
            ("src/binding.gyp", "{}"),
        ]);
        assert!(!v.inspection.has_install_script);
    }

    #[test]
    fn a_hooks_file_triggers() {
        let v = inspect(&[
            ("package.json", r#"{"name":"p"}"#),
            (".hooks/postinstall", "#!/bin/sh\n"),
        ]);
        assert!(v.inspection.has_install_script);
    }

    #[test]
    fn a_file_merely_starting_with_dot_hooks_does_not_trigger() {
        // pnpm's regex is `^\.hooks[\\/]`, so the separator is required.
        let v = inspect(&[
            ("package.json", r#"{"name":"p"}"#),
            (".hooksrc", "{}"),
        ]);
        assert!(!v.inspection.has_install_script);
    }
}
```

- [ ] **Step 3: Run them to confirm they fail**

Run: `cargo test --lib tarball`
Expected: FAIL to compile — nothing in the module exists yet.

- [ ] **Step 4: Implement**

Prepend to `src/tarball.rs`:

```rust
//! Tarball verification and inspection.
//!
//! One pass over the bytes answers everything `pudu.lock` records that
//! `pnpm-lock.yaml` cannot: the sha256 Buck can verify, the compressed size,
//! the bin map, and whether the package runs an install script.
//!
//! Reading `package.json` is not sufficient for either of the last two. The
//! install-script rule fires on a root `binding.gyp` or a `.hooks/` entry as
//! well as on scripts, and `directories.bin` names a directory whose contents
//! decide the bin map. Both need the archive's file list — which is free,
//! since the bytes are already being walked to hash them. See the tarball
//! survey, §2 and §3.

use std::collections::BTreeMap;
use std::io::Read;

use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha256, Sha512};

use crate::error::{VendorError, VendorWarning};

/// What inspecting the archive yields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Inspection {
    pub bin: BTreeMap<String, String>,
    pub has_install_script: bool,
}

/// A tarball whose sha512 matched the lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    /// Lowercase hex — the form `http_archive` expects.
    pub sha256: String,
    /// Compressed byte count: what Buck will download.
    pub size: u64,
    pub inspection: Inspection,
}

/// The subset of `package.json` that matters here.
///
/// Every field is a raw `Value` rather than a typed shape. A published
/// package can carry anything at all in these keys — `"directories": []`,
/// a non-string script — and one malformed manifest six levels down the
/// dependency tree must not fail the whole vendor pass. `Value` defaults to
/// `Null`, so an absent key and a nonsense key both navigate to `None`.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Manifest {
    #[serde(default)]
    pub(crate) bin: serde_json::Value,
    #[serde(default)]
    pub(crate) directories: serde_json::Value,
    #[serde(default)]
    pub(crate) scripts: serde_json::Value,
}

/// The archive, reduced to what inspection needs.
pub(crate) struct Archive {
    pub(crate) manifest: Manifest,
    /// File entries only, relative to the package root, `/`-separated.
    pub(crate) entries: Vec<String>,
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

pub fn sha512_digest(bytes: &[u8]) -> Vec<u8> {
    let mut h = Sha512::new();
    h.update(bytes);
    h.finalize().to_vec()
}

fn sha256_digest(bytes: &[u8]) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().to_vec()
}

/// Decode a `sha512-<base64>` integrity string to its raw digest.
pub fn decode_integrity(key: &str, integrity: &str) -> Result<Vec<u8>, VendorError> {
    let malformed = || VendorError::MalformedIntegrity {
        key: key.to_string(),
        integrity: integrity.to_string(),
    };
    let b64 = integrity.strip_prefix("sha512-").ok_or_else(malformed)?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| malformed())?;
    if raw.len() != 64 {
        return Err(malformed());
    }
    Ok(raw)
}

fn encode_integrity(digest: &[u8]) -> String {
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

/// Verify `bytes` against `integrity`, then inspect the archive.
///
/// The order is the trust chain: sha256 is computed only for bytes whose
/// sha512 already matched what pnpm recorded.
pub fn verify_and_inspect(
    key: &str,
    name: &str,
    url: &str,
    bytes: &[u8],
    integrity: &str,
) -> Result<(Verified, Vec<VendorWarning>), VendorError> {
    let expected = decode_integrity(key, integrity)?;
    let actual = sha512_digest(bytes);
    if actual != expected {
        return Err(VendorError::IntegrityMismatch {
            key: key.to_string(),
            url: url.to_string(),
            expected: integrity.to_string(),
            actual: encode_integrity(&actual),
        });
    }

    let archive = read_archive(key, bytes)?;
    let mut warnings = Vec::new();
    let inspection = Inspection {
        bin: resolve_bins(key, name, &archive, &mut warnings),
        has_install_script: has_install_script(&archive),
    };

    Ok((
        Verified {
            sha256: hex(&sha256_digest(bytes)),
            size: bytes.len() as u64,
            inspection,
        },
        warnings,
    ))
}

/// Walk the archive once, collecting `package.json` and the file list.
fn read_archive(key: &str, bytes: &[u8]) -> Result<Archive, VendorError> {
    let malformed = |reason: String| VendorError::MalformedTarball {
        key: key.to_string(),
        reason,
    };

    let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
    let mut ar = tar::Archive::new(dec);
    let mut entries = Vec::new();
    let mut manifest_text: Option<String> = None;

    for entry in ar
        .entries()
        .map_err(|e| malformed(format!("cannot read tar entries: {e}")))?
    {
        let mut entry = entry.map_err(|e| malformed(format!("cannot read tar entry: {e}")))?;
        let is_file = entry.header().entry_type().is_file();
        let path = entry
            .path()
            .map_err(|e| malformed(format!("cannot read entry path: {e}")))?
            .to_string_lossy()
            .replace('\\', "/");

        let mut parts = path.splitn(2, '/');
        let root = parts.next().unwrap_or_default();
        if root != "package" {
            return Err(malformed(format!(
                "archive is rooted at `{root}`, not `package`"
            )));
        }
        let Some(rel) = parts.next().filter(|r| !r.is_empty()) else {
            continue; // the `package/` directory entry itself
        };

        if !is_file {
            continue;
        }
        if rel == "package.json" {
            let mut s = String::new();
            entry
                .read_to_string(&mut s)
                .map_err(|e| malformed(format!("cannot read package.json: {e}")))?;
            manifest_text = Some(s);
        }
        entries.push(rel.to_string());
    }

    let text = manifest_text.ok_or_else(|| VendorError::MissingPackageJson {
        key: key.to_string(),
    })?;
    let manifest: Manifest = serde_json::from_str(&text)
        .map_err(|e| malformed(format!("package.json is not valid JSON: {e}")))?;

    entries.sort();
    Ok(Archive { manifest, entries })
}

/// pnpm's rule, from `@pnpm/building.pkg-requires-build`: install scripts, a
/// root `binding.gyp`, or anything under `.hooks/`. Survey §2.
fn has_install_script(archive: &Archive) -> bool {
    let script = |k: &str| {
        archive
            .manifest
            .scripts
            .get(k)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| !s.is_empty())
    };
    // `Value::get` on `Null` is `None`, so a manifest with no `scripts` key
    // at all takes the same path as one whose `scripts` is not an object.
    script("preinstall")
        || script("install")
        || script("postinstall")
        || archive
            .entries
            .iter()
            .any(|e| e == "binding.gyp" || e.starts_with(".hooks/"))
}

/// Placeholder until Task 4 implements the real rules.
fn resolve_bins(
    _key: &str,
    _name: &str,
    _archive: &Archive,
    _warnings: &mut Vec<VendorWarning>,
) -> BTreeMap<String, String> {
    BTreeMap::new()
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --lib tarball`
Expected: PASS, 14 tests.

- [ ] **Step 6: Verify the tests can fail**

Apply each mutation, confirm the named test reddens, revert:

1. Drop the `.is_some_and(|s| !s.is_empty())` guard (accept any present key) → `an_empty_install_script_does_not_trigger` FAILS.
2. Change `e.starts_with(".hooks/")` to `e.starts_with(".hooks")` → `a_file_merely_starting_with_dot_hooks_does_not_trigger` FAILS.
3. Change `e == "binding.gyp"` to `e.ends_with("binding.gyp")` → `binding_gyp_below_the_root_does_not_trigger` FAILS.
4. Delete the `root != "package"` check → `a_non_package_root_is_rejected` FAILS.
5. Change `size: bytes.len() as u64` to the decompressed length → `size_is_the_compressed_byte_count` FAILS.

- [ ] **Step 7: Commit**

```bash
git add src/tarball.rs src/lib.rs
git commit -m "feat(vendor): verify tarballs and detect install scripts

pnpm's rule fires on a root binding.gyp or a .hooks/ entry as well as on
scripts, so this reads the archive's file list rather than package.json
alone. Design §4 called it package.json inspection and was wrong."
```

---

### Task 4: `tarball.rs` — bin resolution

`@pnpm/package-bins`, reproduced exactly. The fixture reaches only two of these branches (survey §3: one string form, 26 object forms, zero `directories.bin`, zero rejected names), so every other branch needs a synthetic tarball — which is why Task 3 built them in memory.

**Files:**
- Modify: `src/tarball.rs`

**Interfaces:**
- Consumes: `Archive`, `Manifest`, `Directories` (Task 3); `VendorWarning::{BinNameRejected, BinPathEscapes, BinNameCollision, NonStringBinValue}` (Task 1).
- Produces: a working `resolve_bins`, replacing Task 3's placeholder. The `hasBin` cross-check is **not** here — it needs the lockfile, so it lives in `cli/vendor.rs` (Task 8).

- [ ] **Step 1: Write the failing tests**

Append to `src/tarball.rs`'s `mod tests`:

```rust
    fn bins(files: &[(&str, &str)], name: &str) -> BTreeMap<String, String> {
        let bytes = tarball(files);
        let i = integrity_of(&bytes);
        verify_and_inspect("p@1.0.0", name, URL, &bytes, &i)
            .unwrap()
            .0
            .inspection
            .bin
    }

    fn bins_with_warnings(
        files: &[(&str, &str)],
        name: &str,
    ) -> (BTreeMap<String, String>, Vec<VendorWarning>) {
        let bytes = tarball(files);
        let i = integrity_of(&bytes);
        let (v, w) = verify_and_inspect("p@1.0.0", name, URL, &bytes, &i).unwrap();
        (v.inspection.bin, w)
    }

    #[test]
    fn no_bin_field_yields_no_commands() {
        assert!(bins(&[("package.json", r#"{"name":"p"}"#)], "p").is_empty());
    }

    #[test]
    fn a_string_bin_is_named_after_the_package() {
        let b = bins(&[("package.json", r#"{"name":"p","bin":"cli.js"}"#)], "p");
        assert_eq!(b, BTreeMap::from([("p".to_string(), "cli.js".to_string())]));
    }

    #[test]
    fn a_string_bin_on_a_scoped_package_drops_the_scope() {
        // @babel/parser is the live instance: its command is `parser`.
        let b = bins(
            &[("package.json", r#"{"name":"@babel/parser","bin":"./bin/babel-parser.js"}"#)],
            "@babel/parser",
        );
        assert_eq!(
            b,
            BTreeMap::from([("parser".to_string(), "bin/babel-parser.js".to_string())]),
            "the scope is stripped from the command and `./` from the path"
        );
    }

    #[test]
    fn an_object_bin_keeps_every_key() {
        let b = bins(
            &[(
                "package.json",
                r#"{"name":"p","bin":{"one":"a.js","two":"b/c.js"}}"#,
            )],
            "p",
        );
        assert_eq!(
            b,
            BTreeMap::from([
                ("one".to_string(), "a.js".to_string()),
                ("two".to_string(), "b/c.js".to_string()),
            ])
        );
    }

    #[test]
    fn an_object_bin_key_is_scope_stripped_too() {
        let b = bins(
            &[("package.json", r#"{"name":"p","bin":{"@scope/tool":"t.js"}}"#)],
            "p",
        );
        assert_eq!(b, BTreeMap::from([("tool".to_string(), "t.js".to_string())]));
    }

    #[test]
    fn a_name_that_is_not_url_safe_is_dropped_with_a_warning() {
        let (b, w) = bins_with_warnings(
            &[("package.json", r#"{"name":"p","bin":{"a b":"x.js","ok":"y.js"}}"#)],
            "p",
        );
        assert_eq!(b, BTreeMap::from([("ok".to_string(), "y.js".to_string())]));
        assert!(
            w.iter().any(|x| matches!(x, VendorWarning::BinNameRejected { name, .. } if name == "a b")),
            "{w:?}"
        );
    }

    #[test]
    fn the_dollar_name_is_exempt_from_the_url_safe_rule() {
        let b = bins(&[("package.json", r#"{"name":"p","bin":{"$":"x.js"}}"#)], "p");
        assert_eq!(b, BTreeMap::from([("$".to_string(), "x.js".to_string())]));
    }

    #[test]
    fn a_path_escaping_the_package_is_dropped_with_a_warning() {
        let (b, w) = bins_with_warnings(
            &[(
                "package.json",
                r#"{"name":"p","bin":{"evil":"../../../etc/passwd","ok":"y.js"}}"#,
            )],
            "p",
        );
        assert_eq!(b, BTreeMap::from([("ok".to_string(), "y.js".to_string())]));
        assert!(
            w.iter().any(|x| matches!(x, VendorWarning::BinPathEscapes { name, .. } if name == "evil")),
            "{w:?}"
        );
    }

    #[test]
    fn a_path_that_climbs_then_returns_stays_inside() {
        let b = bins(
            &[("package.json", r#"{"name":"p","bin":{"ok":"lib/../cli.js"}}"#)],
            "p",
        );
        assert_eq!(b, BTreeMap::from([("ok".to_string(), "cli.js".to_string())]));
    }

    #[test]
    fn a_non_string_bin_value_is_dropped_with_a_warning() {
        let (b, w) = bins_with_warnings(
            &[("package.json", r#"{"name":"p","bin":{"bad":42,"ok":"y.js"}}"#)],
            "p",
        );
        assert_eq!(b, BTreeMap::from([("ok".to_string(), "y.js".to_string())]));
        assert!(
            w.iter().any(|x| matches!(x, VendorWarning::NonStringBinValue { name, .. } if name == "bad")),
            "{w:?}"
        );
    }

    #[test]
    fn a_bin_field_that_is_neither_string_nor_object_yields_nothing() {
        // pnpm's `Object.entries(42)` is `[]`, and it does not fall back to
        // directories.bin — the `if (manifest.bin)` branch is already taken.
        let b = bins(
            &[(
                "package.json",
                r#"{"name":"p","bin":42,"directories":{"bin":"tools"}}"#,
            ), ("tools/t.js", "")],
            "p",
        );
        assert!(b.is_empty(), "{b:?}");
    }

    #[test]
    fn directories_bin_is_walked_recursively_and_keyed_on_basename() {
        let b = bins(
            &[
                ("package.json", r#"{"name":"p","directories":{"bin":"tools"}}"#),
                ("tools/one.js", ""),
                ("tools/nested/two.js", ""),
            ],
            "p",
        );
        assert_eq!(
            b,
            BTreeMap::from([
                ("one.js".to_string(), "tools/one.js".to_string()),
                ("two.js".to_string(), "tools/nested/two.js".to_string()),
            ]),
            "nested files collapse to bare basenames"
        );
    }

    #[test]
    fn directories_bin_is_ignored_when_bin_is_present() {
        let b = bins(
            &[
                (
                    "package.json",
                    r#"{"name":"p","bin":"cli.js","directories":{"bin":"tools"}}"#,
                ),
                ("tools/t.js", ""),
            ],
            "p",
        );
        assert_eq!(b, BTreeMap::from([("p".to_string(), "cli.js".to_string())]));
    }

    #[test]
    fn a_bin_name_collision_keeps_the_last_and_warns() {
        let (b, w) = bins_with_warnings(
            &[
                ("package.json", r#"{"name":"p","directories":{"bin":"tools"}}"#),
                ("tools/a/dup.js", "first"),
                ("tools/b/dup.js", "second"),
            ],
            "p",
        );
        assert_eq!(
            b,
            BTreeMap::from([("dup.js".to_string(), "tools/b/dup.js".to_string())]),
            "entries are visited in sorted order, so the last one wins deterministically"
        );
        assert!(
            w.iter().any(|x| matches!(x, VendorWarning::BinNameCollision { name, .. } if name == "dup.js")),
            "{w:?}"
        );
    }
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test --lib tarball`
Expected: the new tests FAIL — `resolve_bins` is still the placeholder returning an empty map. `no_bin_field_yields_no_commands` and `a_bin_field_that_is_neither_string_nor_object_yields_nothing` will pass, which is expected: they assert emptiness.

- [ ] **Step 3: Implement**

Replace the placeholder `resolve_bins` in `src/tarball.rs` with:

```rust
/// `@pnpm/package-bins`, reproduced. Survey §3.
///
/// A `bin` field takes precedence over `directories.bin` even when it yields
/// nothing — pnpm's `if (manifest.bin)` branch is already taken by then, so
/// there is no fallback. The branch is a JavaScript truthiness test, so
/// `null`, `false`, and `""` fall through to `directories.bin` while `42` and
/// `[]` do not.
fn resolve_bins(
    key: &str,
    name: &str,
    archive: &Archive,
    warnings: &mut Vec<VendorWarning>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();

    match &archive.manifest.bin {
        serde_json::Value::String(path) if !path.is_empty() => {
            insert_bin(key, command_name(name), path, &mut out, warnings);
        }
        serde_json::Value::Object(map) => {
            for (raw, value) in map {
                match value.as_str() {
                    Some(path) => insert_bin(key, command_name(raw), path, &mut out, warnings),
                    None => warnings.push(VendorWarning::NonStringBinValue {
                        key: key.to_string(),
                        name: raw.clone(),
                    }),
                }
            }
        }
        // Present but unusable. pnpm's `if (manifest.bin)` branch is already
        // taken, so there is no fall back to `directories.bin`.
        serde_json::Value::Bool(true) | serde_json::Value::Number(_) | serde_json::Value::Array(_) => {}
        // Absent, or falsy in JavaScript's sense (`null`, `false`, `""`),
        // which is what `if (manifest.bin)` actually tests.
        _ => {
            if let Some(dir) = archive
                .manifest
                .directories
                .get("bin")
                .and_then(serde_json::Value::as_str)
                && let Some(normalized) = contained_path(dir)
            {
                let prefix = format!("{normalized}/");
                // `archive.entries` is sorted, so "last wins" on a collision
                // is deterministic rather than dependent on archive order.
                for entry in &archive.entries {
                    if let Some(rest) = entry.strip_prefix(&prefix) {
                        let base = rest.rsplit('/').next().unwrap_or(rest);
                        insert_named(key, base, entry, &mut out, warnings);
                    }
                }
            }
        }
    }

    out
}

/// `@scope/tool` → `tool`; `tool` → `tool`.
///
/// pnpm slices from `indexOf('/') + 1`, which for a name with no slash is
/// `slice(0)` — the whole string.
fn command_name(raw: &str) -> &str {
    if raw.starts_with('@') {
        raw.split_once('/').map_or(raw, |(_, rest)| rest)
    } else {
        raw
    }
}

/// Whether `s` survives pnpm's `binName !== encodeURIComponent(binName)`
/// check: the unreserved set `encodeURIComponent` leaves alone.
///
/// An empty name passes, exactly as it does in JavaScript.
fn is_url_safe(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')'))
}

/// Normalize a package-relative path, returning `None` when it escapes the
/// package root.
///
/// A leading `/` is *not* an escape: pnpm joins onto the package path, so
/// `/etc/passwd` lands inside the package and `is-subdir` accepts it.
fn contained_path(rel: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            p => out.push(p),
        }
    }
    (!out.is_empty()).then(|| out.join("/"))
}

fn insert_bin(
    key: &str,
    name: &str,
    path: &str,
    out: &mut BTreeMap<String, String>,
    warnings: &mut Vec<VendorWarning>,
) {
    if !is_url_safe(name) && name != "$" {
        warnings.push(VendorWarning::BinNameRejected {
            key: key.to_string(),
            name: name.to_string(),
        });
        return;
    }
    let Some(normalized) = contained_path(path) else {
        warnings.push(VendorWarning::BinPathEscapes {
            key: key.to_string(),
            name: name.to_string(),
            path: path.to_string(),
        });
        return;
    };
    if out.insert(name.to_string(), normalized).is_some() {
        warnings.push(VendorWarning::BinNameCollision {
            key: key.to_string(),
            name: name.to_string(),
        });
    }
}

/// The `directories.bin` path: the name comes from the filesystem, so the
/// URL-safe and containment checks that guard a manifest-declared name do
/// not apply — the entry is already inside the archive.
fn insert_named(
    key: &str,
    name: &str,
    path: &str,
    out: &mut BTreeMap<String, String>,
    warnings: &mut Vec<VendorWarning>,
) {
    if out.insert(name.to_string(), path.to_string()).is_some() {
        warnings.push(VendorWarning::BinNameCollision {
            key: key.to_string(),
            name: name.to_string(),
        });
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib tarball`
Expected: PASS, 28 tests.

- [ ] **Step 5: Verify the tests can fail**

Apply each mutation, confirm the named test reddens, revert:

1. Make `command_name` return `raw` unchanged → `a_string_bin_on_a_scoped_package_drops_the_scope` FAILS.
2. Add `'$'` to `is_url_safe`'s allowed set and delete the `name != "$"` clause → nothing should fail; this is the check that the exemption is a *separate* rule. If `the_dollar_name_is_exempt_from_the_url_safe_rule` still passes, instead delete the `&& name != "$"` clause alone and confirm it FAILS.
3. Change `contained_path`'s `".." => { out.pop()?; }` to `".." => { out.pop(); }` → `a_path_escaping_the_package_is_dropped_with_a_warning` FAILS.
4. Change the `Some(_) => {}` arm to fall through to the `None` arm's body → `a_bin_field_that_is_neither_string_nor_object_yields_nothing` FAILS.
5. Change `rest.rsplit('/')` to `rest.split('/')` → `directories_bin_is_walked_recursively_and_keyed_on_basename` FAILS.
6. Remove the `is_some()` collision check in `insert_named` → `a_bin_name_collision_keeps_the_last_and_warns` FAILS.

- [ ] **Step 6: Commit**

```bash
git add src/tarball.rs
git commit -m "feat(vendor): resolve bin maps the way pnpm does

Scope-stripped command names, the encodeURIComponent rejection with its \$
exemption, containment, and the directories.bin walk. The fixture reaches
only two of these branches, so the rest are covered by synthetic tarballs."
```

---

### Task 5: `sidecar.rs` — `pudu.lock` render, load, and staleness

Byte-identical output is an exit criterion, so rendering is by hand: the format is the spec rather than a property of a serializer's table-inlining heuristics. Parsing uses the `toml` crate normally.

**Files:**
- Create: `src/sidecar.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `VendorError::SidecarMalformed` (Task 1).
- Produces:
  - `pub const SIDECAR_VERSION: u32 = 1;`
  - `pub struct Entry { pub url: String, pub sha512: String, pub sha256: String, pub size: u64, pub bin: BTreeMap<String, String>, pub has_install_script: bool }`
  - `pub struct Sidecar { pub entries: BTreeMap<String, Entry> }` with `pub fn render(&self) -> String`
  - `pub enum Loaded { Absent, WrongVersion(u32), Present(Sidecar) }` and `pub fn load(path: &Path) -> Result<Loaded, VendorError>`
  - `pub struct Expected { pub url: String, pub sha512: String }`
  - `pub enum Difference { Missing(String), Extra(String), UrlChanged { key, was, now }, Sha512Changed { key, was, now }, UnsupportedVersion { found: u32 } }` implementing `Display`
  - `pub fn staleness(expected: &BTreeMap<String, Expected>, loaded: &Loaded) -> Vec<Difference>`

- [ ] **Step 1: Declare the module**

Add `pub mod sidecar;` to `src/lib.rs`.

- [ ] **Step 2: Write the failing tests**

Create `src/sidecar.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str) -> Entry {
        Entry {
            url: url.to_string(),
            sha512: "sha512-AAAA".to_string(),
            sha256: "ff00".to_string(),
            size: 42,
            bin: BTreeMap::new(),
            has_install_script: false,
        }
    }

    fn sidecar(pairs: &[(&str, Entry)]) -> Sidecar {
        Sidecar {
            entries: pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        }
    }

    #[test]
    fn rendering_is_stable_and_sorted_regardless_of_insertion_order() {
        let a = sidecar(&[("b@1.0.0", entry("u2")), ("a@1.0.0", entry("u1"))]);
        let b = sidecar(&[("a@1.0.0", entry("u1")), ("b@1.0.0", entry("u2"))]);
        assert_eq!(a.render(), b.render());
        let text = a.render();
        assert!(
            text.find("[\"a@1.0.0\"]").unwrap() < text.find("[\"b@1.0.0\"]").unwrap(),
            "entries must be sorted:\n{text}"
        );
    }

    #[test]
    fn the_generated_header_and_version_lead_the_file() {
        let text = sidecar(&[("a@1.0.0", entry("u"))]).render();
        assert!(
            text.starts_with("# @generated by pudu. Do not edit by hand.\nversion = 1\n"),
            "{text}"
        );
    }

    #[test]
    fn an_empty_bin_and_a_false_flag_are_omitted() {
        let text = sidecar(&[("a@1.0.0", entry("u"))]).render();
        assert!(!text.contains("bin ="), "{text}");
        assert!(!text.contains("has_install_script"), "{text}");
    }

    #[test]
    fn a_bin_map_and_a_true_flag_are_rendered_inline() {
        let mut e = entry("u");
        e.bin = BTreeMap::from([
            ("z".to_string(), "z.js".to_string()),
            ("a".to_string(), "a.js".to_string()),
        ]);
        e.has_install_script = true;
        let text = sidecar(&[("a@1.0.0", e)]).render();
        assert!(text.contains(r#"bin = { a = "a.js", z = "z.js" }"#), "{text}");
        assert!(text.contains("has_install_script = true"), "{text}");
    }

    #[test]
    fn render_then_load_round_trips() {
        let mut e = entry("https://example.com/a.tgz");
        e.bin = BTreeMap::from([("a".to_string(), "bin/a.js".to_string())]);
        e.has_install_script = true;
        let original = sidecar(&[("@scope/a@1.0.0", e), ("b@2.0.0", entry("u2"))]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pudu.lock");
        std::fs::write(&path, original.render()).unwrap();

        let Loaded::Present(parsed) = load(&path).unwrap() else {
            panic!("expected a parsed sidecar");
        };
        assert_eq!(parsed, original);
        assert_eq!(parsed.render(), original.render());
    }

    #[test]
    fn a_missing_file_loads_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            load(&dir.path().join("nope.lock")).unwrap(),
            Loaded::Absent
        ));
    }

    #[test]
    fn a_future_version_loads_as_wrong_version_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pudu.lock");
        std::fs::write(&path, "version = 7\n").unwrap();
        assert!(matches!(load(&path).unwrap(), Loaded::WrongVersion(7)));
    }

    #[test]
    fn unparseable_toml_is_an_error_naming_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pudu.lock");
        std::fs::write(&path, "this is not toml [[[").unwrap();
        let err = load(&path).unwrap_err();
        let VendorError::SidecarMalformed { path: p, .. } = &err else {
            panic!("wrong variant: {err:?}");
        };
        assert_eq!(p, &path);
    }

    #[test]
    fn an_entry_missing_a_required_field_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pudu.lock");
        std::fs::write(&path, "version = 1\n\n[\"a@1.0.0\"]\nurl = \"u\"\n").unwrap();
        assert!(matches!(
            load(&path).unwrap_err(),
            VendorError::SidecarMalformed { .. }
        ));
    }

    // --- staleness --------------------------------------------------------

    fn expected(pairs: &[(&str, &str, &str)]) -> BTreeMap<String, Expected> {
        pairs
            .iter()
            .map(|(k, url, sha)| {
                (
                    k.to_string(),
                    Expected {
                        url: url.to_string(),
                        sha512: sha.to_string(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn a_matching_sidecar_is_not_stale() {
        let e = expected(&[("a@1.0.0", "u1", "sha512-AAAA")]);
        let s = Loaded::Present(sidecar(&[("a@1.0.0", entry("u1"))]));
        assert_eq!(staleness(&e, &s), vec![]);
    }

    #[test]
    fn a_package_with_no_entry_is_missing() {
        let e = expected(&[("a@1.0.0", "u1", "sha512-AAAA")]);
        let s = Loaded::Present(Sidecar::default());
        assert_eq!(
            staleness(&e, &s),
            vec![Difference::Missing("a@1.0.0".to_string())]
        );
    }

    #[test]
    fn an_entry_for_a_package_no_longer_in_the_graph_is_extra() {
        let e = expected(&[]);
        let s = Loaded::Present(sidecar(&[("gone@1.0.0", entry("u"))]));
        assert_eq!(
            staleness(&e, &s),
            vec![Difference::Extra("gone@1.0.0".to_string())]
        );
    }

    #[test]
    fn a_changed_url_is_reported_with_both_values() {
        let e = expected(&[("a@1.0.0", "u2", "sha512-AAAA")]);
        let s = Loaded::Present(sidecar(&[("a@1.0.0", entry("u1"))]));
        assert_eq!(
            staleness(&e, &s),
            vec![Difference::UrlChanged {
                key: "a@1.0.0".to_string(),
                was: "u1".to_string(),
                now: "u2".to_string(),
            }]
        );
    }

    #[test]
    fn a_republished_version_is_caught_by_the_sha512() {
        let e = expected(&[("a@1.0.0", "u1", "sha512-BBBB")]);
        let s = Loaded::Present(sidecar(&[("a@1.0.0", entry("u1"))]));
        assert_eq!(
            staleness(&e, &s),
            vec![Difference::Sha512Changed {
                key: "a@1.0.0".to_string(),
                was: "sha512-AAAA".to_string(),
                now: "sha512-BBBB".to_string(),
            }]
        );
    }

    #[test]
    fn an_absent_sidecar_reports_every_package_as_missing() {
        let e = expected(&[("a@1.0.0", "u1", "s"), ("b@1.0.0", "u2", "s")]);
        assert_eq!(staleness(&e, &Loaded::Absent).len(), 2);
    }

    #[test]
    fn a_wrong_version_is_a_single_difference_not_a_flood() {
        let e = expected(&[("a@1.0.0", "u1", "s"), ("b@1.0.0", "u2", "s")]);
        assert_eq!(
            staleness(&e, &Loaded::WrongVersion(7)),
            vec![Difference::UnsupportedVersion { found: 7 }],
            "a sidecar pudu cannot read is one problem, not one per package"
        );
    }

    #[test]
    fn every_difference_renders_a_message_naming_its_subject() {
        let cases = [
            (Difference::Missing("a@1".into()), "a@1"),
            (Difference::Extra("b@2".into()), "b@2"),
            (
                Difference::UrlChanged {
                    key: "c@3".into(),
                    was: "old".into(),
                    now: "new".into(),
                },
                "c@3",
            ),
            (
                Difference::Sha512Changed {
                    key: "d@4".into(),
                    was: "old".into(),
                    now: "new".into(),
                },
                "d@4",
            ),
            (Difference::UnsupportedVersion { found: 7 }, "7"),
        ];
        for (d, needle) in cases {
            let s = d.to_string();
            assert!(s.contains(needle), "{s:?} must contain {needle:?}");
        }
    }
}
```

- [ ] **Step 3: Run them to confirm they fail**

Run: `cargo test --lib sidecar`
Expected: FAIL to compile — nothing in the module exists yet.

- [ ] **Step 4: Implement**

Prepend to `src/sidecar.rs`:

```rust
//! The `pudu.lock` sidecar: what `pudu vendor` records and `--check` compares.
//!
//! Keyed on `name@version` rather than the snapshot key, because a tarball has
//! no peer dependencies — one entry serves every peer instance of a package.
//!
//! Rendering is by hand. Byte-identical output across runs is an exit
//! criterion, and writing the format out makes it the spec rather than a
//! property of a serializer's table-inlining heuristics.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::Deserialize;

use crate::error::VendorError;

pub const SIDECAR_VERSION: u32 = 1;

const HEADER: &str = "# @generated by pudu. Do not edit by hand.\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub url: String,
    /// The lockfile's integrity string verbatim, `sha512-` prefix included.
    pub sha512: String,
    /// Lowercase hex — what `http_archive` expects.
    pub sha256: String,
    /// Compressed bytes.
    pub size: u64,
    pub bin: BTreeMap<String, String>,
    pub has_install_script: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sidecar {
    pub entries: BTreeMap<String, Entry>,
}

impl Sidecar {
    pub fn render(&self) -> String {
        let mut out = String::from(HEADER);
        out.push_str(&format!("version = {SIDECAR_VERSION}\n"));
        for (key, e) in &self.entries {
            out.push('\n');
            out.push_str(&format!("[{}]\n", quote(key)));
            out.push_str(&format!("url = {}\n", quote(&e.url)));
            out.push_str(&format!("sha512 = {}\n", quote(&e.sha512)));
            out.push_str(&format!("sha256 = {}\n", quote(&e.sha256)));
            out.push_str(&format!("size = {}\n", e.size));
            if !e.bin.is_empty() {
                let pairs: Vec<String> = e
                    .bin
                    .iter()
                    .map(|(n, p)| format!("{} = {}", quote(n), quote(p)))
                    .collect();
                out.push_str(&format!("bin = {{ {} }}\n", pairs.join(", ")));
            }
            if e.has_install_script {
                out.push_str("has_install_script = true\n");
            }
        }
        out
    }
}

/// A TOML basic string. Keys are quoted too: `@scope/name@1.0.0` is not a
/// bare key.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What reading `pudu.lock` found.
///
/// A version pudu cannot read is not an error: `--check` reports it as one
/// difference, and `vendor` rebuilds from scratch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    Absent,
    WrongVersion(u32),
    Present(Sidecar),
}

#[derive(Deserialize)]
struct VersionProbe {
    version: u32,
}

#[derive(Deserialize)]
struct RawSidecar {
    #[serde(rename = "version")]
    _version: u32,
    #[serde(flatten)]
    entries: BTreeMap<String, RawEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    url: String,
    sha512: String,
    sha256: String,
    size: u64,
    #[serde(default)]
    bin: BTreeMap<String, String>,
    #[serde(default)]
    has_install_script: bool,
}

pub fn load(path: &Path) -> Result<Loaded, VendorError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Loaded::Absent),
        Err(e) => {
            return Err(VendorError::SidecarMalformed {
                path: path.to_path_buf(),
                reason: e.to_string(),
            });
        }
    };

    let malformed = |e: toml::de::Error| VendorError::SidecarMalformed {
        path: path.to_path_buf(),
        reason: e.to_string(),
    };

    // The version is read first and on its own, so a future format whose
    // entries do not fit `RawEntry` reports "wrong version" rather than a
    // parse error blaming the user for a file pudu generated.
    let probe: VersionProbe = toml::from_str(&text).map_err(malformed)?;
    if probe.version != SIDECAR_VERSION {
        return Ok(Loaded::WrongVersion(probe.version));
    }

    let raw: RawSidecar = toml::from_str(&text).map_err(malformed)?;
    Ok(Loaded::Present(Sidecar {
        entries: raw
            .entries
            .into_iter()
            .map(|(k, e)| {
                (
                    k,
                    Entry {
                        url: e.url,
                        sha512: e.sha512,
                        sha256: e.sha256,
                        size: e.size,
                        bin: e.bin,
                        has_install_script: e.has_install_script,
                    },
                )
            })
            .collect(),
    }))
}

/// What `vendor` expects a sidecar entry to say, computed offline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expected {
    pub url: String,
    pub sha512: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Difference {
    Missing(String),
    Extra(String),
    UrlChanged { key: String, was: String, now: String },
    Sha512Changed { key: String, was: String, now: String },
    UnsupportedVersion { found: u32 },
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Difference::Missing(key) => write!(f, "pudu.lock has no entry for `{key}`"),
            Difference::Extra(key) => {
                write!(f, "pudu.lock has an entry for `{key}`, which is no longer needed")
            }
            Difference::UrlChanged { key, was, now } => {
                write!(f, "`{key}`: url is `{was}`, expected `{now}`")
            }
            Difference::Sha512Changed { key, was, now } => {
                write!(f, "`{key}`: sha512 is `{was}`, expected `{now}`")
            }
            Difference::UnsupportedVersion { found } => write!(
                f,
                "pudu.lock says version {found}; this pudu writes version {SIDECAR_VERSION}"
            ),
        }
    }
}

/// Compare what the lockfile and config imply against what is on disk.
///
/// Offline by construction: nothing here reads a tarball. That is the whole
/// point of `--check` as a CI gate.
pub fn staleness(expected: &BTreeMap<String, Expected>, loaded: &Loaded) -> Vec<Difference> {
    let empty = BTreeMap::new();
    let found = match loaded {
        // One problem, not one per package.
        Loaded::WrongVersion(found) => {
            return vec![Difference::UnsupportedVersion { found: *found }];
        }
        Loaded::Absent => &empty,
        Loaded::Present(s) => &s.entries,
    };

    let mut out = Vec::new();
    for (key, want) in expected {
        match found.get(key) {
            None => out.push(Difference::Missing(key.clone())),
            Some(have) => {
                if have.url != want.url {
                    out.push(Difference::UrlChanged {
                        key: key.clone(),
                        was: have.url.clone(),
                        now: want.url.clone(),
                    });
                }
                if have.sha512 != want.sha512 {
                    out.push(Difference::Sha512Changed {
                        key: key.clone(),
                        was: have.sha512.clone(),
                        now: want.sha512.clone(),
                    });
                }
            }
        }
    }
    for key in found.keys() {
        if !expected.contains_key(key) {
            out.push(Difference::Extra(key.clone()));
        }
    }
    out
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --lib sidecar`
Expected: PASS, 17 tests.

- [ ] **Step 6: Verify the tests can fail**

Apply each mutation, confirm the named test reddens, revert:

1. Change `render`'s loop to `for (key, e) in self.entries.iter().rev()` → `rendering_is_stable_and_sorted_regardless_of_insertion_order` FAILS on the ordering assertion.
2. Always render `has_install_script` → `an_empty_bin_and_a_false_flag_are_omitted` FAILS.
3. Delete the `probe.version != SIDECAR_VERSION` early return → `a_future_version_loads_as_wrong_version_not_an_error` FAILS.
4. Delete the `Extra` loop → `an_entry_for_a_package_no_longer_in_the_graph_is_extra` FAILS.
5. Drop the `sha512` comparison → `a_republished_version_is_caught_by_the_sha512` FAILS.
6. Make `WrongVersion` fall through to the per-package diff → `a_wrong_version_is_a_single_difference_not_a_flood` FAILS.
7. Remove `#[serde(deny_unknown_fields)]` from `RawEntry` and delete the `size` field → `an_entry_missing_a_required_field_is_an_error` FAILS.

- [ ] **Step 7: Commit**

```bash
git add src/sidecar.rs src/lib.rs
git commit -m "feat(vendor): render, load, and diff the pudu.lock sidecar"
```

---

### Task 6: `cache.rs` — the integrity-addressed store

Content-addressed by the integrity the lockfile already records, so a lookup needs no network to compute its key. That is what makes `--no-network` against a warm cache work at all.

**Files:**
- Create: `src/cache.rs`
- Modify: `src/lib.rs`, `src/error.rs`

**Interfaces:**
- Consumes: `tarball::{decode_integrity, sha512_digest, hex}` (Task 3); `VendorError::CacheUnavailable` (Task 1).
- Produces: `pub struct Cache` with `open()`, `with_root(PathBuf)`, `path_for(&self, key, integrity)`, `get(&self, key, integrity) -> Option<Vec<u8>>`, `put(&self, key, integrity, bytes) -> Result<(), VendorError>`; and `VendorError::CacheWriteFailed`.

- [ ] **Step 1: Add the error variant**

In `src/error.rs`, add to `VendorError`:

```rust
    #[error("cannot write to the cache at {path}")]
    #[diagnostic(
        code(pudu::vendor::cache_write_failed),
        help("set PUDU_CACHE_DIR to a writable directory")
    )]
    CacheWriteFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
```

and classify it as an I/O failure rather than bad input, in `VendorError::exit_code`:

```rust
            VendorError::HttpStatus { .. }
            | VendorError::Transport { .. }
            | VendorError::CacheWriteFailed { .. } => ExitCode::Internal,
```

- [ ] **Step 2: Declare the module**

Add `pub mod cache;` to `src/lib.rs`.

- [ ] **Step 3: Write the failing tests**

Create `src/cache.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tarball::{hex, sha512_digest};

    fn integrity_of(bytes: &[u8]) -> String {
        use base64::Engine as _;
        format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(sha512_digest(bytes))
        )
    }

    #[test]
    fn the_path_is_derived_from_the_digest_not_the_url() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::with_root(dir.path().to_path_buf());
        let bytes = b"hello";
        let h = hex(&sha512_digest(bytes));
        let p = c.path_for("p@1.0.0", &integrity_of(bytes)).unwrap();
        assert_eq!(
            p,
            dir.path()
                .join("tarballs")
                .join("sha512")
                .join(&h[..2])
                .join(format!("{h}.tgz"))
        );
    }

    #[test]
    fn a_malformed_integrity_has_no_path() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::with_root(dir.path().to_path_buf());
        assert!(c.path_for("p@1.0.0", "sha1-abc").is_err());
    }

    #[test]
    fn put_then_get_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::with_root(dir.path().to_path_buf());
        let bytes = b"some tarball bytes".to_vec();
        let i = integrity_of(&bytes);
        c.put("p@1.0.0", &i, &bytes).unwrap();
        assert_eq!(c.get("p@1.0.0", &i), Some(bytes));
    }

    #[test]
    fn a_miss_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::with_root(dir.path().to_path_buf());
        assert_eq!(c.get("p@1.0.0", &integrity_of(b"never stored")), None);
    }

    #[test]
    fn corrupt_cached_bytes_read_as_a_miss() {
        // A truncated or tampered cache entry must not be trusted just
        // because it sits at the right path — `--no-network` depends on this.
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::with_root(dir.path().to_path_buf());
        let bytes = b"good bytes".to_vec();
        let i = integrity_of(&bytes);
        c.put("p@1.0.0", &i, &bytes).unwrap();

        let path = c.path_for("p@1.0.0", &i).unwrap();
        std::fs::write(&path, b"tampered").unwrap();

        assert_eq!(c.get("p@1.0.0", &i), None);
    }

    #[test]
    fn put_overwrites_an_existing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::with_root(dir.path().to_path_buf());
        let bytes = b"bytes".to_vec();
        let i = integrity_of(&bytes);
        c.put("p@1.0.0", &i, &bytes).unwrap();
        c.put("p@1.0.0", &i, &bytes).unwrap();
        assert_eq!(c.get("p@1.0.0", &i), Some(bytes));
    }

    #[test]
    fn pudu_cache_dir_overrides_the_os_cache_directory() {
        // `open()` reads the environment, which is process-global, so this
        // test sets and restores it rather than running in parallel with a
        // second environment-reading test. There is only one.
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("PUDU_CACHE_DIR");
        unsafe { std::env::set_var("PUDU_CACHE_DIR", dir.path()) };
        let c = Cache::open().unwrap();
        assert_eq!(c.root(), dir.path());
        match previous {
            Some(v) => unsafe { std::env::set_var("PUDU_CACHE_DIR", v) },
            None => unsafe { std::env::remove_var("PUDU_CACHE_DIR") },
        }
    }
}
```

`std::env::set_var` is `unsafe` in edition 2024; the `unsafe` blocks above are required, not optional.

- [ ] **Step 4: Run them to confirm they fail**

Run: `cargo test --lib cache`
Expected: FAIL to compile — `Cache` does not exist.

- [ ] **Step 5: Implement**

Prepend to `src/cache.rs`:

```rust
//! The tarball cache: `~/.cache/pudu/tarballs/sha512/<ab>/<hex>.tgz`.
//!
//! Addressed by the integrity `pnpm-lock.yaml` already records, so the path
//! of a wanted tarball is computable with no network at all. `--no-network`
//! against a warm cache depends on exactly that.
//!
//! Cached bytes are re-verified on read. The hash is cheap next to the I/O,
//! and it is what makes a cache hit as trustworthy as a fresh download.

use std::path::{Path, PathBuf};

use crate::error::VendorError;
use crate::tarball::{decode_integrity, hex, sha512_digest};

pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// `PUDU_CACHE_DIR`, else the OS cache directory plus `pudu`.
    ///
    /// `PUDU_CACHE_DIR` exists so the integration tests are hermetic; it is
    /// deliberately not advertised in `--help`.
    pub fn open() -> Result<Self, VendorError> {
        let root = match std::env::var_os("PUDU_CACHE_DIR") {
            Some(v) => PathBuf::from(v),
            None => dirs::cache_dir()
                .ok_or(VendorError::CacheUnavailable)?
                .join("pudu"),
        };
        Ok(Self { root })
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_for(&self, key: &str, integrity: &str) -> Result<PathBuf, VendorError> {
        let digest = hex(&decode_integrity(key, integrity)?);
        Ok(self
            .root
            .join("tarballs")
            .join("sha512")
            .join(&digest[..2])
            .join(format!("{digest}.tgz")))
    }

    /// The cached bytes, if they are present *and* still hash correctly.
    ///
    /// A corrupt entry reads as a miss rather than an error: the right
    /// response is to fetch it again, and under `--no-network` the caller
    /// already reports the miss precisely.
    pub fn get(&self, key: &str, integrity: &str) -> Option<Vec<u8>> {
        let path = self.path_for(key, integrity).ok()?;
        let bytes = std::fs::read(path).ok()?;
        let expected = decode_integrity(key, integrity).ok()?;
        (sha512_digest(&bytes) == expected).then_some(bytes)
    }

    /// Write via a temporary file in the same directory, then rename, so a
    /// killed run never leaves a truncated entry a later run would trust.
    pub fn put(&self, key: &str, integrity: &str, bytes: &[u8]) -> Result<(), VendorError> {
        let path = self.path_for(key, integrity)?;
        let dir = path.parent().unwrap_or(&self.root);
        let failed = |source: std::io::Error| VendorError::CacheWriteFailed {
            path: path.clone(),
            source,
        };
        std::fs::create_dir_all(dir).map_err(failed)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(failed)?;
        std::io::Write::write_all(&mut tmp, bytes).map_err(failed)?;
        tmp.persist(&path)
            .map_err(|e| failed(std::io::Error::from(e)))?;
        Ok(())
    }
}
```

- [ ] **Step 6: Run the tests**

Run: `cargo test --lib cache`
Expected: PASS, 7 tests.

- [ ] **Step 7: Verify the tests can fail**

Apply each mutation, confirm the named test reddens, revert:

1. Make `get` skip the digest comparison (`Some(bytes)`) → `corrupt_cached_bytes_read_as_a_miss` FAILS.
2. Change `path_for` to key on `key` instead of the digest → `the_path_is_derived_from_the_digest_not_the_url` FAILS.
3. Make `open()` ignore `PUDU_CACHE_DIR` → `pudu_cache_dir_overrides_the_os_cache_directory` FAILS.

- [ ] **Step 8: Commit**

```bash
git add src/cache.rs src/lib.rs src/error.rs
git commit -m "feat(vendor): add the integrity-addressed tarball cache"
```

---

### Task 7: `fetch.rs` — the network layer

The only module in S3 that opens a socket. Each worker runs the whole per-package pipeline and discards the bytes before taking the next one, so peak memory is bounded by `--jobs` rather than by the size of the dependency graph.

**Files:**
- Create: `src/fetch.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `Cache` (Task 6); `tarball::{verify_and_inspect, Verified}` (Tasks 3–4); `VendorError::{NetworkDisabled, HttpStatus, Transport}` (Task 1).
- Produces:
  - `pub struct Request { pub key: String, pub name: String, pub url: String, pub integrity: String }`
  - `pub type Outcome = Result<(Verified, Vec<VendorWarning>), VendorError>;`
  - `pub struct Stats { pub downloaded: usize, pub cached: usize }`
  - `pub struct Fetcher` with `pub fn new(jobs: usize, no_network: bool, verbose: bool, cache: Cache) -> Self` and `pub fn run(&self, requests: Vec<Request>) -> (BTreeMap<String, Outcome>, Stats)`

- [ ] **Step 1: Declare the module**

Add `pub mod fetch;` to `src/lib.rs`.

- [ ] **Step 2: Write the failing tests**

Create `src/fetch.rs` with only this test module. `httpmock` is a dev-dependency, so it is available to unit tests inside `src/`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    use httpmock::prelude::*;

    fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
        let mut ar = tar::Builder::new(Vec::new());
        for (path, body) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            ar.append_data(&mut h, format!("package/{path}"), body.as_bytes())
                .unwrap();
        }
        let bytes = ar.into_inner().unwrap();
        let mut gz =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&bytes).unwrap();
        gz.finish().unwrap()
    }

    fn integrity_of(bytes: &[u8]) -> String {
        use base64::Engine as _;
        format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD
                .encode(crate::tarball::sha512_digest(bytes))
        )
    }

    fn request(url: String, integrity: String) -> Request {
        Request {
            key: "p@1.0.0".to_string(),
            name: "p".to_string(),
            url,
            integrity,
        }
    }

    fn body() -> Vec<u8> {
        tarball(&[("package.json", r#"{"name":"p","bin":"cli.js"}"#)])
    }

    #[test]
    fn retryable_covers_transient_statuses_only() {
        for s in [408, 429, 500, 502, 503, 504] {
            assert!(retryable(s), "{s} should be retried");
        }
        for s in [400, 401, 403, 404, 200, 301] {
            assert!(!retryable(s), "{s} must not be retried");
        }
    }

    #[test]
    fn a_successful_fetch_verifies_inspects_and_caches() {
        let server = MockServer::start();
        let bytes = body();
        let m = server.mock(|when, then| {
            when.method(GET).path("/p.tgz");
            then.status(200).body(bytes.clone());
        });

        let dir = tempfile::tempdir().unwrap();
        let f = Fetcher::new(2, false, false, Cache::with_root(dir.path().to_path_buf()));
        let i = integrity_of(&bytes);
        let (out, stats) = f.run(vec![request(server.url("/p.tgz"), i.clone())]);

        let (verified, _) = out["p@1.0.0"].as_ref().unwrap();
        assert_eq!(verified.size, bytes.len() as u64);
        assert_eq!(verified.inspection.bin["p"], "cli.js");
        assert_eq!(stats.downloaded, 1);
        assert_eq!(stats.cached, 0);
        m.assert();

        let warm = Cache::with_root(dir.path().to_path_buf());
        assert_eq!(warm.get("p@1.0.0", &i), Some(bytes));
    }

    #[test]
    fn a_warm_cache_is_used_without_a_request() {
        let bytes = body();
        let i = integrity_of(&bytes);
        let dir = tempfile::tempdir().unwrap();
        Cache::with_root(dir.path().to_path_buf())
            .put("p@1.0.0", &i, &bytes)
            .unwrap();

        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET).path("/p.tgz");
            then.status(500);
        });

        let f = Fetcher::new(2, false, false, Cache::with_root(dir.path().to_path_buf()));
        let (out, stats) = f.run(vec![request(server.url("/p.tgz"), i)]);

        assert!(out["p@1.0.0"].is_ok(), "{:?}", out["p@1.0.0"]);
        assert_eq!(stats.cached, 1);
        assert_eq!(stats.downloaded, 0);
        m.assert_hits(0);
    }

    #[test]
    fn no_network_with_a_cold_cache_names_the_package_and_url() {
        let dir = tempfile::tempdir().unwrap();
        let f = Fetcher::new(1, true, false, Cache::with_root(dir.path().to_path_buf()));
        let bytes = body();
        let (out, _) = f.run(vec![request(
            "https://registry.example/p.tgz".to_string(),
            integrity_of(&bytes),
        )]);

        let err = out["p@1.0.0"].as_ref().unwrap_err();
        let VendorError::NetworkDisabled { key, url } = err else {
            panic!("wrong variant: {err:?}");
        };
        assert_eq!(key, "p@1.0.0");
        assert_eq!(url, "https://registry.example/p.tgz");
    }

    #[test]
    fn no_network_with_a_warm_cache_succeeds() {
        let bytes = body();
        let i = integrity_of(&bytes);
        let dir = tempfile::tempdir().unwrap();
        Cache::with_root(dir.path().to_path_buf())
            .put("p@1.0.0", &i, &bytes)
            .unwrap();

        let f = Fetcher::new(1, true, false, Cache::with_root(dir.path().to_path_buf()));
        let (out, _) = f.run(vec![request(
            "https://registry.example/p.tgz".to_string(),
            i,
        )]);
        assert!(out["p@1.0.0"].is_ok());
    }

    #[test]
    fn a_503_is_retried_three_times_before_giving_up() {
        let server = MockServer::start();
        let bytes = body();
        let fail = server.mock(|when, then| {
            when.method(GET).path("/p.tgz");
            then.status(503);
        });
        let dir = tempfile::tempdir().unwrap();
        let f = Fetcher::new(1, false, false, Cache::with_root(dir.path().to_path_buf()));
        let (out, _) = f.run(vec![request(server.url("/p.tgz"), integrity_of(&bytes))]);

        assert!(out["p@1.0.0"].is_err());
        assert_eq!(
            fail.hits(),
            4,
            "one attempt plus three retries, so a flaky registry is survivable"
        );
    }

    #[test]
    fn a_404_is_not_retried() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET).path("/gone.tgz");
            then.status(404);
        });
        let dir = tempfile::tempdir().unwrap();
        let f = Fetcher::new(1, false, false, Cache::with_root(dir.path().to_path_buf()));
        let (out, _) = f.run(vec![request(
            server.url("/gone.tgz"),
            integrity_of(&body()),
        )]);

        let err = out["p@1.0.0"].as_ref().unwrap_err();
        assert!(matches!(err, VendorError::HttpStatus { status: 404, .. }), "{err:?}");
        assert_eq!(m.hits(), 1, "a 4xx is the registry's final answer");
    }

    #[test]
    fn served_bytes_that_fail_the_integrity_are_rejected_and_not_cached() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/p.tgz");
            then.status(200).body(body());
        });

        let dir = tempfile::tempdir().unwrap();
        let wrong = integrity_of(b"entirely different bytes");
        let f = Fetcher::new(1, false, false, Cache::with_root(dir.path().to_path_buf()));
        let (out, _) = f.run(vec![request(server.url("/p.tgz"), wrong.clone())]);

        let err = out["p@1.0.0"].as_ref().unwrap_err();
        assert!(matches!(err, VendorError::IntegrityMismatch { .. }), "{err:?}");
        assert_eq!(
            Cache::with_root(dir.path().to_path_buf()).get("p@1.0.0", &wrong),
            None,
            "bytes that failed verification must not be readable as a cache hit"
        );
    }

    #[test]
    fn results_are_keyed_and_ordered_independently_of_completion() {
        let server = MockServer::start();
        let bytes = body();
        // No path matcher: every GET this test makes should be served the
        // same body, and the assertion is about result ordering, not routing.
        server.mock(|when, then| {
            when.method(GET);
            then.status(200).body(bytes.clone());
        });

        let i = integrity_of(&bytes);
        let requests: Vec<Request> = ["c@1", "a@1", "b@1"]
            .iter()
            .map(|k| Request {
                key: k.to_string(),
                name: "p".to_string(),
                url: server.url(format!("/{k}.tgz")),
                integrity: i.clone(),
            })
            .collect();

        let dir = tempfile::tempdir().unwrap();
        let f = Fetcher::new(3, false, false, Cache::with_root(dir.path().to_path_buf()));
        let (out, _) = f.run(requests);
        assert_eq!(
            out.keys().collect::<Vec<_>>(),
            vec!["a@1", "b@1", "c@1"],
            "a BTreeMap is what makes determinism hold under parallelism"
        );
    }
}
```

- [ ] **Step 3: Run them to confirm they fail**

Run: `cargo test --lib fetch`
Expected: FAIL to compile — `Fetcher` does not exist.

- [ ] **Step 4: Implement**

Prepend to `src/fetch.rs`:

```rust
//! The network layer: the only module in S3 that opens a socket.
//!
//! Each worker runs the whole per-package pipeline — fetch, cache, verify,
//! inspect — and drops the bytes before taking the next package, so peak
//! memory is bounded by `--jobs` rather than by the dependency graph.
//!
//! Results land in a `BTreeMap`, so output order is a property of the keys
//! rather than of which thread finished first. Determinism under parallelism
//! is by construction here, not by luck.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::cache::Cache;
use crate::error::{VendorError, VendorWarning};
use crate::tarball::{Verified, verify_and_inspect};

/// A hostile or misconfigured registry must not be able to exhaust memory.
/// The largest package on the public registry is far below this.
const MAX_TARBALL: u64 = 256 * 1024 * 1024;

const BACKOFF_MS: [u64; 3] = [250, 500, 1000];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub key: String,
    /// The package name alone. The string-`bin` rule names the command after
    /// the package, and `key` carries a version too.
    pub name: String,
    pub url: String,
    pub integrity: String,
}

pub type Outcome = Result<(Verified, Vec<VendorWarning>), VendorError>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub downloaded: usize,
    pub cached: usize,
}

/// Whether a status is worth retrying. A 4xx is the registry's final answer
/// and is never retried; 408 and 429 are the two that are not.
fn retryable(status: u16) -> bool {
    status == 408 || status == 429 || (500..600).contains(&status)
}

pub struct Fetcher {
    agent: ureq::Agent,
    jobs: usize,
    no_network: bool,
    verbose: bool,
    cache: Cache,
}

impl Fetcher {
    pub fn new(jobs: usize, no_network: bool, verbose: bool, cache: Cache) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(120)))
            .user_agent(concat!("pudu/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            agent,
            jobs: jobs.clamp(1, 64),
            no_network,
            verbose,
            cache,
        }
    }

    pub fn run(&self, requests: Vec<Request>) -> (BTreeMap<String, Outcome>, Stats) {
        let queue = Mutex::new(requests);
        let out: Mutex<BTreeMap<String, Outcome>> = Mutex::new(BTreeMap::new());
        let downloaded = AtomicUsize::new(0);
        let cached = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            for _ in 0..self.jobs {
                scope.spawn(|| {
                    loop {
                        let Some(req) = queue.lock().unwrap().pop() else {
                            break;
                        };
                        let result = self.one(&req, &downloaded, &cached);
                        out.lock().unwrap().insert(req.key.clone(), result);
                    }
                });
            }
        });

        (
            out.into_inner().unwrap(),
            Stats {
                downloaded: downloaded.load(Ordering::Relaxed),
                cached: cached.load(Ordering::Relaxed),
            },
        )
    }

    fn one(&self, req: &Request, downloaded: &AtomicUsize, cached: &AtomicUsize) -> Outcome {
        let bytes = match self.cache.get(&req.key, &req.integrity) {
            Some(bytes) => {
                cached.fetch_add(1, Ordering::Relaxed);
                bytes
            }
            None => {
                if self.no_network {
                    return Err(VendorError::NetworkDisabled {
                        key: req.key.clone(),
                        url: req.url.clone(),
                    });
                }
                if self.verbose {
                    eprintln!("  downloading {}", req.key);
                }
                let bytes = self.download(req)?;
                downloaded.fetch_add(1, Ordering::Relaxed);
                // Cached before verification would mean a poisoned entry a
                // later run reads back; `Cache::get` re-hashes, so a bad
                // entry would read as a miss anyway, but not writing it at
                // all is clearer and cheaper.
                let verified = verify_and_inspect(
                    &req.key,
                    &req.name,
                    &req.url,
                    &bytes,
                    &req.integrity,
                )?;
                self.cache.put(&req.key, &req.integrity, &bytes)?;
                return Ok(verified);
            }
        };

        verify_and_inspect(&req.key, &req.name, &req.url, &bytes, &req.integrity)
    }

    fn download(&self, req: &Request) -> Result<Vec<u8>, VendorError> {
        let mut attempt = 0usize;
        loop {
            let err = match self.agent.get(&req.url).call() {
                Ok(mut resp) => match resp
                    .body_mut()
                    .with_config()
                    .limit(MAX_TARBALL)
                    .read_to_vec()
                {
                    Ok(bytes) => return Ok(bytes),
                    Err(source) => VendorError::Transport {
                        key: req.key.clone(),
                        url: req.url.clone(),
                        source,
                    },
                },
                Err(ureq::Error::StatusCode(status)) => {
                    let e = VendorError::HttpStatus {
                        key: req.key.clone(),
                        url: req.url.clone(),
                        status,
                    };
                    if !retryable(status) {
                        return Err(e);
                    }
                    e
                }
                Err(source) => VendorError::Transport {
                    key: req.key.clone(),
                    url: req.url.clone(),
                    source,
                },
            };

            if attempt >= BACKOFF_MS.len() {
                return Err(err);
            }
            std::thread::sleep(Duration::from_millis(BACKOFF_MS[attempt]));
            attempt += 1;
        }
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --lib fetch`
Expected: PASS, 9 tests. The retry test sleeps ~1.75s; that is the only slow one.

- [ ] **Step 6: Verify the tests can fail**

Apply each mutation, confirm the named test reddens, revert:

1. Make `retryable` return `true` for every status → `a_404_is_not_retried` FAILS on the hit count.
2. Change `BACKOFF_MS` to a one-element array → `a_503_is_retried_three_times_before_giving_up` FAILS on the hit count.
3. Move the `self.cache.put` call before `verify_and_inspect` → `served_bytes_that_fail_the_integrity_are_rejected_and_not_cached` FAILS.
4. Make `one` ignore the cache and always download → `a_warm_cache_is_used_without_a_request` FAILS.
5. Change `out` to a `HashMap` and collect into a `Vec` in insertion order → `results_are_keyed_and_ordered_independently_of_completion` FAILS.
6. Drop the `no_network` check → `no_network_with_a_cold_cache_names_the_package_and_url` FAILS.

- [ ] **Step 7: Commit**

```bash
git add src/fetch.rs src/lib.rs Cargo.toml Cargo.lock
git commit -m "feat(vendor): fetch tarballs in parallel with retries and a cache"
```

---

### Task 8: `cli/context.rs` and `cli/vendor.rs` — orchestration

The largest task: it is where the pruned graph, the registry, the cache, the fetcher, and the sidecar meet.

`debug.rs` owns a private `load()` that skips `Config::validate`. `vendor` cannot reuse it — an invalid registry URL has to be rejected *before* pudu fetches from it. Both variants move into `context.rs`, with `debug`'s behaviour unchanged.

**Files:**
- Create: `src/cli/context.rs`, `src/cli/vendor.rs`
- Modify: `src/cli/mod.rs`, `src/cli/debug.rs`
- Modify: `tests/snapshots/help__help_output_is_stable.snap` (via `cargo insta accept`)

**Interfaces:**
- Consumes: `registry::tarball_url`; `sidecar::{load, staleness, Loaded, Entry, Sidecar, Expected}`; `fetch::{Fetcher, Request}`; `cache::Cache`; `platform::prune::prune`; `lock::Graph`; `lock::types::Resolution`.
- Produces: `pub fn run(check: bool, jobs: usize, no_network: bool, verbose: bool) -> anyhow::Result<()>` in `cli::vendor`; `pub fn load_lenient()` / `pub fn load_validated()` in `cli::context`.

- [ ] **Step 1: Extract the loaders into `context.rs`**

Create `src/cli/context.rs` by moving `debug.rs`'s `load()` verbatim — including its comments, which explain the not-found/unreadable distinction and are worth keeping — and adding a validating sibling:

```rust
//! Loading the two files every command starts from.
//!
//! Two variants deliberately. `pudu debug` predates config validation and
//! reads whatever parses, so a developer can inspect a half-finished config;
//! `pudu vendor` fetches over the network from `[registry]`, so an invalid
//! registry URL has to be rejected before it is used.

use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::error::{CliError, ConfigError, render};
use crate::lock::parse_lockfile;
use crate::lock::types::Lockfile;

fn read_config() -> Result<Config> {
    let config_path = Path::new("pudu.toml");
    let config_text =
        std::fs::read_to_string(config_path).map_err(|source| CliError::ConfigUnreadable {
            path: config_path.to_path_buf(),
            source,
        })?;
    Ok(Config::from_str(&config_text, config_path)?)
}

fn read_lockfile(config: &Config) -> Result<Lockfile> {
    let base = std::env::current_dir()?;
    let lockfile_path = base.join(&config.lockfile_path);
    // Distinguish "not found" from "found but unreadable" (e.g. permissions):
    // the latter is not a missing-file problem, and telling the user to edit
    // `lockfile_path` when the path is already correct is actively wrong
    // advice.
    let lock_text = std::fs::read_to_string(&lockfile_path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            ConfigError::LockfileNotFound {
                path: lockfile_path.clone(),
            }
        } else {
            ConfigError::LockfileUnreadable {
                path: lockfile_path.clone(),
                source,
            }
        }
    })?;

    let (lockfile, warnings) = parse_lockfile(&lock_text, &lockfile_path)?;
    for w in &warnings {
        eprint!("{}", render(w));
    }
    Ok(lockfile)
}

/// Load without validating `pudu.toml`. Used by `pudu debug`.
pub fn load_lenient() -> Result<(Config, Lockfile)> {
    let config = read_config()?;
    let lockfile = read_lockfile(&config)?;
    Ok((config, lockfile))
}

/// Load and validate. Validation errors are printed here, so the returned
/// `CliError::ConfigInvalid` is `already_reported` and `main` does not repeat
/// them.
pub fn load_validated() -> Result<(Config, Lockfile)> {
    let config = read_config()?;

    let base = std::env::current_dir()?;
    let (errors, warnings) = config.validate(&base);
    for w in &warnings {
        eprint!("{}", render(w));
    }
    if !errors.is_empty() {
        for e in &errors {
            eprint!("{}", render(e));
        }
        return Err(CliError::ConfigInvalid {
            count: errors.len(),
        }
        .into());
    }

    let lockfile = read_lockfile(&config)?;
    Ok((config, lockfile))
}
```

- [ ] **Step 2: Point `debug.rs` at it**

Delete `load()` and its now-unused imports from `src/cli/debug.rs`, add `use crate::cli::context::load_lenient;`, and change both `let (…) = load()?;` call sites to `load_lenient()?`. Add `pub mod context;` and `pub mod vendor;` to `src/cli/mod.rs`.

- [ ] **Step 3: Confirm nothing regressed**

Run: `cargo test --test debug_print_graph --test debug_platforms`
Expected: PASS, unchanged.

- [ ] **Step 4: Write `vendor.rs`**

```rust
//! `pudu vendor` — the download pass and the `pudu.lock` sidecar.
//!
//! Vendors the union of packages surviving S2's pruning on at least one
//! configured platform. That makes `pudu.lock` a function of `pudu.toml` as
//! well as of the lockfile: adding a platform makes the sidecar stale, and
//! `--check` catches it. Intended, not incidental — a config change genuinely
//! changes which tarballs the build needs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};

use crate::cache::Cache;
use crate::cli::context::load_validated;
use crate::error::{VendorError, VendorWarning, render};
use crate::fetch::{Fetcher, Request};
use crate::lock::Graph;
use crate::lock::types::Resolution;
use crate::platform::prune::prune;
use crate::registry::tarball_url;
use crate::sidecar::{self, Entry, Expected, Loaded, Sidecar};

/// Everything the download pass needs, computed with no network at all.
struct Plan {
    expected: BTreeMap<String, Expected>,
    requests: BTreeMap<String, Request>,
    /// `hasBin` as the lockfile records it, for the cross-check.
    has_bin: BTreeMap<String, bool>,
}

pub fn run(check: bool, jobs: usize, no_network: bool, verbose: bool) -> Result<()> {
    let (config, lockfile) = load_validated()?;
    if config.platforms.is_empty() {
        return Err(VendorError::NoPlatformsConfigured.into());
    }

    let graph = Graph::build(&lockfile)?;
    let (matrix, warnings) = prune(&graph, &config.platforms);
    for w in &warnings {
        eprint!("{}", render(w));
    }

    let plan = build_plan(&graph, &matrix, &config)?;

    let base = std::env::current_dir()?;
    let sidecar_path = base.join(&config.third_party_dir).join("pudu.lock");
    let loaded = sidecar::load(&sidecar_path)?;

    if check {
        return run_check(&plan, &loaded);
    }
    run_vendor(plan, loaded, &sidecar_path, jobs, no_network, verbose)
}

/// Resolve every surviving package to a URL and an integrity, with no
/// network access. `--check` needs exactly this and nothing more.
fn build_plan(
    graph: &Graph,
    matrix: &crate::platform::prune::Matrix,
    config: &crate::config::Config,
) -> Result<Plan> {
    let mut plan = Plan {
        expected: BTreeMap::new(),
        requests: BTreeMap::new(),
        has_bin: BTreeMap::new(),
    };
    // Sorted, so the reported list is stable run to run.
    let mut unsupported: BTreeSet<String> = BTreeSet::new();

    for snapshot_key in matrix.platforms_by_node.keys() {
        let node = &graph.nodes[snapshot_key];
        let key = format!("{}@{}", node.name, node.version);
        // Peer instances of one package share a tarball, so the first
        // snapshot key to reach a given name@version settles it.
        if plan.expected.contains_key(&key) {
            continue;
        }

        let (url, integrity) = match &node.meta.resolution {
            Resolution::Integrity { integrity } => (
                tarball_url(&node.name, &node.version, &config.registry)?.to_string(),
                integrity.clone(),
            ),
            // The private-registry shape: an absolute URL that pnpm recorded,
            // with a hash to check it against. Fetched verbatim.
            Resolution::Tarball {
                tarball,
                integrity: Some(i),
            } => (tarball.clone(), i.clone()),
            // The `github:` shape: no hash exists for these bytes anywhere.
            Resolution::Tarball { .. } => {
                unsupported.insert(format!("{key} (url, no integrity)"));
                continue;
            }
            Resolution::Git { .. } => {
                unsupported.insert(format!("{key} (git)"));
                continue;
            }
            Resolution::Directory { .. } => {
                unsupported.insert(format!("{key} (directory)"));
                continue;
            }
        };

        plan.expected.insert(
            key.clone(),
            Expected {
                url: url.clone(),
                sha512: integrity.clone(),
            },
        );
        plan.has_bin.insert(key.clone(), node.meta.has_bin);
        plan.requests.insert(
            key.clone(),
            Request {
                key,
                name: node.name.clone(),
                url,
                integrity,
            },
        );
    }

    // Reported together rather than raced, so a repo with four git
    // dependencies learns about all four in one run.
    if !unsupported.is_empty() {
        return Err(VendorError::UnsupportedResolution {
            packages: unsupported.into_iter().collect(),
        }
        .into());
    }
    Ok(plan)
}

fn run_check(plan: &Plan, loaded: &Loaded) -> Result<()> {
    let differences = sidecar::staleness(&plan.expected, loaded);
    if differences.is_empty() {
        eprintln!("pudu.lock is up to date ({} packages)", plan.expected.len());
        return Ok(());
    }
    // The error carries the count; the detail goes out here so the user sees
    // every difference, not just how many there were.
    for d in &differences {
        eprintln!("  {d}");
    }
    Err(VendorError::Stale {
        differences: differences.iter().map(ToString::to_string).collect(),
    }
    .into())
}

fn run_vendor(
    plan: Plan,
    loaded: Loaded,
    sidecar_path: &Path,
    jobs: usize,
    no_network: bool,
    verbose: bool,
) -> Result<()> {
    let existing = match &loaded {
        Loaded::Present(s) => s.entries.clone(),
        Loaded::Absent | Loaded::WrongVersion(_) => BTreeMap::new(),
    };

    // Carry over anything already recorded at the same URL and hash. A
    // one-package version bump costs one download. The trade is explicit: a
    // recorded sha256 is never re-checked against upstream once written,
    // which is also what makes pudu.lock an audit artifact.
    let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
    let mut todo: Vec<Request> = Vec::new();
    let mut unchanged = 0usize;
    for (key, req) in plan.requests {
        match existing.get(&key) {
            Some(e) if e.url == req.url && e.sha512 == req.integrity => {
                entries.insert(key, e.clone());
                unchanged += 1;
            }
            _ => todo.push(req),
        }
    }

    let fetcher = Fetcher::new(jobs, no_network, verbose, Cache::open()?);
    let (results, stats) = fetcher.run(todo);

    let mut failures: Vec<VendorError> = Vec::new();
    for (key, outcome) in results {
        match outcome {
            Ok((verified, warnings)) => {
                for w in &warnings {
                    eprint!("{}", render(w));
                }
                let lockfile_says = plan.has_bin[&key];
                let found = verified.inspection.bin.len();
                // A cross-check, never a source of truth: `hasBin` is a flag
                // pnpm derives from registry metadata, and the archive is what
                // the build will actually consume.
                if lockfile_says != (found > 0) {
                    eprint!(
                        "{}",
                        render(&VendorWarning::HasBinDisagreement {
                            key: key.clone(),
                            lockfile: lockfile_says,
                            found,
                        })
                    );
                }
                let want = &plan.expected[&key];
                entries.insert(
                    key,
                    Entry {
                        url: want.url.clone(),
                        sha512: want.sha512.clone(),
                        sha256: verified.sha256,
                        size: verified.size,
                        bin: verified.inspection.bin,
                        has_install_script: verified.inspection.has_install_script,
                    },
                );
            }
            Err(e) => failures.push(e),
        }
    }

    if !failures.is_empty() {
        // `main` renders the returned error, so printing it here too would
        // say the same thing twice. Everything after the first is printed
        // here, under a line that says so.
        if failures.len() > 1 {
            eprintln!(
                "{} packages failed; the first is reported at the end, the rest follow:",
                failures.len()
            );
            for e in failures.iter().skip(1) {
                eprint!("{}", render(e));
            }
        }
        return Err(failures.swap_remove(0).into());
    }

    let sidecar = Sidecar { entries };
    write_atomic(sidecar_path, &sidecar.render())?;
    eprintln!(
        "vendored {} packages ({} downloaded, {} cached, {} unchanged)",
        sidecar.entries.len(),
        stats.downloaded,
        stats.cached,
        unchanged
    );
    Ok(())
}

/// Write via a temporary file and rename, so an interrupted run leaves the
/// previous sidecar intact rather than a half-written one.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("creating a temporary file in {}", dir.display()))?;
    std::io::Write::write_all(&mut tmp, text.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    tmp.persist(path)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}
```

- [ ] **Step 5: Wire up the CLI**

In `src/cli/mod.rs`, change the `Vendor` variant and its dispatch, and correct the now-stale `--no-network` help:

```rust
    /// Fetch tarballs and write pudu.lock.
    Vendor {
        /// Exit non-zero if pudu.lock is stale.
        #[arg(long)]
        check: bool,
        /// Maximum parallel downloads.
        #[arg(long, value_name = "N", default_value_t = 8)]
        jobs: usize,
    },
```

```rust
    /// Forbid all network access.
    #[arg(long, global = true)]
    pub no_network: bool,
```

```rust
            Commands::Vendor { check, jobs } => {
                vendor::run(check, jobs, self.no_network, self.verbose > 0)
            }
```

`self.no_network` and `self.verbose` are read before `self.command` is moved into the `match`, so bind them first:

```rust
        let no_network = self.no_network;
        let verbose = self.verbose > 0;
        match self.command {
```

- [ ] **Step 6: Update the help snapshots**

The verb list and the `--no-network` description both changed.

Run: `cargo insta test --accept --test help` (or `cargo test --test help` then `cargo insta accept`)
Then **read the snapshot diff** and confirm it shows only the `[UNIMPLEMENTED — S3]` marker leaving `vendor` and the `--no-network` wording — nothing else.

- [ ] **Step 7: Confirm it builds and the suite passes**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/cli/ tests/snapshots/
git commit -m "feat(vendor): wire up pudu vendor and --check

Shared loading moves to cli/context.rs in two variants: debug keeps the
lenient one it has always used, vendor gets a validating one because it
fetches over the network from [registry] and an invalid URL must be rejected
before it is used."
```

---

### Task 9: `tests/vendor.rs` — end-to-end coverage against a mock registry

Every test here builds its own lockfile so the served bytes and the recorded integrity always agree, and sets `PUDU_CACHE_DIR` to a temp directory so nothing touches the developer's real cache.

**Files:**
- Create: `tests/vendor.rs`

**Interfaces:**
- Consumes: the `pudu` binary via `assert_cmd`; `common::pudu`.

- [ ] **Step 1: Write the tests**

```rust
//! `pudu vendor` end to end, against a mock registry.
//!
//! Each test builds a lockfile from the bytes it is about to serve, so the
//! recorded integrity and the served tarball cannot drift apart.

mod common;

use std::io::Write as _;
use std::path::Path;

use httpmock::prelude::*;

fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
    let mut ar = tar::Builder::new(Vec::new());
    for (path, body) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        ar.append_data(&mut h, format!("package/{path}"), body.as_bytes())
            .unwrap();
    }
    let bytes = ar.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&bytes).unwrap();
    gz.finish().unwrap()
}

/// `\x20` in the lockfile literals below is a literal space: a `\` at the end
/// of a Rust string line strips the newline *and* the next line's leading
/// whitespace, and YAML indentation is load-bearing.
fn integrity_of(bytes: &[u8]) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha512};
    let mut h = Sha512::new();
    h.update(bytes);
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(h.finalize())
    )
}

struct Fixture {
    dir: tempfile::TempDir,
    cache: tempfile::TempDir,
    server: MockServer,
}

impl Fixture {
    /// A project whose lockfile depends on `left-pad@1.3.0` and `tool@2.0.0`,
    /// both served by a mock registry.
    fn new() -> Self {
        Self::with_scopes(&[])
    }

    fn with_scopes(scopes: &[(&str, &str)]) -> Self {
        let server = MockServer::start();
        let plain = tarball(&[("package.json", r#"{"name":"left-pad"}"#)]);
        let tool = tarball(&[
            ("package.json", r#"{"name":"tool","bin":"cli.js","scripts":{"install":"node x"}}"#),
            ("cli.js", "#!/usr/bin/env node\n"),
        ]);

        server.mock(|when, then| {
            when.method(GET).path("/left-pad/-/left-pad-1.3.0.tgz");
            then.status(200).body(plain.clone());
        });
        server.mock(|when, then| {
            when.method(GET).path("/tool/-/tool-2.0.0.tgz");
            then.status(200).body(tool.clone());
        });

        let dir = tempfile::tempdir().unwrap();
        let mut config = format!(
            "lockfile_path = \"pnpm-lock.yaml\"\n\
             third_party_dir = \"third-party/js\"\n\n\
             [platforms.linux-x64-gnu]\n\
             os = \"linux\"\ncpu = \"x64\"\nlibc = \"glibc\"\n\n\
             [registry]\ndefault = \"{}\"\n",
            server.base_url()
        );
        for (scope, url) in scopes {
            config.push_str(&format!("\"{scope}\" = \"{url}\"\n"));
        }
        std::fs::write(dir.path().join("pudu.toml"), config).unwrap();

        let lock = format!(
            "lockfileVersion: '9.0'\n\n\
             importers:\n\n  .:\n    dependencies:\n\
             \x20     left-pad:\n        specifier: 1.3.0\n        version: 1.3.0\n\
             \x20     tool:\n        specifier: 2.0.0\n        version: 2.0.0\n\n\
             packages:\n\n\
             \x20 left-pad@1.3.0:\n    resolution: {{integrity: {}}}\n\n\
             \x20 tool@2.0.0:\n    resolution: {{integrity: {}}}\n    hasBin: true\n\n\
             snapshots:\n\n  left-pad@1.3.0: {{}}\n\n  tool@2.0.0: {{}}\n",
            integrity_of(&plain),
            integrity_of(&tool),
        );
        std::fs::write(dir.path().join("pnpm-lock.yaml"), lock).unwrap();

        Fixture {
            dir,
            cache: tempfile::tempdir().unwrap(),
            server,
        }
    }

    fn cmd(&self) -> assert_cmd::Command {
        let mut c = common::pudu(self.dir.path());
        c.env("PUDU_CACHE_DIR", self.cache.path());
        c
    }

    fn sidecar_path(&self) -> std::path::PathBuf {
        self.dir.path().join("third-party/js/pudu.lock")
    }

    fn sidecar(&self) -> String {
        std::fs::read_to_string(self.sidecar_path()).unwrap()
    }
}

#[test]
fn vendor_writes_a_sidecar_covering_every_package() {
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();

    let text = f.sidecar();
    assert!(text.starts_with("# @generated by pudu. Do not edit by hand.\nversion = 1\n"), "{text}");
    assert!(text.contains(r#"["left-pad@1.3.0"]"#), "{text}");
    assert!(text.contains(r#"["tool@2.0.0"]"#), "{text}");
    assert!(text.contains(r#"bin = { tool = "cli.js" }"#), "{text}");
    assert!(text.contains("has_install_script = true"), "{text}");
    assert_eq!(
        text.matches("sha256 = ").count(),
        2,
        "every entry records a sha256:\n{text}"
    );
}

#[test]
fn a_second_run_reproduces_the_sidecar_byte_for_byte() {
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();
    let first = f.sidecar();
    f.cmd().arg("vendor").assert().success();
    assert_eq!(first, f.sidecar());
}

#[test]
fn a_second_run_downloads_nothing() {
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();
    let out = f.cmd().arg("vendor").output().unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("0 downloaded") && stderr.contains("2 unchanged"),
        "the second run must carry both entries over:\n{stderr}"
    );
}

#[test]
fn check_exits_zero_when_current() {
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();
    f.cmd().args(["vendor", "--check"]).assert().success();
}

#[test]
fn check_exits_five_and_names_the_package_when_stale() {
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();

    // Drop one entry, keeping the file otherwise valid.
    let text = f.sidecar();
    let cut = text.find(r#"["tool@2.0.0"]"#).unwrap();
    std::fs::write(f.sidecar_path(), &text[..cut]).unwrap();

    let out = f.cmd().args(["vendor", "--check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(5), "stale must exit 5, not 1 or 3");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("tool@2.0.0"), "{stderr}");
}

#[test]
fn check_exits_five_when_the_sidecar_is_absent() {
    let f = Fixture::new();
    let out = f.cmd().args(["vendor", "--check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(5));
}

#[test]
fn check_needs_neither_the_network_nor_a_warm_cache() {
    // The strongest available proof that `--check` opens no socket: run it
    // against a *cold* cache with `--no-network`. Anything that fetched
    // would have to fail.
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();

    let cold = tempfile::tempdir().unwrap();
    common::pudu(f.dir.path())
        .env("PUDU_CACHE_DIR", cold.path())
        .args(["vendor", "--check", "--no-network"])
        .assert()
        .success();
}

#[test]
fn no_network_succeeds_against_a_warm_cache() {
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();
    std::fs::remove_file(f.sidecar_path()).unwrap();
    f.cmd().args(["vendor", "--no-network"]).assert().success();
}

#[test]
fn no_network_against_a_cold_cache_names_the_missing_package() {
    let f = Fixture::new();
    let out = f.cmd().args(["vendor", "--no-network"]).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("left-pad@1.3.0") || stderr.contains("tool@2.0.0"), "{stderr}");
    assert!(stderr.contains("--no-network"), "{stderr}");
}

#[test]
fn a_scope_override_is_used_and_recorded() {
    let scoped = MockServer::start();
    let bytes = tarball(&[("package.json", r#"{"name":"@myorg/thing"}"#)]);
    scoped.mock(|when, then| {
        when.method(GET).path("/@myorg/thing/-/thing-1.0.0.tgz");
        then.status(200).body(bytes.clone());
    });

    let f = Fixture::with_scopes(&[("@myorg", &scoped.base_url())]);
    let lock = format!(
        "lockfileVersion: '9.0'\n\nimporters:\n\n  .:\n    dependencies:\n\
         \x20     '@myorg/thing':\n        specifier: 1.0.0\n        version: 1.0.0\n\n\
         packages:\n\n  '@myorg/thing@1.0.0':\n    resolution: {{integrity: {}}}\n\n\
         snapshots:\n\n  '@myorg/thing@1.0.0': {{}}\n",
        integrity_of(&bytes)
    );
    std::fs::write(f.dir.path().join("pnpm-lock.yaml"), lock).unwrap();

    f.cmd().arg("vendor").assert().success();
    let text = f.sidecar();
    assert!(
        text.contains(&format!("{}/@myorg/thing/-/thing-1.0.0.tgz", scoped.base_url())),
        "the resolved URL must be recorded, not re-derived later:\n{text}"
    );
}

#[test]
fn a_tarball_whose_bytes_fail_the_integrity_aborts_naming_the_package() {
    let server = MockServer::start();
    let served = tarball(&[("package.json", r#"{"name":"left-pad"}"#)]);
    let other = tarball(&[("package.json", r#"{"name":"left-pad","version":"9"}"#)]);
    server.mock(|when, then| {
        when.method(GET).path("/left-pad/-/left-pad-1.3.0.tgz");
        then.status(200).body(served);
    });

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("pudu.toml"),
        format!(
            "lockfile_path = \"pnpm-lock.yaml\"\n\n\
             [platforms.linux-x64-gnu]\nos = \"linux\"\ncpu = \"x64\"\nlibc = \"glibc\"\n\n\
             [registry]\ndefault = \"{}\"\n",
            server.base_url()
        ),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("pnpm-lock.yaml"),
        format!(
            "lockfileVersion: '9.0'\n\nimporters:\n\n  .:\n    dependencies:\n\
             \x20     left-pad:\n        specifier: 1.3.0\n        version: 1.3.0\n\n\
             packages:\n\n  left-pad@1.3.0:\n    resolution: {{integrity: {}}}\n\n\
             snapshots:\n\n  left-pad@1.3.0: {{}}\n",
            // The integrity of bytes the server will not serve.
            integrity_of(&other)
        ),
    )
    .unwrap();

    let cache = tempfile::tempdir().unwrap();
    let out = common::pudu(dir.path())
        .env("PUDU_CACHE_DIR", cache.path())
        .arg("vendor")
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(3));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("left-pad@1.3.0"), "{stderr}");
    assert!(stderr.contains("integrity"), "{stderr}");
    assert!(
        !Path::new(&dir.path().join("third-party/js/pudu.lock")).exists(),
        "a failed run must not write a sidecar"
    );
}

#[test]
fn a_git_resolution_is_refused_by_name() {
    let f = Fixture::new();
    std::fs::write(
        f.dir.path().join("pnpm-lock.yaml"),
        "lockfileVersion: '9.0'\n\nimporters:\n\n  .:\n    dependencies:\n\
         \x20     thing:\n        specifier: github:o/r\n        version: 1.0.0\n\n\
         packages:\n\n  thing@1.0.0:\n    resolution: {tarball: https://codeload.example/x.tgz}\n\n\
         snapshots:\n\n  thing@1.0.0: {}\n",
    )
    .unwrap();

    let out = f.cmd().arg("vendor").output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("thing@1.0.0"), "{stderr}");
}

#[test]
fn a_config_with_no_platforms_refuses_rather_than_writing_an_empty_sidecar() {
    let f = Fixture::new();
    std::fs::write(
        f.dir.path().join("pudu.toml"),
        format!(
            "lockfile_path = \"pnpm-lock.yaml\"\n\n[registry]\ndefault = \"{}\"\n",
            f.server.base_url()
        ),
    )
    .unwrap();

    let out = f.cmd().arg("vendor").output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(!f.sidecar_path().exists());
}

#[test]
fn stdout_stays_empty_and_diagnostics_go_to_stderr() {
    let f = Fixture::new();
    let out = f.cmd().arg("vendor").output().unwrap();
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "vendor produces a file, not a stream: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!out.stderr.is_empty(), "the summary must reach stderr");
}
```

`tar`, `flate2`, `base64`, and `sha2` are normal dependencies, so integration tests can use them directly. `httpmock` is a dev-dependency and is likewise available here.

- [ ] **Step 2: Run them**

Run: `cargo test --test vendor`
Expected: PASS, 14 tests.

- [ ] **Step 3: Verify the tests can fail**

Apply each mutation, confirm the named test reddens, revert:

1. In `run_check`, return `Ok(())` unconditionally → `check_exits_five_and_names_the_package_when_stale` FAILS.
2. Change `VendorError::Stale`'s exit code to `InputInvalid` → the same test FAILS on the code, proving 5 is asserted and not just "non-zero".
3. In `run_vendor`, write the sidecar before checking `failures` → `a_tarball_whose_bytes_fail_the_integrity_aborts_naming_the_package` FAILS.
4. Delete the `config.platforms.is_empty()` guard → `a_config_with_no_platforms_refuses_rather_than_writing_an_empty_sidecar` FAILS.
5. In `build_plan`, use `config.registry.default` instead of `tarball_url` → `a_scope_override_is_used_and_recorded` FAILS.
6. Change the carry-over condition to `existing.contains_key(&key)` → nothing should break here, but note it: `a_second_run_downloads_nothing` still passes because URL and hash are unchanged. This is the case the oracle job (Task 10) and `check_exits_five_and_names_the_package_when_stale` cover instead.
7. Make `run_check` build a `Fetcher` and run it before diffing → `check_needs_neither_the_network_nor_a_warm_cache` FAILS, because a cold cache under `--no-network` cannot serve it.

- [ ] **Step 4: Commit**

```bash
git add tests/vendor.rs
git commit -m "test(vendor): cover the vendor pass end to end against a mock registry"
```

---

### Task 10: the live-registry oracle and its CI job

The registry's own manifests are an independent oracle for three of the four fields `pudu.lock` records. The expectation is computed **in JavaScript**, by a small port of pnpm's rules, so the test compares pudu against a genuinely separate implementation rather than against itself — the same shape as S2's `reference.mjs`.

**Files:**
- Create: `tests/fixtures/lock/real/oracle/capture-manifests.mjs`, `tests/fixtures/lock/real/oracle/manifests.json`
- Create: `tests/vendor_oracle.rs`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: the `pudu` binary; `tests/fixtures/lock/real/pnpm-lock.yaml`.

- [ ] **Step 1: Write the capture script**

Create `tests/fixtures/lock/real/oracle/capture-manifests.mjs`:

```js
#!/usr/bin/env node
// Capture the registry's own view of every package in the real fixture:
// the tarball URL, the bin map, and whether an install script runs.
//
// The expectations are computed here, in JavaScript, by a port of
// @pnpm/package-bins and @pnpm/building.pkg-requires-build. That makes the
// oracle an independent implementation rather than a recording of pudu's
// output. See docs/superpowers/research/2026-08-31-npm-tarball-vendor-survey.md.
//
// Two limits, both deliberate and both currently unreached by this fixture:
//   * `directories.bin` cannot be resolved from a manifest — it needs the
//     archive. Such packages are recorded with bin: null and skipped by the
//     test.
//   * The `.hooks/` install-script trigger is invisible here for the same
//     reason. `gypfile` is npm's own marker for a root binding.gyp and covers
//     the other file-list trigger.
//
// Usage: node capture-manifests.mjs > manifests.json

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const lock = fs.readFileSync(path.join(here, '..', 'pnpm-lock.yaml'), 'utf8');

const keys = [];
let inPackages = false;
for (const line of lock.split('\n')) {
  if (line === 'packages:') { inPackages = true; continue; }
  if (line === 'snapshots:') { inPackages = false; continue; }
  if (!inPackages) continue;
  const m = line.match(/^ {2}'?([^ ']+)'?:$/);
  if (m) keys.push(m[1]);
}

function commandName(raw) {
  return raw[0] === '@' ? raw.slice(raw.indexOf('/') + 1) : raw;
}

function normalize(rel) {
  const out = [];
  for (const part of rel.split('/')) {
    if (part === '' || part === '.') continue;
    if (part === '..') { if (out.pop() === undefined) return null; continue; }
    out.push(part);
  }
  return out.length ? out.join('/') : null;
}

function bins(manifest) {
  if (manifest.bin === undefined || manifest.bin === null) {
    return manifest.directories?.bin ? null : {};
  }
  const pairs = typeof manifest.bin === 'string'
    ? [[manifest.name, manifest.bin]]
    : (typeof manifest.bin === 'object' ? Object.entries(manifest.bin) : []);
  const out = {};
  for (const [rawName, rawPath] of pairs) {
    if (typeof rawPath !== 'string') continue;
    const name = commandName(rawName);
    if (name !== encodeURIComponent(name) && name !== '$') continue;
    const p = normalize(rawPath);
    if (p === null) continue;
    out[name] = p;
  }
  return out;
}

const out = [];
let next = 0;
async function worker() {
  while (next < keys.length) {
    const key = keys[next++];
    const at = key.lastIndexOf('@');
    const name = key.slice(0, at);
    const version = key.slice(at + 1);
    const res = await fetch(
      `https://registry.npmjs.org/${name.replace('/', '%2f')}/${version}`
    );
    if (!res.ok) throw new Error(`${key}: HTTP ${res.status}`);
    const m = await res.json();
    const s = m.scripts ?? {};
    out.push({
      key,
      url: m.dist.tarball,
      bin: bins(m),
      has_install_script: Boolean(s.preinstall || s.install || s.postinstall || m.gypfile),
    });
  }
}
await Promise.all(Array.from({ length: 12 }, worker));
out.sort((a, b) => (a.key < b.key ? -1 : 1));
process.stdout.write(JSON.stringify(out, null, 1) + '\n');
```

- [ ] **Step 2: Capture the oracle**

```bash
cd tests/fixtures/lock/real/oracle
node capture-manifests.mjs > manifests.json
```

Confirm it holds 400 entries and that `esbuild@0.25.12`, `esbuild@0.28.2`, and `fsevents@2.3.3` are the only three with `has_install_script: true`:

```bash
node -e 'const m=require("./manifests.json");console.log(m.length, m.filter(o=>o.has_install_script).map(o=>o.key))'
```

Expected: `400 [ 'esbuild@0.25.12', 'esbuild@0.28.2', 'fsevents@2.3.3' ]`

- [ ] **Step 3: Write the oracle test**

Create `tests/vendor_oracle.rs`:

```rust
//! Differential test against the live npm registry.
//!
//! `#[ignore]`d: it downloads a few hundred tarballs. CI runs it in its own
//! job (`vendor-oracle`); locally, `cargo test --test vendor_oracle --
//! --ignored`.
//!
//! The expectation comes from `oracle/manifests.json`, computed by
//! `capture-manifests.mjs` from the registry's own manifests using a
//! JavaScript port of pnpm's rules. Where the tarball and the manifest
//! disagree, the tarball wins and the disagreement is the finding — the
//! oracle is a cross-check, not the truth.

mod common;

use std::collections::{BTreeMap, BTreeSet};

#[derive(serde::Deserialize)]
struct OracleEntry {
    key: String,
    url: String,
    /// `null` where the manifest cannot answer — a `directories.bin`
    /// package. None exist in this fixture; the field exists so one would be
    /// skipped rather than silently mis-asserted.
    bin: Option<BTreeMap<String, String>>,
    has_install_script: bool,
}

#[derive(serde::Deserialize)]
struct SidecarEntry {
    url: String,
    #[serde(default)]
    bin: BTreeMap<String, String>,
    #[serde(default)]
    has_install_script: bool,
}

#[derive(serde::Deserialize)]
struct Sidecar {
    #[allow(dead_code)]
    version: u32,
    #[serde(flatten)]
    entries: BTreeMap<String, SidecarEntry>,
}

const CONFIG: &str = r#"
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

#[test]
#[ignore = "downloads several hundred tarballs from registry.npmjs.org"]
fn the_vendor_pass_agrees_with_the_registry_on_every_package() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lock/real");
    let oracle: Vec<OracleEntry> =
        serde_json::from_str(&std::fs::read_to_string(fixture.join("oracle/manifests.json")).unwrap())
            .unwrap();
    let oracle: BTreeMap<String, OracleEntry> =
        oracle.into_iter().map(|e| (e.key.clone(), e)).collect();

    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        fixture.join("pnpm-lock.yaml"),
        dir.path().join("pnpm-lock.yaml"),
    )
    .unwrap();
    std::fs::write(dir.path().join("pudu.toml"), CONFIG).unwrap();
    let cache = tempfile::tempdir().unwrap();

    let out = common::pudu(dir.path())
        .env("PUDU_CACHE_DIR", cache.path())
        .arg("vendor")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "vendor failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let text = std::fs::read_to_string(dir.path().join("third-party/js/pudu.lock")).unwrap();
    let sidecar: Sidecar = toml::from_str(&text).unwrap();

    assert!(
        !sidecar.entries.is_empty(),
        "the vendor pass recorded nothing"
    );

    let mut mismatches: Vec<String> = Vec::new();
    for (key, got) in &sidecar.entries {
        let Some(want) = oracle.get(key) else {
            mismatches.push(format!("{key}: vendored but absent from the oracle"));
            continue;
        };
        if got.url != want.url {
            mismatches.push(format!("{key}: url {} != {}", got.url, want.url));
        }
        if let Some(want_bin) = &want.bin
            && &got.bin != want_bin
        {
            mismatches.push(format!("{key}: bin {:?} != {:?}", got.bin, want_bin));
        }
        if got.has_install_script != want.has_install_script {
            mismatches.push(format!(
                "{key}: has_install_script {} != {}",
                got.has_install_script, want.has_install_script
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} disagreement(s) with the registry:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );

    // The vendored set must be the *union* across platforms, not one
    // platform's view and not everything in the lockfile. Three named
    // packages pin all three directions:
    let vendored: BTreeSet<&String> = sidecar.entries.keys().collect();
    assert!(
        vendored.contains(&"fsevents@2.3.3".to_string()),
        "fsevents is darwin-only; vendoring it proves the set is a union, not one platform's view"
    );
    assert!(
        vendored.contains(&"@esbuild/linux-x64@0.25.12".to_string()),
        "the linux platform's own optional dep must be in the union too"
    );
    assert!(
        !vendored.contains(&"@esbuild/win32-x64@0.25.12".to_string()),
        "a win32-only package survives no configured platform and must not be downloaded"
    );

    assert_eq!(
        sidecar.entries.get("fsevents@2.3.3").map(|e| e.has_install_script),
        Some(true),
        "fsevents drives has_install_script from binding.gyp as well as scripts.install"
    );
    assert_eq!(
        sidecar.entries.get("@babel/parser@7.29.8").map(|e| e.bin.clone()),
        Some(BTreeMap::from([(
            "parser".to_string(),
            "bin/babel-parser.js".to_string()
        )])),
        "a string bin on a scoped package is named after the package minus its scope"
    );
}
```

- [ ] **Step 4: Run it**

Run: `cargo test --test vendor_oracle -- --ignored --nocapture`
Expected: PASS. It downloads a few hundred tarballs, so allow a few minutes on a cold cache.

If any package disagrees, **do not adjust the assertion to make it pass.** Read the disagreement: the tarball is the truth, so either pudu has a bug or the oracle's JavaScript port does. Record which in the commit message.

- [ ] **Step 5: Add the CI job**

Append to `.github/workflows/ci.yml`:

```yaml
  vendor-oracle:
    name: vendor oracle (differential vs the npm registry)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: cargo test (ignored live-registry oracle)
        run: cargo test --test vendor_oracle -- --ignored --nocapture
```

- [ ] **Step 6: Verify the oracle test can fail**

Break `command_name` in `src/tarball.rs` so it returns its argument unchanged, re-run the oracle test, and confirm it FAILS naming `@babel/parser@7.29.8`. Revert.

- [ ] **Step 7: Commit**

```bash
git add tests/fixtures/lock/real/oracle/ tests/vendor_oracle.rs .github/workflows/ci.yml
git commit -m "test(vendor): check the vendor pass against the live npm registry

The expectation is computed in JavaScript by a port of pnpm's bin and
install-script rules, so the comparison is against an independent
implementation rather than a recording of pudu's own output."
```

---

## Final verification

After Task 10, run every gate:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo +1.88 check --all-targets
cargo test --test vendor_oracle -- --ignored
```

Then confirm the spec's exit criteria by hand against the built binary, in a scratch directory using the real fixture:

1. `pudu vendor` twice → the two `pudu.lock` files are byte-identical (`sha256sum` both).
2. A tampered integrity in the lockfile → an error naming the package, the URL, and both hashes.
3. `pudu vendor --check` → exit 0 when current, exit 5 when stale, no socket either way.
4. `--no-network` → succeeds warm, errors cold naming the package and URL.
5. A scoped override → resolves to the right host and the URL is recorded.
6. `fsevents@2.3.3` → `has_install_script = true` with a darwin platform configured.
7. `@babel/parser@7.29.8` → `bin = { parser = "bin/babel-parser.js" }`.

Update `docs/superpowers/TECH_DEBT.md` with anything found, and the roadmap's specs index with the S3 row, before opening the PR.
