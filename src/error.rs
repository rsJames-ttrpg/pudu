//! Error and warning types for pudu, plus the one place diagnostics are
//! rendered.
//!
//! Three layers, deliberately:
//!
//! * **Typed, per-module `thiserror` enums** ([`ConfigError`], [`DeriveError`])
//!   for library-internal failures, so tests assert on variants rather than
//!   message text (spec §6).
//! * **Typed warning enums** ([`ConfigWarning`], [`DeriveWarning`]) alongside
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
    /// Configuration invalid: validation failed, or `pudu.toml` is malformed.
    ConfigInvalid = 3,
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

    #[error("pudu debug requires a subcommand{}", match unknown {
        Some(u) => format!(": unknown `{u}`"),
        None => String::new(),
    })]
    #[diagnostic(
        code(pudu::usage::debug_subcommand),
        help("none exist yet; S1 adds `print-graph`")
    )]
    DebugNeedsSubcommand { unknown: Option<String> },

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
}

impl CliError {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            CliError::Unimplemented { .. } => ExitCode::Unimplemented,
            CliError::ConfigExists { .. } | CliError::DebugNeedsSubcommand { .. } => {
                ExitCode::Usage
            }
            CliError::ConfigUnreadable { .. } | CliError::ConfigInvalid { .. } => {
                ExitCode::ConfigInvalid
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
pub fn render_cli(err: &anyhow::Error) -> String {
    match as_diagnostic(err) {
        Some(d) => render(d),
        // No `code`/`help` to show, so `MietteDiagnostic` gives it the same
        // frame as everything else without inventing metadata.
        None => render(&miette::MietteDiagnostic::new(format!("{err:#}"))),
    }
}

/// The exit code an `anyhow` error from the CLI boundary maps to.
///
/// Anything unclassified is [`ExitCode::Internal`]: an unexpected I/O failure
/// is not a configuration problem, and must not be reported as one.
pub fn exit_code(err: &anyhow::Error) -> ExitCode {
    if let Some(e) = err.downcast_ref::<CliError>() {
        return e.exit_code();
    }
    if err.downcast_ref::<ConfigError>().is_some() || err.downcast_ref::<DeriveError>().is_some() {
        return ExitCode::ConfigInvalid;
    }
    ExitCode::Internal
}

fn as_diagnostic(err: &anyhow::Error) -> Option<&dyn Diagnostic> {
    if let Some(e) = err.downcast_ref::<CliError>() {
        return Some(e);
    }
    if let Some(e) = err.downcast_ref::<ConfigError>() {
        return Some(e);
    }
    if let Some(e) = err.downcast_ref::<DeriveError>() {
        return Some(e);
    }
    None
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
        let cases: [(anyhow::Error, ExitCode); 4] = [
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
            (ConfigError::NoPlatforms.into(), ExitCode::ConfigInvalid),
            (anyhow::anyhow!("disk on fire"), ExitCode::Internal),
        ];
        for (err, want) in cases {
            assert_eq!(exit_code(&err), want, "{err:#}");
        }
    }
}
