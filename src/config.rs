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

pub(crate) fn default_third_party_dir() -> PathBuf {
    PathBuf::from("third-party/js")
}
pub(crate) fn default_registry_url() -> Url {
    Url::parse("https://registry.npmjs.org").expect("valid literal URL")
}
pub(crate) fn default_file_name() -> String {
    "BUCK".to_string()
}
pub(crate) fn default_node_toolchain() -> String {
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
        Self {
            default: default_registry_url(),
            scopes: BTreeMap::new(),
        }
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
            allow_local_overrides: default_true(),
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
        Self {
            file_name: default_file_name(),
            node_toolchain: default_node_toolchain(),
        }
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
        if let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next())
            && !owner.is_empty()
            && !repo.is_empty()
        {
            return Ok(FixupRegistry::Github {
                owner: owner.into(),
                repo: repo.into(),
            });
        }
    }
    Err(ConfigError::BadFixupRegistry {
        value: value.to_string(),
    })
}

impl Config {
    /// Parse `pudu.toml` text. `path` is used only for error messages.
    pub fn from_str(text: &str, path: &Path) -> Result<Config> {
        let raw: RawConfig = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;

        Ok(Config {
            lockfile_path: raw.lockfile_path,
            third_party_dir: raw.third_party_dir,
            platforms: raw.platforms,
            registry: RegistryConfig {
                default: raw.registry.default,
                scopes: raw.registry.scopes,
            },
            fixups: FixupsConfig {
                registry: parse_fixup_registry(&raw.fixups.registry)?,
                registry_rev: raw.fixups.registry_rev,
                allow_local_overrides: raw.fixups.allow_local_overrides,
            },
            scripts: ScriptsConfig {
                allow: raw.scripts.allow,
            },
            buck: BuckConfig {
                file_name: raw.buck.file_name,
                node_toolchain: raw.buck.node_toolchain,
            },
        })
    }

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
                errors.push(ConfigError::WindowsUnsupported {
                    platform: name.clone(),
                });
            }
            if p.libc.is_some() && p.os != Os::Linux {
                errors.push(ConfigError::LibcOnNonLinux {
                    platform: name.clone(),
                });
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
                errors.push(ConfigError::BadRegistryScope {
                    scope: scope.clone(),
                });
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
            warnings
                .push("`[fixups].registry` is set but `registry_rev` is not pinned".to_string());
        }

        (errors, warnings)
    }
}

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
        assert_eq!(
            c.registry.scopes["@myorg"].as_str(),
            "https://npm.example.com/"
        );
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
    fn applies_field_level_defaults_when_tables_are_partial() {
        // Every table is present but only partially populated, so the
        // field-level `#[serde(default = ...)]` attributes on RawRegistry,
        // RawBuck, and RawFixups fire (not just the struct-level `Default`
        // impls used when a whole table is omitted).
        let c = Config::from_str(
            r#"
lockfile_path = "x"
[platforms.p]
os = "linux"
cpu = "x64"

[registry]
"@myorg" = "https://npm.example.com"

[buck]
file_name = "CUSTOM_BUCK"

[fixups]
registry_rev = "deadbeef"
"#,
            Path::new("pudu.toml"),
        )
        .unwrap();

        assert_eq!(c.registry.default.as_str(), "https://registry.npmjs.org/");
        assert_eq!(c.buck.file_name, "CUSTOM_BUCK");
        assert_eq!(c.buck.node_toolchain, "toolchains//:node");
        assert_eq!(c.fixups.registry, FixupRegistry::None);
        assert!(c.fixups.allow_local_overrides);
    }

    #[test]
    fn explicit_allow_local_overrides_false_is_honored() {
        let c = Config::from_str(
            "lockfile_path = \"x\"\n[platforms.p]\nos=\"linux\"\ncpu=\"x64\"\n[fixups]\nallow_local_overrides = false\n",
            Path::new("pudu.toml"),
        )
        .unwrap();
        assert!(!c.fixups.allow_local_overrides);
    }

    #[test]
    fn rejects_unknown_top_level_key() {
        let err = Config::from_str(
            "lockfile_path = \"x\"\nwidgets = 3\n[platforms.p]\nos=\"linux\"\ncpu=\"x64\"\n",
            Path::new("pudu.toml"),
        )
        .unwrap_err();
        assert!(
            err.source_message().contains("widgets"),
            "{}",
            err.source_message()
        );
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
            (
                "file:///tmp/reg",
                FixupRegistry::File(PathBuf::from("/tmp/reg")),
            ),
            (
                "github.com/owner/repo",
                FixupRegistry::Github {
                    owner: "owner".into(),
                    repo: "repo".into(),
                },
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

    #[test]
    fn rejects_malformed_github_fixup_registry_forms() {
        // These all reach the `github.com/` prefix branch and must fail the
        // (Some, Some, None) arity/non-empty-segment match, not just the
        // prefix check.
        for input in [
            "github.com/a/b/c", // too many segments
            "github.com/a",     // too few segments
            "github.com/",      // empty owner and repo
            "github.com/a/",    // empty repo
        ] {
            let text = format!(
                "lockfile_path=\"x\"\n[platforms.p]\nos=\"linux\"\ncpu=\"x64\"\n[fixups]\nregistry=\"{input}\"\n"
            );
            let err = Config::from_str(&text, Path::new("pudu.toml")).unwrap_err();
            assert!(
                matches!(err, ConfigError::BadFixupRegistry { .. }),
                "for input {input}"
            );
        }
    }

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
        let c = cfg(
            "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.darwin-arm64]\nos=\"darwin\"\ncpu=\"arm64\"\nlibc=\"glibc\"\n",
        );
        let (errors, _) = c.validate(d.path());
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::LibcOnNonLinux { .. })),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_windows_platform() {
        let d = tempdir_with_lockfile();
        let c =
            cfg("lockfile_path=\"pnpm-lock.yaml\"\n[platforms.win]\nos=\"win32\"\ncpu=\"x64\"\n");
        let (errors, _) = c.validate(d.path());
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::WindowsUnsupported { .. })),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_duplicate_platform_triples() {
        let d = tempdir_with_lockfile();
        let c = cfg(
            "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"linux\"\ncpu=\"x64\"\nlibc=\"glibc\"\n[platforms.b]\nos=\"linux\"\ncpu=\"x64\"\nlibc=\"glibc\"\n",
        );
        let (errors, _) = c.validate(d.path());
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::DuplicatePlatform { .. })),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_bad_constraint_label() {
        let d = tempdir_with_lockfile();
        let c = cfg(
            "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"linux\"\ncpu=\"x64\"\nconstraints=[\"not-a-label\"]\n",
        );
        let (errors, _) = c.validate(d.path());
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::BadConstraintLabel { .. })),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_scope_without_at_sign() {
        let d = tempdir_with_lockfile();
        let c = cfg(
            "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"linux\"\ncpu=\"x64\"\n[registry]\nmyorg=\"https://x.example.com\"\n",
        );
        let (errors, _) = c.validate(d.path());
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::BadRegistryScope { .. })),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_bad_package_name_in_allow() {
        let d = tempdir_with_lockfile();
        let c = cfg(
            "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"linux\"\ncpu=\"x64\"\n[scripts]\nallow=[\"Not A Package\"]\n",
        );
        let (errors, _) = c.validate(d.path());
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::BadPackageName { .. })),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_missing_lockfile() {
        let d = tempfile::tempdir().unwrap();
        let (errors, _) = cfg(GOOD).validate(d.path());
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConfigError::LockfileNotFound { .. })),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_empty_platforms() {
        let d = tempdir_with_lockfile();
        let c = cfg("lockfile_path=\"pnpm-lock.yaml\"\n");
        let (errors, _) = c.validate(d.path());
        assert!(
            errors.iter().any(|e| matches!(e, ConfigError::NoPlatforms)),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_does_not_create_third_party_dir() {
        // `pudu config check` is specified as side-effect free: validating
        // must not materialize `third_party_dir`. GOOD's third_party_dir is
        // "third-party/js", two nonexistent levels below the tempdir, which
        // exercises the multi-level ancestor walk in `check_writable`.
        let d = tempdir_with_lockfile();
        let tpd = d.path().join("third-party");
        assert!(!tpd.exists());
        let (errors, _) = cfg(GOOD).validate(d.path());
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            !tpd.exists(),
            "validate() must not create third_party_dir or its ancestors"
        );
        assert!(!tpd.join("js").exists());
    }

    #[test]
    fn warns_on_single_platform_and_unpinned_registry() {
        let d = tempdir_with_lockfile();
        let c = cfg(
            "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"linux\"\ncpu=\"x64\"\n[fixups]\nregistry=\"github.com/o/r\"\n",
        );
        let (errors, warnings) = c.validate(d.path());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }
}
