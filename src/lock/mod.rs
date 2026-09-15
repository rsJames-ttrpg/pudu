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
            // `9.0` parses as a float. A fixed one-decimal format
            // (`format!("{n:.1}")`) truncates a value like `9.12` to "9.1",
            // misreporting the actual unsupported version to the user.
            // `Display` prints the minimal digits needed instead — but for
            // a whole number it drops the decimal point entirely (`9.0`
            // becomes "9"), which would make a legitimately-supported bare
            // `lockfileVersion: 9.0` compare unequal to `SUPPORTED_VERSION`
            // ("9.0") and get rejected as unsupported. So: use the minimal
            // `Display` form, but restore a trailing ".0" when it has no
            // decimal point at all.
            //
            // F2 (fix round 2): that ".0" restoration must not apply to a
            // non-finite value. `Display` on `f64::NAN`/`INFINITY` prints
            // "NaN"/"inf" (no '.'), and blindly appending ".0" produced
            // "NaN.0" / "inf.0" — strings that appear nowhere in the
            // lockfile, which is worse than the old `{n:.1}` behavior this
            // fixup replaced (that at least printed the honest "NaN"/
            // "inf"). `1e30` also has no '.', but restoring nonfinite-only
            // still leaves its 31-digit `Display` form unabbreviated; that
            // is unrelated to F2 (rejection is still correct either way)
            // and out of scope here — F2 is only about not naming a
            // version string the file does not contain.
            Self::Num(n) => {
                let s = format!("{n}");
                if !n.is_finite() || s.contains('.') {
                    s
                } else {
                    format!("{s}.0")
                }
            }
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

    let mut lockfile: Lockfile =
        serde_norway::from_str(text).map_err(|source| LockError::Yaml {
            path: path.to_path_buf(),
            source,
        })?;
    lockfile.lockfile_version = found.expect("gated to Some(SUPPORTED_VERSION) above");

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

    /// TD-S1-08: `Lockfile::lockfile_version` must carry the version
    /// `parse_lockfile` actually observed, not merely default to it — the
    /// struct's `#[serde(skip)]` field defaults to `""`, so this test would
    /// fail (empty string) if `parse_lockfile` ever stopped setting it.
    #[test]
    fn parsed_lockfile_reports_the_version_it_observed() {
        let (quoted, _) = parse(MINIMAL).unwrap();
        assert_eq!(quoted.lockfile_version, "9.0");

        let (bare, _) = parse("lockfileVersion: 9.0\nimporters: {}\n").unwrap();
        assert_eq!(bare.lockfile_version, "9.0");
    }

    /// TD-S1-02: a bare-numeric `lockfileVersion` must not be truncated to
    /// one decimal when rendered into an error message. `9.10` is not a
    /// useful example here — it parses to the same f64 as `9.1`, so
    /// truncation loses nothing for that specific input. `9.12` does show
    /// the bug: `format!("{n:.1}")` would round it to "9.1", misreporting
    /// the version actually found.
    #[test]
    fn a_bare_numeric_version_is_not_truncated_to_one_decimal() {
        assert_eq!(RawVersion::Num(9.12).normalize(), "9.12");
    }

    /// The counterpart to the above: a whole-number bare version must still
    /// render with its decimal point, so it compares equal to
    /// `SUPPORTED_VERSION` ("9.0") rather than becoming "9" and being
    /// rejected as unsupported.
    #[test]
    fn a_whole_number_bare_version_keeps_its_decimal_point() {
        assert_eq!(RawVersion::Num(9.0).normalize(), "9.0");
    }

    /// F2 (fix round 2): the ".0" restoration above exists only to keep a
    /// legitimate whole-number version (`9.0`) comparing equal to
    /// `SUPPORTED_VERSION`. Applied to a non-finite value it instead
    /// glues ".0" onto `Display`'s "NaN"/"inf", producing a version string
    /// ("NaN.0", "inf.0") that appears nowhere in the lockfile — worse
    /// than the honest (if imprecise) "NaN"/"inf" the old `{n:.1}`
    /// formatting produced. Neither is a real pnpm lockfileVersion, so
    /// rejection is correct either way; only the message text is at stake.
    #[test]
    fn a_non_finite_bare_version_is_not_given_a_fabricated_decimal_point() {
        assert_eq!(RawVersion::Num(f64::NAN).normalize(), "NaN");
        assert_eq!(RawVersion::Num(f64::INFINITY).normalize(), "inf");
        assert_eq!(RawVersion::Num(f64::NEG_INFINITY).normalize(), "-inf");
    }

    /// The end-to-end counterpart: a lockfile whose `lockfileVersion` is a
    /// YAML `.nan`/`.inf` special float must be rejected with a message
    /// naming what pnpm-lock.yaml actually contains, not a string with a
    /// fabricated ".0" that appears nowhere in the file.
    #[test]
    fn a_lockfile_with_a_non_finite_version_is_rejected_without_a_fabricated_version_string() {
        for (src_version, must_not_contain) in [(".nan", "NaN.0"), (".inf", "inf.0")] {
            let src = format!("lockfileVersion: {src_version}\nimporters: {{}}\n");
            let err = parse(&src).expect_err("a non-finite version must be rejected");
            let msg = format!("{err}");
            assert!(
                !msg.contains(must_not_contain),
                "must not name a version string absent from the file: {msg}"
            );
        }
    }

    /// The old `format!("{n:.1}")` did not merely truncate when *reporting* a
    /// version — it rounded, and `normalize()` feeds the supported-version
    /// comparison itself. So `8.96`, `9.04` and `9.0499` all became "9.0" and
    /// were accepted as a supported v9.0 lockfile, silently, on a value pudu
    /// has never been tested against. That is the more serious half of
    /// TD-S1-02 and it went unrecorded by the row, so it is pinned here
    /// rather than left to the two tests that only cover the message text.
    #[test]
    fn a_near_miss_bare_version_is_not_rounded_into_the_supported_one() {
        for near in ["8.96", "9.04", "9.0499"] {
            let src = format!("lockfileVersion: {near}\nimporters: {{}}\n");
            let err = parse(&src).expect_err("a near-miss version must not be accepted as 9.0");
            let msg = format!("{err}");
            assert!(
                msg.contains(near),
                "{near} must be rejected and named, not rounded to 9.0: {msg}"
            );
        }
    }

    #[test]
    fn an_unsupported_bare_numeric_version_names_itself_precisely() {
        let err = parse("lockfileVersion: 9.12\nimporters: {}\n").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("9.12"),
            "must name the version actually found, not a truncated one: {msg}"
        );
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

    #[test]
    fn malformed_yaml_is_a_yaml_error() {
        // Unclosed flow mapping: the most likely real-world failure mode, and
        // nothing previously pinned it to a variant.
        let err = parse("lockfileVersion: '9.0'\nimporters: {\n").unwrap_err();
        assert!(
            matches!(err, LockError::Yaml { .. }),
            "expected LockError::Yaml, got {err:?}"
        );
        let msg = format!("{err}");
        assert!(msg.contains("pnpm-lock.yaml"), "must name the file: {msg}");
    }
}
