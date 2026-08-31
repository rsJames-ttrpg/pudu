//! Reading `pnpm-lock.yaml` v9.

pub mod types;

use std::path::Path;

use serde::Deserialize;

use crate::error::{LockError, LockWarning};
pub use types::*;

/// The only lockfile version pudu reads.
pub const SUPPORTED_VERSION: &str = "9.0";

/// Top-level keys pudu knows about. Anything else warns (design §8.2).
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
pub fn parse_lockfile(text: &str, path: &Path) -> Result<(Lockfile, Vec<LockWarning>), LockError> {
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

    // An absent key and every spelling of an explicit null (`~`, `null`, a
    // bare `patchedDependencies:`) all deserialize to `None`, so only a
    // present-and-non-empty mapping can reach the error.
    if probe
        .patched_dependencies
        .as_ref()
        .is_some_and(|v| !is_empty_mapping(v))
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
    fn empty_or_absent_patched_dependencies_is_tolerated() {
        let empty = format!("{MINIMAL}patchedDependencies: {{}}\n");
        assert!(parse(&empty).is_ok(), "empty mapping must not be an error");

        let null = format!("{MINIMAL}patchedDependencies:\n");
        assert!(parse(&null).is_ok(), "null value must not be an error");
    }

    #[test]
    fn patched_dependencies_is_an_error() {
        let text = format!(
            "{MINIMAL}patchedDependencies:\n  foo@1.0.0:\n    hash: abc\n    path: patches/foo.patch\n"
        );
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
        let text = format!(
            "{MINIMAL}catalogs:\n  default:\n    react:\n      specifier: ^18\n      version: 18.3.1\noverrides:\n  foo: 1.0.0\n"
        );
        let (_, warnings) = parse(&text).expect("must parse");
        assert!(warnings.is_empty(), "must not warn: {warnings:?}");
    }

    #[test]
    fn unknown_top_level_key_warns_and_continues() {
        let text = format!("{MINIMAL}someFutureKey: 1\n");
        let (_, warnings) = parse(&text).expect("must still parse");
        assert!(
            warnings.iter().any(
                |w| matches!(w, LockWarning::UnknownTopLevelKey { key } if key == "someFutureKey")
            ),
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
            warnings.iter().any(
                |w| matches!(w, LockWarning::DeprecatedPackage { key, .. } if key == "glob@10.4.5")
            ),
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
