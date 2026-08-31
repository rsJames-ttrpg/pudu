//! `pudu init` — detect a pnpm workspace and scaffold a pudu project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

use crate::cli::toolchain::{self, AppendOutcome};
use crate::config::Platform;
use crate::error::{DeriveError, DeriveWarning, InitWarning, render};
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
    pub warnings: Vec<DeriveWarning>,
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
///
/// Reports rather than silently discarding: a non-string entry
/// (`cpu: [123]`) is warned about (TD-S0-09), and a bare scalar
/// (`os: linux` instead of `os: [linux]`) is a hard error (TD-S0-08) — see
/// [`DeriveError::AxisNotASequence`] for why that one can't just warn.
fn axis(
    v: &serde_norway::Value,
    key: &str,
    warnings: &mut Vec<DeriveWarning>,
) -> Result<Vec<String>, DeriveError> {
    let Some(entry) = v.get(key) else {
        return Ok(Vec::new());
    };

    let Some(seq) = entry.as_sequence() else {
        return Err(DeriveError::AxisNotASequence {
            key: key.to_string(),
            example: match key {
                "os" => "linux",
                "cpu" => "x64",
                _ => "glibc",
            }
            .to_string(),
        });
    };

    let mut out = Vec::with_capacity(seq.len());
    let mut reported = false;
    for item in seq {
        match item.as_str() {
            Some(s) => out.push(s.to_string()),
            None if !reported => {
                // Report once per axis: a list of ten bad entries is one
                // mistake, not ten.
                reported = true;
                warnings.push(DeriveWarning::NonStringAxisEntry {
                    key: key.to_string(),
                });
            }
            None => {}
        }
    }
    Ok(out)
}

/// Whether an axis key was present in the yaml at all (as opposed to
/// absent, which falls back to the host value).
///
/// A malformed axis (a bare scalar like `os: linux`) never reaches this
/// point: [`axis`] returns [`DeriveError::AxisNotASequence`] for it, which
/// `derive_platforms` propagates immediately. So by the time this is
/// consulted, a present key is always a sequence.
fn axis_present(v: &serde_norway::Value, key: &str) -> bool {
    v.get(key).is_some()
}

/// Expand `supportedArchitectures` into a platform matrix, or return the
/// default set when it is absent.
///
/// Rules (spec §3.2): cross-product os × cpu; `libc` applies only to linux;
/// `win32` is skipped with a warning; `current` resolves to the host.
pub fn derive_platforms(workspace_yaml: Option<&str>) -> Result<DerivedPlatforms, DeriveError> {
    let mut warnings = Vec::new();

    let sa = match workspace_yaml {
        None => None,
        Some(text) => {
            serde_norway::from_str::<WorkspaceYaml>(text)
                .map_err(|source| DeriveError::WorkspaceParse {
                    path: PathBuf::from("pnpm-workspace.yaml"),
                    source,
                })?
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
                warnings.push(DeriveWarning::UnknownKey { key: k.to_string() });
            }
        }
    } else {
        warnings.push(DeriveWarning::SupportedArchitecturesNotAMapping);
    }

    let mut oses = Vec::new();
    for raw in axis(&sa, "os", &mut warnings)? {
        match raw.as_str() {
            "current" => oses.push(host_os()),
            "linux" => oses.push(Os::Linux),
            "darwin" => oses.push(Os::Darwin),
            "win32" => warnings.push(DeriveWarning::Win32Skipped),
            other => warnings.push(DeriveWarning::UnknownOs {
                value: other.to_string(),
            }),
        }
    }

    let mut cpus = Vec::new();
    for raw in axis(&sa, "cpu", &mut warnings)? {
        match raw.as_str() {
            "current" => cpus.push(host_cpu()),
            "x64" => cpus.push(Cpu::X64),
            "arm64" => cpus.push(Cpu::Arm64),
            other => warnings.push(DeriveWarning::UnknownCpu {
                value: other.to_string(),
            }),
        }
    }

    let mut libcs = Vec::new();
    for raw in axis(&sa, "libc", &mut warnings)? {
        match raw.as_str() {
            "current" | "glibc" => libcs.push(Libc::Glibc),
            "musl" => libcs.push(Libc::Musl),
            other => warnings.push(DeriveWarning::UnknownLibc {
                value: other.to_string(),
            }),
        }
    }

    // Only fall back to the host/default when the axis key was absent
    // entirely. A malformed axis (not a sequence) already returned
    // `DeriveError::AxisNotASequence` above. When the key was present as a
    // valid sequence but every entry was filtered out (e.g. `os: [win32]`),
    // the axis stays empty so the cross-product below yields no platforms
    // and this surfaces as an error rather than silently substituting the
    // host.
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
        // The warnings ride along on the error: a user told "no usable
        // platforms" needs to know *why* each candidate was dropped. They are
        // carried as `#[related]` data rather than concatenated into the
        // message, so tests can assert on them.
        return Err(DeriveError::NoUsablePlatforms {
            axis: empty_axis,
            warnings,
        });
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

/// Render the starter `pudu.toml`.
///
/// Default values are taken from the `default_*` helpers in [`crate::config`]
/// rather than restated as literals, so the generated file cannot drift from
/// what pudu itself assumes when a key is omitted. `tests::generated_config_matches_config_defaults`
/// pins the ones that must still be written out by hand.
fn render_config(
    lockfile_path: &str,
    third_party_dir: &str,
    node_toolchain: &str,
    platforms: &BTreeMap<String, Platform>,
    detected: bool,
) -> String {
    let mut s = String::new();
    s.push_str("# Generated by `pudu init`. Edit freely.\n");
    s.push_str("# Full schema: https://github.com/rsJames-ttrpg/pudu/blob/main/docs/superpowers/specs/2026-08-30-pudu-design.md\n");
    if !detected {
        s.push_str("\n# TODO: pudu init could not find a pnpm-lock.yaml.\n");
        s.push_str("# Edit `lockfile_path`, then run `pudu config check`.\n");
    }
    s.push('\n');
    s.push_str(&format!("lockfile_path   = \"{lockfile_path}\"\n"));
    s.push_str(&format!("third_party_dir = \"{third_party_dir}\"\n\n"));

    for (name, p) in platforms {
        s.push_str(&format!("[platforms.{name}]\n"));
        s.push_str(&format!("os   = \"{}\"\n", p.os.as_npm()));
        s.push_str(&format!("cpu  = \"{}\"\n", p.cpu.as_npm()));
        if let Some(l) = p.libc {
            s.push_str(&format!("libc = \"{}\"\n", l.as_npm()));
        }
        s.push('\n');
    }

    // `Url` normalizes a bare authority to a trailing slash; the generated
    // file keeps the friendlier spelling, which parses back to the same URL.
    let registry = crate::config::default_registry_url();
    let registry = registry.as_str().trim_end_matches('/');
    s.push_str(&format!("[registry]\ndefault = \"{registry}\"\n\n"));
    s.push_str("[fixups]\n");
    s.push_str("# Community fixup registry. Leave as \"none\" until a v0.1.0+ release exists.\n");
    s.push_str("registry              = \"none\"\n");
    s.push_str("allow_local_overrides = true\n\n");
    s.push_str("[scripts]\n");
    s.push_str("# Packages whose lifecycle scripts are acknowledged (not run). See design §6.\n");
    s.push_str("allow = []\n\n");
    s.push_str("[buck]\n");
    s.push_str(&format!(
        "file_name      = \"{}\"\n",
        crate::config::default_file_name()
    ));
    s.push_str(&format!("node_toolchain = \"{node_toolchain}\"\n"));
    s
}

/// Render a path as a Buck-style forward-slashed relative path.
fn slashed(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Scaffold a pudu project in `path` (default: the current directory).
pub fn run(force: bool, path: Option<PathBuf>) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    // Absolutize `root` before detecting: `detect`'s upward walk terminates
    // at an empty parent, so a relative `path` (including `.`) cannot ascend
    // above the cwd and would silently fail to find a lockfile that a bare
    // `pudu init` (which starts from the absolute cwd) would find fine.
    // Joining an already-absolute `path` onto `cwd` correctly yields `path`
    // unchanged, so this is safe for both cases.
    let root = match path {
        Some(p) => cwd.join(p),
        None => cwd,
    };

    // M6: without this, the first write fails with a bare
    // "No such file or directory" naming pudu.toml rather than the missing
    // directory. `pudu init <new-dir>` creating that directory matches
    // `git init <dir>`.
    if !root.is_dir() {
        std::fs::create_dir_all(&root)
            .with_context(|| format!("cannot create directory {}", root.display()))?;
    }

    let config_path = root.join("pudu.toml");
    if config_path.exists() && !force {
        return Err(crate::error::CliError::ConfigExists { path: config_path }.into());
    }

    let found = detect(&root);
    let lockfile_rel = match &found {
        Some(d) => {
            slashed(&pathdiff::diff_paths(&d.lockfile, &root).unwrap_or_else(|| d.lockfile.clone()))
        }
        None => "TODO: path to your pnpm-lock.yaml".to_string(),
    };

    let ws_text = found
        .as_ref()
        .and_then(|d| d.workspace_yaml.as_ref())
        .map(|p| std::fs::read_to_string(p).with_context(|| format!("cannot read {}", p.display())))
        .transpose()?;

    let derived = derive_platforms(ws_text.as_deref())?;
    // Through the shared renderer, so a `DeriveWarning` looks the same here
    // as it does when it rides along on `DeriveError::NoUsablePlatforms`.
    for w in &derived.warnings {
        eprint!("{}", render(w));
    }

    let third_party_dir = crate::config::default_third_party_dir();
    let third_party_rel = slashed(&third_party_dir);

    // The `root//` cell in the load label is anchored at the Buck cell root,
    // which pudu cannot see. The lockfile directory is the best available
    // proxy; when init's root sits below it, the label needs that prefix, and
    // the guess is worth saying out loud (I8).
    let lockfile_dir = found.as_ref().and_then(|d| d.lockfile.parent());
    let cell_prefix = match lockfile_dir {
        Some(dir) if dir != root => {
            let rel = pathdiff::diff_paths(&root, dir).unwrap_or_default();
            if rel.as_os_str().is_empty() || rel.starts_with("..") {
                None
            } else {
                eprint!(
                    "{}",
                    render(&InitWarning::CellRootGuess {
                        init_root: root.clone(),
                        lockfile_dir: dir.to_path_buf(),
                    })
                );
                Some(slashed(&rel))
            }
        }
        _ => None,
    };
    let bzl_label = match &cell_prefix {
        Some(prefix) => format!("root//{prefix}/{third_party_rel}:toolchains.bzl"),
        None => format!("root//{third_party_rel}:toolchains.bzl"),
    };

    // toolchains/BUCK — the one user-owned file pudu writes into. `--force`
    // does apply here, gating whether an existing pudu-managed block gets
    // refreshed; see toolchain::apply.
    //
    // Computed *before* pudu.toml is written, because the outcome decides
    // which `[buck] node_toolchain` label the config must record (I1); the
    // write itself is deferred so the "wrote ..." lines stay in file order.
    let tc_dir = root.join("toolchains");
    let tc_path = tc_dir.join("BUCK");
    let existing = match std::fs::read_to_string(&tc_path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(e).with_context(|| format!("cannot read {}", tc_path.display()));
        }
    };
    let block = toolchain::managed_block(&bzl_label);
    let (tc_written, outcome) = toolchain::apply(existing.as_deref(), &block, force);

    let node_toolchain = match &outcome {
        AppendOutcome::ExistingToolchain { name, .. } => format!("toolchains//:{name}"),
        _ => crate::config::default_node_toolchain(),
    };

    // pudu.toml
    std::fs::write(
        &config_path,
        render_config(
            &lockfile_rel,
            &third_party_rel,
            &node_toolchain,
            &derived.platforms,
            found.is_some(),
        ),
    )
    .with_context(|| format!("cannot write {}", config_path.display()))?;
    println!("wrote {}", config_path.display());

    // third-party/js skeleton. `--force` governs pudu.toml and the
    // toolchains/BUCK managed block only (spec, commit f4c5b0c): files under
    // third-party/js/ are user-owned once they exist — pudu's own generated
    // toolchains.bzl says "Safe to edit" — so they are never overwritten,
    // `--force` or not.
    let tp = root.join(&third_party_dir);
    std::fs::create_dir_all(tp.join("fixups"))
        .with_context(|| format!("cannot create {}", tp.join("fixups").display()))?;
    let mut wrote_any = false;
    for (rel, contents) in [
        ("BUCK", PLACEHOLDER_BUCK),
        ("toolchains.bzl", TOOLCHAINS_BZL),
        (".gitignore", THIRD_PARTY_GITIGNORE),
        ("fixups/.gitkeep", ""),
    ] {
        let p = tp.join(rel);
        if p.exists() {
            eprint!(
                "{}",
                render(&InitWarning::ThirdPartyFileExists { path: p.clone() })
            );
            continue;
        }
        std::fs::write(&p, contents).with_context(|| format!("cannot write {}", p.display()))?;
        wrote_any = true;
    }
    // M4: every file was already there, so claiming a write would be a lie.
    if wrote_any {
        println!("wrote {}", tp.display());
    }

    let mut next_steps = "pudu vendor && pudu buckify";
    if let Some(text) = tc_written {
        std::fs::create_dir_all(&tc_dir)
            .with_context(|| format!("cannot create {}", tc_dir.display()))?;
        std::fs::write(&tc_path, text)
            .with_context(|| format!("cannot write {}", tc_path.display()))?;
    }
    match outcome {
        AppendOutcome::Created | AppendOutcome::Appended | AppendOutcome::Replaced => {
            println!("wrote {}", tc_path.display());
        }
        AppendOutcome::AlreadyManaged => {
            println!(
                "{} already has a pudu-managed block (pass --force to refresh)",
                tc_path.display()
            );
        }
        AppendOutcome::ExistingToolchain { name, parsed } => {
            eprint!(
                "{}",
                render(&InitWarning::ExistingToolchain {
                    path: tc_path.clone(),
                    name: name.clone(),
                    recorded: node_toolchain.clone(),
                })
            );
            if !parsed {
                eprint!(
                    "{}",
                    render(&InitWarning::ToolchainNameUnparsed {
                        path: tc_path.clone(),
                        name: name.clone(),
                    })
                );
            }
            next_steps =
                "check `[buck] node_toolchain` in pudu.toml, then pudu vendor && pudu buckify";
        }
        AppendOutcome::Unparseable => {
            eprint!(
                "{}",
                render(&InitWarning::UnbalancedMarkers {
                    path: tc_path.clone(),
                })
            );
            // The block itself is content to copy, not a diagnostic, so it is
            // printed plainly under the rendered warning.
            eprintln!("\n{block}");
            next_steps = "add the block above to toolchains/BUCK, then pudu vendor && pudu buckify";
        }
    }

    if found.is_none() {
        println!("\nNext: edit `lockfile_path` in pudu.toml, then pudu config check");
    } else {
        println!("\nNext: {next_steps}");
    }
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
        assert_eq!(d.warnings, vec![DeriveWarning::Win32Skipped]);
    }

    #[test]
    fn only_win32_is_an_error() {
        let yaml = "supportedArchitectures:\n  os: [win32]\n  cpu: [x64]\n";
        let err = derive_platforms(Some(yaml)).unwrap_err();
        assert!(
            matches!(
                err,
                DeriveError::NoUsablePlatforms { axis: "os", ref warnings }
                    if warnings == &[DeriveWarning::Win32Skipped]
            ),
            "{err:?}"
        );
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
        // The axis is named, and the warnings explaining *why* it emptied
        // ride along on the error rather than being dropped.
        assert!(
            matches!(
                err,
                DeriveError::NoUsablePlatforms { axis: "libc", ref warnings }
                    if warnings == &[DeriveWarning::UnknownLibc { value: "uclibc".to_string() }]
            ),
            "{err:?}"
        );
    }

    #[test]
    fn unknown_keys_warn_rather_than_fail() {
        let yaml = "supportedArchitectures:\n  os: [linux]\n  cpu: [x64]\n  future: [thing]\n";
        let d = derive_platforms(Some(yaml)).unwrap();
        assert_eq!(d.platforms.len(), 1);
        assert_eq!(
            d.warnings,
            vec![DeriveWarning::UnknownKey {
                key: "future".to_string()
            }]
        );
    }

    /// I7: `render_config` and `config.rs` are two hand-maintained
    /// representations of the same defaults. Parse what init writes and
    /// assert, field by field, that it agrees with the defaults `config.rs`
    /// applies to a minimal config — so renaming a default in one place
    /// cannot silently leave the other behind.
    #[test]
    fn generated_config_matches_config_defaults() {
        use crate::config::Config;

        let defaults = Config::from_str(
            "lockfile_path = \"pnpm-lock.yaml\"\n[platforms.linux-x64-gnu]\nos = \"linux\"\ncpu = \"x64\"\nlibc = \"glibc\"\n",
            Path::new("pudu.toml"),
        )
        .unwrap();

        let rendered = render_config(
            "pnpm-lock.yaml",
            &slashed(&crate::config::default_third_party_dir()),
            &crate::config::default_node_toolchain(),
            &defaults.platforms,
            true,
        );
        let generated = Config::from_str(&rendered, Path::new("pudu.toml")).unwrap();

        assert_eq!(generated.lockfile_path, defaults.lockfile_path);
        assert_eq!(generated.third_party_dir, defaults.third_party_dir);
        assert_eq!(generated.registry.default, defaults.registry.default);
        assert_eq!(generated.registry.scopes, defaults.registry.scopes);
        assert_eq!(generated.fixups.registry, defaults.fixups.registry);
        assert_eq!(generated.fixups.registry_rev, defaults.fixups.registry_rev);
        assert_eq!(
            generated.fixups.allow_local_overrides,
            defaults.fixups.allow_local_overrides
        );
        assert_eq!(generated.scripts.allow, defaults.scripts.allow);
        assert_eq!(generated.buck.file_name, defaults.buck.file_name);
        assert_eq!(generated.buck.node_toolchain, defaults.buck.node_toolchain);
        assert_eq!(generated.platforms, defaults.platforms);
        // The whole struct, so a field added later is covered without
        // anyone remembering to extend the list above.
        assert_eq!(generated, defaults);
    }

    #[test]
    fn absent_supported_architectures_uses_defaults() {
        let d = derive_platforms(Some("packages:\n  - packages/*\n")).unwrap();
        assert_eq!(d.platforms.len(), 3);
    }

    /// TD-S0-08: `os: linux` (a bare scalar, not a sequence) is a plausible
    /// typo, and a strictly more likely one than `os: [win32]` — which
    /// already hard-errors as `NoUsablePlatforms`. Letting the bare-scalar
    /// case merely warn and silently fall back to the host value would be
    /// the more forgiving outcome for the more likely mistake, and that
    /// fallback gets written into `pudu.toml` where it stops looking like a
    /// warning at all. So this is a hard error naming the axis, not a
    /// warning.
    #[test]
    fn a_non_sequence_axis_is_a_hard_error_naming_the_axis() {
        let yaml = "supportedArchitectures:\n  os: linux\n  cpu: [x64]\n";
        let err = derive_platforms(Some(yaml)).expect_err("must be fatal");
        assert!(
            matches!(&err, DeriveError::AxisNotASequence { key, .. } if key == "os"),
            "error: {err:?}"
        );
    }

    /// TD-S0-08: a non-mapping `supportedArchitectures` was ignored in
    /// silence, which is the worst outcome — the user's intent vanishes.
    #[test]
    fn a_non_mapping_supported_architectures_warns() {
        let yaml = "supportedArchitectures: linux\n";
        let d = derive_platforms(Some(yaml)).expect("must not be fatal");
        assert!(
            d.warnings
                .iter()
                .any(|w| matches!(w, DeriveWarning::SupportedArchitecturesNotAMapping)),
            "warnings: {:?}",
            d.warnings
        );
    }

    /// TD-S0-09: a non-string entry was dropped in silence.
    #[test]
    fn a_non_string_axis_entry_warns_rather_than_vanishing() {
        let yaml = "supportedArchitectures:\n  os: [linux]\n  cpu: [123, x64]\n";
        let d = derive_platforms(Some(yaml)).expect("must not be fatal");
        assert!(
            d.warnings.iter().any(|w| matches!(
                w,
                DeriveWarning::NonStringAxisEntry { key, .. } if key == "cpu"
            )),
            "warnings: {:?}",
            d.warnings
        );
    }

    /// TD-S0-09: the unknown-`cpu` arm had no test at all. `os` and `libc`
    /// were already covered; this closes the gap.
    #[test]
    fn an_unknown_cpu_value_warns() {
        let yaml = "supportedArchitectures:\n  os: [linux]\n  cpu: [ppc64, x64]\n";
        let d = derive_platforms(Some(yaml)).expect("must not be fatal");
        assert!(
            d.warnings.iter().any(|w| matches!(
                w,
                DeriveWarning::UnknownCpu { value } if value == "ppc64"
            )),
            "warnings: {:?}",
            d.warnings
        );
    }
}
