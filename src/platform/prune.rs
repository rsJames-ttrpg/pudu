//! Per-platform pruning of the instance graph.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::config::Platform;
use crate::error::PlatformWarning;
use crate::lock::graph::{EdgeKind, Graph};
use crate::platform::admits_platform;

/// A non-optional edge dropped because its target is excluded here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DroppedEdge {
    /// Snapshot key of the package that declared the dependency.
    pub dependent: String,
    /// The `node_modules/` link name, which may differ from the target's own
    /// name when the edge is an npm alias.
    pub link_name: String,
    /// Snapshot key of the excluded dependency.
    pub target: String,
}

/// One platform's view of the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlatformView {
    /// Snapshot keys that survive here.
    pub nodes: BTreeSet<String>,
    /// Snapshot keys excluded here. Stored rather than recomputed because
    /// `pudu debug platforms` prints it on every run; `nodes` and `pruned`
    /// partition the graph.
    pub pruned: BTreeSet<String>,
    /// Non-optional edges dropped because their target is excluded here.
    /// Optional edges dropped this way are the normal case — every
    /// `@esbuild/*` on every platform but one — and are not retained.
    pub dropped_required_edges: Vec<DroppedEdge>,
}

/// The per-platform view of the graph, and its transpose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Matrix {
    /// One view per configured platform.
    pub views: BTreeMap<String, PlatformView>,
    /// For each package that survives somewhere, the platforms it is on.
    /// S4 turns this into `select()` keys. A package on no platform is
    /// absent entirely rather than present with an empty set, so a
    /// `select()` with no arms cannot be generated from it.
    pub platforms_by_node: BTreeMap<String, BTreeSet<String>>,
}

/// Prune the graph for each configured platform.
///
/// A node survives iff its own `os`/`cpu`/`libc` admit the platform. This is
/// a per-package decision consulting only that package's fields — not its
/// parents, not its dependencies.
///
/// There is deliberately **no reachability sweep**: a node that survives but
/// has become unreachable from every root stays in the view. Per-package
/// matching alone reproduced pnpm's install set exactly on all four captured
/// oracles (survey §5), so a sweep would be unverifiable extra machinery.
/// The bound on that evidence: all 90 platform-gated keys in the fixture are
/// leaves, so it cannot distinguish the two designs. Were a gated package
/// with its own subtree to appear, the failure mode is an orphan left in the
/// store — fat, not incorrect. Tracked as TD-S2-01.
pub fn prune(
    graph: &Graph,
    platforms: &BTreeMap<String, Platform>,
) -> (Matrix, Vec<PlatformWarning>) {
    let mut views = BTreeMap::new();
    let mut platforms_by_node: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut warnings = Vec::new();

    for (platform_name, platform) in platforms {
        let mut nodes = BTreeSet::new();
        let mut pruned = BTreeSet::new();

        for (key, node) in &graph.nodes {
            if admits_platform(&node.meta, platform) {
                nodes.insert(key.clone());
                platforms_by_node
                    .entry(key.clone())
                    .or_default()
                    .insert(platform_name.clone());
            } else {
                pruned.insert(key.clone());
            }
        }

        // An edge is dropped when its target is excluded here. Edges from a
        // dependent that is itself excluded are not "dropped" — the whole
        // subtree is absent, and reporting them would be noise.
        let mut dropped_required_edges = Vec::new();
        for key in &nodes {
            let node = &graph.nodes[key];
            for edge in &node.edges {
                if nodes.contains(&edge.target) {
                    continue;
                }
                if edge.kind == EdgeKind::Optional {
                    continue;
                }
                dropped_required_edges.push(DroppedEdge {
                    dependent: key.clone(),
                    link_name: edge.link_name.clone(),
                    target: edge.target.clone(),
                });
                warnings.push(PlatformWarning::RequiredDependencyExcluded {
                    dependent: key.clone(),
                    target: edge.target.clone(),
                    platform: platform_name.clone(),
                });
            }
        }

        views.insert(
            platform_name.clone(),
            PlatformView {
                nodes,
                pruned,
                dropped_required_edges,
            },
        );
    }

    // Aggregated into one diagnostic: on real input this set is large (the
    // committed fixture has 90 gated packages against a handful of
    // platforms), and one warning per package would train the user to
    // ignore warnings.
    let excluded_everywhere: Vec<String> = graph
        .nodes
        .keys()
        .filter(|k| !platforms_by_node.contains_key(*k))
        .cloned()
        .collect();
    if !excluded_everywhere.is_empty() {
        warnings.push(PlatformWarning::ExcludedEverywhere {
            packages: excluded_everywhere,
            platforms: platforms.keys().cloned().collect(),
        });
    }

    (
        Matrix {
            views,
            platforms_by_node,
        },
        warnings,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::graph::{Edge, EdgeKind, Node};
    use crate::lock::types::{PackageMeta, Resolution};
    use crate::platform::{Cpu, Libc, Os};

    fn meta(os: Option<&[&str]>, cpu: Option<&[&str]>) -> PackageMeta {
        PackageMeta {
            resolution: Resolution::Integrity {
                integrity: "sha512-t".into(),
            },
            engines: Default::default(),
            os: os.map(|v| v.iter().map(|s| s.to_string()).collect()),
            cpu: cpu.map(|v| v.iter().map(|s| s.to_string()).collect()),
            libc: None,
            has_bin: false,
            deprecated: None,
            peer_dependencies: Default::default(),
            peer_dependencies_meta: Default::default(),
            bundled_dependencies: Vec::new(),
        }
    }

    fn node(name: &str, version: &str, m: PackageMeta, edges: Vec<Edge>) -> Node {
        Node {
            name: name.into(),
            version: version.into(),
            peers: Vec::new(),
            target_name: format!("{name}@{version}").replace('/', "+"),
            optional: false,
            meta: m,
            edges,
        }
    }

    fn edge(link: &str, target: &str, kind: EdgeKind) -> Edge {
        Edge {
            link_name: link.into(),
            target: target.into(),
            kind,
        }
    }

    fn graph(nodes: Vec<(&str, Node)>) -> Graph {
        Graph {
            nodes: nodes.into_iter().map(|(k, n)| (k.to_string(), n)).collect(),
            roots: Vec::new(),
            cycles: Vec::new(),
        }
    }

    fn platforms() -> BTreeMap<String, Platform> {
        BTreeMap::from([
            (
                "linux-x64-gnu".to_string(),
                Platform {
                    os: Os::Linux,
                    cpu: Cpu::X64,
                    libc: Some(Libc::Glibc),
                    constraints: None,
                },
            ),
            (
                "darwin-arm64".to_string(),
                Platform {
                    os: Os::Darwin,
                    cpu: Cpu::Arm64,
                    libc: None,
                    constraints: None,
                },
            ),
        ])
    }

    #[test]
    fn a_gated_package_survives_only_where_its_fields_admit() {
        let g = graph(vec![
            ("app@1.0.0", node("app", "1.0.0", meta(None, None), vec![])),
            (
                "fsevents@2.3.3",
                node("fsevents", "2.3.3", meta(Some(&["darwin"]), None), vec![]),
            ),
        ]);
        let (m, _) = prune(&g, &platforms());
        assert!(m.views["linux-x64-gnu"].nodes.contains("app@1.0.0"));
        assert!(!m.views["linux-x64-gnu"].nodes.contains("fsevents@2.3.3"));
        assert!(m.views["linux-x64-gnu"].pruned.contains("fsevents@2.3.3"));
        assert!(m.views["darwin-arm64"].nodes.contains("fsevents@2.3.3"));
    }

    /// `nodes` and `pruned` partition the graph — every key is in exactly
    /// one of them, on every platform.
    #[test]
    fn nodes_and_pruned_partition_the_graph() {
        let g = graph(vec![
            ("app@1.0.0", node("app", "1.0.0", meta(None, None), vec![])),
            (
                "fsevents@2.3.3",
                node("fsevents", "2.3.3", meta(Some(&["darwin"]), None), vec![]),
            ),
            (
                "win-only@1.0.0",
                node("win-only", "1.0.0", meta(Some(&["win32"]), None), vec![]),
            ),
        ]);
        let (m, _) = prune(&g, &platforms());
        for (name, view) in &m.views {
            assert_eq!(
                view.nodes.len() + view.pruned.len(),
                g.nodes.len(),
                "{name} must partition the graph"
            );
            assert!(view.nodes.is_disjoint(&view.pruned), "{name} overlaps");
        }
    }

    #[test]
    fn platforms_by_node_is_the_transpose_of_views() {
        let g = graph(vec![
            ("app@1.0.0", node("app", "1.0.0", meta(None, None), vec![])),
            (
                "fsevents@2.3.3",
                node("fsevents", "2.3.3", meta(Some(&["darwin"]), None), vec![]),
            ),
        ]);
        let (m, _) = prune(&g, &platforms());
        assert_eq!(
            m.platforms_by_node["app@1.0.0"],
            BTreeSet::from(["darwin-arm64".to_string(), "linux-x64-gnu".to_string()])
        );
        assert_eq!(
            m.platforms_by_node["fsevents@2.3.3"],
            BTreeSet::from(["darwin-arm64".to_string()])
        );
    }

    /// A package on no platform must not appear in the transpose at all —
    /// an empty entry would make S4 emit a `select()` with no arms.
    #[test]
    fn a_package_on_no_platform_is_absent_from_the_transpose() {
        let g = graph(vec![(
            "win-only@1.0.0",
            node("win-only", "1.0.0", meta(Some(&["win32"]), None), vec![]),
        )]);
        let (m, _) = prune(&g, &platforms());
        assert!(!m.platforms_by_node.contains_key("win-only@1.0.0"));
    }

    #[test]
    fn a_dropped_optional_edge_is_silent() {
        let g = graph(vec![
            (
                "esbuild@0.25.12",
                node(
                    "esbuild",
                    "0.25.12",
                    meta(None, None),
                    vec![edge(
                        "@esbuild/darwin-arm64",
                        "@esbuild/darwin-arm64@0.25.12",
                        EdgeKind::Optional,
                    )],
                ),
            ),
            (
                "@esbuild/darwin-arm64@0.25.12",
                node(
                    "@esbuild/darwin-arm64",
                    "0.25.12",
                    meta(Some(&["darwin"]), None),
                    vec![],
                ),
            ),
        ]);
        let (m, warnings) = prune(&g, &platforms());
        assert!(m.views["linux-x64-gnu"].dropped_required_edges.is_empty());
        assert!(
            !warnings
                .iter()
                .any(|w| matches!(w, PlatformWarning::RequiredDependencyExcluded { .. })),
            "an excluded optional dependency is the normal case and must be silent"
        );
    }

    #[test]
    fn a_dropped_required_edge_warns_and_is_recorded() {
        let g = graph(vec![
            (
                "my-app@1.0.0",
                node(
                    "my-app",
                    "1.0.0",
                    meta(None, None),
                    vec![edge("fsevents", "fsevents@2.3.3", EdgeKind::Prod)],
                ),
            ),
            (
                "fsevents@2.3.3",
                node("fsevents", "2.3.3", meta(Some(&["darwin"]), None), vec![]),
            ),
        ]);
        let (m, warnings) = prune(&g, &platforms());

        let dropped = &m.views["linux-x64-gnu"].dropped_required_edges;
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].dependent, "my-app@1.0.0");
        assert_eq!(dropped[0].link_name, "fsevents");
        assert_eq!(dropped[0].target, "fsevents@2.3.3");
        assert!(m.views["darwin-arm64"].dropped_required_edges.is_empty());

        let w: Vec<_> = warnings
            .iter()
            .filter(|w| matches!(w, PlatformWarning::RequiredDependencyExcluded { .. }))
            .collect();
        assert_eq!(w.len(), 1, "one warning for one dropped required edge");
    }

    /// An edge whose dependent is itself pruned on that platform is not a
    /// dropped edge — the whole subtree is simply absent, and warning about
    /// it would be noise.
    #[test]
    fn an_edge_from_a_pruned_dependent_does_not_warn() {
        let g = graph(vec![
            (
                "win-tool@1.0.0",
                node(
                    "win-tool",
                    "1.0.0",
                    meta(Some(&["win32"]), None),
                    vec![edge("fsevents", "fsevents@2.3.3", EdgeKind::Prod)],
                ),
            ),
            (
                "fsevents@2.3.3",
                node("fsevents", "2.3.3", meta(Some(&["darwin"]), None), vec![]),
            ),
        ]);
        let (m, warnings) = prune(&g, &platforms());
        assert!(m.views["linux-x64-gnu"].dropped_required_edges.is_empty());
        assert!(
            !warnings
                .iter()
                .any(|w| matches!(w, PlatformWarning::RequiredDependencyExcluded { .. })),
            "the dependent is absent here; its edges are not 'dropped'"
        );
    }

    #[test]
    fn excluded_everywhere_warns_once_for_all_such_packages() {
        let g = graph(vec![
            ("app@1.0.0", node("app", "1.0.0", meta(None, None), vec![])),
            (
                "a-win@1.0.0",
                node("a-win", "1.0.0", meta(Some(&["win32"]), None), vec![]),
            ),
            (
                "b-win@1.0.0",
                node("b-win", "1.0.0", meta(Some(&["win32"]), None), vec![]),
            ),
            (
                "c-aix@1.0.0",
                node("c-aix", "1.0.0", meta(Some(&["aix"]), None), vec![]),
            ),
        ]);
        let (_, warnings) = prune(&g, &platforms());
        let everywhere: Vec<_> = warnings
            .iter()
            .filter_map(|w| match w {
                PlatformWarning::ExcludedEverywhere { packages, .. } => Some(packages),
                _ => None,
            })
            .collect();
        assert_eq!(everywhere.len(), 1, "exactly one aggregated warning");
        assert_eq!(
            everywhere[0],
            &vec![
                "a-win@1.0.0".to_string(),
                "b-win@1.0.0".to_string(),
                "c-aix@1.0.0".to_string()
            ]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>(),
            "sorted, all three, once"
        );
    }

    #[test]
    fn no_excluded_everywhere_warning_when_every_package_survives_somewhere() {
        let g = graph(vec![
            ("app@1.0.0", node("app", "1.0.0", meta(None, None), vec![])),
            (
                "fsevents@2.3.3",
                node("fsevents", "2.3.3", meta(Some(&["darwin"]), None), vec![]),
            ),
        ]);
        let (_, warnings) = prune(&g, &platforms());
        assert!(
            !warnings
                .iter()
                .any(|w| matches!(w, PlatformWarning::ExcludedEverywhere { .. })),
            "every package is on some platform"
        );
    }

    /// Determinism: warnings come out in a stable order regardless of how
    /// the graph was built, because every collection is a BTree.
    #[test]
    fn output_is_deterministic_across_runs() {
        let g = graph(vec![
            ("app@1.0.0", node("app", "1.0.0", meta(None, None), vec![])),
            (
                "a-win@1.0.0",
                node("a-win", "1.0.0", meta(Some(&["win32"]), None), vec![]),
            ),
            (
                "z-win@1.0.0",
                node("z-win", "1.0.0", meta(Some(&["win32"]), None), vec![]),
            ),
        ]);
        let (m1, w1) = prune(&g, &platforms());
        let (m2, w2) = prune(&g, &platforms());
        assert_eq!(m1.views, m2.views);
        assert_eq!(m1.platforms_by_node, m2.platforms_by_node);
        assert_eq!(w1, w2);
    }
}
