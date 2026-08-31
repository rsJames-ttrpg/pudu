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
        Self {
            auto_install_peers: true,
            exclude_links_from_lockfile: false,
        }
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
    /// Encoded exactly like a snapshot edge value.
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
// Untagged: serde tries each variant and picks the one that fits, so the
// variant is chosen by which key is present. `deny_unknown_fields` is
// deliberately absent — it is not meaningful on an untagged enum, and
// tolerating extra keys matches the rest of these types. A map matching no
// variant (`{mystery: 1}`) still fails, which is the behaviour wanted.
#[serde(untagged)]
pub enum Resolution {
    Integrity {
        integrity: String,
    },
    Tarball {
        tarball: String,
    },
    Git {
        repo: String,
        commit: String,
    },
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
    /// see the alias rule in the graph-construction task.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub transitive_peer_dependencies: Vec<String>,
}

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
        let i: PackageMeta =
            serde_norway::from_str("resolution: {integrity: sha512-abc}\n").unwrap();
        assert!(matches!(i.resolution, Resolution::Integrity { .. }));
        let t: PackageMeta =
            serde_norway::from_str("resolution: {tarball: https://x/y.tgz}\n").unwrap();
        assert!(matches!(t.resolution, Resolution::Tarball { .. }));
        let d: PackageMeta =
            serde_norway::from_str("resolution: {directory: ../lib, type: directory}\n").unwrap();
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
