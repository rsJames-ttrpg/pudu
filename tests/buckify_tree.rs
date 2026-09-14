//! Properties of the emitted `node_modules` trees, and of the buck2 fixture
//! that exercises them.
//!
//! `buck2 build` is not a gate for anything in this file. A dangling
//! `node_modules` symlink, a tree missing from a rule's hidden inputs and a
//! wrong `../` depth all build clean and fail only when Node runs (S5 design
//! §1.3), so the properties have to be asserted directly.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use pudu::buck::store::{self, Tree};
use pudu::lock::{Graph, parse_lockfile};
use pudu::packages::{Entry, Loaded};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/buck/01-pure-js")
}

/// The fixture's own lockfile and package table, as `store::build` takes them.
fn fixture_graph() -> (Graph, BTreeMap<String, Entry>) {
    let lock_path = fixture_dir().join("pnpm-lock.yaml");
    let text = std::fs::read_to_string(&lock_path).expect("fixture lockfile is readable");
    let (lockfile, _) = parse_lockfile(&text, &lock_path).expect("fixture lockfile parses");
    let graph = Graph::build(&lockfile).expect("fixture lockfile resolves to a graph");

    // The real table, not a hand-written stand-in: `bin` is what produces the
    // `.bin/*` links, and those are the deepest relative targets in the tree.
    let table = match pudu::packages::load(&fixture_dir().join("third-party/js/packages.toml")) {
        Ok(Loaded::Present(t)) => t,
        other => panic!("fixture packages.toml must load as a current table, got {other:?}"),
    };
    (graph, table.entries)
}

/// Resolve `target` against `dest`'s parent directory, textually.
///
/// Textually, and not through the filesystem: these paths describe a tree
/// that does not exist yet, and `..` in them must be resolved the way the
/// path string says rather than the way a realpath would after following an
/// earlier symlink component. Returns `None` if the target climbs above the
/// tree root, which is itself a bug.
fn resolve(dest: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = dest.split('/').collect();
    parts.pop(); // the link's own name; a top-level dest leaves this empty
    for component in target.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            c => parts.push(c),
        }
    }
    Some(parts.join("/"))
}

/// Is `path` a copied package, or a file inside one?
///
/// A `.bin/*` link points *inside* a copy — at
/// `.pnpm/semver@7.6.3/node_modules/semver/bin/semver.js`, say — so an exact
/// hit is not the only way to be resolvable.
fn lands_in_a_copy(tree: &Tree, path: &str) -> bool {
    tree.copies.contains_key(path)
        || tree.copies.keys().any(|c| {
            path.len() > c.len() && path.starts_with(c) && path.as_bytes()[c.len()] == b'/'
        })
}

/// A tree whose sibling link points at a package that was pruned away would
/// dangle. `buck2 build` accepts a dangling symlink silently, so the only
/// gate is that pudu never emits one.
#[test]
fn no_link_target_is_missing_from_the_copies_map() {
    let (graph, entries) = fixture_graph();
    let nodes: BTreeSet<String> = graph.nodes.keys().cloned().collect();
    let importers: BTreeSet<&str> = graph.roots.iter().map(|r| r.importer.as_str()).collect();

    let mut checked = 0usize;
    for importer in importers {
        let tree = store::build(&graph, &nodes, &entries, importer);
        for (dest, target) in &tree.links {
            let resolved = resolve(dest, target).unwrap_or_else(|| {
                panic!("{importer}: link {dest} -> {target} climbs above the tree root")
            });
            assert!(
                lands_in_a_copy(&tree, &resolved),
                "{importer}: link {dest} -> {target} resolves to {resolved}, \
                 which is neither a copied package nor a path inside one; \
                 it would dangle, and a dangling link builds clean"
            );
            checked += 1;
        }
    }

    // A sweep over an empty map passes vacuously and proves nothing.
    assert!(checked > 0, "the sweep examined no links at all");
}

/// `tests/fixtures/buck/01-pure-js/third-party/js/toolchains.bzl` is a
/// hand-copy of what `pudu init` writes. Nothing regenerates it, so without
/// this the fixture can quietly stop testing the file users actually get.
#[test]
fn the_fixtures_toolchains_bzl_matches_what_init_writes() {
    let on_disk = std::fs::read_to_string(fixture_dir().join("third-party/js/toolchains.bzl"))
        .expect("fixture toolchains.bzl is readable");
    assert_eq!(
        on_disk,
        pudu::cli::init::TOOLCHAINS_BZL,
        "the fixture's toolchains.bzl has drifted from init's; re-copy the constant"
    );
}

/// And the fixture's `toolchains/BUCK` carries a hand-copy of the block
/// `pudu init` appends, for the fixture's own `third-party/js` label.
#[test]
fn the_fixtures_toolchains_buck_contains_inits_managed_block() {
    let on_disk = std::fs::read_to_string(fixture_dir().join("toolchains/BUCK"))
        .expect("fixture toolchains/BUCK is readable");
    let block = pudu::cli::toolchain::managed_block("@root//third-party/js:toolchains.bzl");
    assert!(
        on_disk.contains(&block),
        "the fixture's toolchains/BUCK has drifted from init's managed block.\n\
         expected to contain:\n{block}\nfound:\n{on_disk}"
    );
}

#[cfg(test)]
mod resolve_tests {
    use super::resolve;

    /// The helper is the whole test's oracle, so its own `..` handling is
    /// pinned rather than assumed.
    #[test]
    fn a_sibling_target_resolves_against_the_links_own_directory() {
        assert_eq!(
            resolve(
                ".pnpm/debug@4.3.4/node_modules/ms",
                "../../ms@2.1.2/node_modules/ms"
            )
            .as_deref(),
            Some(".pnpm/ms@2.1.2/node_modules/ms")
        );
    }

    #[test]
    fn a_top_level_target_resolves_against_the_tree_root() {
        assert_eq!(
            resolve("debug", ".pnpm/debug@4.3.4/node_modules/debug").as_deref(),
            Some(".pnpm/debug@4.3.4/node_modules/debug")
        );
    }

    #[test]
    fn climbing_above_the_tree_root_is_not_a_path() {
        assert_eq!(resolve("debug", "../../elsewhere"), None);
    }
}
