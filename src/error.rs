//! Error and warning types for pudu, plus the one place diagnostics are
//! rendered.
//!
//! Three layers, deliberately:
//!
//! * **Typed, per-module `thiserror` enums** ([`ConfigError`], [`DeriveError`])
//!   for library-internal failures, so tests assert on variants rather than
//!   message text (spec §6).
//! * **Typed warning enums** ([`ConfigWarning`], [`DeriveWarning`],
//!   [`InitWarning`]) alongside
//!   them, for the same reason — a warning is a diagnostic too, and
//!   `Vec<String>` forced its tests to grep prose.
//! * **[`anyhow`] at the CLI boundary**, with [`CliError`] carrying the cases
//!   whose exit code is not "something unexpected happened".
//!
//! Every message names a field or a file by path, and carries line/column
//! where the source position is known.
//!
//! All of these live in one module so `Display` conventions stay uniform and
//! [`render`] is the single implementation of "what a diagnostic looks like".

use std::fmt::Write as _;
use std::io::IsTerminal as _;
use std::path::PathBuf;

use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, ConfigError>;

// --- Exit codes -----------------------------------------------------------

/// Process exit codes (spec §6.1). CI scripts branch on these, so they are
/// part of the CLI contract.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    /// Success.
    Ok = 0,
    /// Internal or unexpected error — I/O failure, anything unclassified.
    Internal = 1,
    /// Usage error. Also clap's own code for a bad command line.
    Usage = 2,
    /// Input invalid: `pudu.toml` failed validation, or a file pudu reads
    /// (`pudu.toml`, `pnpm-lock.yaml`, a fixup file) is missing or malformed.
    InputInvalid = 3,
    /// The verb is registered but not implemented yet.
    Unimplemented = 4,
    /// `pudu vendor --check` found `packages.toml` out of date. Distinct from
    /// `InputInvalid` so CI can tell "regenerate the package table" from
    /// "your config is wrong" without parsing the message.
    Stale = 5,
}

impl ExitCode {
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

// --- CLI-level errors -----------------------------------------------------

/// CLI failures whose exit code is not [`ExitCode::Internal`].
///
/// Everything else reaching `main` is an `anyhow::Error` and exits 1; these
/// are the cases a CI script wants to tell apart.
#[derive(Debug, Error, Diagnostic)]
pub enum CliError {
    #[error("pudu {verb} is not implemented yet (planned for {stage})")]
    #[diagnostic(
        code(pudu::unimplemented),
        help("follow it at https://github.com/rsJames-ttrpg/pudu")
    )]
    Unimplemented { verb: String, stage: String },

    #[error("{path} already exists; pass --force to overwrite")]
    #[diagnostic(code(pudu::init::config_exists))]
    ConfigExists { path: PathBuf },

    #[error("cannot read {path}")]
    #[diagnostic(
        code(pudu::config::unreadable),
        help("run `pudu init` to create one, or pass -C <path>")
    )]
    ConfigUnreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{count} error(s) in pudu.toml")]
    #[diagnostic(code(pudu::config::invalid))]
    ConfigInvalid { count: usize },

    #[error("cannot change directory to {path}")]
    #[diagnostic(
        code(pudu::usage::bad_directory),
        help("`-C` takes a path to an existing directory")
    )]
    BadDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl CliError {
    /// Whether the subcommand already printed the detail, so `main` must not
    /// render this error a second time. `config check` prints one diagnostic
    /// per problem (or the JSON envelope); the summary that follows would be
    /// a third telling of the same news.
    pub fn already_reported(&self) -> bool {
        matches!(self, CliError::ConfigInvalid { .. })
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            CliError::Unimplemented { .. } => ExitCode::Unimplemented,
            CliError::ConfigExists { .. } | CliError::BadDirectory { .. } => ExitCode::Usage,
            CliError::ConfigUnreadable { .. } | CliError::ConfigInvalid { .. } => {
                ExitCode::InputInvalid
            }
        }
    }
}

// --- Config errors --------------------------------------------------------

#[derive(Debug, Error, Diagnostic)]
pub enum ConfigError {
    #[error("failed to parse {path}")]
    #[diagnostic(code(pudu::config::parse))]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

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

    #[error("registry `{key}` is `{url}`, which is not an http(s) URL")]
    #[diagnostic(
        code(pudu::config::bad_registry_url),
        help("registries are fetched over the network; use an absolute http:// or https:// URL")
    )]
    BadRegistryUrl { key: String, url: String },

    #[error("`[fixups].registry` value `{value}` has no absolute path after `file://`")]
    #[diagnostic(
        code(pudu::config::bad_fixup_registry_path),
        help("use `file:///absolute/path/to/registry`")
    )]
    BadFixupRegistryPath { value: String },

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

    #[error("cannot read lockfile {path}")]
    #[diagnostic(
        code(pudu::config::lockfile_unreadable),
        help("the path is correct but the file could not be read; check its permissions")
    )]
    LockfileUnreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

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
}

/// Non-fatal findings from [`crate::config::Config::validate`].
///
/// Typed rather than `Vec<String>` so tests assert on variants, and so S1–S4
/// can add warnings without anyone grepping prose.
#[derive(Debug, Clone, PartialEq, Eq, Error, Diagnostic)]
pub enum ConfigWarning {
    #[error(
        "only one platform is configured (`{name}`); generated rules will not vary by platform"
    )]
    #[diagnostic(
        severity(Warning),
        code(pudu::config::single_platform),
        help("add another `[platforms.<name>]` table if that was not intentional")
    )]
    SinglePlatform { name: String },

    #[error("`[fixups].registry` is set but `registry_rev` is not pinned")]
    #[diagnostic(
        severity(Warning),
        code(pudu::config::unpinned_fixup_registry),
        help("set `[fixups].registry_rev` so fixups resolve reproducibly")
    )]
    UnpinnedFixupRegistry,
}

// --- Lockfile errors -------------------------------------------------------

/// Lockfile parse and graph-construction failures.
///
/// A malformed lockfile is an *input* error, like a malformed `pudu.toml` —
/// both are files the user hands pudu, so both exit 3.
#[derive(Debug, Error, Diagnostic)]
pub enum LockError {
    #[error(
        "unsupported lockfileVersion: {} (pudu supports 9.0)",
        .found.as_deref().unwrap_or("absent")
    )]
    #[diagnostic(
        code(pudu::lock::unsupported_version),
        help(
            "pudu supports lockfileVersion 9.0. Run `pnpm install` with pnpm 9 or newer to upgrade this lockfile."
        )
    )]
    UnsupportedVersion { found: Option<String> },

    #[error("could not parse {path}")]
    #[diagnostic(code(pudu::lock::yaml))]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_norway::Error,
    },

    #[error("invalid snapshot key `{key}` at byte {offset}: {reason}")]
    #[diagnostic(code(pudu::lock::key_parse))]
    KeyParse {
        key: String,
        offset: usize,
        reason: String,
    },

    #[error("snapshot `{snapshot}` has no entry under `packages:` for `{base}`")]
    #[diagnostic(
        code(pudu::lock::missing_package_meta),
        help("The lockfile is inconsistent. Re-run `pnpm install` to regenerate it.")
    )]
    MissingPackageMeta { snapshot: String, base: String },

    #[error(
        "`{from}` depends on `{link_name}`, which resolves to `{resolved}` — absent from `snapshots:`"
    )]
    #[diagnostic(
        code(pudu::lock::unresolved_edge),
        help("The lockfile is inconsistent. Re-run `pnpm install` to regenerate it.")
    )]
    UnresolvedEdge {
        from: String,
        link_name: String,
        resolved: String,
    },

    #[error("`{a}` and `{b}` both map to the Buck target name `{target}`")]
    #[diagnostic(code(pudu::lock::target_name_collision))]
    TargetNameCollision {
        a: String,
        b: String,
        target: String,
    },

    #[error("`{snapshot}` lists `{link_name}` in both `dependencies` and `optionalDependencies`")]
    #[diagnostic(
        code(pudu::lock::duplicate_link_name),
        help("The lockfile is inconsistent. Re-run `pnpm install` to regenerate it.")
    )]
    DuplicateLinkName { snapshot: String, link_name: String },

    #[error("this lockfile uses patchedDependencies, which pudu cannot reproduce")]
    #[diagnostic(
        code(pudu::lock::patched_dependencies),
        help(
            "A patch changes a package's contents, so ignoring it would emit a build that silently does not match your source. Remove the patch, or wait for pudu to support it."
        )
    )]
    PatchedDependencies,

    #[error("this lockfile was written with excludeLinksFromLockfile: true")]
    #[diagnostic(
        code(pudu::lock::excluded_links),
        help(
            "`link:` dependencies are omitted from the lockfile, so the dependency graph would be silently incomplete. Set excludeLinksFromLockfile=false in .npmrc and re-run `pnpm install`."
        )
    )]
    ExcludedLinks,
}

/// Non-fatal lockfile observations.
#[derive(Debug, Clone, PartialEq, Eq, Error, Diagnostic)]
pub enum LockWarning {
    #[error("unrecognised top-level key `{key}` in the lockfile")]
    #[diagnostic(
        severity(Warning),
        code(pudu::lock::unknown_top_level_key),
        help(
            "pudu does not read this key. If it changes how dependencies resolve, the generated build may be wrong."
        )
    )]
    UnknownTopLevelKey { key: String },

    #[error("`{key}` is deprecated: {message}")]
    #[diagnostic(severity(Warning), code(pudu::lock::deprecated_package))]
    DeprecatedPackage { key: String, message: String },
}

// --- Platform pruning (S2) ------------------------------------------------

/// Non-fatal findings from per-platform pruning.
///
/// S2 introduces no hard errors: every condition here is a property of
/// somebody's dependency tree rather than of pudu's input being malformed,
/// and none of them makes the rest of the output wrong.
#[derive(Debug, Clone, PartialEq, Eq, Error, Diagnostic)]
pub enum PlatformWarning {
    #[error("`{dependent}` requires `{target}`, which is excluded on platform `{platform}`")]
    #[diagnostic(
        severity(Warning),
        code(pudu::platform::required_dependency_excluded),
        help(
            "pudu drops the dependency for this platform. pnpm would install it anyway; if the package is genuinely needed here, it may need a fixup."
        )
    )]
    RequiredDependencyExcluded {
        dependent: String,
        target: String,
        platform: String,
    },

    #[error(
        "{} package(s) are excluded on every configured platform ({}): {}",
        packages.len(),
        platforms.join(", "),
        capped_list(packages, 10)
    )]
    #[diagnostic(
        severity(Warning),
        code(pudu::platform::excluded_everywhere),
        help(
            "these packages appear in no generated target. That is expected for the platform-specific binaries of a package like `esbuild`, and worth checking for anything else."
        )
    )]
    ExcludedEverywhere {
        packages: Vec<String>,
        platforms: Vec<String>,
    },
}

/// Render a name list as a comma-separated paragraph, capped so a large
/// aggregate (the real fixture has 78 names) stays readable. The count
/// still comes from the caller (e.g. `packages.len()`), so it is always
/// exact even when the inline list is truncated.
fn capped_list(items: &[String], cap: usize) -> String {
    if items.len() <= cap {
        items.join(", ")
    } else {
        format!(
            "{}, …and {} more",
            items[..cap].join(", "),
            items.len() - cap
        )
    }
}

// --- Vendor (S3) ----------------------------------------------------------

/// Failures of `pudu vendor`.
#[derive(Debug, Error, Diagnostic)]
pub enum VendorError {
    #[error("packages.toml is out of date ({} difference(s))", differences.len())]
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

    #[error(
        "{key}: tarball does not match the integrity recorded in pnpm-lock.yaml\n  url:      {url}\n  expected: {expected}\n  actual:   {actual}"
    )]
    #[diagnostic(
        code(pudu::vendor::integrity_mismatch),
        help(
            "the registry served different bytes than pnpm recorded. Do not ignore this: it means the tarball changed after your lockfile was written. Compare the two hashes and the URL above to tell a misconfigured mirror from a republished tarball from an attack."
        )
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
        help(
            "every entry in an npm tarball must nest under the same root directory (usually `package/`, but not always — `@types/*` packages use their own name); an archive whose entries disagree about that root cannot be extracted"
        )
    )]
    MalformedTarball { key: String, reason: String },

    #[error("{key}: tarball's root directory contains no package.json")]
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

    #[error("cannot read {path}: {reason}")]
    #[diagnostic(
        code(pudu::vendor::table_malformed),
        help("packages.toml is generated; delete it and run `pudu vendor` to rebuild it")
    )]
    TableMalformed { path: PathBuf, reason: String },

    #[error("{key}: {url} is not in the cache and --no-network was given")]
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
            // A registry that is down or refusing, and either half of the
            // cache being unusable, are properties of the environment rather
            // than of the user's input, so these stay unclassified rather
            // than claiming pudu.toml or the lockfile is at fault. A machine
            // with no `$HOME` must not be told its config is wrong — and
            // `CacheUnavailable` and `CacheWriteFailed` carry the same help
            // text, so they cannot sensibly carry different exit codes.
            VendorError::HttpStatus { .. }
            | VendorError::Transport { .. }
            | VendorError::CacheWriteFailed { .. }
            | VendorError::CacheUnavailable => ExitCode::Internal,
            VendorError::UnsupportedResolution { .. }
            | VendorError::IntegrityMismatch { .. }
            | VendorError::MalformedTarball { .. }
            | VendorError::MissingPackageJson { .. }
            | VendorError::MalformedIntegrity { .. }
            | VendorError::BadDerivedUrl { .. }
            | VendorError::TableMalformed { .. }
            | VendorError::NetworkDisabled { .. } => ExitCode::InputInvalid,
        }
    }
}

/// Non-fatal findings from inspecting a tarball.
///
/// Every one of these is a property of somebody's published package rather
/// than of pudu's input being malformed, and none makes the rest of the
/// package table wrong.
#[derive(Debug, Clone, PartialEq, Eq, Error, Diagnostic)]
pub enum VendorWarning {
    #[error(
        "{key}: pnpm-lock.yaml says hasBin: {lockfile}, but the tarball yields {found} command(s)"
    )]
    #[diagnostic(
        severity(Warning),
        code(pudu::vendor::has_bin_disagreement),
        help(
            "pudu records what the tarball contains; `hasBin` is a flag pnpm derives from registry metadata"
        )
    )]
    HasBinDisagreement {
        key: String,
        lockfile: bool,
        found: usize,
    },

    #[error("{key}: dropping bin `{name}` — the name is not URL-safe")]
    #[diagnostic(severity(Warning), code(pudu::vendor::bin_name_rejected))]
    BinNameRejected { key: String, name: String },

    // `contained_path` returns `None` for three shapes, not one: a path that
    // climbs out of the package (`../../etc/passwd`), the empty string, and
    // one that normalizes away to nothing (`.`, `./`). Dropping all three is
    // right; calling all three an escape is not.
    #[error("{key}: dropping bin `{name}` — its path `{path}` is empty or escapes the package")]
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

// --- Platform derivation (pudu init) -------------------------------------

/// Non-fatal findings from `pudu init`'s `supportedArchitectures` expansion.
#[derive(Debug, Clone, PartialEq, Eq, Error, Diagnostic)]
pub enum DeriveWarning {
    #[error("pnpm-workspace.yaml: ignoring unrecognized supportedArchitectures key `{key}`")]
    #[diagnostic(severity(Warning), code(pudu::init::unknown_architecture_key))]
    UnknownKey { key: String },

    #[error(
        "pnpm-workspace.yaml: skipping `win32` — Windows is a Phase 2 deliverable, \
         see https://github.com/rsJames-ttrpg/pudu/blob/main/docs/superpowers/specs/2026-08-30-pudu-roadmap.md"
    )]
    #[diagnostic(severity(Warning), code(pudu::init::win32_skipped))]
    Win32Skipped,

    #[error("pnpm-workspace.yaml: ignoring unknown os `{value}`")]
    #[diagnostic(severity(Warning), code(pudu::init::unknown_os))]
    UnknownOs { value: String },

    #[error("pnpm-workspace.yaml: ignoring unknown cpu `{value}`")]
    #[diagnostic(severity(Warning), code(pudu::init::unknown_cpu))]
    UnknownCpu { value: String },

    #[error("pnpm-workspace.yaml: ignoring unknown libc `{value}`")]
    #[diagnostic(severity(Warning), code(pudu::init::unknown_libc))]
    UnknownLibc { value: String },

    #[error("pnpm-workspace.yaml: supportedArchitectures must be a mapping of os/cpu/libc lists")]
    #[diagnostic(
        severity(Warning),
        code(pudu::init::supported_architectures_not_a_mapping),
        help("the block is ignored; see https://pnpm.io/settings#supportedarchitectures")
    )]
    SupportedArchitecturesNotAMapping,

    #[error("pnpm-workspace.yaml: ignoring non-string entry in supportedArchitectures.{key}")]
    #[diagnostic(
        severity(Warning),
        code(pudu::init::non_string_axis_entry),
        help("every entry must be a quoted or bare string, e.g. `cpu: [x64, arm64]`")
    )]
    NonStringAxisEntry { key: String },
}

/// Failures of `pudu init`'s `supportedArchitectures` expansion.
#[derive(Debug, Error, Diagnostic)]
pub enum DeriveError {
    #[error("cannot parse {path}")]
    #[diagnostic(code(pudu::init::workspace_parse))]
    WorkspaceParse {
        path: PathBuf,
        #[source]
        source: serde_norway::Error,
    },

    #[error(
        "pnpm-workspace.yaml: supportedArchitectures.{key} must be a list, e.g. `{key}: [{example}]`"
    )]
    #[diagnostic(
        code(pudu::init::axis_not_a_sequence),
        help(
            "a bare scalar like `{key}: {example}` is a more likely typo than an empty match, \
             so it is rejected rather than silently falling back to the host platform; wrap it \
             in brackets to make it a one-entry list, e.g. `{key}: [{example}]`"
        )
    )]
    AxisNotASequence { key: String, example: String },

    #[error(
        "pnpm-workspace.yaml declares no supported platforms pudu can target \
         (the `{axis}` axis of `supportedArchitectures` resolved to nothing usable)"
    )]
    #[diagnostic(
        code(pudu::init::no_usable_platforms),
        help("edit `supportedArchitectures`, or remove it to accept pudu's defaults")
    )]
    NoUsablePlatforms {
        axis: &'static str,
        /// The warnings collected while expanding, so a user told "no usable
        /// platforms" also learns *why* each candidate was dropped. Carried
        /// structurally (miette renders them as related diagnostics) rather
        /// than concatenated into the message.
        #[related]
        warnings: Vec<DeriveWarning>,
    },
}

/// Non-fatal findings from `pudu init`'s **scaffolding** pass.
///
/// Separate from [`DeriveWarning`] rather than merged into it: `DeriveWarning`
/// is part of the contract of the pure `derive_platforms` function — it is
/// returned in `DerivedPlatforms::warnings` and carried as `#[related]` on
/// [`DeriveError::NoUsablePlatforms`], where a "third-party/js/BUCK exists"
/// finding would be nonsense. The variant sets are genuinely disjoint (spec
/// §6), so they are two enums.
#[derive(Debug, Clone, PartialEq, Eq, Error, Diagnostic)]
pub enum InitWarning {
    #[error(
        "initializing in {init_root}, but pnpm-lock.yaml is in {lockfile_dir}; \
         assuming the Buck cell root is the latter for the `root//` load label in toolchains/BUCK"
    )]
    #[diagnostic(
        severity(Warning),
        code(pudu::init::cell_root_guess),
        help("edit the load label in toolchains/BUCK if your cell root differs")
    )]
    CellRootGuess {
        init_root: PathBuf,
        lockfile_dir: PathBuf,
    },

    #[error("{path} exists; leaving it alone")]
    #[diagnostic(
        severity(Warning),
        code(pudu::init::third_party_file_exists),
        help("files under third-party/js are yours once they exist; --force does not touch them")
    )]
    ThirdPartyFileExists { path: PathBuf },

    #[error("{path} already declares a node toolchain (`:{name}`); leaving it alone")]
    #[diagnostic(
        severity(Warning),
        code(pudu::init::existing_toolchain),
        help("recorded as `[buck] node_toolchain = \"{recorded}\"` in pudu.toml")
    )]
    ExistingToolchain {
        path: PathBuf,
        name: String,
        recorded: String,
    },

    #[error(
        "could not read the target name out of the `system_node_toolchain(...)` call in {path}; \
         assumed `{name}`"
    )]
    #[diagnostic(
        severity(Warning),
        code(pudu::init::toolchain_name_unparsed),
        help("check `[buck] node_toolchain` in pudu.toml")
    )]
    ToolchainNameUnparsed { path: PathBuf, name: String },

    #[error("{path} has unbalanced pudu markers; not modifying it")]
    #[diagnostic(
        severity(Warning),
        code(pudu::init::unbalanced_markers),
        help("add the block printed below by hand, then re-run `pudu config check`")
    )]
    UnbalancedMarkers { path: PathBuf },
}

// --- Rendering ------------------------------------------------------------

/// An error's own message plus its cause chain, joined with `: `.
///
/// A sourceless error therefore renders exactly once — the bug the old
/// `source_message()` helper caused by falling back to `to_string()`
/// (TD-S0-16). Used for the `--format json` envelope, which needs plain
/// strings rather than a rendered diagnostic.
pub fn full_message(err: &dyn std::error::Error) -> String {
    let mut s = err.to_string();
    let mut cur = err.source();
    while let Some(e) = cur {
        let _ = write!(s, ": {e}");
        cur = e.source();
    }
    s
}

/// The handler `main` and `config check` share, so every diagnostic pudu
/// prints looks the same.
///
/// Line wrapping is off: messages name absolute paths and Buck labels, and a
/// path broken across lines is neither greppable nor copy-pasteable.
fn handler() -> GraphicalReportHandler {
    let color = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let theme = if color {
        GraphicalTheme::unicode()
    } else {
        GraphicalTheme::unicode_nocolor()
    };
    GraphicalReportHandler::new_themed(theme).with_wrap_lines(false)
}

/// Render a diagnostic the way pudu prints it: message, `code`, cause chain,
/// `help`, and any related diagnostics. Ends with a newline.
pub fn render(diagnostic: &dyn Diagnostic) -> String {
    let mut out = String::new();
    if handler().render_report(&mut out, diagnostic).is_err() {
        // The only failure mode of a `String` writer is OOM; fall back to the
        // plain message rather than losing the error entirely.
        return format!("{}\n", full_message(diagnostic));
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Render an `anyhow` error from the CLI boundary, recovering its typed
/// diagnostic when it has one so `code` and `help` reach the user.
///
/// **A typed error must carry its own complete message.** This renders the
/// typed diagnostic found in the chain, so `.context(...)` added over a
/// `CliError`/`ConfigError`/`DeriveError` is silently dropped from the
/// output (it still classifies, since `downcast_ref` walks the chain). Use
/// `.context()` over `io::Error` and friends; add a variant, or a field, when
/// a typed error needs to say more.
pub fn render_cli(err: &anyhow::Error) -> String {
    match as_diagnostic(err) {
        Some(d) => render(d),
        // No `code`/`help` to show, so `MietteDiagnostic` gives it the same
        // frame as everything else without inventing metadata.
        None => render(&miette::MietteDiagnostic::new(format!("{err:#}"))),
    }
}

/// The typed-error registry: **the one place a new typed error is added.**
///
/// Each arm names a type and how to get its [`ExitCode`]. The macro derives
/// both halves of classification — the `&dyn Diagnostic` used for rendering
/// and the exit code — from that single list, so a half-registration (which
/// used to mean either "renders without `code`/`help`" or "silently exits 1")
/// is not expressible. Adding `LockfileError` in S1 is one line here.
macro_rules! typed_errors {
    ($($ty:ty => $code:expr),+ $(,)?) => {
        /// Recover the typed diagnostic behind an `anyhow::Error` from the
        /// CLI boundary, together with the exit code it maps to.
        ///
        /// `None` means unclassified, which is [`ExitCode::Internal`]: an
        /// unexpected I/O failure is not a configuration problem and must not
        /// be reported as one.
        pub fn classify(err: &anyhow::Error) -> Option<(&dyn Diagnostic, ExitCode)> {
            $(
                if let Some(e) = err.downcast_ref::<$ty>() {
                    let code: fn(&$ty) -> ExitCode = $code;
                    return Some((e as &dyn Diagnostic, code(e)));
                }
            )+
            None
        }

        /// Every registered type, by name — so a test can assert its sample
        /// set covers the registry.
        pub const REGISTERED_ERRORS: &[&str] = &[$(stringify!($ty)),+];
    };
}

typed_errors! {
    CliError => CliError::exit_code,
    ConfigError => |_| ExitCode::InputInvalid,
    DeriveError => |_| ExitCode::InputInvalid,
    LockError => |_| ExitCode::InputInvalid,
    VendorError => VendorError::exit_code,
}

/// The exit code an `anyhow` error from the CLI boundary maps to.
pub fn exit_code(err: &anyhow::Error) -> ExitCode {
    classify(err).map_or(ExitCode::Internal, |(_, code)| code)
}

fn as_diagnostic(err: &anyhow::Error) -> Option<&dyn Diagnostic> {
    classify(err).map(|(d, _)| d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn lockfile_not_found_names_the_resolved_path() {
        let e = ConfigError::LockfileNotFound {
            path: PathBuf::from("/repo/pnpm-lock.yaml"),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("/repo/pnpm-lock.yaml"),
            "message must name the file: {msg}"
        );
    }

    #[test]
    fn parse_error_reports_line_and_column() {
        let bad = "lockfile_path = \n";
        let inner = toml::from_str::<toml::Value>(bad).unwrap_err();
        let e = ConfigError::Parse {
            path: PathBuf::from("/repo/pudu.toml"),
            source: inner,
        };
        let msg = full_message(&e);
        assert!(
            msg.contains("line"),
            "parse errors must carry a line: {msg}"
        );
        assert!(msg.contains("/repo/pudu.toml"), "{msg}");
    }

    /// TD-S0-16: `source_message()` fell back to `to_string()`, so an error
    /// with no `#[source]` was printed twice by `config check`.
    #[test]
    fn a_sourceless_error_renders_exactly_once() {
        let e = ConfigError::BadFixupRegistryPath {
            value: "file://".to_string(),
        };
        let once = e.to_string();
        assert_eq!(full_message(&e), once);
        assert_eq!(
            render(&e).matches(&once as &str).count(),
            1,
            "the message must appear once:\n{}",
            render(&e)
        );
    }

    /// Spec §6 promises rendering does not wrap, because pudu's messages name
    /// absolute paths and Buck labels: at miette's default width of 80 a long
    /// path is broken mid-segment across three lines, which is neither
    /// greppable nor copy-pasteable.
    #[test]
    fn a_long_path_is_never_broken_across_lines() {
        let path = format!(
            "/tmp/{}/pnpm-lock.yaml",
            (0..8)
                .map(|i| format!("very-long-directory-segment-{i:02}"))
                .collect::<Vec<_>>()
                .join("/")
        );
        assert!(
            path.len() > 100,
            "the probe path must exceed any wrap width"
        );
        let out = render(&ConfigError::LockfileNotFound {
            path: PathBuf::from(&path),
        });
        assert!(
            out.lines().any(|l| l.contains(&path)),
            "the path must survive on one line:\n{out}"
        );
    }

    #[test]
    fn libc_on_non_linux_names_the_platform() {
        let e = ConfigError::LibcOnNonLinux {
            platform: "darwin-arm64".into(),
        };
        assert!(e.to_string().contains("darwin-arm64"));
    }

    /// TD-S0-18: the `help(...)` strings existed but never reached a user.
    #[test]
    fn rendering_surfaces_the_code_and_the_help_text() {
        let out = render(&ConfigError::LibcOnNonLinux {
            platform: "darwin-arm64".into(),
        });
        assert!(out.contains("pudu::config::libc_on_non_linux"), "{out}");
        assert!(
            out.contains("remove `libc`, or change `os` to \"linux\""),
            "{out}"
        );
    }

    #[test]
    fn related_warnings_are_rendered_with_the_error() {
        let out = render(&DeriveError::NoUsablePlatforms {
            axis: "libc",
            warnings: vec![DeriveWarning::UnknownLibc {
                value: "uclibc".into(),
            }],
        });
        assert!(out.contains("libc"), "{out}");
        assert!(out.contains("uclibc"), "the reason must survive:\n{out}");
    }

    #[test]
    fn axis_not_a_sequence_renders_its_help_text() {
        let out = render(&DeriveError::AxisNotASequence {
            key: "os".into(),
            example: "linux".into(),
        });
        assert!(out.contains("os: [linux]"), "{out}");
        assert!(
            out.contains("typo"),
            "help must explain why this is fatal, not just how to fix it: {out}"
        );
    }

    #[test]
    fn exit_codes_are_classified() {
        let cases: [(anyhow::Error, ExitCode); 10] = [
            (
                CliError::Unimplemented {
                    verb: "vendor".into(),
                    stage: "S3".into(),
                }
                .into(),
                ExitCode::Unimplemented,
            ),
            (
                CliError::ConfigExists {
                    path: "pudu.toml".into(),
                }
                .into(),
                ExitCode::Usage,
            ),
            (
                CliError::BadDirectory {
                    path: "/no/such/dir".into(),
                    source: std::io::Error::from(std::io::ErrorKind::NotFound),
                }
                .into(),
                ExitCode::Usage,
            ),
            (ConfigError::NoPlatforms.into(), ExitCode::InputInvalid),
            (anyhow::anyhow!("disk on fire"), ExitCode::Internal),
            (
                anyhow::Error::from(VendorError::Stale {
                    differences: vec![
                        "packages.toml has no entry for `left-pad@1.3.0`".to_string(),
                    ],
                }),
                ExitCode::Stale,
            ),
            (
                anyhow::Error::from(VendorError::NetworkDisabled {
                    key: "left-pad@1.3.0".to_string(),
                    url: "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz".to_string(),
                }),
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
            // Both halves of "the cache is unusable" are environment
            // failures carrying identical help text, so they must classify
            // identically. `CacheUnavailable` used to be 3, telling a user
            // whose machine has no `$HOME` that their config was wrong.
            (
                anyhow::Error::from(VendorError::CacheUnavailable),
                ExitCode::Internal,
            ),
            (
                anyhow::Error::from(VendorError::CacheWriteFailed {
                    path: "/read-only/pudu".into(),
                    source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                }),
                ExitCode::Internal,
            ),
        ];
        for (err, want) in cases {
            assert_eq!(exit_code(&err), want, "{err:#}");
        }
    }

    /// One sample per registered type. Bound to [`REGISTERED_ERRORS`] by the
    /// test below, so adding a type to the `typed_errors!` registry without a
    /// sample here fails the build's tests.
    fn samples() -> Vec<(&'static str, anyhow::Error, ExitCode)> {
        vec![
            (
                "CliError",
                CliError::Unimplemented {
                    verb: "vendor".into(),
                    stage: "S3".into(),
                }
                .into(),
                ExitCode::Unimplemented,
            ),
            (
                "ConfigError",
                ConfigError::NoPlatforms.into(),
                ExitCode::InputInvalid,
            ),
            (
                "DeriveError",
                DeriveError::NoUsablePlatforms {
                    axis: "os",
                    warnings: vec![],
                }
                .into(),
                ExitCode::InputInvalid,
            ),
            (
                "LockError",
                LockError::PatchedDependencies.into(),
                ExitCode::InputInvalid,
            ),
            (
                "VendorError",
                VendorError::NetworkDisabled {
                    key: "left-pad@1.3.0".to_string(),
                    url: "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz".to_string(),
                }
                .into(),
                ExitCode::InputInvalid,
            ),
        ]
    }

    /// Q2/1: registration used to be two hand-maintained downcast chains, and
    /// registering in only one of them silently produced either a diagnostic
    /// with no `code`/`help` or an exit of 1. `classify` returns both halves
    /// from one registry; this pins that both halves are real for every
    /// registered type.
    #[test]
    fn every_registered_error_classifies_with_a_code_and_an_exit_code() {
        for (name, err, want) in samples() {
            let (diag, code) = classify(&err)
                .unwrap_or_else(|| panic!("{name} is registered but does not classify"));
            assert_eq!(code, want, "{name} exit code");
            assert_ne!(
                code,
                ExitCode::Internal,
                "{name} must not fall through to the unclassified code"
            );
            assert!(
                diag.code().is_some(),
                "{name} must render with a diagnostic code"
            );
            assert!(
                render(diag).contains(&diag.code().unwrap().to_string()),
                "{name}'s code must reach the rendered output"
            );
        }
    }

    #[test]
    fn the_samples_cover_the_whole_registry() {
        let covered: Vec<&str> = samples().into_iter().map(|(n, _, _)| n).collect();
        assert_eq!(
            covered, REGISTERED_ERRORS,
            "every type in the `typed_errors!` registry needs a sample above"
        );
    }

    #[test]
    fn required_dependency_excluded_names_all_three_parties() {
        let w = PlatformWarning::RequiredDependencyExcluded {
            dependent: "my-app@1.0.0".into(),
            target: "fsevents@2.3.3".into(),
            platform: "linux-x64-gnu".into(),
        };
        let msg = w.to_string();
        assert!(msg.contains("my-app@1.0.0"), "names the dependent: {msg}");
        assert!(msg.contains("fsevents@2.3.3"), "names the target: {msg}");
        assert!(msg.contains("linux-x64-gnu"), "names the platform: {msg}");
    }

    /// Fires once for the whole set, not once per package: on the committed
    /// fixture a per-package warning would print ~60 times and train the
    /// user to ignore warnings.
    #[test]
    fn excluded_everywhere_aggregates_into_one_message() {
        let w = PlatformWarning::ExcludedEverywhere {
            packages: vec![
                "@esbuild/aix-ppc64@0.25.12".into(),
                "@esbuild/sunos-x64@0.25.12".into(),
            ],
            platforms: vec!["linux-x64-gnu".into(), "darwin-arm64".into()],
        };
        let msg = w.to_string();
        assert!(msg.contains("@esbuild/aix-ppc64@0.25.12"), "{msg}");
        assert!(msg.contains("@esbuild/sunos-x64@0.25.12"), "{msg}");
        assert!(msg.contains("2 package(s)"), "states how many: {msg}");
    }

    /// On the real fixture this renders 78 package names as one unwrapped
    /// paragraph, which is unreadable. Cap the inline list at 10, still
    /// stating the full count.
    #[test]
    fn excluded_everywhere_caps_the_inline_list_but_keeps_the_full_count() {
        let packages: Vec<String> = (0..15).map(|i| format!("pkg-{i}@1.0.0")).collect();
        let w = PlatformWarning::ExcludedEverywhere {
            packages: packages.clone(),
            platforms: vec!["linux-x64-gnu".into()],
        };
        let msg = w.to_string();
        assert!(msg.contains("15 package(s)"), "full count: {msg}");
        for p in &packages[..10] {
            assert!(msg.contains(p.as_str()), "first 10 listed: {msg}");
        }
        for p in &packages[10..] {
            assert!(!msg.contains(p.as_str()), "not listed past the cap: {msg}");
        }
        assert!(msg.contains("…and 5 more"), "{msg}");
    }

    #[test]
    fn platform_warnings_render_at_warning_severity_with_a_code() {
        let w1 = PlatformWarning::RequiredDependencyExcluded {
            dependent: "a@1".into(),
            target: "b@2".into(),
            platform: "p".into(),
        };
        assert_eq!(w1.severity(), Some(miette::Severity::Warning));
        assert!(w1.code().is_some(), "every diagnostic carries a code");
        let out1 = render(&w1);
        assert!(
            out1.contains("pudu::platform::required_dependency_excluded"),
            "{out1}"
        );
        assert!(out1.contains("fixup"), "{out1}");

        let w2 = PlatformWarning::ExcludedEverywhere {
            packages: vec!["b@2".into()],
            platforms: vec!["p".into()],
        };
        assert_eq!(w2.severity(), Some(miette::Severity::Warning));
        assert!(w2.code().is_some(), "every diagnostic carries a code");
        let out2 = render(&w2);
        assert!(
            out2.contains("pudu::platform::excluded_everywhere"),
            "{out2}"
        );
        assert!(out2.contains("generated target"), "{out2}");
    }
}
