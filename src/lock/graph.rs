//! The instance graph: one node per snapshot key.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::LockError;
use crate::lock::snapshot_key::SnapshotKey;
use crate::lock::types::{Lockfile, PackageMeta};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Graph {
    /// Keyed by canonical snapshot key.
    pub nodes: BTreeMap<String, Node>,
    pub roots: Vec<Root>,
    /// Populated by cycle detection. Cycles are normal — see the S1 spec §7.
    pub cycles: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    pub name: String,
    pub version: String,
    pub peers: Vec<String>,
    pub target_name: String,
    pub optional: bool,
    pub meta: PackageMeta,
    /// Sorted by `link_name` for determinism.
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Edge {
    /// The directory name under `node_modules/`. May differ from the target
    /// package's own name when the dependency is an npm alias.
    pub link_name: String,
    /// Canonical snapshot key of the dependency.
    pub target: String,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    Prod,
    Optional,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Root {
    pub importer: String,
    pub link_name: String,
    /// `None` for `link:`/`file:`/`workspace:` roots, which resolve to another
    /// importer rather than a package. S5 makes those real.
    pub target: Option<String>,
    pub specifier: String,
    pub kind: RootKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RootKind {
    Prod,
    Dev,
    Optional,
}

/// Resolve one dependency edge to a snapshot key.
///
/// The value is not always a bare version — npm aliases encode a complete
/// `name@version`, in which case `link_name` is only a directory name:
///
/// ```text
/// string-width:     5.1.2                -> string-width@5.1.2
/// string-width-cjs: string-width@4.2.3   -> string-width@4.2.3
/// eslint:           9.39.2(jiti@2.6.1)   -> eslint@9.39.2(jiti@2.6.1)
/// ```
pub fn resolve_edge(link_name: &str, value: &str) -> String {
    let head = strip_peer_suffix(value);
    // An '@' beyond position 0 in the head means the value already names a
    // package. Position 0 is excluded so a scoped alias target still works.
    if head.char_indices().any(|(i, c)| c == '@' && i > 0) {
        value.to_string()
    } else {
        format!("{link_name}@{value}")
    }
}

/// The value with any peer suffix removed — everything from the first `(`
/// onward. A plain left-to-right search suffices: the *first* `(` in the
/// string is always the depth-0 one (any nested `(` can only occur after it),
/// so there is no need to track paren depth here.
fn strip_peer_suffix(s: &str) -> &str {
    match s.find('(') {
        Some(i) => &s[..i],
        None => s,
    }
}

/// True for a specifier that points at another importer rather than a package.
fn is_link_specifier(specifier: &str, version: &str) -> bool {
    specifier.starts_with("workspace:")
        || specifier.starts_with("link:")
        || specifier.starts_with("file:")
        || version.starts_with("link:")
        || version.starts_with("file:")
}

/// Node colours for the iterative DFS.
#[derive(Clone, Copy, PartialEq)]
enum Colour {
    White,
    Grey,
    Black,
}

/// Find cycles with an explicit stack.
///
/// Iterative by necessity: real lockfiles reach 800+ nodes with deep chains,
/// and a recursive DFS overflows. Cycles are normal in npm graphs (`@babel`,
/// `eslint`, `browserslist` all have them), so this reports rather than
/// rejects — see the S1 spec §7 for why that is safe under the single
/// `filegroup` store.
///
/// Each reported cycle is a *closed* path — the entry node is repeated at
/// the end, per S1 spec §10, so a two-node cycle is `["a", "b", "a"]` and a
/// self-edge is `["a", "a"]`. The closed form reads unambiguously as a walk
/// back to its start, which matters because this is what `pudu debug
/// print-graph` renders as a human diagnostic.
///
/// Deduplication: a cycle's identity is its node set, not the path text —
/// the same cycle found from a different DFS start yields the same nodes in
/// a rotated order, and would otherwise be reported once per entry point
/// that can reach it. So the dedup key is the node set (a `BTreeSet`),
/// which is rotation-invariant; the closing repeat of the entry node does
/// not add anything to the set, so it does not affect dedup. The reported
/// cycle itself keeps its original discovery order.
///
/// **This is not an exhaustive enumeration of simple cycles.** The scan
/// reports one cycle per DFS back edge, so a cycle reachable only through a
/// cross edge into an already-finished (black) node is never discovered. For
/// example, given `a->p->x->a` and `a->b->c->x`, only `a-p-x-a` is reported;
/// the equally real `a-b-c-x-a` is not, because the `c->x` cross edge lands
/// on a black node and is skipped.
///
/// That is acceptable because `cycles` is a human-facing diagnostic in
/// `pudu debug print-graph` and never an input to a correctness decision.
/// **Do not build a later stage on the assumption that this list is
/// complete** — that would need Johnson's algorithm, not a colour DFS.
fn find_cycles(nodes: &BTreeMap<String, Node>) -> Vec<Vec<String>> {
    let mut colour: BTreeMap<&str, Colour> =
        nodes.keys().map(|k| (k.as_str(), Colour::White)).collect();
    let mut cycles = Vec::new();
    let mut seen: std::collections::BTreeSet<std::collections::BTreeSet<String>> =
        Default::default();

    for start in nodes.keys() {
        if colour[start.as_str()] != Colour::White {
            continue;
        }
        // Explicit stack of (node, index of the next edge to visit),
        // mirrored by `path`, the sequence of grey nodes from `start` down
        // to the node currently on top of `stack`. The two stay in step:
        // a node is pushed onto both when it turns grey, and popped from
        // both when its edges are exhausted and it turns black.
        let mut stack: Vec<(&str, usize)> = vec![(start.as_str(), 0)];
        let mut path: Vec<&str> = vec![start.as_str()];
        colour.insert(start.as_str(), Colour::Grey);

        while let Some((node, edge_idx)) = stack.pop() {
            let edges = &nodes[node].edges;
            if edge_idx < edges.len() {
                stack.push((node, edge_idx + 1));
                let next = edges[edge_idx].target.as_str();
                match colour[next] {
                    Colour::Grey => {
                        // Back edge: the cycle is the path from `next` on.
                        let pos = path
                            .iter()
                            .position(|n| *n == next)
                            .expect("a grey node is always on the current path");
                        let mut cycle: Vec<String> =
                            path[pos..].iter().map(|s| s.to_string()).collect();
                        let key: std::collections::BTreeSet<String> =
                            cycle.iter().cloned().collect();
                        cycle.push(next.to_string());
                        if seen.insert(key) {
                            cycles.push(cycle);
                        }
                    }
                    Colour::White => {
                        colour.insert(next, Colour::Grey);
                        stack.push((next, 0));
                        path.push(next);
                    }
                    Colour::Black => {}
                }
            } else {
                colour.insert(node, Colour::Black);
                path.pop();
            }
        }
    }
    cycles
}

impl Graph {
    pub fn build(lockfile: &Lockfile) -> Result<Self, LockError> {
        let mut nodes = BTreeMap::new();
        let mut by_target: BTreeMap<String, String> = BTreeMap::new();

        for (raw_key, entry) in &lockfile.snapshots {
            let key = SnapshotKey::parse(raw_key).map_err(|e| LockError::KeyParse {
                key: e.key,
                offset: e.offset,
                reason: e.reason.to_string(),
            })?;
            let base = key.base();
            let meta = lockfile.packages.get(&base).cloned().ok_or_else(|| {
                LockError::MissingPackageMeta {
                    snapshot: raw_key.clone(),
                    base: base.clone(),
                }
            })?;

            let target_name = key.target_name();
            if let Some(other) = by_target.get(&target_name) {
                return Err(LockError::TargetNameCollision {
                    a: other.clone(),
                    b: raw_key.clone(),
                    target: target_name,
                });
            }
            by_target.insert(target_name.clone(), raw_key.clone());

            // A link name naming one `node_modules/` slot cannot legally
            // appear in both `dependencies` and `optionalDependencies` — pnpm
            // never emits that. Reject rather than merge or let one silently
            // shadow the other.
            if let Some(link_name) = entry
                .dependencies
                .keys()
                .find(|k| entry.optional_dependencies.contains_key(*k))
            {
                return Err(LockError::DuplicateLinkName {
                    snapshot: raw_key.clone(),
                    link_name: link_name.clone(),
                });
            }

            let mut edges: Vec<Edge> = entry
                .dependencies
                .iter()
                .map(|(n, v)| (n, v, EdgeKind::Prod))
                .chain(
                    entry
                        .optional_dependencies
                        .iter()
                        .map(|(n, v)| (n, v, EdgeKind::Optional)),
                )
                .map(|(link_name, value, kind)| Edge {
                    link_name: link_name.clone(),
                    target: resolve_edge(link_name, value),
                    kind,
                })
                .collect();
            edges.sort_by(|a, b| a.link_name.cmp(&b.link_name));

            nodes.insert(
                raw_key.clone(),
                Node {
                    name: key.name.clone(),
                    version: key.version.clone(),
                    peers: key.peers.iter().map(SnapshotKey::canonical).collect(),
                    target_name,
                    optional: entry.optional,
                    meta,
                    edges,
                },
            );
        }

        // Validate every edge target after all nodes exist, so a forward
        // reference is not mistaken for a dangling one.
        for (from, node) in &nodes {
            for edge in &node.edges {
                if !nodes.contains_key(&edge.target) {
                    return Err(LockError::UnresolvedEdge {
                        from: from.clone(),
                        link_name: edge.link_name.clone(),
                        resolved: edge.target.clone(),
                    });
                }
            }
        }

        let mut roots = Vec::new();
        for (importer, imp) in &lockfile.importers {
            let groups = [
                (&imp.dependencies, RootKind::Prod),
                (&imp.dev_dependencies, RootKind::Dev),
                (&imp.optional_dependencies, RootKind::Optional),
            ];
            for (deps, kind) in groups {
                for (link_name, dep) in deps {
                    let target = if is_link_specifier(&dep.specifier, &dep.version) {
                        None
                    } else {
                        Some(resolve_edge(link_name, &dep.version))
                    };
                    if let Some(t) = &target
                        && !nodes.contains_key(t)
                    {
                        return Err(LockError::UnresolvedEdge {
                            from: format!("importer {importer}"),
                            link_name: link_name.clone(),
                            resolved: t.clone(),
                        });
                    }
                    roots.push(Root {
                        importer: importer.clone(),
                        link_name: link_name.clone(),
                        target,
                        specifier: dep.specifier.clone(),
                        kind,
                    });
                }
            }
        }

        let cycles = find_cycles(&nodes);
        Ok(Self {
            nodes,
            roots,
            cycles,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::parse_lockfile;
    use std::path::Path;

    fn build(yaml: &str) -> Graph {
        let (lf, _) = parse_lockfile(yaml, Path::new("/x/pnpm-lock.yaml")).expect("parses");
        Graph::build(&lf).expect("builds")
    }

    const ALIAS_LOCK: &str = r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      '@isaacs/cliui':
        specifier: ^8.0.2
        version: 8.0.2
packages:
  '@isaacs/cliui@8.0.2':
    resolution: {integrity: sha512-a}
  string-width@5.1.2:
    resolution: {integrity: sha512-b}
  string-width@4.2.3:
    resolution: {integrity: sha512-c}
snapshots:
  '@isaacs/cliui@8.0.2':
    dependencies:
      string-width: 5.1.2
      string-width-cjs: string-width@4.2.3
  string-width@5.1.2: {}
  string-width@4.2.3: {}
"#;

    #[test]
    fn alias_edge_resolves_to_the_aliased_package_and_keeps_the_link_name() {
        let g = build(ALIAS_LOCK);
        let cliui = &g.nodes["@isaacs/cliui@8.0.2"];
        let aliased = cliui
            .edges
            .iter()
            .find(|e| e.link_name == "string-width-cjs")
            .expect("the alias edge must survive under its link name");
        // BOTH halves matter: resolving alone would pass with link_name lost.
        assert_eq!(
            aliased.target, "string-width@4.2.3",
            "resolves to the aliased package"
        );
        assert_eq!(
            aliased.link_name, "string-width-cjs",
            "link name is retained"
        );

        let plain = cliui
            .edges
            .iter()
            .find(|e| e.link_name == "string-width")
            .unwrap();
        assert_eq!(plain.target, "string-width@5.1.2");
    }

    #[test]
    fn peer_suffixed_edge_value_resolves_to_the_suffixed_key() {
        let g = build(
            r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0:
    resolution: {integrity: sha512-a}
  eslint@9.39.2:
    resolution: {integrity: sha512-b}
  jiti@2.6.1:
    resolution: {integrity: sha512-c}
snapshots:
  a@1.0.0:
    dependencies:
      eslint: 9.39.2(jiti@2.6.1)
  'eslint@9.39.2(jiti@2.6.1)':
    dependencies:
      jiti: 2.6.1
  jiti@2.6.1: {}
"#,
        );
        assert_eq!(
            g.nodes["a@1.0.0"].edges[0].target,
            "eslint@9.39.2(jiti@2.6.1)"
        );
    }

    #[test]
    fn peer_instances_are_separate_nodes_sharing_one_packages_entry() {
        let g = build(
            r#"
lockfileVersion: '9.0'
importers: {}
packages:
  dom@1.0.0:
    resolution: {integrity: sha512-a}
  react@17.0.0:
    resolution: {integrity: sha512-b}
  react@18.0.0:
    resolution: {integrity: sha512-c}
snapshots:
  'dom@1.0.0(react@17.0.0)':
    dependencies: {react: 17.0.0}
  'dom@1.0.0(react@18.0.0)':
    dependencies: {react: 18.0.0}
  react@17.0.0: {}
  react@18.0.0: {}
"#,
        );
        assert!(g.nodes.contains_key("dom@1.0.0(react@17.0.0)"));
        assert!(g.nodes.contains_key("dom@1.0.0(react@18.0.0)"));
        assert_ne!(
            g.nodes["dom@1.0.0(react@17.0.0)"].target_name,
            g.nodes["dom@1.0.0(react@18.0.0)"].target_name,
            "distinct instances need distinct Buck targets"
        );
    }

    #[test]
    fn roots_carry_their_importer_and_kind() {
        let g = build(
            r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      a: {specifier: ^1, version: 1.0.0}
  packages/app:
    devDependencies:
      a: {specifier: ^1, version: 1.0.0}
    optionalDependencies:
      b: {specifier: ^1, version: 1.0.0}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  b@1.0.0: {resolution: {integrity: sha512-b}}
snapshots:
  a@1.0.0: {}
  b@1.0.0: {}
"#,
        );
        assert_eq!(g.roots.len(), 3);
        assert!(
            g.roots
                .iter()
                .any(|r| r.importer == "." && r.kind == RootKind::Prod)
        );
        assert!(
            g.roots
                .iter()
                .any(|r| r.importer == "packages/app" && r.kind == RootKind::Dev)
        );
        assert!(g.roots.iter().any(|r| r.kind == RootKind::Optional));
    }

    #[test]
    fn workspace_specifier_root_is_recorded_but_not_resolved() {
        let g = build(
            r#"
lockfileVersion: '9.0'
importers:
  packages/app:
    dependencies:
      '@fixture/lib': {specifier: 'workspace:*', version: link:../lib}
packages: {}
snapshots: {}
"#,
        );
        let r = &g.roots[0];
        assert_eq!(r.link_name, "@fixture/lib");
        assert!(r.target.is_none(), "a link: root resolves to no node at S1");
        assert_eq!(r.specifier, "workspace:*");
    }

    #[test]
    fn optional_edges_are_tagged() {
        let g = build(
            r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  b@1.0.0: {resolution: {integrity: sha512-b}}
snapshots:
  a@1.0.0:
    optionalDependencies: {b: 1.0.0}
  b@1.0.0: {}
"#,
        );
        assert_eq!(g.nodes["a@1.0.0"].edges[0].kind, EdgeKind::Optional);
    }

    #[test]
    fn missing_package_metadata_names_snapshot_and_base() {
        let (lf, _) = parse_lockfile(
            "lockfileVersion: '9.0'\nimporters: {}\npackages: {}\nsnapshots:\n  a@1.0.0: {}\n",
            Path::new("/x"),
        )
        .unwrap();
        let e = Graph::build(&lf).unwrap_err();
        let m = format!("{e}");
        assert!(m.contains("a@1.0.0"), "{m}");
    }

    #[test]
    fn unresolved_edge_names_source_link_and_target() {
        let (lf, _) = parse_lockfile(
            r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
snapshots:
  a@1.0.0:
    dependencies: {ghost: 9.9.9}
"#,
            Path::new("/x"),
        )
        .unwrap();
        let e = Graph::build(&lf).unwrap_err();
        let m = format!("{e}");
        assert!(
            m.contains("a@1.0.0") && m.contains("ghost") && m.contains("ghost@9.9.9"),
            "{m}"
        );
    }

    /// The brief's version of this test only asserted that the two strings
    /// mangle to the same target name — it never drove `Graph::build`, so it
    /// would pass even if collision detection were entirely absent. This
    /// strengthens it: `x@1.0.0(a@1.0.0)` flattens (via `depPathToFilename`'s
    /// `(`/`)` -> `_` escaping) to the literal string `x@1.0.0_a@1.0.0` —
    /// which, read back as a snapshot key in its own right, has no parens and
    /// so `target_name` is the identity on it. Both keys therefore really do
    /// collide under the real algorithm, and both are keys a lockfile could
    /// contain simultaneously.
    #[test]
    fn target_name_collision_is_an_error() {
        let a = "x@1.0.0(a@1.0.0)";
        let b = "x@1.0.0_a@1.0.0";
        assert_eq!(
            crate::lock::snapshot_key::target_name(a),
            crate::lock::snapshot_key::target_name(b),
            "sanity: the two keys must actually mangle to the same target name"
        );

        let (lf, _) = parse_lockfile(
            r#"
lockfileVersion: '9.0'
importers: {}
packages:
  x@1.0.0:
    resolution: {integrity: sha512-a}
  x@1.0.0_a@1.0.0:
    resolution: {integrity: sha512-b}
  a@1.0.0:
    resolution: {integrity: sha512-c}
snapshots:
  'x@1.0.0(a@1.0.0)': {}
  x@1.0.0_a@1.0.0: {}
  a@1.0.0: {}
"#,
            Path::new("/x"),
        )
        .unwrap();
        let e = Graph::build(&lf).unwrap_err();
        assert!(
            matches!(e, LockError::TargetNameCollision { .. }),
            "expected a TargetNameCollision, got {e:?}"
        );
        let m = format!("{e}");
        assert!(
            m.contains("x@1.0.0(a@1.0.0)") && m.contains("x@1.0.0_a@1.0.0"),
            "{m}"
        );
    }

    #[test]
    fn edges_are_sorted_by_link_name() {
        let g = build(
            r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  m@1.0.0: {resolution: {integrity: sha512-m}}
  z@1.0.0: {resolution: {integrity: sha512-z}}
snapshots:
  a@1.0.0:
    dependencies: {z: 1.0.0, m: 1.0.0}
  m@1.0.0: {}
  z@1.0.0: {}
"#,
        );
        let names: Vec<_> = g.nodes["a@1.0.0"]
            .edges
            .iter()
            .map(|e| e.link_name.as_str())
            .collect();
        assert_eq!(names, vec!["m", "z"], "deterministic order");
    }

    #[test]
    fn duplicate_link_name_across_dependencies_and_optional_is_an_error() {
        let (lf, _) = parse_lockfile(
            r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  b@1.0.0: {resolution: {integrity: sha512-b}}
snapshots:
  a@1.0.0:
    dependencies: {b: 1.0.0}
    optionalDependencies: {b: 1.0.0}
  b@1.0.0: {}
"#,
            Path::new("/x"),
        )
        .unwrap();
        let e = Graph::build(&lf).unwrap_err();
        assert!(
            matches!(e, LockError::DuplicateLinkName { .. }),
            "expected a DuplicateLinkName, got {e:?}"
        );
        let m = format!("{e}");
        assert!(m.contains("a@1.0.0") && m.contains('b'), "{m}");
    }

    /// Regression coverage for the three shapes verified by hand in review:
    /// a future edit to `resolve_edge` is most likely to break these
    /// silently, since none was previously asserted directly.
    #[test]
    fn resolve_edge_handles_scoped_and_peer_shapes() {
        assert_eq!(
            resolve_edge("foo", "@scope/bar@1.0.0"),
            "@scope/bar@1.0.0",
            "a scoped alias target is used verbatim"
        );
        assert_eq!(
            resolve_edge("@scope/foo", "1.0.0"),
            "@scope/foo@1.0.0",
            "a scoped link name with a bare version still concatenates"
        );
        assert_eq!(
            resolve_edge("foo", "1.0.0(bar@2.0.0)"),
            "foo@1.0.0(bar@2.0.0)",
            "an '@' inside the peer suffix must not be mistaken for an alias"
        );
    }
}

#[cfg(test)]
mod cycle_tests {
    use super::*;
    use crate::lock::parse_lockfile;
    use std::path::Path;

    fn build(yaml: &str) -> Graph {
        let (lf, _) = parse_lockfile(yaml, Path::new("/x")).unwrap();
        Graph::build(&lf).unwrap()
    }

    #[test]
    fn two_node_cycle_is_recorded_and_does_not_error() {
        let g = build(
            r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  b@1.0.0: {resolution: {integrity: sha512-b}}
snapshots:
  a@1.0.0:
    dependencies: {b: 1.0.0}
  b@1.0.0:
    dependencies: {a: 1.0.0}
"#,
        );
        assert_eq!(g.cycles.len(), 1, "one cycle: {:?}", g.cycles);
        // Closed path: the entry node repeats at the end (S1 spec §10).
        assert_eq!(
            g.cycles[0],
            vec![
                "a@1.0.0".to_string(),
                "b@1.0.0".to_string(),
                "a@1.0.0".to_string()
            ]
        );
    }

    #[test]
    fn self_edge_is_a_cycle_of_length_one() {
        let g = build(
            r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
snapshots:
  a@1.0.0:
    dependencies: {a: 1.0.0}
"#,
        );
        assert_eq!(g.cycles.len(), 1, "{:?}", g.cycles);
        // Closed path of length one: the entry node repeats immediately.
        assert_eq!(
            g.cycles[0],
            vec!["a@1.0.0".to_string(), "a@1.0.0".to_string()]
        );
    }

    #[test]
    fn an_acyclic_graph_reports_no_cycles() {
        let g = build(
            r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  b@1.0.0: {resolution: {integrity: sha512-b}}
snapshots:
  a@1.0.0:
    dependencies: {b: 1.0.0}
  b@1.0.0: {}
"#,
        );
        assert!(g.cycles.is_empty());
    }

    #[test]
    fn deep_chain_does_not_overflow_the_stack() {
        // A recursive DFS blows the stack here. Real lockfiles reach 800+
        // nodes; this is the same shape, larger.
        const N: usize = 10_000;
        let mut y = String::from("lockfileVersion: '9.0'\nimporters: {}\npackages:\n");
        for i in 0..N {
            y.push_str(&format!(
                "  p{i}@1.0.0: {{resolution: {{integrity: sha512-x}}}}\n"
            ));
        }
        y.push_str("snapshots:\n");
        for i in 0..N {
            y.push_str(&format!("  p{i}@1.0.0:\n"));
            if i + 1 < N {
                y.push_str(&format!("    dependencies: {{p{}: 1.0.0}}\n", i + 1));
            }
        }
        let g = build(&y);
        assert_eq!(g.nodes.len(), N);
        assert!(g.cycles.is_empty());
    }

    #[test]
    fn cycles_are_deterministic_across_runs() {
        let y = r#"
lockfileVersion: '9.0'
importers: {}
packages:
  a@1.0.0: {resolution: {integrity: sha512-a}}
  b@1.0.0: {resolution: {integrity: sha512-b}}
  c@1.0.0: {resolution: {integrity: sha512-c}}
snapshots:
  a@1.0.0: {dependencies: {b: 1.0.0}}
  b@1.0.0: {dependencies: {c: 1.0.0}}
  c@1.0.0: {dependencies: {a: 1.0.0}}
"#;
        let g1 = build(y);
        let g2 = build(y);
        assert_eq!(g1.cycles, g2.cycles);
        // A deterministic-but-wrong implementation (e.g. always returning an
        // empty Vec, or one fixed rotation regardless of traversal order)
        // would also satisfy the equality above. Pin down the actual content
        // so this test can't pass against such an implementation.
        assert_eq!(g1.cycles.len(), 1, "{:?}", g1.cycles);
        // Closed path: first and last entries are the same (entry) node,
        // and the interior is the full a-b-c cycle in exactly one order.
        assert_eq!(
            g1.cycles[0],
            vec![
                "a@1.0.0".to_string(),
                "b@1.0.0".to_string(),
                "c@1.0.0".to_string(),
                "a@1.0.0".to_string(),
            ]
        );
    }

    #[test]
    fn cycle_reachable_from_two_roots_is_reported_once() {
        // x -> a -> b -> a (cycle), and y -> a -> b -> a (same cycle, found
        // again from a different DFS start). Must be deduplicated.
        let g = build(
            r#"
lockfileVersion: '9.0'
importers: {}
packages:
  x@1.0.0: {resolution: {integrity: sha512-x}}
  y@1.0.0: {resolution: {integrity: sha512-y}}
  a@1.0.0: {resolution: {integrity: sha512-a}}
  b@1.0.0: {resolution: {integrity: sha512-b}}
snapshots:
  x@1.0.0: {dependencies: {a: 1.0.0}}
  y@1.0.0: {dependencies: {a: 1.0.0}}
  a@1.0.0: {dependencies: {b: 1.0.0}}
  b@1.0.0: {dependencies: {a: 1.0.0}}
"#,
        );
        assert_eq!(g.cycles.len(), 1, "{:?}", g.cycles);
        // Closed path, and the closing repeat of the entry node must not
        // have caused the dedup key (a node set) to treat this as two
        // cycles: the set {a, b} is unaffected by the trailing repeat.
        assert_eq!(
            g.cycles[0],
            vec![
                "a@1.0.0".to_string(),
                "b@1.0.0".to_string(),
                "a@1.0.0".to_string()
            ]
        );
    }
}
