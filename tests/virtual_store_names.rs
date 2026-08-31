//! Differential test against pnpm itself.
//!
//! `virtual-store-listing.txt` is the exact set of directory names pnpm
//! created for the committed lockfile. Pudu's target naming is a port of
//! pnpm's `depPathToFilename`, so it must reproduce all of them.

use std::collections::BTreeSet;
use std::path::Path;

use pudu::lock::parse_lockfile;
use pudu::lock::snapshot_key::{MAX_LEN_WITHOUT_HASH, target_name};

fn load() -> (BTreeSet<String>, BTreeSet<String>) {
    let dir = Path::new("tests/fixtures/lock/real");
    let text = std::fs::read_to_string(dir.join("pnpm-lock.yaml")).unwrap();
    let (lockfile, _) = parse_lockfile(&text, &dir.join("pnpm-lock.yaml")).unwrap();
    let produced: BTreeSet<String> = lockfile.snapshots.keys().map(|k| target_name(k)).collect();
    let captured: BTreeSet<String> = std::fs::read_to_string(dir.join("virtual-store-listing.txt"))
        .unwrap()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    (produced, captured)
}

#[test]
fn pudu_reproduces_every_name_pnpm_created() {
    let (produced, captured) = load();
    let missing: Vec<_> = captured.difference(&produced).collect();
    assert!(
        missing.is_empty(),
        "pudu failed to produce {} of pnpm's {} virtual-store names.\n\
         This means the depPathToFilename port has diverged.\nFirst few: {:#?}",
        missing.len(),
        captured.len(),
        missing.iter().take(10).collect::<Vec<_>>()
    );
}

#[test]
fn the_fixture_still_exercises_the_hashed_path() {
    // Guards the fixture itself: a regeneration that resolved differently
    // could silently drop the >120-char case, which is the branch most
    // likely to diverge.
    let (_, captured) = load();
    // The stem/hash split point: `target_name` keeps `MAX_LEN_WITHOUT_HASH -
    // 33` stem bytes, then an underscore, then a 32-hex-char digest prefix.
    // Derived from the constant rather than hardcoded so changing it does not
    // redden this test for the wrong reason.
    let keep = MAX_LEN_WITHOUT_HASH - 33;
    let hashed = captured
        .iter()
        .filter(|n| {
            n.len() == MAX_LEN_WITHOUT_HASH
                && n.as_bytes()[keep] == b'_'
                && n[keep + 1..].chars().all(|c| c.is_ascii_hexdigit())
        })
        .count();
    assert!(
        hashed >= 3,
        "expected at least 3 hashed names, found {hashed}"
    );
}

#[test]
fn the_fixture_still_exercises_aliases_and_nesting() {
    let dir = Path::new("tests/fixtures/lock/real");
    let text = std::fs::read_to_string(dir.join("pnpm-lock.yaml")).unwrap();
    let (lockfile, _) = parse_lockfile(&text, &dir.join("pnpm-lock.yaml")).unwrap();

    let has_alias = lockfile.snapshots.values().any(|s| {
        s.dependencies
            .iter()
            .any(|(link, v)| !v.starts_with(char::is_numeric) && !v.starts_with(link))
    });
    assert!(has_alias, "the alias case has vanished from the fixture");

    let nested = lockfile
        .snapshots
        .keys()
        .filter(|k| k.matches('(').count() > 1)
        .count();
    assert!(nested >= 5, "expected nested peer keys, found {nested}");
}
