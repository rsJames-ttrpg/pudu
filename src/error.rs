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
        let msg = e.source_message();
        assert!(
            msg.contains("line"),
            "parse errors must carry a line: {msg}"
        );
    }

    #[test]
    fn libc_on_non_linux_names_the_platform() {
        let e = ConfigError::LibcOnNonLinux {
            platform: "darwin-arm64".into(),
        };
        assert!(e.to_string().contains("darwin-arm64"));
    }
}
