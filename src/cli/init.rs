//! `pudu init` — detect a pnpm workspace and scaffold a pudu project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::Platform;
use crate::platform::{Cpu, Libc, Os};

/// What an upward walk from the invocation directory found.
pub struct Detected {
    pub lockfile: PathBuf,
    pub workspace_yaml: Option<PathBuf>,
}

/// Walk upward from `start` looking for `pnpm-lock.yaml`.
///
/// The lockfile is the anchor rather than `package.json`: it is pudu's actual
/// input, and a repo holds many manifests but one lockfile per workspace.
pub fn detect(start: &Path) -> Option<Detected> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let lockfile = d.join("pnpm-lock.yaml");
        if lockfile.is_file() {
            let ws = d.join("pnpm-workspace.yaml");
            return Some(Detected {
                lockfile,
                workspace_yaml: ws.is_file().then_some(ws),
            });
        }
        dir = d.parent();
    }
    None
}

#[derive(Debug)]
pub struct DerivedPlatforms {
    pub platforms: BTreeMap<String, Platform>,
    pub warnings: Vec<String>,
}

#[derive(Deserialize)]
struct WorkspaceYaml {
    #[serde(rename = "supportedArchitectures")]
    supported_architectures: Option<serde_norway::Value>,
}

/// Generated platform name: `linux-x64-gnu`, `darwin-arm64`.
pub fn platform_name(os: Os, cpu: Cpu, libc: Option<Libc>) -> String {
    match libc {
        Some(l) => format!("{}-{}-{}", os.as_npm(), cpu.as_npm(), l.short()),
        None => format!("{}-{}", os.as_npm(), cpu.as_npm()),
    }
}

fn host_os() -> Os {
    if cfg!(target_os = "macos") {
        Os::Darwin
    } else {
        Os::Linux
    }
}

fn host_cpu() -> Cpu {
    if cfg!(target_arch = "aarch64") {
        Cpu::Arm64
    } else {
        Cpu::X64
    }
}

/// Read a `supportedArchitectures` axis into a list of strings.
fn axis(v: &serde_norway::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_sequence())
        .map(|s| {
            s.iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether an axis key was present in the yaml at all (as opposed to
/// absent, which falls back to the host value).
fn axis_present(v: &serde_norway::Value, key: &str) -> bool {
    v.get(key).is_some()
}

/// Expand `supportedArchitectures` into a platform matrix, or return the
/// default set when it is absent.
///
/// Rules (spec §3.2): cross-product os × cpu; `libc` applies only to linux;
/// `win32` is skipped with a warning; `current` resolves to the host.
pub fn derive_platforms(workspace_yaml: Option<&str>) -> Result<DerivedPlatforms, String> {
    let mut warnings = Vec::new();

    let sa = match workspace_yaml {
        None => None,
        Some(text) => {
            serde_norway::from_str::<WorkspaceYaml>(text)
                .map_err(|e| format!("cannot parse pnpm-workspace.yaml: {e}"))?
                .supported_architectures
        }
    };

    let Some(sa) = sa else {
        return Ok(DerivedPlatforms {
            platforms: default_platforms(),
            warnings,
        });
    };

    if let Some(map) = sa.as_mapping() {
        for k in map.keys().filter_map(|k| k.as_str()) {
            if !matches!(k, "os" | "cpu" | "libc") {
                warnings.push(format!(
                    "pnpm-workspace.yaml: ignoring unrecognized supportedArchitectures key `{k}`"
                ));
            }
        }
    }

    let mut oses = Vec::new();
    for raw in axis(&sa, "os") {
        match raw.as_str() {
            "current" => oses.push(host_os()),
            "linux" => oses.push(Os::Linux),
            "darwin" => oses.push(Os::Darwin),
            "win32" => warnings.push(
                "pnpm-workspace.yaml: skipping `win32` — Windows is a Phase 2 deliverable, \
                 see docs/superpowers/specs/2026-08-30-pudu-roadmap.md"
                    .to_string(),
            ),
            other => warnings.push(format!(
                "pnpm-workspace.yaml: ignoring unknown os `{other}`"
            )),
        }
    }

    let mut cpus = Vec::new();
    for raw in axis(&sa, "cpu") {
        match raw.as_str() {
            "current" => cpus.push(host_cpu()),
            "x64" => cpus.push(Cpu::X64),
            "arm64" => cpus.push(Cpu::Arm64),
            other => warnings.push(format!(
                "pnpm-workspace.yaml: ignoring unknown cpu `{other}`"
            )),
        }
    }

    let mut libcs = Vec::new();
    for raw in axis(&sa, "libc") {
        match raw.as_str() {
            "current" | "glibc" => libcs.push(Libc::Glibc),
            "musl" => libcs.push(Libc::Musl),
            other => warnings.push(format!(
                "pnpm-workspace.yaml: ignoring unknown libc `{other}`"
            )),
        }
    }

    // Only fall back to the host/default when the axis key was absent
    // entirely. When the key was present but every entry was filtered out
    // (e.g. `os: [win32]`), the axis stays empty so the cross-product below
    // yields no platforms and this surfaces as an error rather than
    // silently substituting the host.
    if oses.is_empty() && !axis_present(&sa, "os") {
        oses.push(host_os());
    }
    if cpus.is_empty() && !axis_present(&sa, "cpu") {
        cpus.push(host_cpu());
    }
    if libcs.is_empty() && !axis_present(&sa, "libc") {
        libcs.push(Libc::Glibc);
    }

    oses.sort();
    oses.dedup();
    cpus.sort();
    cpus.dedup();
    libcs.sort();
    libcs.dedup();

    let mut platforms = BTreeMap::new();
    for os in &oses {
        for cpu in &cpus {
            // `libc` is meaningless off linux — a darwin-musl platform must
            // never be emitted.
            if *os == Os::Linux {
                for libc in &libcs {
                    platforms.insert(
                        platform_name(*os, *cpu, Some(*libc)),
                        Platform {
                            os: *os,
                            cpu: *cpu,
                            libc: Some(*libc),
                            constraints: None,
                        },
                    );
                }
            } else {
                platforms.insert(
                    platform_name(*os, *cpu, None),
                    Platform {
                        os: *os,
                        cpu: *cpu,
                        libc: None,
                        constraints: None,
                    },
                );
            }
        }
    }

    if platforms.is_empty() {
        let empty_axis = if oses.is_empty() {
            "os"
        } else if cpus.is_empty() {
            "cpu"
        } else {
            "libc"
        };
        let mut msg = format!(
            "pnpm-workspace.yaml declares no supported platforms pudu can target \
             (the `{empty_axis}` axis resolved to nothing usable); edit supportedArchitectures or remove it"
        );
        if !warnings.is_empty() {
            msg.push_str("\n\nwarnings encountered while expanding supportedArchitectures:");
            for w in &warnings {
                msg.push_str("\n  - ");
                msg.push_str(w);
            }
        }
        return Err(msg);
    }

    Ok(DerivedPlatforms {
        platforms,
        warnings,
    })
}

fn default_platforms() -> BTreeMap<String, Platform> {
    let mut m = BTreeMap::new();
    for (os, cpu, libc) in [
        (Os::Linux, Cpu::X64, Some(Libc::Glibc)),
        (Os::Linux, Cpu::Arm64, Some(Libc::Glibc)),
        (Os::Darwin, Cpu::Arm64, None),
    ] {
        m.insert(
            platform_name(os, cpu, libc),
            Platform {
                os,
                cpu,
                libc,
                constraints: None,
            },
        );
    }
    m
}

use crate::cli::toolchain::{self, AppendOutcome};

const TOOLCHAINS_BZL: &str = r#"##
## @generated by pudu init. Safe to edit.
##
## The Buck2 prelude ships no Node toolchain, so pudu defines one. Swap the
## `node` attribute for an absolute path, or replace this rule entirely, to
## use a hermetic Node instead of whatever is on PATH.
##

NodeToolchainInfo = provider(fields = {"node": provider_field(typing.Any, default = None)})

def _system_node_toolchain_impl(ctx):
    return [
        DefaultInfo(),
        NodeToolchainInfo(node = RunInfo(args = [ctx.attrs.node])),
    ]

system_node_toolchain = rule(
    impl = _system_node_toolchain_impl,
    attrs = {"node": attrs.string(default = "node")},
    is_toolchain_rule = True,
)
"#;

const THIRD_PARTY_GITIGNORE: &str = "# Populated by `pudu vendor` in vendor mode (v2).\nvendor/\n";

const PLACEHOLDER_BUCK: &str = "# Generated by pudu. Run: pudu buckify\n";

fn render_config(
    lockfile_path: &str,
    platforms: &BTreeMap<String, Platform>,
    detected: bool,
) -> String {
    let mut s = String::new();
    s.push_str("# Generated by `pudu init`. Edit freely.\n");
    s.push_str("# Full schema: docs/superpowers/specs/2026-08-30-pudu-design.md\n");
    if !detected {
        s.push_str("\n# TODO: pudu init could not find a pnpm-lock.yaml.\n");
        s.push_str("# Edit `lockfile_path`, then run `pudu config check`.\n");
    }
    s.push('\n');
    s.push_str(&format!("lockfile_path   = \"{lockfile_path}\"\n"));
    s.push_str("third_party_dir = \"third-party/js\"\n\n");

    for (name, p) in platforms {
        s.push_str(&format!("[platforms.{name}]\n"));
        s.push_str(&format!("os   = \"{}\"\n", p.os.as_npm()));
        s.push_str(&format!("cpu  = \"{}\"\n", p.cpu.as_npm()));
        if let Some(l) = p.libc {
            s.push_str(&format!("libc = \"{}\"\n", l.as_npm()));
        }
        s.push('\n');
    }

    s.push_str("[registry]\ndefault = \"https://registry.npmjs.org\"\n\n");
    s.push_str("[fixups]\n");
    s.push_str("# Community fixup registry. Leave as \"none\" until a v0.1.0+ release exists.\n");
    s.push_str("registry              = \"none\"\n");
    s.push_str("allow_local_overrides = true\n\n");
    s.push_str("[scripts]\n");
    s.push_str("# Packages whose lifecycle scripts are acknowledged (not run). See design §6.\n");
    s.push_str("allow = []\n\n");
    s.push_str("[buck]\n");
    s.push_str("file_name      = \"BUCK\"\n");
    s.push_str("node_toolchain = \"toolchains//:node\"\n");
    s
}

/// Scaffold a pudu project in `path` (default: the current directory).
pub fn run(force: bool, path: Option<PathBuf>) -> anyhow::Result<()> {
    let root = match path {
        Some(p) => p,
        None => std::env::current_dir()?,
    };

    let config_path = root.join("pudu.toml");
    if config_path.exists() && !force {
        anyhow::bail!(
            "{} already exists; pass --force to overwrite",
            config_path.display()
        );
    }

    let found = detect(&root);
    let lockfile_rel = match &found {
        Some(d) => pathdiff::diff_paths(&d.lockfile, &root)
            .unwrap_or_else(|| d.lockfile.clone())
            .to_string_lossy()
            .replace('\\', "/"),
        None => "TODO: path to your pnpm-lock.yaml".to_string(),
    };

    let ws_text = found
        .as_ref()
        .and_then(|d| d.workspace_yaml.as_ref())
        .map(std::fs::read_to_string)
        .transpose()?;

    let derived = derive_platforms(ws_text.as_deref()).map_err(|e| anyhow::anyhow!(e))?;
    for w in &derived.warnings {
        eprintln!("warning: {w}");
    }

    // pudu.toml
    std::fs::write(
        &config_path,
        render_config(&lockfile_rel, &derived.platforms, found.is_some()),
    )?;
    println!("wrote {}", config_path.display());

    // third-party/js skeleton — never overwrite existing contents.
    let tp = root.join("third-party/js");
    std::fs::create_dir_all(tp.join("fixups"))?;
    for (rel, contents) in [
        ("BUCK", PLACEHOLDER_BUCK),
        ("toolchains.bzl", TOOLCHAINS_BZL),
        (".gitignore", THIRD_PARTY_GITIGNORE),
        ("fixups/.gitkeep", ""),
    ] {
        let p = tp.join(rel);
        if p.exists() && !force {
            eprintln!("warning: {} exists; leaving it alone", p.display());
            continue;
        }
        std::fs::write(&p, contents)?;
    }
    println!("wrote {}", tp.display());

    // toolchains/BUCK — the one user-owned file pudu writes into.
    let tc_dir = root.join("toolchains");
    let tc_path = tc_dir.join("BUCK");
    let existing = match std::fs::read_to_string(&tc_path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let block = toolchain::managed_block("root//third-party/js:toolchains.bzl");
    let (written, outcome) = toolchain::apply(existing.as_deref(), &block, force);
    if let Some(text) = written {
        std::fs::create_dir_all(&tc_dir)?;
        std::fs::write(&tc_path, text)?;
    }
    match outcome {
        AppendOutcome::Created | AppendOutcome::Appended | AppendOutcome::Replaced => {
            println!("wrote {}", tc_path.display());
        }
        AppendOutcome::AlreadyManaged => {
            println!("{} already current", tc_path.display());
        }
        AppendOutcome::ExistingToolchain(name) => {
            println!(
                "{} already declares a node toolchain (`:{name}`); leaving it alone.\n\
                 If that is not the toolchain you want pudu to use, set \
                 `[buck] node_toolchain` in pudu.toml.",
                tc_path.display()
            );
        }
        AppendOutcome::Unparseable => {
            println!(
                "{} has unbalanced pudu markers; not modifying it. Add this manually:\n\n{block}",
                tc_path.display()
            );
        }
    }

    println!("\nNext: pudu vendor && pudu buckify");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lockfile_from_a_subdirectory() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        let nested = d.path().join("packages/server");
        std::fs::create_dir_all(&nested).unwrap();

        let found = detect(&nested).expect("walks upward to the lockfile");
        assert_eq!(found.lockfile, d.path().join("pnpm-lock.yaml"));
        assert!(found.workspace_yaml.is_none());
    }

    #[test]
    fn detect_returns_none_without_a_lockfile() {
        let d = tempfile::tempdir().unwrap();
        assert!(detect(d.path()).is_none());
    }

    #[test]
    fn detect_finds_a_sibling_workspace_yaml() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        std::fs::write(
            d.path().join("pnpm-workspace.yaml"),
            "packages:\n  - packages/*\n",
        )
        .unwrap();
        let nested = d.path().join("packages/server");
        std::fs::create_dir_all(&nested).unwrap();

        let found = detect(&nested).expect("walks upward to the lockfile");
        assert_eq!(
            found.workspace_yaml,
            Some(d.path().join("pnpm-workspace.yaml"))
        );
    }

    #[test]
    fn default_platform_set_when_no_workspace_yaml() {
        let d = derive_platforms(None).unwrap();
        let names: Vec<&str> = d.platforms.keys().map(String::as_str).collect();
        assert_eq!(names, ["darwin-arm64", "linux-arm64-gnu", "linux-x64-gnu"]);
        assert!(d.warnings.is_empty());
    }

    #[test]
    fn expands_the_os_cpu_cross_product() {
        let yaml = "supportedArchitectures:\n  os: [linux, darwin]\n  cpu: [x64, arm64]\n";
        let d = derive_platforms(Some(yaml)).unwrap();
        let names: Vec<&str> = d.platforms.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "darwin-arm64",
                "darwin-x64",
                "linux-arm64-gnu",
                "linux-x64-gnu"
            ]
        );
    }

    #[test]
    fn libc_applies_only_to_linux() {
        let yaml =
            "supportedArchitectures:\n  os: [linux, darwin]\n  cpu: [arm64]\n  libc: [musl]\n";
        let d = derive_platforms(Some(yaml)).unwrap();
        assert_eq!(d.platforms["darwin-arm64"].libc, None);
        assert_eq!(d.platforms["linux-arm64-musl"].libc, Some(Libc::Musl));
        assert_eq!(
            d.platforms.len(),
            2,
            "no darwin-musl platform may be emitted"
        );
    }

    #[test]
    fn win32_is_skipped_with_a_warning() {
        let yaml = "supportedArchitectures:\n  os: [linux, win32]\n  cpu: [x64]\n";
        let d = derive_platforms(Some(yaml)).unwrap();
        assert_eq!(d.platforms.len(), 1);
        assert!(d.platforms.contains_key("linux-x64-gnu"));
        assert_eq!(d.warnings.len(), 1);
        assert!(d.warnings[0].contains("win32"), "{:?}", d.warnings);
    }

    #[test]
    fn only_win32_is_an_error() {
        let yaml = "supportedArchitectures:\n  os: [win32]\n  cpu: [x64]\n";
        let err = derive_platforms(Some(yaml)).unwrap_err();
        assert!(err.contains("no supported platforms"), "{err}");
    }

    /// The platform name this host resolves to, given a linux-vs-macos
    /// split and an x64-vs-arm64 split — holds on ubuntu-latest,
    /// ubuntu-24.04-arm, and macos-latest without hardcoding any one of
    /// them.
    fn expected_host_platform_name() -> &'static str {
        if cfg!(target_os = "macos") {
            "darwin-arm64"
        } else if cfg!(target_arch = "aarch64") {
            "linux-arm64-gnu"
        } else {
            "linux-x64-gnu"
        }
    }

    #[test]
    fn current_resolves_to_the_host() {
        let yaml = "supportedArchitectures:\n  os: [current]\n  cpu: [current]\n";
        let d = derive_platforms(Some(yaml)).unwrap();
        let names: Vec<&str> = d.platforms.keys().map(String::as_str).collect();
        assert_eq!(names, [expected_host_platform_name()]);
    }

    #[test]
    fn absent_os_and_cpu_axes_fall_back_to_the_host() {
        // Only `libc` is declared; `os`/`cpu` keys are entirely absent, so
        // both must default to the host rather than staying empty.
        let yaml = "supportedArchitectures:\n  libc: [glibc]\n";
        let d = derive_platforms(Some(yaml)).unwrap();
        let names: Vec<&str> = d.platforms.keys().map(String::as_str).collect();
        assert_eq!(names, [expected_host_platform_name()]);
    }

    #[test]
    fn unusable_libc_axis_names_the_axis_and_surfaces_warnings() {
        let yaml = "supportedArchitectures:\n  os: [linux]\n  cpu: [x64]\n  libc: [uclibc]\n";
        let err = derive_platforms(Some(yaml)).unwrap_err();
        assert!(err.contains("libc"), "{err}");
        assert!(err.contains("uclibc"), "{err}");
    }

    #[test]
    fn unknown_keys_warn_rather_than_fail() {
        let yaml = "supportedArchitectures:\n  os: [linux]\n  cpu: [x64]\n  future: [thing]\n";
        let d = derive_platforms(Some(yaml)).unwrap();
        assert_eq!(d.platforms.len(), 1);
        assert!(
            d.warnings.iter().any(|w| w.contains("future")),
            "{:?}",
            d.warnings
        );
    }

    #[test]
    fn absent_supported_architectures_uses_defaults() {
        let d = derive_platforms(Some("packages:\n  - packages/*\n")).unwrap();
        assert_eq!(d.platforms.len(), 3);
    }
}
