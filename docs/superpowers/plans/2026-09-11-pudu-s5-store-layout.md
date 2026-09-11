# Pudu S5 — Store layout, `node_binary`, `buck2 run` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the package targets S4 emits into a runnable pnpm-shaped store, so `buck2 run` executes first-party JavaScript against real dependencies.

**Architecture:** A pure Rust pass (`src/buck/store.rs`) turns the instance graph into two flat maps — real directories to materialize and relative symlinks to create. A new Buck2 rule in the generated `pudu.bzl` consumes them through a JSON manifest and builds the tree in one action, hardlinking store entries. `node_binary` composes a second action: first-party sources copied, one `node_modules` symlink pointing into the tree.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), Starlark/Buck2 (pinned release `2026-08-22`), Node (toolchain-supplied), insta snapshots, assert_cmd + httpmock.

**Spec:** [`docs/superpowers/specs/2026-09-10-pudu-s5-store-layout-design.md`](../specs/2026-09-10-pudu-s5-store-layout-design.md)

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`. CI builds with `-D warnings`; `cargo clippy` and `cargo fmt --check` must pass.
- Every generated-file map is a `BTreeMap`. Determinism is a tested property, not a convention.
- Third-party text reaching Starlark goes through `format::starlark_string`. Third-party text never reaches a command line at all — that is what the JSON manifest is for (spec §1.5).
- buck2 is pinned to release tag `2026-08-22` for both the binary and the vendored prelude. Do not upgrade either in this stage.
- Generated files get their mode from `fsutil::probe_umask_mode`, never tempfile's `0600`.
- No new third-party Rust dependencies.

---

## Facts resolved before this plan was written

Do not re-derive these. They were measured, not assumed.

**Bin files are already executable, and extraction preserves the bit.**
`semver@7.6.3` ships `package/bin/semver.js` as `-rwxr-xr-x`, and after
`buck2 build '//third-party/js:semver@7.6.3[bin/semver]'` the extracted file is
still `-rwxr-xr-x`. Spec §3.3 raised chmod as an open question; it is answered
**no chmod**. `.bin` entries are symlinks, exactly as pnpm makes them, and they
inherit the tarball's mode.

The residual case — a package shipping a bin at `0644` — is left as a
documented limitation, filed as debt in Task 7. Do **not** "fix" it by
chmodding: with hardlinked store entries, a chmod mutates the inode shared with
buck2's extraction output and corrupts a cached artifact. That is the §1.4
hazard firing from inside our own builder.

**The fixture has no dependency edges at all.** All four packages in
`01-pure-js` are leaves. Sibling links — the part of §3.1 most likely to be
wrong — cannot be exercised by it. Task 5 adds a real graph. These are the
packages, with integrity strings already resolved so no implementer needs to go
looking:

| Key | integrity | dependencies |
|---|---|---|
| `debug@4.3.4` | `sha512-PRWFHuSU3eDtQJPvnNY7Jcket1j0t5OuOsFzPPzsekD52Zl8qUfFIPEiswXqIvHWGVHOgX+7G/vCNNhehwxfkQ==` | `ms: 2.1.2` |
| `ms@2.1.2` | `sha512-sGkPx+VjMtmA6MX27oA4FBFELFCZZ4S4XqeGOXCv68tT+jb3vk/RyaKWP0PTKyWtmLSM0b+adUTEvbs1PEaH2w==` | — |
| `@jridgewell/gen-mapping@0.3.5` | `sha512-IzL8ZoEDIBRWEzlCcRhOaCupYyN5gdIK+Q6fbFdPDg6HqX6jpkItn7DFIpW9LQzXG6Df9sA7+OKnq0qlz/GaQg==` | `@jridgewell/set-array: ^1.2.1`, `@jridgewell/trace-mapping: ^0.3.24`, `@jridgewell/sourcemap-codec: ^1.4.10` |
| `@jridgewell/set-array@1.2.1` | `sha512-R8gLRTZeyp03ymzP/6Lil/28tGeGEzhx1q2k703KGWRAI1VdvPIXdG70VJc2pAMw3NA6JKL5hhFu1sJX0Mnn/A==` | — |
| `@jridgewell/sourcemap-codec@1.5.0` | `sha512-gv3ZRaISU3fjPAgNsriBRqGWQL6quFx04YMPW/zD8XMLsU32mhCCbfbO6KZFLjvYpCZ8zyDEgqsgf+PwPaM7GQ==` | — |
| `@jridgewell/trace-mapping@0.3.25` | `sha512-vNk6aEwybGtawWmy/PzwnGDOjCkLWSD2wqvjGGAgOAwCGWySYXfYoxt00IJkTF+8Lb57DwOb3Aa0o9CApepiYQ==` | `@jridgewell/resolve-uri: ^3.1.0`, `@jridgewell/sourcemap-codec: ^1.4.14` |
| `@jridgewell/resolve-uri@3.1.2` | `sha512-bRISgCIjP20/tbWSPWMEi54QVPRZExkuD9lJL+UIxUKtwVJA8wW1Trb1jMs1RFXo1CBTNZ/5hpC9QvmKWdopKw==` | — |

That graph is chosen deliberately: `debug → ms` is the unscoped sibling case
S4 §1.4 failed on; the `@jridgewell/*` chain is scoped-depending-on-scoped,
which is the `../` arithmetic most likely to be wrong; and
`@jridgewell/sourcemap-codec` is depended on by two different packages, so the
copies map must deduplicate.

**S5 emits one tree over the union of platform views.** The fixture configures
`linux-x64-gnu` and `linux-x64-musl`, but no fixture package is platform-gated,
so the two views are identical and the union is exact. This is only correct
while that holds; Task 4 adds a warning for when it stops.

---

## The path arithmetic

Every symlink target in the tree is relative, and the number of `../` segments
is derived from the link's own path. Getting it wrong produces a dangling
symlink that **builds clean** and fails only when Node runs (spec §1.3). This
is the table to implement against.

Let `tn(k)` be `target_name(k)` (the mangled `.pnpm` directory name) and
`dir(k)` the package's directory under `node_modules/` — one segment normally,
two for a scoped package (`@types/node`).

| Kind | Destination | Target | `../` count |
|---|---|---|---|
| Top-level link | `<link_name>` | `.pnpm/<tn(t)>/node_modules/<dir(t)>` | `segments(link_name) - 1` |
| Sibling link | `.pnpm/<tn(k)>/node_modules/<link_name>` | `<tn(t)>/node_modules/<dir(t)>` | `2 + segments(link_name) - 1` |
| Top-level bin | `.bin/<bin_name>` | `.pnpm/<tn(t)>/node_modules/<dir(t)>/<bin_path>` | `1` |
| Package bin | `.pnpm/<tn(k)>/node_modules/.bin/<bin_name>` | `<tn(t)>/node_modules/<dir(t)>/<bin_path>` | `3` |

Worked, from the spike's verified output:

- `.pnpm/debug@4.3.4/node_modules/ms` → `../../ms@2.1.2/node_modules/ms` (2)
- `.pnpm/x@1.0.0/node_modules/@types/node` → `../../../@types+node@22.20.0/node_modules/@types/node` (3)
- `@types/node` at top level → `../.pnpm/@types+node@22.20.0/node_modules/@types/node` (1)
- `left-pad` at top level → `.pnpm/left-pad@1.3.0/node_modules/left-pad` (0)

---

## File structure

| File | Responsibility |
|---|---|
| `src/buck/store.rs` | **New.** Graph + importer → `Tree { copies, links }`. Pure: no I/O, no Starlark, no label strings. This is where the `../` arithmetic lives, and it is separable from rendering precisely because it is the error-prone part. |
| `src/buck/bzl.rs` | Extended with the `node_modules_tree`, `node_binary` and `node_test` rule bodies and the embedded JS builder. |
| `src/buck/emit.rs` | Extended to render tree targets after the package targets. |
| `src/buck/mod.rs` | `generate()` grows a `trees` parameter. |
| `src/cli/buckify.rs` | Computes trees, warns on divergent platform views. |
| `src/error.rs` | One new `BuckError` variant, `ImporterNameCollision`. |
| `tests/fixtures/buck/01-pure-js/` | Gains seven packages with real edges, and a runnable importer. |
| `.github/workflows/ci.yml` | `buck2 build` → `buck2 run` with a stdout assertion. |

---

## Task 1: Store-path computation

**Files:**
- Create: `src/buck/store.rs`
- Modify: `src/buck/mod.rs` (add `pub mod store;`)

**Interfaces:**
- Consumes: `crate::lock::graph::{Graph, Node, Edge, Root}`, `crate::lock::snapshot_key::target_name`.
- Produces:
  ```rust
  pub struct Tree {
      /// Destination path in the tree → snapshot key whose `[root]` goes there.
      pub copies: BTreeMap<String, String>,
      /// Destination path in the tree → relative symlink target.
      pub links: BTreeMap<String, String>,
  }
  pub fn build(
      graph: &Graph,
      nodes: &BTreeSet<String>,
      entries: &BTreeMap<String, Entry>,
      importer: &str,
  ) -> Tree;
  ```
  `nodes` is the surviving set for the platform view being emitted. Keys absent
  from it are skipped, so pruning is honoured without `store` knowing what a
  platform is. `entries` is the package table, consulted only for `bin` — the
  lockfile does not record bin paths, `packages.toml` does (S3 design §6).

- [ ] **Step 1: Write the failing tests**

Create `src/buck/store.rs` with only the test module and the signatures it
calls. Build the graph through `Graph::build` on a small inline lockfile so the
test exercises the real edge-resolution path rather than hand-built `Node`s —
this is TD-S4-04's lesson: a renderer test that hand-supplies its input cannot
catch wiring.

```rust
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
snapshots:
  debug@4.3.4:
    dependencies:
      ms: 2.1.2
  ms@2.1.2: {}
  '@jridgewell/gen-mapping@0.3.5':
    dependencies:
      '@jridgewell/set-array': 1.2.1
  '@jridgewell/set-array@1.2.1': {}
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
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib buck::store`
Expected: compile error — `store` module does not exist.

- [ ] **Step 3: Write the implementation**

Above the test module in `src/buck/store.rs`:

```rust
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

pub fn build(
    graph: &Graph,
    nodes: &BTreeSet<String>,
    entries: &BTreeMap<String, Entry>,
    importer: &str,
) -> Tree {
    let mut tree = Tree::default();

    // Copies and sibling links, one pass over the surviving nodes.
    for (key, node) in &graph.nodes {
        if !nodes.contains(key) {
            continue;
        }
        let dir = &node.name;
        tree.copies.insert(store_path(key, dir), key.clone());

        for edge in &node.edges {
            let Some(target) = graph.nodes.get(&edge.target) else {
                continue;
            };
            if !nodes.contains(&edge.target) {
                continue;
            }
            let dest = format!(
                ".pnpm/{}/node_modules/{}",
                target_name(key),
                edge.link_name
            );
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
        if !nodes.contains(key) {
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
```

Add `pub mod store;` to `src/buck/mod.rs` alongside the other `pub mod` lines.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib buck::store`
Expected: 9 passed.

- [ ] **Step 5: Prove the depth test actually gates**

This is the mutation step. Temporarily change `let climb = 2 + segments(&edge.link_name) - 1;` to `let climb = 2;` — the constant a careless implementation would write.

Run: `cargo test --lib buck::store`
Expected: `a_scoped_sibling_link_climbs_three_levels` FAILS. If it passes, the
test is not gating and must be fixed before continuing.

Revert the mutation. Re-run: 9 passed.

- [ ] **Step 6: Commit**

```bash
git add src/buck/store.rs src/buck/mod.rs
git commit -m "feat(buck): compute the pnpm store layout from the instance graph"
```

---

## Task 2: The `node_modules_tree` rule

**Files:**
- Modify: `src/buck/bzl.rs`

**Interfaces:**
- Consumes: nothing from Task 1 (this is the Starlark side).
- Produces: `bzl::render()` output now defines `node_modules_tree`, and a
  `NodeToolchainInfo` load from `:toolchains.bzl`.

- [ ] **Step 1: Write the failing tests**

Append to `src/buck/bzl.rs`'s test module:

```rust
#[test]
fn the_tree_rule_is_defined() {
    let s = render();
    assert!(s.contains("node_modules_tree = rule("));
    assert!(s.contains("ctx.actions.declare_output(\"node_modules\", dir = True)"));
}

#[test]
fn the_tree_rule_takes_node_from_the_toolchain() {
    let s = render();
    assert!(s.contains("load(\":toolchains.bzl\", \"NodeToolchainInfo\")"));
    assert!(s.contains("attrs.toolchain_dep(default = \"toolchains//:node\")"));
}

/// Package names, archive roots and bin paths reach the builder as JSON,
/// never as command-line arguments. This is what retires the `strip_prefix`
/// hazard class (S4 §1.4) by construction rather than by escaping.
#[test]
fn tree_inputs_travel_as_a_manifest_not_as_arguments() {
    let s = render();
    assert!(s.contains("ctx.actions.write_json("));
    assert!(s.contains("with_inputs = True"));
}

/// pnpm's own strategy, and what makes a tree cost O(entries) rather than
/// O(bytes). The copy fallback covers a cross-filesystem buck-out.
#[test]
fn the_builder_hardlinks_with_a_copy_fallback() {
    let s = render();
    assert!(s.contains("fs.linkSync"));
    assert!(s.contains("fs.copyFileSync"));
}

#[test]
fn the_macro_documents_why_the_tree_is_not_a_filegroup() {
    let s = render();
    assert!(
        s.contains("filegroup"),
        "the docstring must say why the obvious rule does not work, \
         or the next reader will try it again"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib buck::bzl`
Expected: the five new tests FAIL on `assert!` — `render()` has no tree rule yet.

- [ ] **Step 3: Extend the `BODY` constant**

Add to the top of `src/buck/bzl.rs`'s `BODY` (after the existing
`load("@prelude//:rules.bzl", "http_archive")` line):

```python
load(":toolchains.bzl", "NodeToolchainInfo")
```

Append to `BODY`:

```python
_BUILD_TREE = '''
const fs = require("fs");
const path = require("path");

const [, , manifestPath, outDir] = process.argv;
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));

// Hardlink rather than copy: this is pnpm's own strategy, and it makes a
// tree cost O(entries) instead of O(bytes). The inode is shared with
// buck2's extraction output, so nothing may ever write into the tree.
// linkSync fails across a filesystem boundary; copying is the fallback.
function place(from, to) {
  const st = fs.lstatSync(from);
  if (st.isDirectory()) {
    fs.mkdirSync(to, { recursive: true });
    for (const entry of fs.readdirSync(from)) {
      place(path.join(from, entry), path.join(to, entry));
    }
  } else if (st.isSymbolicLink()) {
    place(fs.realpathSync(from), to);
  } else {
    try {
      fs.linkSync(from, to);
    } catch (e) {
      fs.copyFileSync(from, to);
    }
  }
}

for (const [dest, src] of Object.entries(manifest.copies)) {
  const d = path.join(outDir, dest);
  fs.mkdirSync(path.dirname(d), { recursive: true });
  place(Array.isArray(src) ? src.join("") : src, d);
}

for (const [dest, target] of Object.entries(manifest.links)) {
  const d = path.join(outDir, dest);
  fs.mkdirSync(path.dirname(d), { recursive: true });
  fs.symlinkSync(Array.isArray(target) ? target.join("") : target, d);
}
'''

def _node_modules_tree_impl(ctx):
    """One importer's node_modules, shaped exactly like pnpm's.

    This cannot be a `filegroup`. `copy = False` calls `symlinked_dir`, which
    makes every store leaf a symlink out of the tree; Node realpaths through
    those and sibling lookup escapes with `Cannot find module`. `copy = True`
    fails oppositely, flattening the symlink pnpm's isolation depends on. The
    real shape needs real directories and intra-tree relative symlinks from
    one action, which no `filegroup` mode provides.

    Inputs arrive as a JSON manifest, so no package name, archive root or bin
    path is ever interpolated into a command line.
    """
    out = ctx.actions.declare_output("node_modules", dir = True)
    builder = ctx.actions.write("build_tree.js", _BUILD_TREE)
    copies = {
        dest: dep[DefaultInfo].default_outputs[0]
        for dest, dep in ctx.attrs.copies.items()
    }
    manifest = ctx.actions.write_json(
        "tree_manifest.json",
        {"copies": copies, "links": ctx.attrs.links},
        with_inputs = True,
    )
    node = ctx.attrs._node_toolchain[NodeToolchainInfo].node
    ctx.actions.run(
        cmd_args(node, builder, manifest, out.as_output()),
        category = "node_modules_tree",
    )
    return [DefaultInfo(default_output = out)]

node_modules_tree = rule(
    impl = _node_modules_tree_impl,
    attrs = {
        "copies": attrs.dict(attrs.string(), attrs.dep(), default = {}),
        "links": attrs.dict(attrs.string(), attrs.string(), default = {}),
        "_node_toolchain": attrs.toolchain_dep(default = "toolchains//:node"),
    },
)
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib buck::bzl`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/buck/bzl.rs
git commit -m "feat(buck): emit a node_modules_tree rule that builds a real pnpm store"
```

---

## Task 3: `node_binary` and `node_test`

**Files:**
- Modify: `src/buck/bzl.rs`

**Interfaces:**
- Consumes: `NodeToolchainInfo` (loaded in Task 2).
- Produces: `bzl::render()` output defines `node_binary` and `node_test`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn node_binary_is_defined() {
    let s = render();
    assert!(s.contains("node_binary = rule("));
    assert!(s.contains("node_test = rule("));
}

/// The tree is reached through a symlink that leaves the app output, which
/// is a dependency buck2 does not track structurally. Absent from either
/// place, buck2 does not materialize it and the link dangles — and a
/// dangling link builds clean (S5 design §1.3).
#[test]
fn node_binary_keeps_the_tree_materialized() {
    let s = render();
    assert!(s.contains("other_outputs = [app, tree]"));
    assert!(s.contains("hidden = [app, tree]"));
}

#[test]
fn node_binary_places_the_tree_link_relative_to_the_app_directory() {
    let s = render();
    assert!(s.contains("relative_to = (app, 0)"));
}

#[test]
fn node_test_reports_itself_to_the_test_runner() {
    let s = render();
    assert!(s.contains("ExternalRunnerTestInfo"));
}

/// First-party sources are copied, never hardlinked: a hardlink would share
/// an inode with a file the user is editing, so an in-place write would
/// mutate a build output.
#[test]
fn node_binary_documents_why_first_party_sources_are_copied() {
    let s = render();
    assert!(s.contains("hardlink"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib buck::bzl`
Expected: the five new tests FAIL.

- [ ] **Step 3: Extend `BODY`**

```python
_BUILD_APP = '''
const fs = require("fs");
const path = require("path");

const [, , manifestPath, outDir] = process.argv;
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));

// First-party sources are copied, never hardlinked. A hardlink would share
// an inode with a file in the user's working tree, so an in-place write
// would mutate a build output directly. The store's inodes are different in
// kind: they belong to immutable buck-out extractions.
for (const [dest, src] of Object.entries(manifest.copies)) {
  const d = path.join(outDir, dest);
  fs.mkdirSync(path.dirname(d), { recursive: true });
  fs.copyFileSync(Array.isArray(src) ? src.join("") : src, d);
}

for (const [dest, target] of Object.entries(manifest.links)) {
  const d = path.join(outDir, dest);
  fs.mkdirSync(path.dirname(d), { recursive: true });
  fs.symlinkSync(Array.isArray(target) ? target.join("") : target, d);
}
'''

def _node_app_dir(ctx):
    """The runnable directory: first-party sources plus one node_modules link.

    Node finds `node_modules` by walking up from the main module's directory,
    so the sources and the store have to meet. They meet here. The sources
    are real files, so relative `require`s between them work; `node_modules`
    is a single symlink into the tree, and Node realpaths it *into* the
    genuine store where `.pnpm` isolation holds.
    """
    tree = ctx.attrs.node_modules[DefaultInfo].default_outputs[0]
    app = ctx.actions.declare_output("app", dir = True)
    builder = ctx.actions.write("build_app.js", _BUILD_APP)
    copies = {src.short_path: src for src in ctx.attrs.srcs}
    manifest = ctx.actions.write_json(
        "app_manifest.json",
        {
            "copies": copies,
            # `relative_to = (app, 0)` places the link relative to the app
            # directory itself. One level off and the link dangles, which
            # builds clean and fails only when Node runs.
            "links": {"node_modules": cmd_args(tree, relative_to = (app, 0))},
        },
        with_inputs = True,
    )
    node = ctx.attrs._node_toolchain[NodeToolchainInfo].node
    ctx.actions.run(
        cmd_args(node, builder, manifest, app.as_output()),
        category = "node_app",
    )
    return app, tree

def _node_launcher(ctx, app):
    node = ctx.attrs._node_toolchain[NodeToolchainInfo].node
    return ctx.actions.write(
        "run.sh",
        cmd_args(
            "#!/bin/sh",
            "set -e",
            cmd_args(
                "exec",
                node,
                cmd_args(app, format = "{}/" + ctx.attrs.main),
                "\"$@\"",
                delimiter = " ",
            ),
            "",
            delimiter = "\n",
        ),
        is_executable = True,
        with_inputs = True,
    )

def _node_binary_impl(ctx):
    app, tree = _node_app_dir(ctx)
    launcher = _node_launcher(ctx, app)

    # The tree is reached through a symlink that leaves the app output, so
    # buck2 has no structural edge to it. It must be named in both places or
    # it is not materialized and the link dangles.
    return [
        DefaultInfo(default_output = launcher, other_outputs = [app, tree]),
        RunInfo(args = cmd_args(launcher, hidden = [app, tree])),
    ]

def _node_test_impl(ctx):
    app, tree = _node_app_dir(ctx)
    launcher = _node_launcher(ctx, app)
    return [
        DefaultInfo(default_output = launcher, other_outputs = [app, tree]),
        RunInfo(args = cmd_args(launcher, hidden = [app, tree])),
        ExternalRunnerTestInfo(
            type = "node",
            command = [cmd_args(launcher, hidden = [app, tree])],
        ),
    ]

_NODE_ATTRS = {
    "main": attrs.string(),
    "node_modules": attrs.dep(),
    "srcs": attrs.list(attrs.source(), default = []),
    "_node_toolchain": attrs.toolchain_dep(default = "toolchains//:node"),
}

node_binary = rule(impl = _node_binary_impl, attrs = _NODE_ATTRS)

node_test = rule(impl = _node_test_impl, attrs = _NODE_ATTRS)
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib buck::bzl`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/buck/bzl.rs
git commit -m "feat(buck): emit node_binary and node_test"
```

---

## Task 4: Emit tree targets and wire the pipeline

**Files:**
- Modify: `src/buck/emit.rs`, `src/buck/mod.rs`, `src/cli/buckify.rs`, `src/error.rs`

**Interfaces:**
- Consumes: `store::{Tree, build}` (Task 1).
- Produces:
  ```rust
  // emit.rs
  pub fn render(
      entries: &BTreeMap<String, Entry>,
      trees: &BTreeMap<String, store::Tree>,  // importer name → tree
      third_party_label: &str,
  ) -> Result<String, BuckError>;

  // mod.rs
  pub fn generate(
      entries: &BTreeMap<String, Entry>,
      platforms: &BTreeMap<String, Platform>,
      trees: &BTreeMap<String, store::Tree>,
      third_party_dir: &Path,
  ) -> Result<Generated, BuckError>;

  // emit.rs, also used by buckify.rs for the collision check
  pub fn importer_target_name(importer: &str) -> String;
  ```

- [ ] **Step 1: Write the failing tests**

In `src/buck/emit.rs`'s test module:

```rust
fn tree_of(links: &[(&str, &str)], copies: &[(&str, &str)]) -> store::Tree {
    store::Tree {
        copies: copies
            .iter()
            .map(|(d, k)| (d.to_string(), k.to_string()))
            .collect(),
        links: links
            .iter()
            .map(|(d, t)| (d.to_string(), t.to_string()))
            .collect(),
    }
}

#[test]
fn the_root_importer_is_named_root() {
    assert_eq!(importer_target_name("."), "root");
}

#[test]
fn a_nested_importer_flattens_its_path() {
    assert_eq!(importer_target_name("packages/legacy"), "packages_legacy");
}

#[test]
fn a_tree_target_is_emitted_per_importer() {
    let trees = BTreeMap::from([(
        ".".to_string(),
        tree_of(
            &[("left-pad", ".pnpm/left-pad@1.3.0/node_modules/left-pad")],
            &[(".pnpm/left-pad@1.3.0/node_modules/left-pad", "left-pad@1.3.0")],
        ),
    )]);
    let out = render(&BTreeMap::new(), &trees, "third-party/js").unwrap();
    assert!(out.contains("node_modules_tree(\n    name = \"root_node_modules\","));
    assert!(out.contains("\"left-pad\": \".pnpm/left-pad@1.3.0/node_modules/left-pad\""));
    assert!(out.contains(
        "\".pnpm/left-pad@1.3.0/node_modules/left-pad\": \
         \"//third-party/js:left-pad@1.3.0[root]\""
    ));
}

#[test]
fn the_tree_macro_is_loaded() {
    let trees = BTreeMap::from([(".".to_string(), tree_of(&[], &[]))]);
    let out = render(&BTreeMap::new(), &trees, "third-party/js").unwrap();
    assert!(out.contains("\"node_modules_tree\""));
}

/// Two importers can flatten to the same target name. Emitting both would
/// produce a duplicate-target error from buck2 naming neither importer.
#[test]
fn colliding_importer_names_are_rejected() {
    let trees = BTreeMap::from([
        ("a/b".to_string(), tree_of(&[], &[])),
        ("a_b".to_string(), tree_of(&[], &[])),
    ]);
    let err = render(&BTreeMap::new(), &trees, "third-party/js").unwrap_err();
    assert!(matches!(err, BuckError::ImporterNameCollision { .. }));
}
```

Every existing `render(...)` call in this module takes a new argument — pass
`&BTreeMap::new()` for the trees.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib buck::emit`
Expected: compile error — `importer_target_name` and the new parameter do not exist.

- [ ] **Step 3: Implement**

In `src/error.rs`, add to `BuckError` (and register in `typed_errors!` and
`samples()` exactly as the existing variants are):

```rust
#[error("two importers both map to the Buck target name `{target}`: `{first}` and `{second}`")]
#[diagnostic(
    code(pudu::buckify::importer_name_collision),
    help("rename one of the workspace directories, or give it a distinct path")
)]
ImporterNameCollision {
    target: String,
    first: String,
    second: String,
},
```

In `src/buck/emit.rs`:

```rust
/// An importer path as a Buck target-name stem.
///
/// The root importer is `.`, which is not a name; everything else is a
/// workspace-relative directory path, and `/` is not legal in a target name.
pub fn importer_target_name(importer: &str) -> String {
    if importer == "." {
        "root".to_string()
    } else {
        importer.replace('/', "_")
    }
}
```

Extend `render` to take `trees` and, after the package loop, emit one
`node_modules_tree` per importer in `BTreeMap` order. Build a
`BTreeMap<String, String>` of target name → importer as you go and return
`BuckError::ImporterNameCollision` on a duplicate. Render the load line as
`load("//{label}:pudu.bzl", "node_modules_tree", "npm_package")` — sorted, and
emitted whether or not each is used, so the line is stable.

Each copy destination renders as a label:
`"//{third_party_label}:{target_name(key)}[root]"`, with both the destination
path and the label passed through `starlark_string`.

In `src/buck/mod.rs`, thread `trees` through `generate` into `emit::render`.

In `src/cli/buckify.rs`, immediately **after** the `let entries = match &loaded
{ ... };` binding and before the `buck::generate` call — `store::build` reads
`entries` for bin paths, so it cannot go earlier:

```rust
// S5 emits a single tree over the union of platform views. That is exact
// only while no package is platform-gated; the moment one is, the tree
// carries packages that do not belong on some platform. S5.5 replaces this
// with one tree per platform behind a select().
let views: Vec<&BTreeSet<String>> = matrix.views.values().map(|v| &v.nodes).collect();
if let Some(first) = views.first() {
    if views.iter().any(|v| v != first) {
        eprintln!(
            "warning: configured platforms resolve to different package sets; \
             the generated node_modules trees cover their union and are not \
             yet platform-specific (S5.5)"
        );
    }
}

let union: BTreeSet<String> = matrix.views.values().flat_map(|v| v.nodes.iter().cloned()).collect();
let importers: BTreeSet<&str> = graph.roots.iter().map(|r| r.importer.as_str()).collect();
let trees: BTreeMap<String, buck::store::Tree> = importers
    .into_iter()
    .map(|i| (i.to_string(), buck::store::build(&graph, &union, entries, i)))
    .collect();
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test`
Expected: all pass. Update any insta snapshots with `cargo insta accept` **only
after reading the diff** and confirming the new tree targets are what changed.

- [ ] **Step 5: Commit**

```bash
git add src/buck src/cli/buckify.rs src/error.rs
git commit -m "feat(buckify): emit one node_modules_tree per importer"
```

---

## Task 5: A fixture with real dependency edges

**Files:**
- Modify: `tests/fixtures/buck/01-pure-js/pnpm-lock.yaml`
- Create: `tests/fixtures/buck/01-pure-js/packages/app/{BUCK,package.json,src/index.js,src/greet.js}`
- Modify: `tests/fixtures/buck/01-pure-js/third-party/js/packages.toml` (regenerated)

- [ ] **Step 1: Extend the lockfile**

Add the seven packages from the table at the top of this plan. The root
importer gains `debug` and `@jridgewell/gen-mapping`; add a third importer,
`packages/app`, depending on `debug`. Snapshots carry the dependency edges:

```yaml
  debug@4.3.4:
    dependencies:
      ms: 2.1.2
  '@jridgewell/gen-mapping@0.3.5':
    dependencies:
      '@jridgewell/set-array': 1.2.1
      '@jridgewell/sourcemap-codec': 1.5.0
      '@jridgewell/trace-mapping': 0.3.25
  '@jridgewell/trace-mapping@0.3.25':
    dependencies:
      '@jridgewell/resolve-uri': 3.1.2
      '@jridgewell/sourcemap-codec': 1.5.0
```

Use the integrity strings from the table verbatim. `debug@4.3.4` and the
`@jridgewell/*` packages have no `hasBin`.

- [ ] **Step 2: Write the first-party sources**

`packages/app/src/greet.js` — proves first-party-to-first-party `require`
works, which is the copies-not-symlinks requirement:

```js
const debug = require("debug");

const log = debug("app");

module.exports = function greet() {
  log("greet() called");
  return "pudu";
};
```

`packages/app/src/index.js`:

```js
const greet = require("./greet");

// `debug` depends on `ms`, so resolving it at all proves sibling lookup
// works through the virtual store — the case S4 design §1.4 could not make
// work with a filegroup.
require("debug").enable("app");

console.log("hello from", greet());
console.log("debug resolved to", require.resolve("debug"));
console.log("ms resolved to", require.resolve("ms", {
  paths: [require("path").dirname(require.resolve("debug"))],
}));
```

`packages/app/package.json`:

```json
{
  "name": "app",
  "version": "0.0.0",
  "private": true,
  "dependencies": {
    "debug": "4.3.4"
  }
}
```

`packages/app/BUCK` is **hand-written** — pudu generates nothing outside
`third-party/js`:

```python
load("//third-party/js:pudu.bzl", "node_binary")

node_binary(
    name = "main",
    main = "src/index.js",
    srcs = ["src/index.js", "src/greet.js"],
    node_modules = "//third-party/js:packages_app_node_modules",
    visibility = ["PUBLIC"],
)
```

- [ ] **Step 3: Regenerate the package table and generated files**

```bash
cd tests/fixtures/buck/01-pure-js
cargo run -- vendor
cargo run -- buckify
```

Expected: `packages.toml` gains seven entries; `third-party/js/BUCK` gains the
seven `npm_package` targets and three `node_modules_tree` targets
(`root_node_modules`, `packages_legacy_node_modules`,
`packages_app_node_modules`).

- [ ] **Step 4: Verify the tree with real buck2**

```bash
cd tests/fixtures/buck/01-pure-js
buck2 run //packages/app:main
```

Expected output contains `hello from pudu`, and both resolved paths point
inside `.pnpm/`.

If `ms` fails to resolve, the sibling `../` count is wrong — go back to Task 1
rather than adjusting anything here.

- [ ] **Step 5: Commit**

```bash
git add tests/fixtures/buck/01-pure-js
git commit -m "test(fixture): give 01-pure-js real dependency edges and a runnable importer"
```

---

## Task 6: CI runs, rather than only building

**Files:**
- Modify: `.github/workflows/ci.yml`
- Create: `tests/buckify_tree.rs`

- [ ] **Step 1: Write the integration test**

`tests/buckify_tree.rs`, using the hermetic mock-registry harness from
`tests/buckify.rs` (copy its setup helpers rather than importing across test
binaries):

```rust
/// A tree whose sibling link points at a package that was pruned away would
/// dangle. `buck2 build` accepts a dangling symlink silently, so the only
/// gate is that pudu never emits one.
#[test]
fn no_link_target_is_missing_from_the_copies_map() {
    // Build the fixture's graph, emit every tree, and assert that each link
    // target resolves to a path that either appears in `copies` or is a
    // prefix of one.
}
```

Implement it against `store::build` over the fixture lockfile: for each link,
normalize `dest` + target into a tree-relative path and assert it is a key in
`copies` or a prefix of one. This catches the entire dangling-link class
without needing buck2.

- [ ] **Step 2: Run it**

Run: `cargo test --test buckify_tree`
Expected: PASS.

- [ ] **Step 3: Prove it gates**

Temporarily change `up(climb)` in `store.rs` to `up(climb + 1)`.
Run: `cargo test --test buckify_tree`
Expected: FAIL. Revert.

- [ ] **Step 4: Upgrade the CI job**

In `.github/workflows/ci.yml`, in the `buck2-build` job, after the existing
sub-target builds:

```yaml
      - name: buck2 run (the store must actually resolve)
        # `buck2 build` is not a sufficient gate for this stage. A dangling
        # node_modules symlink, a tree missing from the RunInfo hidden set,
        # and a wrong relative_to depth all BUILD CLEAN and fail only when
        # node runs. See the S5 design, §1.3.
        run: |
          out=$(buck2 run //packages/app:main)
          echo "$out"
          echo "$out" | grep -q "hello from pudu"
          echo "$out" | grep -q "/.pnpm/debug@4.3.4/node_modules/debug/"
          echo "$out" | grep -q "/.pnpm/ms@2.1.2/node_modules/ms/"
```

Rename the job `buck2 build and run (generated output)`.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml tests/buckify_tree.rs
git commit -m "ci: run the generated binary, since a dangling link builds clean"
```

---

## Task 7: Documentation and tech debt

**Files:**
- Modify: `docs/superpowers/specs/2026-08-30-pudu-design.md`, `docs/superpowers/specs/2026-08-30-pudu-roadmap.md`, `docs/superpowers/TECH_DEBT.md`, `docs/superpowers/specs/2026-09-10-pudu-s5-store-layout-design.md`

- [ ] **Step 1: Correct design §8**

Replace the `filegroup`-based `node_modules_tree` example and the
"needs no custom rule at all" sentence with the rule as shipped. The paragraph
already noting the refutation stays; what must go is the code block presenting
a non-working rule as the design.

- [ ] **Step 2: Split the roadmap's S5**

Rewrite the S5 entry to match this stage, and add S5.5 carrying what §2 of the
spec defers: per-platform emission and `select()`, the semver-stable bare
alias, fixtures 02–05, macOS e2e, and the scale measurement. Update the
dependency graph and the specs index. Add a "Shipped" paragraph to S5 in the
style of S0's.

- [ ] **Step 3: File the new debt**

Add to `TECH_DEBT.md`:

| ID | Stage | Item |
|---|---|---|
| TD-S5-01 | S5.5 | An action output containing symlinks *and* hardlinks has not been round-tripped through RE/CAS; no remote-execution backend was available. Relative symlinks are a supported CAS concept (the prelude's own `symlinked_dir` outputs rely on it) and hardlinks are indistinguishable from files at that layer, so the expectation is that it works — but it is untested. |
| TD-S5-02 | S6 | Hardlinked store entries share inodes with buck2's `http_archive` extraction output, so anything writing into `node_modules` corrupts a cached artifact. The main writer, lifecycle install scripts, is forbidden by S6's gate. Revisit at S13 if scripts are ever run. |
| TD-S5-03 | S6 | A package shipping its bin non-executable gets a `node_modules/.bin/` entry that is not directly runnable. `semver@7.6.3` ships `0755` and extraction preserves it, so no fixture package hits this. It cannot be fixed by chmod — that would mutate the shared inode of TD-S5-02 — so the fix is a copied, chmodded bin entry if it ever matters. |

Retarget TD-S1-04 and TD-S2-06 from S5 to S5.5, since `link:` roots and
per-platform `select()` arms both move there.

- [ ] **Step 4: Correct the spec's §3.3**

§3.3 states the executable bit as an open assumption. It is now measured.
Replace it with the finding, and note the residual `0644` case as TD-S5-03.

- [ ] **Step 5: Commit**

```bash
git add docs/
git commit -m "docs: correct design §8, split the roadmap's S5, file S5 debt"
```

---

## Self-review

**Spec coverage.** §1 findings → Tasks 2, 3 (embedded as rule code and as
tests pinning the two §1.3 hazards). §2 scope → Tasks 1–6; the deferred list
→ Task 7's S5.5 entry. §3 store-path computation → Task 1, with §3.1's depth
rule mutation-tested in Task 1 Step 5 and Task 6 Step 3. §3.2 dev deps →
covered by `store::build` consuming every root regardless of `RootKind`; the
fixture's roots are all `Prod`, so this is asserted rather than demonstrated —
acceptable, since the code has no kind filter to get wrong. §3.3 `.bin` →
resolved before planning; §4 rule → Task 2; §4.3 materialization → Task 2
(hardlink) and Task 3 (copy for first-party); §4.4 emission → Task 4; §5
binaries → Task 3; §6 modules → the file-structure table; §7 testing → Tasks
1, 4, 6; §8 exit criteria → Tasks 5 and 6; §9 errors → Task 4's
`ImporterNameCollision`; §10 fallout → Task 7.

**One spec claim this plan contradicts, deliberately:** §3.3 left the bin
executable bit open and asked for a chmod if the assumption held. It does not
hold — the bit is already set and survives extraction — and chmodding would
have corrupted a shared inode. Task 7 Step 4 corrects the spec rather than
leaving the two documents disagreeing.

**Type consistency.** `store::Tree { copies, links }` is constructed in Task 1
and consumed by the same field names in Task 4's `tree_of` helper and
`buckify.rs`. `importer_target_name` is defined in Task 4 and used in the same
task's tests and in Task 5's hand-written `BUCK`
(`packages_app_node_modules` = `importer_target_name("packages/app")` +
`_node_modules`). `generate`'s new parameter order matches its only call site.
The Starlark attribute names `copies`/`links` match between Task 2's rule and
Task 4's renderer.
