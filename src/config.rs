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
}
