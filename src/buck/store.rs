//! The pnpm store layout, as two flat maps.
//!
//! This is pudu's own logic rather than Buck's: the instance graph goes in,
//! and what comes out describes a directory tree — which packages are
//! materialized where, and which relative symlinks join them. Rendering it
//! as Starlark is `emit`'s job; this module never sees a label.
//!
//! Every symlink target is relative, and its `../` count is derived from the
//! link's own path rather than being a constant. A scoped package occupies
//! two path segments, so a link naming one sits a directory deeper than a
//! link naming an unscoped package and must climb one level further. Getting
//! that wrong yields a dangling symlink, which `buck2 build` accepts
//! silently — the failure surfaces only when Node runs (S5 design §1.3).
//!
//! Only nodes reachable from `importer`'s own roots are materialized:
//! walking `graph.nodes[k].edges` from those roots, skipping any target not
//! in the surviving `nodes` set. A package that sits in `nodes` (survives
//! pruning for this platform) but that nothing in this importer's graph
//! actually depends on gets neither a copy nor sibling links — S5 design
//! §3.

use std::collections::{BTreeMap, BTreeSet};

use crate::lock::graph::Graph;
use crate::lock::snapshot_key::target_name;
use crate::packages::Entry;

/// One importer's store layout.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tree {
    /// Destination path in the tree → snapshot key whose `[root]` goes there.
    pub copies: BTreeMap<String, String>,
    /// Destination path in the tree → relative symlink target.
    pub links: BTreeMap<String, String>,
}

/// `n` levels of `../`, or the empty string for zero.
fn up(n: usize) -> String {
    "../".repeat(n)
}

/// How many path segments a `node_modules/` entry name occupies.
///
/// One for `ms`, two for `@jridgewell/set-array`. This is the whole reason
/// the `../` count cannot be a constant.
fn segments(link_name: &str) -> usize {
    link_name.split('/').count()
}

/// Where a package's own directory lives inside the virtual store.
fn store_path(key: &str, dir: &str) -> String {
    format!(".pnpm/{}/node_modules/{dir}", target_name(key))
}

/// The bins a package exposes, as `(bin_name, path_within_the_package)`.
fn bins_of<'a>(
    entries: &'a BTreeMap<String, Entry>,
    key: &str,
) -> impl Iterator<Item = (&'a String, &'a String)> {
    entries.get(key).into_iter().flat_map(|e| e.bin.iter())
}

/// The snapshot keys reachable from `importer`'s roots, walking edges and
/// stopping at anything absent from `nodes`.
///
/// The graph contains cycles (normal in real lockfiles — see
/// `graph::find_cycles`), so this tracks a visited set rather than
/// recursing unguarded.
fn reachable(graph: &Graph, nodes: &BTreeSet<String>, importer: &str) -> BTreeSet<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<&str> = graph
        .roots
        .iter()
        .filter(|r| r.importer == importer)
        .filter_map(|r| r.target.as_deref())
        .filter(|k| nodes.contains(*k))
        .collect();

    while let Some(key) = stack.pop() {
        if !seen.insert(key.to_string()) {
            continue;
        }
        let Some(node) = graph.nodes.get(key) else {
            continue;
        };
        for edge in &node.edges {
            if nodes.contains(&edge.target) && !seen.contains(&edge.target) {
                stack.push(&edge.target);
            }
        }
    }

    seen
}

pub fn build(
    graph: &Graph,
    nodes: &BTreeSet<String>,
    entries: &BTreeMap<String, Entry>,
    importer: &str,
) -> Tree {
    let mut tree = Tree::default();
    let reachable = reachable(graph, nodes, importer);

    // Copies and sibling links, one pass over the reachable nodes.
    for key in &reachable {
        let Some(node) = graph.nodes.get(key) else {
            continue;
        };
        let dir = &node.name;
        tree.copies.insert(store_path(key, dir), key.clone());

        for edge in &node.edges {
            let Some(target) = graph.nodes.get(&edge.target) else {
                continue;
            };
            if !nodes.contains(&edge.target) {
                continue;
            }
            let dest = format!(".pnpm/{}/node_modules/{}", target_name(key), edge.link_name);
            // From the link's own directory, climb to `.pnpm/`: two levels
            // for the `<tn>/node_modules/` pair, plus one more for each
            // extra segment the link name itself occupies.
            let climb = 2 + segments(&edge.link_name) - 1;
            tree.links.insert(
                dest,
                format!(
                    "{}{}/node_modules/{}",
                    up(climb),
                    target_name(&edge.target),
                    target.name
                ),
            );

            // A package's dependencies' bins are visible to it, at
            // `.pnpm/<tn(k)>/node_modules/.bin/`. Three levels below
            // `.pnpm/`: `<tn>`, `node_modules`, `.bin`.
            for (bin_name, bin_path) in bins_of(entries, &edge.target) {
                tree.links.insert(
                    format!(".pnpm/{}/node_modules/.bin/{bin_name}", target_name(key)),
                    format!(
                        "{}{}/node_modules/{}/{bin_path}",
                        up(3),
                        target_name(&edge.target),
                        target.name
                    ),
                );
            }
        }
    }

    // Top-level links, one per root of this importer.
    for root in &graph.roots {
        if root.importer != importer {
            continue;
        }
        let Some(key) = &root.target else {
            continue;
        };
        if !nodes.contains(key) || !reachable.contains(key) {
            continue;
        }
        let Some(node) = graph.nodes.get(key) else {
            continue;
        };
        let climb = segments(&root.link_name) - 1;
        tree.links.insert(
            root.link_name.clone(),
            format!("{}{}", up(climb), store_path(key, &node.name)),
        );

        // A direct dependency's bins go in the importer's own `.bin/`, one
        // level below the tree root.
        for (bin_name, bin_path) in bins_of(entries, key) {
            tree.links.insert(
                format!(".bin/{bin_name}"),
                format!("{}{}/{bin_path}", up(1), store_path(key, &node.name)),
            );
        }
    }

    tree
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::lock::Graph;
    use crate::lock::types::Lockfile;
    use crate::packages::Entry;

    /// A lockfile with one importer depending on `debug`, which depends on
    /// `ms`; plus a scoped package depending on a scoped package.
    fn lockfile() -> Lockfile {
        serde_norway::from_str(
            r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      debug:
        specifier: 4.3.4
        version: 4.3.4
      '@jridgewell/gen-mapping':
        specifier: 0.3.5
        version: 0.3.5
packages:
  debug@4.3.4:
    resolution: {integrity: sha512-x}
  ms@2.1.2:
    resolution: {integrity: sha512-x}
  '@jridgewell/gen-mapping@0.3.5':
    resolution: {integrity: sha512-x}
  '@jridgewell/set-array@1.2.1':
    resolution: {integrity: sha512-x}
  left-pad@1.3.0:
    resolution: {integrity: sha512-x}
snapshots:
  debug@4.3.4:
    dependencies:
      ms: 2.1.2
  ms@2.1.2: {}
  '@jridgewell/gen-mapping@0.3.5':
    dependencies:
      '@jridgewell/set-array': 1.2.1
  '@jridgewell/set-array@1.2.1': {}
  left-pad@1.3.0: {}
"#,
        )
        .unwrap()
    }

    /// Only `bin` is read from the table, so the rest is filler.
    fn entries(bins: &[(&str, &str, &str)]) -> BTreeMap<String, Entry> {
        let mut out: BTreeMap<String, Entry> = BTreeMap::new();
        for (key, name, path) in bins {
            out.entry(key.to_string())
                .or_insert_with(|| Entry {
                    url: String::new(),
                    sha512: String::new(),
                    sha256: String::new(),
                    size: 0,
                    root: "package".to_string(),
                    bin: BTreeMap::new(),
                    has_install_script: false,
                })
                .bin
                .insert(name.to_string(), path.to_string());
        }
        out
    }

    fn tree() -> Tree {
        let graph = Graph::build(&lockfile()).unwrap();
        let nodes: BTreeSet<String> = graph.nodes.keys().cloned().collect();
        build(&graph, &nodes, &entries(&[]), ".")
    }

    #[test]
    fn every_reachable_package_is_copied_into_the_virtual_store() {
        let t = tree();
        assert_eq!(
            t.copies.get(".pnpm/debug@4.3.4/node_modules/debug"),
            Some(&"debug@4.3.4".to_string())
        );
        assert_eq!(
            t.copies
                .get(".pnpm/@jridgewell+gen-mapping@0.3.5/node_modules/@jridgewell/gen-mapping"),
            Some(&"@jridgewell/gen-mapping@0.3.5".to_string())
        );
    }

    #[test]
    fn an_unscoped_top_level_link_needs_no_parent_prefix() {
        assert_eq!(
            tree().links.get("debug"),
            Some(&".pnpm/debug@4.3.4/node_modules/debug".to_string())
        );
    }

    #[test]
    fn a_scoped_top_level_link_climbs_one_level() {
        assert_eq!(
            tree().links.get("@jridgewell/gen-mapping"),
            Some(
                &"../.pnpm/@jridgewell+gen-mapping@0.3.5/node_modules/@jridgewell/gen-mapping"
                    .to_string()
            )
        );
    }

    #[test]
    fn an_unscoped_sibling_link_climbs_two_levels() {
        assert_eq!(
            tree().links.get(".pnpm/debug@4.3.4/node_modules/ms"),
            Some(&"../../ms@2.1.2/node_modules/ms".to_string())
        );
    }

    /// The case a constant `../../` gets wrong. A scoped sibling sits one
    /// directory deeper, so it needs three levels, not two.
    #[test]
    fn a_scoped_sibling_link_climbs_three_levels() {
        assert_eq!(
            tree()
                .links
                .get(".pnpm/@jridgewell+gen-mapping@0.3.5/node_modules/@jridgewell/set-array"),
            Some(
                &"../../../@jridgewell+set-array@1.2.1/node_modules/@jridgewell/set-array"
                    .to_string()
            )
        );
    }

    /// A direct dependency's bin is linked at the importer's top level, one
    /// directory below the tree root.
    #[test]
    fn a_direct_dependencys_bin_is_linked_at_the_top_level() {
        let graph = Graph::build(&lockfile()).unwrap();
        let nodes: BTreeSet<String> = graph.nodes.keys().cloned().collect();
        let t = build(
            &graph,
            &nodes,
            &entries(&[("debug@4.3.4", "debug", "bin/debug.js")]),
            ".",
        );
        assert_eq!(
            t.links.get(".bin/debug"),
            Some(&"../.pnpm/debug@4.3.4/node_modules/debug/bin/debug.js".to_string())
        );
    }

    /// And a dependency's own bin is linked inside that dependency's virtual
    /// store directory, three levels below `.pnpm/`.
    #[test]
    fn a_transitive_bin_is_linked_inside_the_dependents_store_directory() {
        let graph = Graph::build(&lockfile()).unwrap();
        let nodes: BTreeSet<String> = graph.nodes.keys().cloned().collect();
        let t = build(
            &graph,
            &nodes,
            &entries(&[("ms@2.1.2", "ms", "bin/ms.js")]),
            ".",
        );
        assert_eq!(
            t.links.get(".pnpm/debug@4.3.4/node_modules/.bin/ms"),
            Some(&"../../../ms@2.1.2/node_modules/ms/bin/ms.js".to_string())
        );
    }

    #[test]
    fn a_pruned_package_is_absent_from_copies_and_links() {
        let graph = Graph::build(&lockfile()).unwrap();
        let nodes: BTreeSet<String> = graph
            .nodes
            .keys()
            .filter(|k| *k != "ms@2.1.2")
            .cloned()
            .collect();
        let t = build(&graph, &nodes, &entries(&[]), ".");
        assert!(!t.copies.contains_key(".pnpm/ms@2.1.2/node_modules/ms"));
        assert!(
            !t.links.contains_key(".pnpm/debug@4.3.4/node_modules/ms"),
            "a link to a pruned package would dangle, and a dangling link builds clean"
        );
    }

    #[test]
    fn a_second_importer_gets_only_its_own_top_level_links() {
        let t = tree();
        assert!(t.links.contains_key("debug"));
        assert!(!t.links.contains_key("ms"), "ms is transitive, not direct");
    }

    /// `left-pad` is in `nodes` but nothing links to it, so it must not
    /// appear in the tree at all.
    #[test]
    fn a_package_no_importer_depends_on_is_not_copied_into_the_tree() {
        let graph = Graph::build(&lockfile()).unwrap();
        let mut nodes: BTreeSet<String> = graph.nodes.keys().cloned().collect();
        nodes.insert("left-pad@1.3.0".to_string());
        let t = build(&graph, &nodes, &entries(&[]), ".");
        assert!(!t.copies.values().any(|k| k == "left-pad@1.3.0"));
    }
}
