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
use crate::platform::prune::prune;
use crate::registry::tarball_url;
use crate::sidecar::{self, Entry, Expected, Loaded, Sidecar};

/// Everything the download pass needs, computed with no network at all.
struct Plan {
    expected: BTreeMap<String, Expected>,
    requests: BTreeMap<String, Request>,
    /// `hasBin` as the lockfile records it, for the cross-check.
    has_bin: BTreeMap<String, bool>,
}

pub fn run(check: bool, jobs: usize, no_network: bool, verbose: bool) -> Result<()> {
    let (config, lockfile) = load_validated()?;
    if config.platforms.is_empty() {
        return Err(VendorError::NoPlatformsConfigured.into());
    }

    let graph = Graph::build(&lockfile)?;
    let (matrix, warnings) = prune(&graph, &config.platforms);
    for w in &warnings {
        eprint!("{}", render(w));
    }

    let plan = build_plan(&graph, &matrix, &config)?;

    let base = std::env::current_dir()?;
    let sidecar_path = base.join(&config.third_party_dir).join("pudu.lock");
    let loaded = sidecar::load(&sidecar_path)?;

    if check {
        return run_check(&plan, &loaded);
    }
    run_vendor(plan, loaded, &sidecar_path, jobs, no_network, verbose)
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

fn run_check(plan: &Plan, loaded: &Loaded) -> Result<()> {
    let differences = sidecar::staleness(&plan.expected, loaded);
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
    sidecar_path: &Path,
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
        match existing.get(&key) {
            Some(e) if e.url == req.url && e.sha512 == req.integrity => {
                entries.insert(key, e.clone());
                unchanged += 1;
            }
            _ => todo.push(req),
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

    let sidecar = Sidecar { entries };
    write_atomic(sidecar_path, &sidecar.render())?;
    eprintln!(
        "vendored {} packages ({} downloaded, {} cached, {} unchanged)",
        sidecar.entries.len(),
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
