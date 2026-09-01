//! `pudu vendor` — the download pass and the `pudu.lock` sidecar.
//!
//! Vendors the union of packages surviving S2's pruning on at least one
//! configured platform. That makes `pudu.lock` a function of `pudu.toml` as
//! well as of the lockfile: adding a platform makes the sidecar stale, and
//! `--check` catches it. Intended, not incidental — a config change genuinely
//! changes which tarballs the build needs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};

use crate::cache::Cache;
use crate::cli::context::load_validated;
use crate::error::{VendorError, VendorWarning, render};
use crate::fetch::{Fetcher, Request};
use crate::lock::Graph;
use crate::lock::types::Resolution;
use crate::packages::{self, Entry, Expected, Loaded, PackageTable};
use crate::platform::prune::prune;
use crate::registry::tarball_url;

/// Everything the download pass needs, computed with no network at all.
#[derive(Debug)]
struct Plan {
    expected: BTreeMap<String, Expected>,
    requests: BTreeMap<String, Request>,
    /// `hasBin` as the lockfile records it, for the cross-check.
    has_bin: BTreeMap<String, bool>,
}

pub fn run(check: bool, jobs: usize, no_network: bool, verbose: bool) -> Result<()> {
    // No empty-platforms guard here: `load_validated()` runs
    // `Config::validate`, which already rejects an empty `[platforms]` table
    // with `ConfigError::NoPlatforms` (exit 3). A second, unreachable check
    // was only a second thing to keep in step.
    let (config, lockfile) = load_validated()?;

    let graph = Graph::build(&lockfile)?;
    let (matrix, warnings) = prune(&graph, &config.platforms);
    for w in &warnings {
        eprint!("{}", render(w));
    }

    let plan = build_plan(&graph, &matrix, &config)?;

    let base = std::env::current_dir()?;
    let table_path = base.join(&config.third_party_dir).join("pudu.lock");
    let loaded = packages::load(&table_path)?;

    if check {
        return run_check(&plan, &loaded);
    }
    run_vendor(plan, loaded, &table_path, jobs, no_network, verbose)
}

/// Resolve every surviving package to a URL and an integrity, with no
/// network access. `--check` needs exactly this and nothing more.
fn build_plan(
    graph: &Graph,
    matrix: &crate::platform::prune::Matrix,
    config: &crate::config::Config,
) -> Result<Plan> {
    let mut plan = Plan {
        expected: BTreeMap::new(),
        requests: BTreeMap::new(),
        has_bin: BTreeMap::new(),
    };
    // Sorted, so the reported list is stable run to run.
    let mut unsupported: BTreeSet<String> = BTreeSet::new();

    for snapshot_key in matrix.platforms_by_node.keys() {
        let node = &graph.nodes[snapshot_key];
        let key = format!("{}@{}", node.name, node.version);
        // Peer instances of one package share a tarball, so the first
        // snapshot key to reach a given name@version settles it.
        if plan.expected.contains_key(&key) {
            continue;
        }

        let (url, integrity) = match &node.meta.resolution {
            Resolution::Integrity { integrity } => (
                tarball_url(&node.name, &node.version, &config.registry)?.to_string(),
                integrity.clone(),
            ),
            // The private-registry shape: an absolute URL that pnpm recorded,
            // with a hash to check it against. Fetched verbatim.
            Resolution::Tarball {
                tarball,
                integrity: Some(i),
            } => (tarball.clone(), i.clone()),
            // The `github:` shape: no hash exists for these bytes anywhere.
            Resolution::Tarball { .. } => {
                unsupported.insert(format!("{key} (url, no integrity)"));
                continue;
            }
            Resolution::Git { .. } => {
                unsupported.insert(format!("{key} (git)"));
                continue;
            }
            Resolution::Directory { .. } => {
                unsupported.insert(format!("{key} (directory)"));
                continue;
            }
        };

        plan.expected.insert(
            key.clone(),
            Expected {
                url: url.clone(),
                sha512: integrity.clone(),
            },
        );
        plan.has_bin.insert(key.clone(), node.meta.has_bin);
        plan.requests.insert(
            key.clone(),
            Request {
                key,
                name: node.name.clone(),
                url,
                integrity,
            },
        );
    }

    // Reported together rather than raced, so a repo with four git
    // dependencies learns about all four in one run.
    if !unsupported.is_empty() {
        return Err(VendorError::UnsupportedResolution {
            packages: unsupported.into_iter().collect(),
        }
        .into());
    }
    Ok(plan)
}

/// Whether an existing sidecar entry already reflects `req`, so no fetch is
/// needed.
///
/// Both fields must match. The URL alone is not enough: a package can move
/// registries (or an alternate mirror is configured) without pnpm's recorded
/// hash changing, and carrying that over would silently keep serving the old
/// bytes. The hash alone is not enough either — that is the same mistake in
/// the other direction, keeping a stale URL forever merely because the bytes
/// it once fetched happen to coincide with what is wanted now.
fn is_carried_over(existing: Option<&Entry>, req: &Request) -> bool {
    existing.is_some_and(|e| e.url == req.url && e.sha512 == req.integrity)
}

fn run_check(plan: &Plan, loaded: &Loaded) -> Result<()> {
    let differences = packages::staleness(&plan.expected, loaded);
    if differences.is_empty() {
        eprintln!("pudu.lock is up to date ({} packages)", plan.expected.len());
        return Ok(());
    }
    // The error carries the count; the detail goes out here so the user sees
    // every difference, not just how many there were.
    for d in &differences {
        eprintln!("  {d}");
    }
    Err(VendorError::Stale {
        differences: differences.iter().map(ToString::to_string).collect(),
    }
    .into())
}

fn run_vendor(
    plan: Plan,
    loaded: Loaded,
    table_path: &Path,
    jobs: usize,
    no_network: bool,
    verbose: bool,
) -> Result<()> {
    let existing = match &loaded {
        Loaded::Present(s) => s.entries.clone(),
        Loaded::Absent | Loaded::WrongVersion(_) => BTreeMap::new(),
    };

    // Carry over anything already recorded at the same URL and hash. A
    // one-package version bump costs one download. The trade is explicit: a
    // recorded sha256 is never re-checked against upstream once written,
    // which is also what makes pudu.lock an audit artifact.
    let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
    let mut todo: Vec<Request> = Vec::new();
    let mut unchanged = 0usize;
    for (key, req) in plan.requests {
        match existing
            .get(&key)
            .filter(|e| is_carried_over(Some(e), &req))
        {
            Some(e) => {
                entries.insert(key, e.clone());
                unchanged += 1;
            }
            None => todo.push(req),
        }
    }

    let fetcher = Fetcher::new(jobs, no_network, verbose, Cache::open()?);
    let (results, stats) = fetcher.run(todo);

    let mut failures: Vec<VendorError> = Vec::new();
    for (key, outcome) in results {
        match outcome {
            Ok((verified, warnings)) => {
                for w in &warnings {
                    eprint!("{}", render(w));
                }
                let lockfile_says = plan.has_bin[&key];
                let found = verified.inspection.bin.len();
                // A cross-check, never a source of truth: `hasBin` is a flag
                // pnpm derives from registry metadata, and the archive is what
                // the build will actually consume.
                if lockfile_says != (found > 0) {
                    eprint!(
                        "{}",
                        render(&VendorWarning::HasBinDisagreement {
                            key: key.clone(),
                            lockfile: lockfile_says,
                            found,
                        })
                    );
                }
                let want = &plan.expected[&key];
                entries.insert(
                    key,
                    Entry {
                        url: want.url.clone(),
                        sha512: want.sha512.clone(),
                        sha256: verified.sha256,
                        size: verified.size,
                        root: verified.inspection.root,
                        bin: verified.inspection.bin,
                        has_install_script: verified.inspection.has_install_script,
                    },
                );
            }
            Err(e) => failures.push(e),
        }
    }

    if !failures.is_empty() {
        // `main` renders the returned error, so printing it here too would
        // say the same thing twice. Everything after the first is printed
        // here, under a line that says so.
        if failures.len() > 1 {
            eprintln!(
                "{} packages failed; the first is reported at the end, the rest follow:",
                failures.len()
            );
            for e in failures.iter().skip(1) {
                eprint!("{}", render(e));
            }
        }
        return Err(failures.swap_remove(0).into());
    }

    let table = PackageTable { entries };
    write_atomic(table_path, &table.render())?;
    eprintln!(
        "vendored {} packages ({} downloaded, {} cached, {} unchanged)",
        table.entries.len(),
        stats.downloaded,
        stats.cached,
        unchanged
    );
    Ok(())
}

/// Write via a temporary file and rename, so an interrupted run leaves the
/// previous sidecar intact rather than a half-written one.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("creating a temporary file in {}", dir.display()))?;
    std::io::Write::write_all(&mut tmp, text.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    tmp.persist(path)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use url::Url;

    use super::*;
    use crate::config::{
        BuckConfig, Config, FixupRegistry, FixupsConfig, Platform, RegistryConfig, ScriptsConfig,
    };
    use crate::lock::graph::{Graph, Node};
    use crate::lock::types::{PackageMeta, Resolution};
    use crate::platform::prune::Matrix;
    use crate::platform::{Cpu, Os};

    // --- Fixture builders ---------------------------------------------

    fn registry_config(default: &str, scopes: &[(&str, &str)]) -> RegistryConfig {
        RegistryConfig {
            default: Url::parse(default).unwrap(),
            scopes: scopes
                .iter()
                .map(|(k, v)| (k.to_string(), Url::parse(v).unwrap()))
                .collect(),
        }
    }

    /// A minimal but complete `Config`. `build_plan` reads only `.registry`,
    /// so every other field is a placeholder — but a placeholder that type
    /// checks, so a future field added to `Config` fails this file to
    /// compile rather than silently defaulting.
    fn config(registry: RegistryConfig) -> Config {
        Config {
            lockfile_path: "pnpm-lock.yaml".into(),
            third_party_dir: "third-party/node".into(),
            platforms: BTreeMap::from([(
                "linux-x64-gnu".to_string(),
                Platform {
                    os: Os::Linux,
                    cpu: Cpu::X64,
                    libc: None,
                    constraints: None,
                },
            )]),
            registry,
            fixups: FixupsConfig {
                registry: FixupRegistry::None,
                registry_rev: None,
                allow_local_overrides: false,
            },
            scripts: ScriptsConfig::default(),
            buck: BuckConfig {
                file_name: "BUCK".to_string(),
                node_toolchain: "toolchains//:node".to_string(),
            },
        }
    }

    fn meta(resolution: Resolution) -> PackageMeta {
        PackageMeta {
            resolution,
            engines: Default::default(),
            os: None,
            cpu: None,
            libc: None,
            has_bin: false,
            deprecated: None,
            peer_dependencies: Default::default(),
            peer_dependencies_meta: Default::default(),
            bundled_dependencies: Vec::new(),
        }
    }

    fn node(name: &str, version: &str, m: PackageMeta) -> Node {
        Node {
            name: name.into(),
            version: version.into(),
            peers: Vec::new(),
            target_name: format!("{name}@{version}").replace('/', "+"),
            optional: false,
            meta: m,
            edges: Vec::new(),
        }
    }

    fn graph(nodes: Vec<(&str, Node)>) -> Graph {
        Graph {
            nodes: nodes.into_iter().map(|(k, n)| (k.to_string(), n)).collect(),
            roots: Vec::new(),
            cycles: Vec::new(),
        }
    }

    /// A `Matrix` naming exactly `snapshot_keys` as survivors, on one
    /// nominal platform. `build_plan` reads only `platforms_by_node`, so
    /// `views` is left empty — real pruning is S2's own tests' job.
    fn matrix(snapshot_keys: &[&str]) -> Matrix {
        Matrix {
            views: BTreeMap::new(),
            platforms_by_node: snapshot_keys
                .iter()
                .map(|k| (k.to_string(), BTreeSet::from(["linux-x64-gnu".to_string()])))
                .collect(),
        }
    }

    fn integrity(s: &str) -> Resolution {
        Resolution::Integrity {
            integrity: s.to_string(),
        }
    }

    fn as_vendor_error(err: &anyhow::Error) -> &VendorError {
        err.downcast_ref::<VendorError>()
            .unwrap_or_else(|| panic!("expected a VendorError, got: {err:#}"))
    }

    // --- Resolution dispatch --------------------------------------------

    #[test]
    fn integrity_resolution_derives_the_url_via_tarball_url_and_honours_scope() {
        let g = graph(vec![(
            "@scope/pkg@1.0.0",
            node("@scope/pkg", "1.0.0", meta(integrity("sha512-abc"))),
        )]);
        let m = matrix(&["@scope/pkg@1.0.0"]);
        let cfg = config(registry_config(
            "https://registry.npmjs.org",
            &[("@scope", "https://npm.mycorp.example")],
        ));

        let plan = build_plan(&g, &m, &cfg).unwrap();
        let req = &plan.requests["@scope/pkg@1.0.0"];
        assert_eq!(
            req.url,
            "https://npm.mycorp.example/@scope/pkg/-/pkg-1.0.0.tgz"
        );
        assert_eq!(req.integrity, "sha512-abc");
        assert_eq!(plan.expected["@scope/pkg@1.0.0"].url, req.url);
    }

    #[test]
    fn tarball_with_integrity_is_fetched_at_the_recorded_url_verbatim() {
        // The registry-derived URL would be
        // `https://registry.npmjs.org/pkg/-/pkg-1.0.0.tgz` — deliberately
        // different from the recorded one below, so a regression that
        // re-derives instead of trusting the recorded URL is caught.
        let g = graph(vec![(
            "pkg@1.0.0",
            node(
                "pkg",
                "1.0.0",
                meta(Resolution::Tarball {
                    tarball: "https://mirror.example/pkg-1.0.0.tgz".to_string(),
                    integrity: Some("sha512-def".to_string()),
                }),
            ),
        )]);
        let m = matrix(&["pkg@1.0.0"]);
        let cfg = config(registry_config("https://registry.npmjs.org", &[]));

        let plan = build_plan(&g, &m, &cfg).unwrap();
        let req = &plan.requests["pkg@1.0.0"];
        assert_eq!(req.url, "https://mirror.example/pkg-1.0.0.tgz");
        assert_eq!(req.integrity, "sha512-def");
    }

    #[test]
    fn tarball_without_integrity_is_unsupported() {
        let g = graph(vec![(
            "pkg@1.0.0",
            node(
                "pkg",
                "1.0.0",
                meta(Resolution::Tarball {
                    tarball: "https://mirror.example/pkg-1.0.0.tgz".to_string(),
                    integrity: None,
                }),
            ),
        )]);
        let m = matrix(&["pkg@1.0.0"]);
        let cfg = config(registry_config("https://registry.npmjs.org", &[]));

        let err = build_plan(&g, &m, &cfg).unwrap_err();
        match as_vendor_error(&err) {
            VendorError::UnsupportedResolution { packages } => {
                assert_eq!(packages, &["pkg@1.0.0 (url, no integrity)".to_string()]);
            }
            other => panic!("expected UnsupportedResolution, got {other:?}"),
        }
    }

    #[test]
    fn git_resolution_is_unsupported() {
        let g = graph(vec![(
            "pkg@1.0.0",
            node(
                "pkg",
                "1.0.0",
                meta(Resolution::Git {
                    repo: "https://example.com/pkg.git".to_string(),
                    commit: "abc123".to_string(),
                }),
            ),
        )]);
        let m = matrix(&["pkg@1.0.0"]);
        let cfg = config(registry_config("https://registry.npmjs.org", &[]));

        let err = build_plan(&g, &m, &cfg).unwrap_err();
        match as_vendor_error(&err) {
            VendorError::UnsupportedResolution { packages } => {
                assert_eq!(packages, &["pkg@1.0.0 (git)".to_string()]);
            }
            other => panic!("expected UnsupportedResolution, got {other:?}"),
        }
    }

    #[test]
    fn directory_resolution_is_unsupported() {
        let g = graph(vec![(
            "pkg@1.0.0",
            node(
                "pkg",
                "1.0.0",
                meta(Resolution::Directory {
                    directory: "../lib".to_string(),
                    kind: "directory".to_string(),
                }),
            ),
        )]);
        let m = matrix(&["pkg@1.0.0"]);
        let cfg = config(registry_config("https://registry.npmjs.org", &[]));

        let err = build_plan(&g, &m, &cfg).unwrap_err();
        match as_vendor_error(&err) {
            VendorError::UnsupportedResolution { packages } => {
                assert_eq!(packages, &["pkg@1.0.0 (directory)".to_string()]);
            }
            other => panic!("expected UnsupportedResolution, got {other:?}"),
        }
    }

    /// Three unsupported packages of three different kinds are all named in
    /// one error, sorted — not the first one raced to the front.
    #[test]
    fn unsupported_resolutions_are_collected_together_not_raced() {
        let g = graph(vec![
            (
                "zgit@1.0.0",
                node(
                    "zgit",
                    "1.0.0",
                    meta(Resolution::Git {
                        repo: "https://example.com/zgit.git".to_string(),
                        commit: "abc".to_string(),
                    }),
                ),
            ),
            (
                "adir@1.0.0",
                node(
                    "adir",
                    "1.0.0",
                    meta(Resolution::Directory {
                        directory: "../lib".to_string(),
                        kind: "directory".to_string(),
                    }),
                ),
            ),
            (
                "murl@1.0.0",
                node(
                    "murl",
                    "1.0.0",
                    meta(Resolution::Tarball {
                        tarball: "https://mirror.example/m.tgz".to_string(),
                        integrity: None,
                    }),
                ),
            ),
        ]);
        let m = matrix(&["zgit@1.0.0", "adir@1.0.0", "murl@1.0.0"]);
        let cfg = config(registry_config("https://registry.npmjs.org", &[]));

        let err = build_plan(&g, &m, &cfg).unwrap_err();
        match as_vendor_error(&err) {
            VendorError::UnsupportedResolution { packages } => {
                assert_eq!(
                    packages,
                    &[
                        "adir@1.0.0 (directory)".to_string(),
                        "murl@1.0.0 (url, no integrity)".to_string(),
                        "zgit@1.0.0 (git)".to_string(),
                    ]
                );
            }
            other => panic!("expected UnsupportedResolution, got {other:?}"),
        }
    }

    // --- Peer-instance dedup ---------------------------------------------

    /// Two snapshot keys for the same `name@version` collapse to one
    /// request. The second instance is deliberately given a resolution
    /// `build_plan` cannot handle (`Git`) rather than a matching
    /// `Integrity`: if the first-settles guard were ever removed, the
    /// second instance would be processed too, hit the `Git` arm, and turn
    /// this into an `UnsupportedResolution` error — so this test can tell
    /// the dedup guard apart from "the map happens to collapse duplicate
    /// keys anyway."
    #[test]
    fn two_snapshot_keys_for_the_same_name_at_version_share_one_request() {
        let g = graph(vec![
            (
                "pkg@1.0.0(peer@1.0.0)",
                node("pkg", "1.0.0", meta(integrity("sha512-abc"))),
            ),
            (
                "pkg@1.0.0(peer@2.0.0)",
                node(
                    "pkg",
                    "1.0.0",
                    meta(Resolution::Git {
                        repo: "https://example.com/pkg.git".to_string(),
                        commit: "abc".to_string(),
                    }),
                ),
            ),
        ]);
        let m = matrix(&["pkg@1.0.0(peer@1.0.0)", "pkg@1.0.0(peer@2.0.0)"]);
        let cfg = config(registry_config("https://registry.npmjs.org", &[]));

        let plan = build_plan(&g, &m, &cfg).unwrap();
        assert_eq!(plan.requests.len(), 1);
        assert_eq!(plan.expected.len(), 1);
        assert!(plan.requests.contains_key("pkg@1.0.0"));
        assert_eq!(plan.requests["pkg@1.0.0"].integrity, "sha512-abc");
    }

    // --- Carry-over --------------------------------------------------------

    fn entry(url: &str, sha512: &str) -> Entry {
        Entry {
            url: url.to_string(),
            sha512: sha512.to_string(),
            sha256: "deadbeef".to_string(),
            size: 1,
            root: "package".to_string(),
            bin: BTreeMap::new(),
            has_install_script: false,
        }
    }

    fn request(url: &str, integrity: &str) -> Request {
        Request {
            key: "pkg@1.0.0".to_string(),
            name: "pkg".to_string(),
            url: url.to_string(),
            integrity: integrity.to_string(),
        }
    }

    #[test]
    fn matching_url_and_sha512_is_carried_over() {
        let existing = entry("https://x/pkg.tgz", "sha512-abc");
        let req = request("https://x/pkg.tgz", "sha512-abc");
        assert!(is_carried_over(Some(&existing), &req));
    }

    #[test]
    fn matching_url_but_different_sha512_is_not_carried_over() {
        let existing = entry("https://x/pkg.tgz", "sha512-abc");
        let req = request("https://x/pkg.tgz", "sha512-different");
        assert!(!is_carried_over(Some(&existing), &req));
    }

    /// The case that would silently keep a stale URL if the check were ever
    /// loosened to compare the hash alone.
    #[test]
    fn matching_sha512_but_different_url_is_not_carried_over() {
        let existing = entry("https://old.example/pkg.tgz", "sha512-abc");
        let req = request("https://new.example/pkg.tgz", "sha512-abc");
        assert!(!is_carried_over(Some(&existing), &req));
    }

    #[test]
    fn no_existing_entry_is_not_carried_over() {
        let req = request("https://x/pkg.tgz", "sha512-abc");
        assert!(!is_carried_over(None, &req));
    }
}
