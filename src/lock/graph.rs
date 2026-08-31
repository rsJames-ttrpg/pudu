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

fn strip_peer_suffix(s: &str) -> &str {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => {
                if depth == 0 {
                    return &s[..i];
                }
                depth += 1;
            }
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    s
}

/// True for a specifier that points at another importer rather than a package.
fn is_link_specifier(specifier: &str, version: &str) -> bool {
    specifier.starts_with("workspace:")
        || specifier.starts_with("link:")
        || specifier.starts_with("file:")
        || version.starts_with("link:")
        || version.starts_with("file:")
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

        Ok(Self {
            nodes,
            roots,
            cycles: Vec::new(),
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
}
