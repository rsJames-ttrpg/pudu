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
        packages.join(", ")
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
    fn exit_codes_are_classified() {
        let cases: [(anyhow::Error, ExitCode); 5] = [
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
