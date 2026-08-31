//! Differential test: pudu's pruning against pnpm's own install set.
//!
//! Each oracle under `tests/fixtures/lock/real/oracle/` is the exact set of
//! directories pnpm created in `node_modules/.pnpm/` for one platform. Pudu
//! must reproduce every one. See `oracle/capture.sh` for provenance.

use std::collections::{BTreeMap, BTreeSet};

use pudu::config::Platform;
use pudu::lock::graph::Graph;
use pudu::lock::parse_lockfile;
use pudu::lock::snapshot_key::target_name;
use pudu::platform::prune::prune;
use pudu::platform::{Cpu, Libc, Os};

const FIXTURE: &str = "tests/fixtures/lock/real";

fn read(rel: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURE}/{rel}"))
        .unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn oracle(name: &str) -> BTreeSet<String> {
    read(&format!("oracle/{name}.txt"))
        .lines()
        .map(str::to_string)
        .collect()
}

/// Optional dependencies pnpm skipped for `engines` rather than platform.
/// Pudu does not model `engines` (spec §7.1), so its survivor set is a
/// superset of pnpm's by exactly this set.
fn engine_excluded() -> BTreeSet<String> {
    read("oracle/engine-excluded.txt")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn platform(os: Os, cpu: Cpu, libc: Option<Libc>) -> Platform {
    Platform {
        os,
        cpu,
        libc,
        constraints: None,
    }
}

fn survivors(platform_name: &str, p: Platform) -> BTreeSet<String> {
    let text = read("pnpm-lock.yaml");
    let (lockfile, _) = parse_lockfile(&text, std::path::Path::new("pnpm-lock.yaml"))
        .expect("fixture lockfile parses");
    let graph = Graph::build(&lockfile).expect("fixture graph builds");
    let platforms = BTreeMap::from([(platform_name.to_string(), p)]);
    let (matrix, _) = prune(&graph, &platforms);
    let skip = engine_excluded();
    matrix.views[platform_name]
        .nodes
        .iter()
        .filter(|k| !skip.contains(*k))
        .map(|k| target_name(k))
        .collect()
}

fn assert_reproduces(name: &str, p: Platform) {
    let got = survivors(name, p);
    let want = oracle(name);
    let extra: Vec<_> = got.difference(&want).collect();
    let missing: Vec<_> = want.difference(&got).collect();
    assert!(
        extra.is_empty() && missing.is_empty(),
        "{name}: pudu kept {} pnpm did not {extra:?}; pnpm kept {} pudu did not {missing:?}",
        extra.len(),
        missing.len()
    );
}

#[test]
fn reproduces_pnpm_on_linux_x64_gnu() {
    assert_reproduces(
        "linux-x64-gnu",
        platform(Os::Linux, Cpu::X64, Some(Libc::Glibc)),
    );
}

#[test]
fn reproduces_pnpm_on_linux_x64_musl() {
    assert_reproduces(
        "linux-x64-musl",
        platform(Os::Linux, Cpu::X64, Some(Libc::Musl)),
    );
}

#[test]
fn reproduces_pnpm_on_linux_arm64_gnu() {
    assert_reproduces(
        "linux-arm64-gnu",
        platform(Os::Linux, Cpu::Arm64, Some(Libc::Glibc)),
    );
}

#[test]
fn reproduces_pnpm_on_darwin_arm64() {
    assert_reproduces("darwin-arm64", platform(Os::Darwin, Cpu::Arm64, None));
}

/// The oracles must stay pinned to S1's fixture: `linux-x64-gnu` is the
/// platform the committed virtual-store listing was captured on, so the two
/// files must agree or one of them is stale.
#[test]
fn linux_x64_gnu_oracle_matches_the_s1_virtual_store_listing() {
    let listing: BTreeSet<String> = read("virtual-store-listing.txt")
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(oracle("linux-x64-gnu"), listing);
}

/// The roadmap's demo criterion: each configured platform resolves
/// esbuild's ~20 optional deps to exactly one `@esbuild/*` per version.
#[test]
fn each_platform_keeps_exactly_one_esbuild_binary_per_version() {
    for (name, p) in [
        (
            "linux-x64-gnu",
            platform(Os::Linux, Cpu::X64, Some(Libc::Glibc)),
        ),
        (
            "linux-arm64-gnu",
            platform(Os::Linux, Cpu::Arm64, Some(Libc::Glibc)),
        ),
        ("darwin-arm64", platform(Os::Darwin, Cpu::Arm64, None)),
    ] {
        let kept: Vec<String> = survivors(name, p)
            .into_iter()
            .filter(|k| k.starts_with("@esbuild+"))
            .collect();
        // The fixture pins two esbuild versions, so exactly two survive.
        assert_eq!(kept.len(), 2, "{name} kept {kept:?}");
        assert!(
            kept.iter().any(|k| k.contains("0.25.12")) && kept.iter().any(|k| k.contains("0.28.2")),
            "{name} must keep one binary per esbuild version: {kept:?}"
        );
    }
}
