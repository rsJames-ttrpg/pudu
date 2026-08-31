# Pudu S2 — Platform Model & Optional-Dependency Pruning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn S1's platform-independent instance graph into a per-platform view, and derive each configured platform's Buck2 constraint labels.

**Architecture:** A faithful port of pnpm's `checkList` decides whether a package's npm `os`/`cpu`/`libc` fields admit a platform. Pruning applies that per package — no reachability sweep — producing a matrix plus its transpose for S4's `select()` keys. Constraint mapping is separate and emits the abi constraint only when it discriminates between configured platforms.

**Tech Stack:** Rust 2024, serde, thiserror + miette, clap derive. Node + pnpm for test oracles only (never at runtime).

## Global Constraints

- **Edition 2024, MSRV `1.88`.** `cargo check --all-targets` must pass on 1.88; no newer language feature.
- **`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` must both pass** before every commit.
- **Pudu never runs pnpm at runtime.** Node appears only in `#[ignore]`d developer tests.
- **Determinism:** same inputs ⇒ byte-identical output. Use `BTreeMap`/`BTreeSet` throughout; never `HashMap`/`HashSet` in anything that reaches output.
- **Every message names a field or a file by path** (`src/error.rs` module doc).
- **Warnings are typed enums deriving `Diagnostic`**, never `Vec<String>`. They are *not* added to the `typed_errors!` registry — that macro maps errors to exit codes, and warnings have none.
- **Invented JSON fields are `snake_case`;** fields echoing the lockfile keep pnpm's spelling.
- **The spec governs.** `docs/superpowers/specs/2026-08-31-pudu-s2-platforms-design.md`. Its evidence is `docs/superpowers/research/2026-08-31-pnpm-platform-matching-survey.md`. If code and spec disagree, stop and report it rather than choosing.
- **Do not infer `libc` from a package's name.** Spec §7.2. Guessing that `lightningcss-linux-x64-musl` is a musl build would make pudu prune packages pnpm installs and break the oracle agreement that is S2's whole correctness argument.

---

## File Structure

| File | Responsibility |
|---|---|
| `src/platform/mod.rs` | *(moved from `src/platform.rs`)* `Os`/`Cpu`/`Libc` enums, their npm spellings, submodule re-exports |
| `src/platform/matching.rs` | `admits` (the `checkList` port) and `admits_platform` (axis selection) |
| `src/platform/prune.rs` | `Matrix`, `PlatformView`, `DroppedEdge`, `prune` |
| `src/platform/constraints.rs` | `constraint_labels` and the conditional-abi rule |
| `src/error.rs` | `PlatformWarning` (modify) |
| `src/cli/debug.rs` | `platforms()` (modify) |
| `src/cli/mod.rs` | `DebugCommands::Platforms` (modify) |
| `src/cli/init.rs` | TD-S0-08 / TD-S0-09 fixes (modify) |
| `tests/fixtures/lock/real/oracle/` | four captured listings, `capture.sh`, `engine-excluded.txt` |
| `tests/platform_oracle.rs` | pruning vs the four captured oracles |
| `tests/platform_fuzz.rs` | `#[ignore]`d differential fuzz vs pnpm's real `checkPlatform` |

---

### Task 1: The matcher — `admits`

The heart of S2. Every rule below was verified against pnpm's real
implementation; see survey §1 for the evidence table. Two of them are
counter-intuitive and are the reason this task is separate and heavily
tested.

**Files:**
- Create: `src/platform/matching.rs`
- Create: `src/platform/mod.rs` (moved from `src/platform.rs`)
- Delete: `src/platform.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub fn admits(field: Option<&[String]>, current: &str) -> bool`

- [ ] **Step 1: Move the module, without changing its content**

`src/platform.rs` becomes `src/platform/mod.rs`. The `Os`, `Cpu`, `Libc`
enums, their `as_npm()`/`short()` methods, and all six existing tests move
verbatim — this is a file move, not a rewrite. Use `git mv` so history
follows:

```bash
mkdir -p src/platform
git mv src/platform.rs src/platform/mod.rs
```

Then append the submodule declaration and re-export to the end of the
`use` block at the top of `src/platform/mod.rs` (after the existing
`use serde::{Deserialize, Serialize};`):

```rust
pub mod matching;

pub use matching::admits;
```

`admits_platform` joins this re-export in Task 2, when it exists. Adding it
now would fail to compile.

Verify the move alone compiles before writing anything new:

```bash
cargo check --all-targets
```

Expected: PASS. `src/lib.rs` already says `pub mod platform;`, which now
resolves to the directory — no change needed there.

- [ ] **Step 2: Write the failing tests**

Create `src/platform/matching.rs` containing only this test module (no
implementation yet). Every case is transcribed from the survey's verified
table; each test is named for the *rule* it pins, not the input, so a
future "simplification" that breaks the rule fails a test whose name says
what was broken.

```rust
//! Does a package's npm platform field admit a given platform?

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience: build the `Option<&[String]>` shape `admits` takes.
    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn absent_field_admits_everything() {
        assert!(admits(None, "linux"));
        assert!(admits(None, "wasm32"));
    }

    #[test]
    fn positive_entry_admits_only_itself() {
        assert!(admits(Some(&list(&["linux"])), "linux"));
        assert!(!admits(Some(&list(&["darwin"])), "linux"));
    }

    #[test]
    fn negation_excludes_its_own_value() {
        assert!(admits(Some(&list(&["!win32"])), "linux"));
        assert!(!admits(Some(&list(&["!win32"])), "win32"));
    }

    #[test]
    fn all_negative_list_admits_anything_it_does_not_name() {
        assert!(admits(Some(&list(&["!win32", "!darwin"])), "linux"));
        assert!(!admits(Some(&list(&["!win32", "!darwin"])), "darwin"));
    }

    /// pnpm's rule is `matched || negations == list.len()`. A list mixing
    /// negative and positive entries therefore requires an explicit
    /// positive hit: negations only ever subtract, they never widen.
    /// `["!win32", "darwin"]` does NOT mean "anything but win32".
    #[test]
    fn mixed_list_requires_an_explicit_positive_hit() {
        assert!(
            !admits(Some(&list(&["!win32", "darwin"])), "linux"),
            "a mixed list must not admit a value no positive entry names"
        );
        assert!(admits(Some(&list(&["!win32", "linux"])), "linux"));
        assert!(!admits(Some(&list(&["!win32", "linux"])), "win32"));
    }

    /// `any` is special ONLY as a singleton list. In any other position it
    /// is an ordinary token that matches nothing.
    #[test]
    fn any_is_special_only_as_a_singleton() {
        assert!(admits(Some(&list(&["any"])), "linux"));
        assert!(
            !admits(Some(&list(&["any", "darwin"])), "linux"),
            "`any` alongside another entry is an ordinary token"
        );
    }

    #[test]
    fn empty_list_admits_everything() {
        // `matched=false, negations=0, len=0` satisfies `negations == len`.
        assert!(admits(Some(&[]), "linux"));
    }

    /// Unknown tokens are ordinary positives that match nothing. They must
    /// never error: the committed fixture alone carries seven `os` and
    /// seven `cpu` values outside pudu's enums.
    #[test]
    fn unknown_tokens_match_nothing_and_never_panic() {
        assert!(!admits(Some(&list(&["wasm32"])), "x64"));
        assert!(!admits(Some(&list(&["openharmony"])), "linux"));
        assert!(admits(Some(&list(&["loong64", "x64"])), "x64"));
    }

    #[test]
    fn negation_of_an_unknown_token_still_admits() {
        assert!(admits(Some(&list(&["!openharmony"])), "linux"));
    }

    /// A bare `!` has an empty body, which equals no platform value.
    #[test]
    fn bare_bang_is_a_negation_of_the_empty_string() {
        assert!(admits(Some(&list(&["!"])), "linux"));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test --lib platform::matching
```

Expected: FAIL to compile with `cannot find function `admits` in this scope`.

- [ ] **Step 4: Write the implementation**

Insert above the `#[cfg(test)]` module in `src/platform/matching.rs`:

```rust
/// Does a package's npm platform field admit `current`?
///
/// A port of pnpm's `checkList` (`@pnpm/package-is-installable`), evaluated
/// for a single `current` value because pudu considers one platform at a
/// time. `field` is the raw list from the lockfile with negation intact;
/// `None` is an absent field.
///
/// The final rule — `matched || negations == list.len()` — is pnpm's, and
/// carries two consequences worth stating because they are not what a
/// reader expects:
///
/// * A list mixing negative and positive entries requires an explicit
///   positive hit. `["!win32", "darwin"]` does not admit linux.
/// * An empty list admits everything, since `0 == 0`.
///
/// pnpm additionally discards non-string list entries before matching.
/// YAML gives pudu a `Vec<String>`, so a non-string entry is rejected by
/// serde long before reaching here; the divergence is unreachable and is
/// noted only so a reader comparing the two implementations is not left
/// wondering.
pub fn admits(field: Option<&[String]>, current: &str) -> bool {
    let Some(list) = field else { return true };

    // `any` is special only as a singleton — `["any", "darwin"]` is an
    // ordinary two-entry positive list.
    if list.len() == 1 && list[0] == "any" {
        return true;
    }

    let mut matched = false;
    let mut negations = 0usize;

    for entry in list {
        if let Some(body) = entry.strip_prefix('!') {
            if body == current {
                return false;
            }
            negations += 1;
        } else if entry == current {
            matched = true;
        }
    }

    matched || negations == list.len()
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test --lib platform::matching
```

Expected: PASS, 10 tests.

- [ ] **Step 6: Verify the tests can actually fail**

A test that cannot fail is worse than no test. Break the implementation
three ways and confirm the *named* test reddens each time, then revert:

1. Change `list.len() == 1 && list[0] == "any"` to `list.iter().any(|e| e == "any")`
   → `any_is_special_only_as_a_singleton` must fail.
2. Change `matched || negations == list.len()` to `matched || negations > 0`
   → `mixed_list_requires_an_explicit_positive_hit` must fail.
3. Change `return false` inside the negation branch to `negations += 1`
   → `negation_excludes_its_own_value` must fail.

If any mutation leaves the suite green, the test for that rule is not
pulling its weight — strengthen it before moving on.

- [ ] **Step 7: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -A
git commit -m "feat(platform): port pnpm's checkList as \`admits\`

Verified against @pnpm/package-is-installable rather than written from
intuition. The two rules intuition gets wrong are pinned by name: a mixed
list like [\"!win32\", \"darwin\"] needs an explicit positive hit, and \`any\`
is special only as a singleton."
```

---

### Task 2: Axis selection — `admits_platform`

**Files:**
- Modify: `src/platform/matching.rs`

**Interfaces:**
- Consumes: `admits(Option<&[String]>, &str) -> bool` (Task 1); `crate::config::Platform { os: Os, cpu: Cpu, libc: Option<Libc>, constraints: Option<Vec<String>> }`; `crate::lock::types::PackageMeta { os: Option<Vec<String>>, cpu: Option<Vec<String>>, libc: Option<Vec<String>>, .. }`.
- Produces: `pub fn admits_platform(meta: &PackageMeta, platform: &Platform) -> bool`

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` block in `src/platform/matching.rs`:

```rust
    // These tests live in `platform::matching`, so `use super::*` brings in
    // this module's items — not the parent module's enums, which must be
    // named explicitly.
    use crate::config::Platform;
    use crate::lock::types::{PackageMeta, Resolution};
    use crate::platform::{Cpu, Libc, Os};

    /// A `PackageMeta` carrying only the three platform axes; every other
    /// field takes its default.
    fn meta(os: Option<&[&str]>, cpu: Option<&[&str]>, libc: Option<&[&str]>) -> PackageMeta {
        PackageMeta {
            resolution: Resolution::Integrity {
                integrity: "sha512-test".to_string(),
            },
            engines: Default::default(),
            os: os.map(list),
            cpu: cpu.map(list),
            libc: libc.map(list),
            has_bin: false,
            deprecated: None,
            peer_dependencies: Default::default(),
            peer_dependencies_meta: Default::default(),
            bundled_dependencies: Vec::new(),
        }
    }

    fn platform(os: Os, cpu: Cpu, libc: Option<Libc>) -> Platform {
        Platform { os, cpu, libc, constraints: None }
    }

    #[test]
    fn all_three_axes_must_admit() {
        let p = platform(Os::Linux, Cpu::X64, Some(Libc::Glibc));
        assert!(admits_platform(&meta(Some(&["linux"]), Some(&["x64"]), None), &p));
        assert!(!admits_platform(&meta(Some(&["darwin"]), Some(&["x64"]), None), &p));
        assert!(!admits_platform(&meta(Some(&["linux"]), Some(&["arm64"]), None), &p));
        assert!(!admits_platform(
            &meta(Some(&["linux"]), Some(&["x64"]), Some(&["musl"])),
            &p
        ));
    }

    #[test]
    fn a_package_with_no_platform_fields_survives_every_platform() {
        for p in [
            platform(Os::Linux, Cpu::X64, Some(Libc::Glibc)),
            platform(Os::Darwin, Cpu::Arm64, None),
            platform(Os::Win32, Cpu::X64, None),
        ] {
            assert!(admits_platform(&meta(None, None, None), &p));
        }
    }

    /// pnpm evaluates the libc axis only when a libc is detectable, which it
    /// is not on macOS — so a Mac never checks libc, whatever a package
    /// declares. A platform with no configured libc reproduces that.
    #[test]
    fn libc_axis_is_skipped_when_the_platform_declares_none() {
        let mac = platform(Os::Darwin, Cpu::Arm64, None);
        assert!(admits_platform(
            &meta(Some(&["darwin"]), Some(&["arm64"]), Some(&["musl"])),
            &mac
        ));
        assert!(admits_platform(
            &meta(Some(&["darwin"]), Some(&["arm64"]), Some(&["glibc"])),
            &mac
        ));
    }

    #[test]
    fn libc_axis_discriminates_when_the_platform_declares_one() {
        let gnu = platform(Os::Linux, Cpu::X64, Some(Libc::Glibc));
        let musl = platform(Os::Linux, Cpu::X64, Some(Libc::Musl));
        let m = meta(Some(&["linux"]), Some(&["x64"]), Some(&["musl"]));
        assert!(!admits_platform(&m, &gnu));
        assert!(admits_platform(&m, &musl));
    }

    /// The npm spelling is `glibc`, not `gnu` — `gnu` is the *Buck* spelling.
    /// Matching against `Libc::short()` here would silently prune every
    /// glibc-gated package.
    #[test]
    fn libc_matches_the_npm_spelling_not_the_buck_one() {
        let gnu = platform(Os::Linux, Cpu::X64, Some(Libc::Glibc));
        assert!(admits_platform(
            &meta(None, None, Some(&["glibc"])),
            &gnu
        ));
        assert!(!admits_platform(&meta(None, None, Some(&["gnu"])), &gnu));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --lib platform::matching
```

Expected: FAIL to compile with `cannot find function `admits_platform``.

- [ ] **Step 3: Write the implementation**

Append to `src/platform/matching.rs`, after `admits`:

```rust
use crate::config::Platform;
use crate::lock::types::PackageMeta;

/// Does a package survive on a platform? All three axes must admit.
///
/// The `libc` axis is skipped entirely when the platform declares no libc.
/// That reproduces pnpm's own behaviour on a machine with no detectable
/// libc — a Mac, where `detect-libc` reports `unknown` and the axis is
/// never checked. See spec §3 and survey §1.
///
/// Each axis matches npm's vocabulary: `linux`/`darwin`/`win32`,
/// `x64`/`arm64`, `glibc`/`musl`. Note this is `Libc::as_npm` (`glibc`) and
/// NOT `Libc::short` (`gnu`), which is the Buck spelling used only by
/// constraint labels and generated platform names.
pub fn admits_platform(meta: &PackageMeta, platform: &Platform) -> bool {
    admits(meta.os.as_deref(), platform.os.as_npm())
        && admits(meta.cpu.as_deref(), platform.cpu.as_npm())
        && match platform.libc {
            Some(libc) => admits(meta.libc.as_deref(), libc.as_npm()),
            None => true,
        }
}
```

Move the two `use` lines to the top of the file with the others if
`cargo fmt` or clippy prefers; they are written here inline only to show
what the function needs.

Then widen the re-export in `src/platform/mod.rs`, which Task 1 left naming
only `admits`:

```rust
pub use matching::{admits, admits_platform};
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test --lib platform::matching
```

Expected: PASS, 15 tests.

- [ ] **Step 5: Verify the tests can fail**

Change `libc.as_npm()` to `libc.short()` → `libc_matches_the_npm_spelling_not_the_buck_one`
must fail. Change the `None => true` arm to `None => false` →
`libc_axis_is_skipped_when_the_platform_declares_none` must fail. Revert both.

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -A
git commit -m "feat(platform): axis selection, with libc skipped when unconfigured

A platform declaring no libc skips the axis entirely, reproducing pnpm on a
machine with no detectable libc. Matching uses the npm spelling \`glibc\`,
not the Buck spelling \`gnu\`; a test pins the difference."
```

---

### Task 3: `PlatformWarning`

**Files:**
- Modify: `src/error.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  pub enum PlatformWarning {
      RequiredDependencyExcluded { dependent: String, target: String, platform: String },
      ExcludedEverywhere { packages: Vec<String>, platforms: Vec<String> },
  }
  ```

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` block at the end of `src/error.rs`:

```rust
    #[test]
    fn required_dependency_excluded_names_all_three_parties() {
        let w = PlatformWarning::RequiredDependencyExcluded {
            dependent: "my-app@1.0.0".into(),
            target: "fsevents@2.3.3".into(),
            platform: "linux-x64-gnu".into(),
        };
        let msg = w.to_string();
        assert!(msg.contains("my-app@1.0.0"), "names the dependent: {msg}");
        assert!(msg.contains("fsevents@2.3.3"), "names the target: {msg}");
        assert!(msg.contains("linux-x64-gnu"), "names the platform: {msg}");
    }

    /// Fires once for the whole set, not once per package: on the committed
    /// fixture a per-package warning would print ~60 times and train the
    /// user to ignore warnings.
    #[test]
    fn excluded_everywhere_aggregates_into_one_message() {
        let w = PlatformWarning::ExcludedEverywhere {
            packages: vec!["@esbuild/aix-ppc64@0.25.12".into(), "@esbuild/sunos-x64@0.25.12".into()],
            platforms: vec!["linux-x64-gnu".into(), "darwin-arm64".into()],
        };
        let msg = w.to_string();
        assert!(msg.contains("@esbuild/aix-ppc64@0.25.12"), "{msg}");
        assert!(msg.contains("@esbuild/sunos-x64@0.25.12"), "{msg}");
        assert!(msg.contains('2'), "states how many: {msg}");
    }

    #[test]
    fn platform_warnings_render_at_warning_severity_with_a_code() {
        for w in [
            PlatformWarning::RequiredDependencyExcluded {
                dependent: "a@1".into(),
                target: "b@2".into(),
                platform: "p".into(),
            },
            PlatformWarning::ExcludedEverywhere {
                packages: vec!["b@2".into()],
                platforms: vec!["p".into()],
            },
        ] {
            assert_eq!(w.severity(), Some(miette::Severity::Warning));
            assert!(w.code().is_some(), "every diagnostic carries a code");
            // `render` is the single definition of what a diagnostic looks
            // like; a warning must survive it without losing its message.
            assert!(render(&w).contains(&w.to_string().lines().next().unwrap().to_string()));
        }
    }
```

`severity()` and `code()` come from `miette::Diagnostic`, which is already
in scope in that module via `use miette::{Diagnostic, ...}`. If the test
module does not import it, add `use miette::Diagnostic as _;` inside
`mod tests`.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --lib error::
```

Expected: FAIL to compile with `cannot find type `PlatformWarning``.

- [ ] **Step 3: Write the implementation**

Add to `src/error.rs`, immediately after the `LockWarning` enum (keeping
the file's existing section-comment style):

```rust
// --- Platform pruning (S2) ------------------------------------------------

/// Non-fatal findings from per-platform pruning.
///
/// S2 introduces no hard errors: every condition here is a property of
/// somebody's dependency tree rather than of pudu's input being malformed,
/// and none of them makes the rest of the output wrong.
#[derive(Debug, Clone, PartialEq, Eq, Error, Diagnostic)]
pub enum PlatformWarning {
    #[error(
        "`{dependent}` requires `{target}`, which is excluded on platform `{platform}`"
    )]
    #[diagnostic(
        severity(Warning),
        code(pudu::platform::required_dependency_excluded),
        help(
            "pudu drops the dependency for this platform. pnpm would install it anyway; if the package is genuinely needed here, it may need a fixup."
        )
    )]
    RequiredDependencyExcluded {
        dependent: String,
        target: String,
        platform: String,
    },

    #[error(
        "{} package(s) are excluded on every configured platform ({}): {}",
        packages.len(),
        platforms.join(", "),
        packages.join(", ")
    )]
    #[diagnostic(
        severity(Warning),
        code(pudu::platform::excluded_everywhere),
        help(
            "these packages appear in no generated target. That is expected for the platform-specific binaries of a package like `esbuild`, and worth checking for anything else."
        )
    )]
    ExcludedEverywhere {
        packages: Vec<String>,
        platforms: Vec<String>,
    },
}
```

Do **not** add `PlatformWarning` to the `typed_errors!` registry. That
macro maps errors to exit codes; warnings have none, and the registry's
companion test would then demand an exit code for a warning.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test --lib error::
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -A
git commit -m "feat(error): PlatformWarning for per-platform pruning

Two warnings, no new errors: every S2 finding is a property of somebody's
dependency tree, not of malformed input. ExcludedEverywhere aggregates,
because per-package it would fire ~60 times on the committed fixture."
```

---

### Task 4: Pruning

**Files:**
- Create: `src/platform/prune.rs`
- Modify: `src/platform/mod.rs` (add `pub mod prune;`)

**Interfaces:**
- Consumes: `admits_platform(&PackageMeta, &Platform) -> bool` (Task 2); `PlatformWarning` (Task 3); `crate::lock::graph::{Graph, Node, Edge, EdgeKind}` where `Graph { nodes: BTreeMap<String, Node>, .. }`, `Node { meta: PackageMeta, edges: Vec<Edge>, .. }`, `Edge { link_name: String, target: String, kind: EdgeKind }`, `EdgeKind::{Prod, Optional}`.
- Produces:
  ```rust
  pub struct DroppedEdge { pub dependent: String, pub link_name: String, pub target: String }
  pub struct PlatformView { pub nodes: BTreeSet<String>, pub pruned: BTreeSet<String>, pub dropped_required_edges: Vec<DroppedEdge> }
  pub struct Matrix { pub views: BTreeMap<String, PlatformView>, pub platforms_by_node: BTreeMap<String, BTreeSet<String>> }
  pub fn prune(graph: &Graph, platforms: &BTreeMap<String, Platform>) -> (Matrix, Vec<PlatformWarning>)
  ```

- [ ] **Step 1: Write the failing tests**

Create `src/platform/prune.rs` with only this test module. The helper
builds a tiny graph directly rather than parsing YAML, so each test states
exactly the shape it is about.

```rust
//! Per-platform pruning of the instance graph.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::graph::{Edge, EdgeKind, Node};
    use crate::lock::types::{PackageMeta, Resolution};
    use crate::platform::{Cpu, Libc, Os};

    fn meta(os: Option<&[&str]>, cpu: Option<&[&str]>) -> PackageMeta {
        PackageMeta {
            resolution: Resolution::Integrity { integrity: "sha512-t".into() },
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
        Edge { link_name: link.into(), target: target.into(), kind }
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
                Platform { os: Os::Linux, cpu: Cpu::X64, libc: Some(Libc::Glibc), constraints: None },
            ),
            (
                "darwin-arm64".to_string(),
                Platform { os: Os::Darwin, cpu: Cpu::Arm64, libc: None, constraints: None },
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
            ("fsevents@2.3.3", node("fsevents", "2.3.3", meta(Some(&["darwin"]), None), vec![])),
            ("win-only@1.0.0", node("win-only", "1.0.0", meta(Some(&["win32"]), None), vec![])),
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
            ("fsevents@2.3.3", node("fsevents", "2.3.3", meta(Some(&["darwin"]), None), vec![])),
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
                node("esbuild", "0.25.12", meta(None, None), vec![edge(
                    "@esbuild/darwin-arm64",
                    "@esbuild/darwin-arm64@0.25.12",
                    EdgeKind::Optional,
                )]),
            ),
            (
                "@esbuild/darwin-arm64@0.25.12",
                node("@esbuild/darwin-arm64", "0.25.12", meta(Some(&["darwin"]), None), vec![]),
            ),
        ]);
        let (m, warnings) = prune(&g, &platforms());
        assert!(m.views["linux-x64-gnu"].dropped_required_edges.is_empty());
        assert!(
            !warnings.iter().any(|w| matches!(w, PlatformWarning::RequiredDependencyExcluded { .. })),
            "an excluded optional dependency is the normal case and must be silent"
        );
    }

    #[test]
    fn a_dropped_required_edge_warns_and_is_recorded() {
        let g = graph(vec![
            (
                "my-app@1.0.0",
                node("my-app", "1.0.0", meta(None, None), vec![edge(
                    "fsevents",
                    "fsevents@2.3.3",
                    EdgeKind::Prod,
                )]),
            ),
            ("fsevents@2.3.3", node("fsevents", "2.3.3", meta(Some(&["darwin"]), None), vec![])),
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
                node("win-tool", "1.0.0", meta(Some(&["win32"]), None), vec![edge(
                    "fsevents",
                    "fsevents@2.3.3",
                    EdgeKind::Prod,
                )]),
            ),
            ("fsevents@2.3.3", node("fsevents", "2.3.3", meta(Some(&["darwin"]), None), vec![])),
        ]);
        let (m, warnings) = prune(&g, &platforms());
        assert!(m.views["linux-x64-gnu"].dropped_required_edges.is_empty());
        assert!(
            !warnings.iter().any(|w| matches!(w, PlatformWarning::RequiredDependencyExcluded { .. })),
            "the dependent is absent here; its edges are not 'dropped'"
        );
    }

    #[test]
    fn excluded_everywhere_warns_once_for_all_such_packages() {
        let g = graph(vec![
            ("app@1.0.0", node("app", "1.0.0", meta(None, None), vec![])),
            ("a-win@1.0.0", node("a-win", "1.0.0", meta(Some(&["win32"]), None), vec![])),
            ("b-win@1.0.0", node("b-win", "1.0.0", meta(Some(&["win32"]), None), vec![])),
            ("c-aix@1.0.0", node("c-aix", "1.0.0", meta(Some(&["aix"]), None), vec![])),
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
            &vec!["a-win@1.0.0".to_string(), "b-win@1.0.0".to_string(), "c-aix@1.0.0".to_string()]
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
            ("fsevents@2.3.3", node("fsevents", "2.3.3", meta(Some(&["darwin"]), None), vec![])),
        ]);
        let (_, warnings) = prune(&g, &platforms());
        assert!(
            !warnings.iter().any(|w| matches!(w, PlatformWarning::ExcludedEverywhere { .. })),
            "every package is on some platform"
        );
    }

    /// Determinism: warnings come out in a stable order regardless of how
    /// the graph was built, because every collection is a BTree.
    #[test]
    fn output_is_deterministic_across_runs() {
        let g = graph(vec![
            ("app@1.0.0", node("app", "1.0.0", meta(None, None), vec![])),
            ("a-win@1.0.0", node("a-win", "1.0.0", meta(Some(&["win32"]), None), vec![])),
            ("z-win@1.0.0", node("z-win", "1.0.0", meta(Some(&["win32"]), None), vec![])),
        ]);
        let (m1, w1) = prune(&g, &platforms());
        let (m2, w2) = prune(&g, &platforms());
        assert_eq!(m1.views, m2.views);
        assert_eq!(m1.platforms_by_node, m2.platforms_by_node);
        assert_eq!(w1, w2);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --lib platform::prune
```

Expected: FAIL to compile — `prune`, `Matrix`, `PlatformView` undefined.

- [ ] **Step 3: Write the implementation**

Insert above the test module in `src/platform/prune.rs`:

```rust
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
            PlatformView { nodes, pruned, dropped_required_edges },
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

    (Matrix { views, platforms_by_node }, warnings)
}
```

`graph.nodes` is a `BTreeMap`, so `excluded_everywhere` is already sorted
and needs no explicit sort.

Add to `src/platform/mod.rs`, beside `pub mod matching;`:

```rust
pub mod prune;
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test --lib platform::prune
```

Expected: PASS, 10 tests.

- [ ] **Step 5: Verify the tests can fail**

Three mutations, each of which must redden the test whose name describes it:

1. Drop the `if edge.kind == EdgeKind::Optional { continue; }` guard →
   `a_dropped_optional_edge_is_silent` must fail.
2. Change the edge loop from `for key in &nodes` to iterate all
   `graph.nodes` → `an_edge_from_a_pruned_dependent_does_not_warn` must fail.
3. Emit one `ExcludedEverywhere` per package instead of one aggregate →
   `excluded_everywhere_warns_once_for_all_such_packages` must fail.

Revert all three.

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -A
git commit -m "feat(platform): per-platform pruning with its transpose

Per-package field matching only; no reachability sweep, which reproduced
all four captured oracles exactly. A package on no platform is absent from
the transpose rather than present with an empty set, so S4 cannot generate
a select() with no arms."
```

---

### Task 5: Constraint mapping

**Files:**
- Create: `src/platform/constraints.rs`
- Modify: `src/platform/mod.rs` (add `pub mod constraints;`)

**Interfaces:**
- Consumes: `crate::config::Platform`; `crate::platform::{Os, Cpu, Libc}`.
- Produces: `pub fn constraint_labels(platform: &Platform, all: &BTreeMap<String, Platform>) -> Vec<String>`

- [ ] **Step 1: Write the failing tests**

Create `src/platform/constraints.rs` with only this test module:

```rust
//! Mapping a configured platform to Buck2 constraint labels.

#[cfg(test)]
mod tests {
    // `use super::*` already brings in `Platform`, `BTreeMap` and the
    // `Os`/`Cpu`/`Libc` enums from this module's own imports.
    use super::*;

    fn p(os: Os, cpu: Cpu, libc: Option<Libc>) -> Platform {
        Platform { os, cpu, libc, constraints: None }
    }

    fn only(platform: Platform) -> (Platform, BTreeMap<String, Platform>) {
        let all = BTreeMap::from([("solo".to_string(), platform.clone())]);
        (platform, all)
    }

    #[test]
    fn maps_os_and_cpu_to_prelude_labels() {
        let (plat, all) = only(p(Os::Linux, Cpu::X64, None));
        assert_eq!(
            constraint_labels(&plat, &all),
            vec![
                "prelude//cpu/constraints:x86_64".to_string(),
                "prelude//os/constraints:linux".to_string(),
            ]
        );
    }

    /// npm's vocabulary and the prelude's differ on two of seven values.
    /// A "simplification" that lowercased the npm name would break both.
    #[test]
    fn npm_and_prelude_vocabularies_differ_on_darwin_and_glibc() {
        let (plat, all) = only(p(Os::Darwin, Cpu::Arm64, None));
        let labels = constraint_labels(&plat, &all);
        assert!(
            labels.contains(&"prelude//os/constraints:macos".to_string()),
            "npm `darwin` is the prelude's `macos`: {labels:?}"
        );
        assert!(!labels.iter().any(|l| l.contains("darwin")), "{labels:?}");

        // `glibc` is the prelude's `gnu`; exercised via the abi rule below.
        let all = BTreeMap::from([
            ("linux-x64-gnu".to_string(), p(Os::Linux, Cpu::X64, Some(Libc::Glibc))),
            ("linux-x64-musl".to_string(), p(Os::Linux, Cpu::X64, Some(Libc::Musl))),
        ]);
        let labels = constraint_labels(&all["linux-x64-gnu"], &all);
        assert!(
            labels.contains(&"prelude//abi/constraints:gnu".to_string()),
            "npm `glibc` is the prelude's `gnu`: {labels:?}"
        );
        assert!(!labels.iter().any(|l| l.contains("glibc")), "{labels:?}");
    }

    #[test]
    fn maps_win32_to_windows() {
        let (plat, all) = only(p(Os::Win32, Cpu::X64, None));
        assert!(
            constraint_labels(&plat, &all)
                .contains(&"prelude//os/constraints:windows".to_string())
        );
    }

    #[test]
    fn labels_are_sorted() {
        let all = BTreeMap::from([
            ("linux-x64-gnu".to_string(), p(Os::Linux, Cpu::X64, Some(Libc::Glibc))),
            ("linux-x64-musl".to_string(), p(Os::Linux, Cpu::X64, Some(Libc::Musl))),
        ]);
        let labels = constraint_labels(&all["linux-x64-gnu"], &all);
        let mut sorted = labels.clone();
        sorted.sort();
        assert_eq!(labels, sorted, "emitted constraint_values must be sorted");
    }

    /// A glibc-only configuration needs zero user wiring: the prelude's
    /// default platform sets os and cpu from `host_info()` and nothing sets
    /// an abi constraint, so emitting one would fail to match.
    #[test]
    fn glibc_only_configuration_emits_no_abi_constraint() {
        let all = BTreeMap::from([
            ("linux-x64-gnu".to_string(), p(Os::Linux, Cpu::X64, Some(Libc::Glibc))),
            ("darwin-arm64".to_string(), p(Os::Darwin, Cpu::Arm64, None)),
        ]);
        for name in ["linux-x64-gnu", "darwin-arm64"] {
            let labels = constraint_labels(&all[name], &all);
            assert!(
                !labels.iter().any(|l| l.contains("abi")),
                "{name} must gain no abi constraint: {labels:?}"
            );
        }
    }

    /// When two platforms share os+cpu and differ in libc, the abi
    /// constraint discriminates — and BOTH gain it, not just the musl one.
    #[test]
    fn gnu_plus_musl_emits_the_abi_constraint_on_both() {
        let all = BTreeMap::from([
            ("linux-x64-gnu".to_string(), p(Os::Linux, Cpu::X64, Some(Libc::Glibc))),
            ("linux-x64-musl".to_string(), p(Os::Linux, Cpu::X64, Some(Libc::Musl))),
        ]);
        assert!(
            constraint_labels(&all["linux-x64-gnu"], &all)
                .contains(&"prelude//abi/constraints:gnu".to_string())
        );
        assert!(
            constraint_labels(&all["linux-x64-musl"], &all)
                .contains(&"prelude//abi/constraints:musl".to_string())
        );
    }

    /// The abi rule is keyed on os+cpu. Two platforms differing in libc but
    /// also in cpu do not discriminate each other.
    #[test]
    fn differing_libc_on_a_different_cpu_does_not_discriminate() {
        let all = BTreeMap::from([
            ("linux-x64-gnu".to_string(), p(Os::Linux, Cpu::X64, Some(Libc::Glibc))),
            ("linux-arm64-musl".to_string(), p(Os::Linux, Cpu::Arm64, Some(Libc::Musl))),
        ]);
        for name in ["linux-x64-gnu", "linux-arm64-musl"] {
            let labels = constraint_labels(&all[name], &all);
            assert!(!labels.iter().any(|l| l.contains("abi")), "{name}: {labels:?}");
        }
    }

    #[test]
    fn a_platform_with_no_libc_never_gains_an_abi_constraint() {
        let all = BTreeMap::from([
            ("linux-x64".to_string(), p(Os::Linux, Cpu::X64, None)),
            ("linux-x64-musl".to_string(), p(Os::Linux, Cpu::X64, Some(Libc::Musl))),
        ]);
        let labels = constraint_labels(&all["linux-x64"], &all);
        assert!(!labels.iter().any(|l| l.contains("abi")), "{labels:?}");
    }

    #[test]
    fn constraints_override_replaces_generated_labels_entirely() {
        let plat = Platform {
            os: Os::Linux,
            cpu: Cpu::X64,
            libc: Some(Libc::Glibc),
            constraints: Some(vec![
                "ovr_config//os:linux".to_string(),
                "ovr_config//cpu:x86_64".to_string(),
            ]),
        };
        let all = BTreeMap::from([
            ("corp-linux".to_string(), plat.clone()),
            ("linux-x64-musl".to_string(), p(Os::Linux, Cpu::X64, Some(Libc::Musl))),
        ]);
        let labels = constraint_labels(&plat, &all);
        // Verbatim, in the user's order — not sorted, and with no abi label
        // even though the platform set would otherwise discriminate.
        assert_eq!(
            labels,
            vec![
                "ovr_config//os:linux".to_string(),
                "ovr_config//cpu:x86_64".to_string(),
            ]
        );
        assert!(!labels.iter().any(|l| l.starts_with("prelude//")));
    }

    #[test]
    fn an_empty_constraints_override_is_honoured_as_written() {
        let plat = Platform {
            os: Os::Linux,
            cpu: Cpu::X64,
            libc: None,
            constraints: Some(Vec::new()),
        };
        let all = BTreeMap::from([("bare".to_string(), plat.clone())]);
        assert!(
            constraint_labels(&plat, &all).is_empty(),
            "an explicit empty list is a request, not an absence"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --lib platform::constraints
```

Expected: FAIL to compile — `constraint_labels` undefined.

- [ ] **Step 3: Write the implementation**

Insert above the test module in `src/platform/constraints.rs`:

```rust
use std::collections::BTreeMap;

use crate::config::Platform;
use crate::platform::{Cpu, Libc, Os};

/// The Buck2 constraint labels a platform selects on.
///
/// Generated labels are returned sorted, so the emitted `constraint_values`
/// list is deterministic without the caller sorting. A `constraints = [...]`
/// override is returned verbatim in the user's own order (§5.3).
///
/// `all` is the full configured platform set, needed because the abi
/// constraint is conditional on the *set*, not on this platform alone.
pub fn constraint_labels(
    platform: &Platform,
    all: &BTreeMap<String, Platform>,
) -> Vec<String> {
    // The escape hatch replaces the generated labels wholesale, including
    // any abi label the rule below would have added. `os`/`cpu`/`libc`
    // continue to drive npm field matching; only emission is overridden.
    if let Some(overrides) = &platform.constraints {
        return overrides.clone();
    }

    let mut labels = vec![os_label(platform.os).to_string(), cpu_label(platform.cpu).to_string()];

    if let Some(libc) = platform.libc
        && abi_discriminates(platform, all)
    {
        labels.push(abi_label(libc).to_string());
    }

    labels.sort();
    labels
}

/// Does the abi constraint distinguish this platform from another configured
/// one?
///
/// `prelude//platforms:default` derives its configuration from `host_info()`
/// and sets only cpu and os; nothing sets an abi constraint by default. So
/// the abi label is emitted only when it discriminates — when some other
/// configured platform shares this one's os and cpu but declares a
/// *different* libc. A glibc-only configuration, the common case, therefore
/// needs zero user wiring (design §7).
///
/// A platform need not exclude itself from this scan: it shares its own os,
/// cpu and libc, so it can never be its own discriminator.
fn abi_discriminates(platform: &Platform, all: &BTreeMap<String, Platform>) -> bool {
    let Some(libc) = platform.libc else { return false };
    all.values().any(|other| {
        other.os == platform.os
            && other.cpu == platform.cpu
            && other.libc.is_some_and(|l| l != libc)
    })
}

/// npm's `darwin` is the prelude's `macos`, and `win32` is `windows` —
/// neither is a pass-through.
fn os_label(os: Os) -> &'static str {
    match os {
        Os::Linux => "prelude//os/constraints:linux",
        Os::Darwin => "prelude//os/constraints:macos",
        Os::Win32 => "prelude//os/constraints:windows",
    }
}

fn cpu_label(cpu: Cpu) -> &'static str {
    match cpu {
        Cpu::X64 => "prelude//cpu/constraints:x86_64",
        Cpu::Arm64 => "prelude//cpu/constraints:arm64",
    }
}

/// npm's `glibc` is the prelude's `gnu`.
fn abi_label(libc: Libc) -> &'static str {
    match libc {
        Libc::Glibc => "prelude//abi/constraints:gnu",
        Libc::Musl => "prelude//abi/constraints:musl",
    }
}
```

The `if let ... && ...` let-chain is stable in edition 2024 on MSRV 1.88,
which is the declared floor.

Add to `src/platform/mod.rs`:

```rust
pub mod constraints;
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test --lib platform::constraints
```

Expected: PASS, 10 tests.

- [ ] **Step 5: Verify the tests can fail**

1. Change `Os::Darwin` to map to `prelude//os/constraints:darwin` →
   `npm_and_prelude_vocabularies_differ_on_darwin_and_glibc` must fail.
2. Make `abi_discriminates` return `platform.libc.is_some()` →
   `glibc_only_configuration_emits_no_abi_constraint` must fail.
3. Change `other.libc.is_some_and(|l| l != libc)` to `other.libc != Some(libc)`
   → `a_platform_with_no_libc_never_gains_an_abi_constraint` must fail (the
   `linux-x64` entry would start discriminating `linux-x64-musl`).
4. Sort the override before returning →
   `constraints_override_replaces_generated_labels_entirely` must fail.

Revert all four.

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -A
git commit -m "feat(platform): Buck2 constraint labels with a conditional abi

The abi label is emitted only when it discriminates between configured
platforms, so a glibc-only config needs zero user wiring. Tests pin the two
places npm and prelude vocabularies differ: darwin->macos, glibc->gnu."
```

---

### Task 6: `pudu debug platforms`

**Files:**
- Modify: `src/cli/debug.rs`
- Modify: `src/cli/mod.rs`
- Create: `tests/debug_platforms.rs`

**Interfaces:**
- Consumes: `prune(&Graph, &BTreeMap<String, Platform>) -> (Matrix, Vec<PlatformWarning>)` (Task 4); `constraint_labels(&Platform, &BTreeMap<String, Platform>) -> Vec<String>` (Task 5); the existing `print_graph` for the config/lockfile loading idiom.
- Produces: `pub fn platforms() -> anyhow::Result<()>`

- [ ] **Step 1: Factor out the shared loading, then write the failing tests**

`print_graph` already loads `pudu.toml`, resolves `lockfile_path`, reads the
lockfile distinguishing not-found from unreadable, parses it, and prints
warnings. `platforms()` needs all of it. Extract it rather than duplicating —
duplication here would be a review finding, and the two would drift.

Add to `src/cli/debug.rs`, above `print_graph`:

```rust
/// Load `pudu.toml` and the lockfile it names, printing any lockfile
/// warnings to stderr.
///
/// Shared by every `pudu debug` subcommand: they all start from the same
/// two files, and the not-found/unreadable distinction below is worth
/// stating once.
fn load() -> Result<(Config, Lockfile)> {
    let config_path = Path::new("pudu.toml");
    let config_text =
        std::fs::read_to_string(config_path).map_err(|source| CliError::ConfigUnreadable {
            path: config_path.to_path_buf(),
            source,
        })?;
    let config = Config::from_str(&config_text, config_path)?;

    let base = std::env::current_dir()?;
    let lockfile_path = base.join(&config.lockfile_path);
    // Distinguish "not found" from "found but unreadable" (e.g. permissions):
    // the latter is not a missing-file problem, and telling the user to edit
    // `lockfile_path` when the path is already correct is actively wrong
    // advice.
    let lock_text = std::fs::read_to_string(&lockfile_path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            ConfigError::LockfileNotFound { path: lockfile_path.clone() }
        } else {
            ConfigError::LockfileUnreadable { path: lockfile_path.clone(), source }
        }
    })?;

    let (lockfile, warnings) = parse_lockfile(&lock_text, &lockfile_path)?;
    for w in &warnings {
        eprint!("{}", render(w));
    }
    Ok((config, lockfile))
}
```

Then rewrite `print_graph`'s body to use it, keeping its existing JSON
output and the `lockfile_version` comment verbatim:

```rust
pub fn print_graph() -> Result<()> {
    let (_config, lockfile) = load()?;
    let graph = Graph::build(&lockfile)?;
    let out = serde_json::json!({
        // The constant, not an observation of the parsed file: `parse_lockfile`
        // already rejected anything but `SUPPORTED_VERSION` above, so this
        // field can never disagree with the binary. A test asserting
        // `== "9.0"` therefore cannot catch a regression here — it would need
        // to instead assert against the gate in `parse_lockfile`/`LockError`.
        "lockfile_version": SUPPORTED_VERSION,
        "settings": lockfile.settings,
        "roots": graph.roots,
        "nodes": graph.nodes,
        "cycles": graph.cycles,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
```

Add `use crate::lock::types::Lockfile;` to the imports.

Now create `tests/debug_platforms.rs`. `tests/common/mod.rs` already
provides `pudu()` (an `assert_cmd::Command`), `GOOD_CONFIG`, and
`project(...)`; read it before writing, and follow
`tests/debug_print_graph.rs` for the established shape of a debug-command
test.

```rust
//! `pudu debug platforms` — the hidden per-platform pruning view.

use std::process::Command;

/// A tempdir holding a `pudu.toml` and a `pnpm-lock.yaml`.
///
/// `tests/common`'s `project` takes only a config and writes a stub
/// lockfile, and `scratch_with_lockfile` writes a `pudu.toml` with no
/// platforms — this test needs both halves to be its own, so it builds the
/// directory here rather than widening a helper three other test crates
/// share.
fn project(config: &str, lockfile: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    std::fs::write(d.path().join("pudu.toml"), config).expect("write pudu.toml");
    std::fs::write(d.path().join("pnpm-lock.yaml"), lockfile).expect("write lockfile");
    d
}

/// A lockfile with one ungated package and two platform-gated ones.
const LOCK: &str = r#"lockfileVersion: '9.0'

importers:

  .:
    dependencies:
      app:
        specifier: 1.0.0
        version: 1.0.0

packages:

  app@1.0.0:
    resolution: {integrity: sha512-app}

  '@esbuild/linux-x64@0.25.12':
    resolution: {integrity: sha512-lin}
    cpu: [x64]
    os: [linux]

  '@esbuild/darwin-arm64@0.25.12':
    resolution: {integrity: sha512-dar}
    cpu: [arm64]
    os: [darwin]

snapshots:

  app@1.0.0:
    optionalDependencies:
      '@esbuild/linux-x64': 0.25.12
      '@esbuild/darwin-arm64': 0.25.12

  '@esbuild/linux-x64@0.25.12':
    optional: true

  '@esbuild/darwin-arm64@0.25.12':
    optional: true
"#;

const CONFIG: &str = r#"lockfile_path   = "pnpm-lock.yaml"
third_party_dir = "third-party/js"

[platforms.linux-x64-gnu]
os   = "linux"
cpu  = "x64"
libc = "glibc"

[platforms.darwin-arm64]
os  = "darwin"
cpu = "arm64"
"#;

fn run() -> (serde_json::Value, String) {
    let dir = project(CONFIG, LOCK);
    let out = Command::new(env!("CARGO_BIN_EXE_pudu"))
        .args(["debug", "platforms"])
        .current_dir(dir.path())
        .output()
        .expect("run pudu");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    (
        serde_json::from_slice(&out.stdout).expect("stdout is JSON"),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn prints_one_entry_per_configured_platform() {
    let (json, _) = run();
    let p = json["platforms"].as_object().expect("platforms is an object");
    assert_eq!(p.len(), 2);
    assert!(p.contains_key("linux-x64-gnu"));
    assert!(p.contains_key("darwin-arm64"));
}

#[test]
fn each_platform_keeps_its_own_gated_package_and_prunes_the_other() {
    let (json, _) = run();
    let lin = &json["platforms"]["linux-x64-gnu"];
    assert_eq!(lin["node_count"], 2);
    assert_eq!(
        lin["pruned"].as_array().unwrap(),
        &vec![serde_json::json!("@esbuild/darwin-arm64@0.25.12")]
    );

    let mac = &json["platforms"]["darwin-arm64"];
    assert_eq!(
        mac["pruned"].as_array().unwrap(),
        &vec![serde_json::json!("@esbuild/linux-x64@0.25.12")]
    );
}

#[test]
fn reports_the_platform_axes_and_generated_constraints() {
    let (json, _) = run();
    let lin = &json["platforms"]["linux-x64-gnu"];
    assert_eq!(lin["os"], "linux");
    assert_eq!(lin["cpu"], "x64");
    assert_eq!(lin["libc"], "glibc");
    // Sorted, and with no abi constraint: only one libc is configured, so
    // abi does not discriminate.
    assert_eq!(
        lin["constraints"].as_array().unwrap(),
        &vec![
            serde_json::json!("prelude//cpu/constraints:x86_64"),
            serde_json::json!("prelude//os/constraints:linux"),
        ]
    );
    assert_eq!(lin["constraints_overridden"], false);

    let mac = &json["platforms"]["darwin-arm64"];
    assert!(mac["libc"].is_null(), "darwin configures no libc");
}

#[test]
fn an_excluded_optional_dependency_is_not_a_dropped_required_edge() {
    let (json, _) = run();
    for name in ["linux-x64-gnu", "darwin-arm64"] {
        assert!(
            json["platforms"][name]["dropped_required_edges"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{name}"
        );
    }
}

/// stdout must stay machine-parseable, so diagnostics go to stderr.
#[test]
fn stdout_is_pure_json_and_warnings_go_to_stderr() {
    let (json, _) = run();
    assert!(json.is_object());
}

#[test]
fn output_is_byte_identical_across_runs() {
    let dir = project(CONFIG, LOCK);
    let a = Command::new(env!("CARGO_BIN_EXE_pudu"))
        .args(["debug", "platforms"])
        .current_dir(dir.path())
        .output()
        .expect("run pudu");
    let b = Command::new(env!("CARGO_BIN_EXE_pudu"))
        .args(["debug", "platforms"])
        .current_dir(dir.path())
        .output()
        .expect("run pudu");
    assert_eq!(a.stdout, b.stdout);
}
```

Note this file deliberately does **not** `mod common;`. Its helpers do not
fit: `project` takes only a config and writes a stub lockfile, and
`scratch_with_lockfile` writes a `pudu.toml` carrying no `[platforms]`
tables. Widening either would touch three other test crates that already
depend on their exact shapes (see the `#[allow(dead_code)]` note in
`tests/common/mod.rs` — every file under `tests/` compiles as its own
crate). A six-line local helper is the smaller change.

`tempfile` is already a dependency of the crate, so it is available here.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --test debug_platforms
```

Expected: FAIL — `error: unrecognized subcommand 'platforms'`.

- [ ] **Step 3: Wire the subcommand**

In `src/cli/mod.rs`, add to `DebugCommands`:

```rust
    /// Print the per-platform pruning view as JSON.
    Platforms,
```

and to the dispatch `match`:

```rust
                DebugCommands::Platforms => debug::platforms(),
```

- [ ] **Step 4: Implement the command**

Append to `src/cli/debug.rs`:

```rust
/// Print the per-platform pruning view as JSON on stdout.
///
/// Warnings go to stderr via [`render`]; the JSON goes to stdout, so stdout
/// stays machine-parseable.
///
/// Every field here is pudu's own invention rather than an echo of the
/// lockfile, so every key is `snake_case` (S1's key-spelling rule).
pub fn platforms() -> Result<()> {
    let (config, lockfile) = load()?;
    let graph = Graph::build(&lockfile)?;
    let (matrix, warnings) = prune(&graph, &config.platforms);
    for w in &warnings {
        eprint!("{}", render(w));
    }

    let mut out = serde_json::Map::new();
    for (name, platform) in &config.platforms {
        let view = &matrix.views[name];
        out.insert(
            name.clone(),
            serde_json::json!({
                "os": platform.os.as_npm(),
                "cpu": platform.cpu.as_npm(),
                "libc": platform.libc.map(|l| l.as_npm()),
                "constraints": constraint_labels(platform, &config.platforms),
                // Recorded so a user debugging a mis-selected target can see
                // the escape hatch applied without re-reading their config.
                "constraints_overridden": platform.constraints.is_some(),
                "node_count": view.nodes.len(),
                "pruned": view.pruned,
                "dropped_required_edges": view.dropped_required_edges,
            }),
        );
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({ "platforms": out }))?
    );
    Ok(())
}
```

Add the imports it needs at the top of `src/cli/debug.rs`:

```rust
use crate::platform::constraints::constraint_labels;
use crate::platform::prune::prune;
```

`config.platforms` is a `BTreeMap`, and `serde_json::Map` preserves
insertion order (or sorts, under the `preserve_order` feature being off),
so iterating it gives deterministic output either way.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test --test debug_platforms
```

Expected: PASS, 6 tests.

- [ ] **Step 6: Confirm the help output still hides the debug surface**

S0 pinned the top-level help with an insta snapshot. Adding a hidden
subcommand must not change it:

```bash
cargo test
```

Expected: PASS, including the help snapshot. If it changed, the new
subcommand is not hidden — `Debug` carries `#[command(hide = true)]` at the
`Commands` level, which is what hides the whole surface.

- [ ] **Step 7: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -A
git commit -m "feat(cli): hidden \`pudu debug platforms\`

Prints the per-platform view as JSON on stdout, warnings on stderr. The
config/lockfile loading shared with print-graph is factored into \`load\`
rather than duplicated."
```

---

### Task 7: The captured per-platform oracles

The strongest correctness evidence in the stage: pudu's pruning must
reproduce, exactly, the set of packages pnpm itself installs.

**Files:**
- Create: `tests/fixtures/lock/real/oracle/capture.sh`
- Create: `tests/fixtures/lock/real/oracle/linux-x64-gnu.txt`
- Create: `tests/fixtures/lock/real/oracle/linux-x64-musl.txt`
- Create: `tests/fixtures/lock/real/oracle/linux-arm64-gnu.txt`
- Create: `tests/fixtures/lock/real/oracle/darwin-arm64.txt`
- Create: `tests/fixtures/lock/real/oracle/engine-excluded.txt`
- Create: `tests/platform_oracle.rs`
- Modify: `tests/fixtures/lock/real/README.md`

**Interfaces:**
- Consumes: `prune` (Task 4); `crate::lock::snapshot_key::target_name(&str) -> String` (S1).
- Produces: the committed oracle files.

- [ ] **Step 1: Write the capture script**

Provenance must be executable, not prose. Create
`tests/fixtures/lock/real/oracle/capture.sh`:

```bash
#!/usr/bin/env bash
# Regenerate the per-platform pruning oracles.
#
# Each oracle is the exact set of directory names pnpm creates in
# `node_modules/.pnpm/` when installing this fixture's lockfile with
# `supportedArchitectures` pinned to one platform. `tests/platform_oracle.rs`
# asserts pudu reproduces each set exactly.
#
# Requires pnpm and node on PATH. Run from anywhere:
#     ./tests/fixtures/lock/real/oracle/capture.sh
#
# IMPORTANT: regenerate ALL files together, including engine-excluded.txt,
# and update the versions recorded in ../README.md. An oracle captured with
# one node version and an exclusion list from another is silently wrong.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$(dirname "$HERE")"

capture() {
  local out="$1" os="$2" cpu="$3" libc="$4"
  local w
  w="$(mktemp -d)"
  cp -r "$SRC"/. "$w"/
  rm -rf "$w/node_modules" "$w"/packages/*/node_modules "$w/oracle"
  cat > "$w/pnpm-workspace.yaml" <<YAML
packages:
  - "packages/*"
supportedArchitectures:
  os:
    - $os
  cpu:
    - $cpu
  libc:
    - $libc
YAML
  ( cd "$w" && pnpm install --ignore-scripts --frozen-lockfile >/dev/null 2>&1 )
  if ! diff -q "$SRC/pnpm-lock.yaml" "$w/pnpm-lock.yaml" >/dev/null; then
    echo "FATAL: the lockfile drifted capturing $os/$cpu/$libc" >&2
    exit 1
  fi
  ls "$w/node_modules/.pnpm" | grep -vx 'node_modules' | grep -vx 'lock.yaml' \
    | LC_ALL=C sort > "$HERE/$out"
  rm -rf "$w"
  echo "  $out: $(wc -l < "$HERE/$out") directories"
}

echo "pnpm $(pnpm --version), node $(node --version)"
capture linux-x64-gnu.txt   linux  x64   glibc
capture linux-x64-musl.txt  linux  x64   musl
capture linux-arm64-gnu.txt linux  arm64 glibc
capture darwin-arm64.txt    darwin arm64 glibc

# pnpm skips an OPTIONAL dependency that fails `engines`, so the listings
# above are not a pure platform oracle. Pudu does not model `engines`
# (node version is not a platform axis), so the test subtracts this set.
# It depends on the node version used here — regenerate it with the rest.
node "$HERE/engine-excluded.mjs" > "$HERE/engine-excluded.txt"
echo "  engine-excluded.txt: $(wc -l < "$HERE/engine-excluded.txt") key(s)"
```

And its helper, `tests/fixtures/lock/real/oracle/engine-excluded.mjs`:

```javascript
// Print the snapshot keys pnpm skips for `engines` rather than platform:
// optional dependencies whose `engines` the running node does not satisfy.
//
// Needs `@pnpm/package-is-installable` and `yaml`; run via `npx`:
//     npx -p @pnpm/package-is-installable -p yaml node engine-excluded.mjs
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const YAML = await import('yaml');
const { checkEngine } = await import('@pnpm/package-is-installable/lib/checkEngine.js');

const lock = YAML.parse(fs.readFileSync(path.join(here, '..', 'pnpm-lock.yaml'), 'utf8'));
const base = (k) => { const i = k.indexOf('('); return i < 0 ? k : k.slice(0, i); };

const out = [];
for (const [key, snap] of Object.entries(lock.snapshots)) {
  if (snap?.optional !== true) continue;             // only optional deps are skipped
  const meta = lock.packages[base(key)];
  if (!meta?.engines) continue;
  if (checkEngine(key, meta.engines, { node: process.version.slice(1), pnpm: '10.21.0' }) != null) {
    out.push(key);
  }
}
out.sort();
for (const k of out) console.log(k);
```

```bash
chmod +x tests/fixtures/lock/real/oracle/capture.sh
```

- [ ] **Step 2: Capture the oracles**

```bash
./tests/fixtures/lock/real/oracle/capture.sh
```

Expected output, matching the survey's validated capture:

```
  linux-x64-gnu.txt: 316 directories
  linux-x64-musl.txt: 316 directories
  linux-arm64-gnu.txt: 316 directories
  darwin-arm64.txt: 315 directories
```

If any count differs, **stop and report it**: either the lockfile changed
or pnpm's behaviour did, and the spec's validation no longer holds. Do not
adjust the expected numbers to match.

Then confirm the linux-x64-gnu oracle equals S1's committed listing, which
is what keeps the two fixtures from drifting apart:

```bash
diff <(LC_ALL=C sort tests/fixtures/lock/real/virtual-store-listing.txt) \
     tests/fixtures/lock/real/oracle/linux-x64-gnu.txt && echo IDENTICAL
```

Expected: `IDENTICAL`.

- [ ] **Step 3: Write the failing test**

Create `tests/platform_oracle.rs`:

```rust
//! Differential test: pudu's pruning against pnpm's own install set.
//!
//! Each oracle under `tests/fixtures/lock/real/oracle/` is the exact set of
//! directories pnpm created in `node_modules/.pnpm/` for one platform. Pudu
//! must reproduce every one. See `oracle/capture.sh` for provenance.

use std::collections::{BTreeMap, BTreeSet};

use pudu::config::Platform;
use pudu::lock::parse_lockfile;
use pudu::lock::graph::Graph;
use pudu::lock::snapshot_key::target_name;
use pudu::platform::prune::prune;
use pudu::platform::{Cpu, Libc, Os};

const FIXTURE: &str = "tests/fixtures/lock/real";

fn read(rel: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURE}/{rel}"))
        .unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn oracle(name: &str) -> BTreeSet<String> {
    read(&format!("oracle/{name}.txt")).lines().map(str::to_string).collect()
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
    Platform { os, cpu, libc, constraints: None }
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
    assert_reproduces("linux-x64-gnu", platform(Os::Linux, Cpu::X64, Some(Libc::Glibc)));
}

#[test]
fn reproduces_pnpm_on_linux_x64_musl() {
    assert_reproduces("linux-x64-musl", platform(Os::Linux, Cpu::X64, Some(Libc::Musl)));
}

#[test]
fn reproduces_pnpm_on_linux_arm64_gnu() {
    assert_reproduces("linux-arm64-gnu", platform(Os::Linux, Cpu::Arm64, Some(Libc::Glibc)));
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
    let listing: BTreeSet<String> =
        read("virtual-store-listing.txt").lines().map(str::to_string).collect();
    assert_eq!(oracle("linux-x64-gnu"), listing);
}

/// The roadmap's demo criterion: each configured platform resolves
/// esbuild's ~20 optional deps to exactly one `@esbuild/*` per version.
#[test]
fn each_platform_keeps_exactly_one_esbuild_binary_per_version() {
    for (name, p) in [
        ("linux-x64-gnu", platform(Os::Linux, Cpu::X64, Some(Libc::Glibc))),
        ("linux-arm64-gnu", platform(Os::Linux, Cpu::Arm64, Some(Libc::Glibc))),
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
```

`pudu::lock::graph` and `pudu::lock::snapshot_key` must be reachable from
outside the crate. If they are private, make the modules `pub` in
`src/lock/mod.rs` — S1 already re-exports `Graph` and `parse_lockfile`, so
prefer whatever path already works and adjust the `use` lines to match
rather than widening visibility unnecessarily.

- [ ] **Step 4: Run the test to verify it fails, then passes**

Run before the oracles exist to confirm the test is wired to real data:

```bash
cargo test --test platform_oracle
```

Expected after Step 2's capture: **PASS**, 6 tests. If a platform test
fails, do not adjust the oracle — the oracle is pnpm's answer and pudu is
what must change.

- [ ] **Step 5: Verify the test can fail**

Temporarily change `admits` so the `any`-singleton check reads
`list.iter().any(|e| e == "any")`, and confirm the oracle tests still pass
(no fixture entry uses `any`) — then change `admits_platform` to ignore the
`cpu` axis and confirm **all four** platform tests fail. Revert.

This is the point of the exercise: it proves the oracle test is sensitive to
the pruning logic and not merely to the fixture parsing.

- [ ] **Step 6: Document the oracles in the fixture README**

Append to `tests/fixtures/lock/real/README.md`:

```markdown
## Per-platform pruning oracles (S2)

`oracle/*.txt` are the same kind of capture as `virtual-store-listing.txt`,
one per platform: the exact directories pnpm created in `node_modules/.pnpm/`
with `supportedArchitectures` pinned. `tests/platform_oracle.rs` asserts pudu
reproduces each set exactly.

| platform | directories |
|---|---|
| linux-x64-gnu | 316 |
| linux-x64-musl | 316 |
| linux-arm64-gnu | 316 |
| darwin-arm64 | 315 |

`oracle/linux-x64-gnu.txt` is byte-identical to `virtual-store-listing.txt`,
which is asserted by a test — S1's fixture was captured on a glibc x86_64
host, and the two must not drift apart.

Captured with **pnpm 10.21.0** and **node v24.6.0** by
`oracle/capture.sh`. Regenerate every file together, including
`engine-excluded.txt`, and update the versions here.

### Why `engine-excluded.txt` exists

pnpm skips an **optional** dependency that fails its `engines` check, so a
virtual-store listing is not a pure *platform* oracle. Pudu deliberately does
not model `engines` — node version is not a platform axis (design §5) — so
its survivor set is a superset of pnpm's by exactly that set, and the test
subtracts it.

At the recorded node version this is one package,
`@napi-rs/lzma-linux-x64-gnu@1.5.1`, which wants
`node: ^22.20 || ^24.12 || >=25`. It is eligible by platform on linux-x64 and
skipped by engines, and it was the single discrepancy when this design was
validated. On node ≥ 24.12 the file is empty — which is correct, not broken.

### What these oracles cannot catch

Every one of the fixture's 90 platform-gated snapshot keys is a **leaf**: none
has dependencies of its own. So these oracles cannot distinguish pudu's
per-package pruning from a design that also sweeps away packages left
unreachable when their only parent is pruned. They show the simpler design is
not wrong here, not that a sweep is unnecessary in general (TD-S2-01).

They also carry no `libc` field and no negation — see the platform matching
survey §2 for why no public-registry install can produce either. That coverage
comes from `tests/platform_fuzz.rs` instead.
```

- [ ] **Step 7: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -A
git commit -m "test(platform): pruning reproduces pnpm's install set on four platforms

Each oracle is the exact virtual-store listing pnpm produced with
supportedArchitectures pinned. The test subtracts engine-excluded optional
deps, which pnpm skips for reasons pudu deliberately does not model, and
asserts the linux-x64-gnu oracle still equals S1's committed listing so the
two fixtures cannot drift apart."
```

---

### Task 8: Differential fuzz against pnpm's matcher

Where libc and negation coverage actually comes from: no public-registry
fixture can produce either (survey §2), so the matcher is checked against
pnpm's real implementation instead.

**Files:**
- Create: `tests/platform_fuzz.rs`
- Create: `tests/fixtures/platform/reference.mjs`

**Interfaces:**
- Consumes: `admits(Option<&[String]>, &str) -> bool` (Task 1).
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Write the reference harness**

Create `tests/fixtures/platform/reference.mjs`. It reads one JSON case per
line on stdin and writes one `true`/`false` per line, so the Rust side pays
a single process spawn for thousands of cases.

```javascript
// Reference oracle for `pudu::platform::admits`, backed by pnpm's own
// matcher. Reads one JSON object per line: {"list": [...] | null,
// "current": "linux", "axis": "os"}. Writes "true"/"false" per line.
//
// pnpm exposes the rule only through `checkPlatform`, which evaluates three
// axes at once, so each case is asked as a package declaring ONLY the axis
// under test — the other two are left absent and admit everything.
import readline from 'node:readline';

const { checkPlatform } = await import('@pnpm/package-is-installable/lib/checkPlatform.js');

const rl = readline.createInterface({ input: process.stdin, terminal: false });
const out = [];
for await (const line of rl) {
  if (!line.trim()) continue;
  const { list, current, axis } = JSON.parse(line);

  // `supportedArchitectures` pins the "current" value for each axis. The
  // libc axis is additionally gated on the HOST having a detectable libc,
  // so this harness must run on Linux for libc cases to be meaningful.
  const sa = { os: ['linux'], cpu: ['x64'], libc: ['glibc'] };
  sa[axis] = [current];

  const wanted = { os: ['any'], cpu: ['any'], libc: ['any'] };
  wanted[axis] = list === null ? ['any'] : list;

  out.push(checkPlatform('probe', wanted, sa) === null ? 'true' : 'false');
}
process.stdout.write(out.join('\n') + '\n');
```

- [ ] **Step 2: Write the fuzz test**

Create `tests/platform_fuzz.rs`:

```rust
//! Differential fuzz: `admits` against pnpm's real `checkPlatform`.
//!
//! `#[ignore]`d by default so the suite needs neither node nor a network.
//! Run explicitly:
//!
//! ```sh
//! cd tests/fixtures/platform && npm install @pnpm/package-is-installable && cd -
//! cargo test --test platform_fuzz -- --ignored --nocapture
//! ```
//!
//! This is where `libc` and negation coverage comes from. No install against
//! the public npm registry can produce a lockfile carrying a `libc` field —
//! pnpm fetches npm's abbreviated packument, which omits it — so no fixture
//! can exercise those paths. See the platform matching survey §2.

use std::io::Write;
use std::process::{Command, Stdio};

use pudu::platform::admits;

/// A tiny deterministic PRNG: the corpus must be reproducible, and pulling
/// in a dependency for a developer-only test is not worth it.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const OS_TOKENS: &[&str] =
    &["linux", "darwin", "win32", "aix", "android", "freebsd", "sunos", "openharmony", "any"];
const CPU_TOKENS: &[&str] =
    &["x64", "arm64", "arm", "ia32", "ppc64", "s390x", "wasm32", "loong64", "any"];
const LIBC_TOKENS: &[&str] = &["glibc", "musl", "any", "unknown"];

fn tokens(axis: &str) -> &'static [&'static str] {
    match axis {
        "os" => OS_TOKENS,
        "cpu" => CPU_TOKENS,
        _ => LIBC_TOKENS,
    }
}

/// One generated case: a field list (or absence) and the value to test it
/// against.
struct Case {
    axis: &'static str,
    list: Option<Vec<String>>,
    current: String,
}

fn generate(count: usize) -> Vec<Case> {
    let mut rng = Rng(0x5EED_1234_ABCD_9876);
    let mut cases = Vec::with_capacity(count);

    for i in 0..count {
        let axis = ["os", "cpu", "libc"][i % 3];
        let pool = tokens(axis);
        let current = pool[rng.below(pool.len())].to_string();

        // Shapes, weighted so the interesting ones are well covered:
        // absent, empty, singleton, multi, all-negative, and mixed.
        let list = match rng.below(10) {
            0 => None,
            1 => Some(Vec::new()),
            2..=4 => Some(vec![pool[rng.below(pool.len())].to_string()]),
            5 | 6 => {
                let n = 1 + rng.below(3);
                Some((0..n).map(|_| pool[rng.below(pool.len())].to_string()).collect())
            }
            7 | 8 => {
                let n = 1 + rng.below(3);
                Some((0..n).map(|_| format!("!{}", pool[rng.below(pool.len())])).collect())
            }
            _ => {
                // Mixed positive and negative — the shape whose rule is
                // least intuitive and most worth fuzzing.
                let n = 2 + rng.below(3);
                Some(
                    (0..n)
                        .map(|j| {
                            let t = pool[rng.below(pool.len())];
                            if j % 2 == 0 { format!("!{t}") } else { t.to_string() }
                        })
                        .collect(),
                )
            }
        };

        cases.push(Case { axis, list, current });
    }
    cases
}

#[test]
#[ignore = "requires node and @pnpm/package-is-installable; run explicitly"]
fn admits_agrees_with_pnpm_check_platform() {
    let cases = generate(3000);

    let mut input = String::new();
    for c in &cases {
        let list = match &c.list {
            None => "null".to_string(),
            Some(v) => serde_json::to_string(v).expect("serialize list"),
        };
        input.push_str(&format!(
            r#"{{"list":{list},"current":{},"axis":{}}}"#,
            serde_json::to_string(&c.current).unwrap(),
            serde_json::to_string(c.axis).unwrap()
        ));
        input.push('\n');
    }

    let mut child = Command::new("node")
        .arg("reference.mjs")
        .current_dir("tests/fixtures/platform")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn node — see this file's header for setup");
    child.stdin.as_mut().unwrap().write_all(input.as_bytes()).expect("write cases");
    let out = child.wait_with_output().expect("run reference");
    assert!(out.status.success(), "reference harness failed");

    let expected: Vec<bool> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l == "true")
        .collect();
    assert_eq!(expected.len(), cases.len(), "one verdict per case");

    let mut disagreements = Vec::new();
    for (c, want) in cases.iter().zip(expected) {
        let got = admits(c.list.as_deref(), &c.current);
        if got != want {
            disagreements.push(format!(
                "axis={} list={:?} current={} pudu={} pnpm={}",
                c.axis, c.list, c.current, got, want
            ));
        }
    }

    assert!(
        disagreements.is_empty(),
        "{} of {} cases disagree with pnpm:\n{}",
        disagreements.len(),
        cases.len(),
        disagreements.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
    );

    println!("{} cases, zero disagreements with pnpm", cases.len());
}
```

Add a `.gitignore` entry so the reference module is never committed:

```bash
printf 'node_modules/\npackage-lock.json\npackage.json\n' > tests/fixtures/platform/.gitignore
```

- [ ] **Step 3: Run the fuzz**

```bash
cd tests/fixtures/platform && npm install @pnpm/package-is-installable && cd -
cargo test --test platform_fuzz -- --ignored --nocapture
```

Expected: PASS, printing `3000 cases, zero disagreements with pnpm`.

**If any case disagrees, pudu is wrong, not pnpm.** Fix `admits` and record
what the disagreement was — it is exactly the kind of rule this test exists
to find.

Note the libc axis is only meaningful when the harness runs on a host with a
detectable libc (Linux). On macOS pnpm skips libc entirely, so those cases
would all return `true` from the reference; if you are on a Mac, say so in
the task report rather than reporting a clean run.

- [ ] **Step 4: Confirm the default suite does not need node**

```bash
cargo test
```

Expected: PASS, with `platform_fuzz` reported as ignored. The suite must not
require node.

- [ ] **Step 5: Record the run in the fixture README**

Append to `tests/fixtures/lock/real/README.md`, under the S2 section added
in Task 7:

```markdown
### Differential fuzz

`tests/platform_fuzz.rs` checks `admits` against pnpm's real `checkPlatform`
over 3000 generated cases spanning absent, empty, singleton, multi-entry,
all-negative and mixed lists on all three axes. It is `#[ignore]`d so the
default suite needs no node.

Last run: **3000 cases, zero disagreements**, against
`@pnpm/package-is-installable@1000.0.21` on node v24.6.0.
```

If the actual run reports different numbers or a different reference
version, record what actually happened rather than this text.

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -A
git commit -m "test(platform): fuzz \`admits\` against pnpm's real checkPlatform

3000 generated cases across all three axes. This is where libc and negation
coverage comes from: npm's abbreviated packument omits libc, so no
public-registry fixture can exercise that path at all."
```

---

### Task 9: Close TD-S0-08 and TD-S0-09, and update the ledger

Both rows were opened against `pudu init`'s `supportedArchitectures`
expansion and targeted at S2, which is the stage that makes that parsing
load-bearing.

**Files:**
- Modify: `src/cli/init.rs`
- Modify: `src/error.rs`
- Modify: `docs/superpowers/TECH_DEBT.md`

**Interfaces:**
- Consumes: the existing `fn axis(&serde_norway::Value, &str) -> Vec<String>` and `DeriveWarning` in `src/cli/init.rs` / `src/error.rs`.
- Produces: two new `DeriveWarning` variants.

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` block in `src/cli/init.rs`. Read the existing
tests first and follow their shape for calling `derive_platforms`.

```rust
    /// TD-S0-08: `os: linux` (a bare scalar, not a sequence) is a plausible
    /// typo. It must say so, not fail elsewhere with a misleading message.
    #[test]
    fn a_non_sequence_axis_warns_naming_the_axis() {
        let yaml = "supportedArchitectures:\n  os: linux\n  cpu: [x64]\n";
        let d = derive_platforms(Some(yaml)).expect("must not be fatal");
        assert!(
            d.warnings.iter().any(|w| matches!(
                w,
                DeriveWarning::AxisNotASequence { key } if key == "os"
            )),
            "warnings: {:?}",
            d.warnings
        );
    }

    /// TD-S0-08: a non-mapping `supportedArchitectures` was ignored in
    /// silence, which is the worst outcome — the user's intent vanishes.
    #[test]
    fn a_non_mapping_supported_architectures_warns() {
        let yaml = "supportedArchitectures: linux\n";
        let d = derive_platforms(Some(yaml)).expect("must not be fatal");
        assert!(
            d.warnings
                .iter()
                .any(|w| matches!(w, DeriveWarning::SupportedArchitecturesNotAMapping)),
            "warnings: {:?}",
            d.warnings
        );
    }

    /// TD-S0-09: a non-string entry was dropped in silence.
    #[test]
    fn a_non_string_axis_entry_warns_rather_than_vanishing() {
        let yaml = "supportedArchitectures:\n  os: [linux]\n  cpu: [123, x64]\n";
        let d = derive_platforms(Some(yaml)).expect("must not be fatal");
        assert!(
            d.warnings.iter().any(|w| matches!(
                w,
                DeriveWarning::NonStringAxisEntry { key, .. } if key == "cpu"
            )),
            "warnings: {:?}",
            d.warnings
        );
    }

    /// TD-S0-09: the unknown-`cpu` arm had no test at all. `os` and `libc`
    /// were already covered; this closes the gap.
    #[test]
    fn an_unknown_cpu_value_warns() {
        let yaml = "supportedArchitectures:\n  os: [linux]\n  cpu: [ppc64, x64]\n";
        let d = derive_platforms(Some(yaml)).expect("must not be fatal");
        assert!(
            d.warnings.iter().any(|w| matches!(
                w,
                DeriveWarning::UnknownCpu { value } if value == "ppc64"
            )),
            "warnings: {:?}",
            d.warnings
        );
    }
```

The exact `DeriveWarning::UnknownCpu` variant name may differ — read
`src/error.rs` and use the real one. If the unknown-cpu variant genuinely
does not exist, add it beside the existing `UnknownOs` / `UnknownLibc`
variants, matching their message and `help` style.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --lib cli::init
```

Expected: FAIL to compile — the new `DeriveWarning` variants do not exist.

- [ ] **Step 3: Add the warning variants**

In `src/error.rs`, beside the existing `DeriveWarning` variants:

```rust
    #[error("pnpm-workspace.yaml: supportedArchitectures.{key} must be a list, e.g. `{key}: [{example}]`")]
    #[diagnostic(
        severity(Warning),
        code(pudu::init::axis_not_a_sequence),
        help("a bare value is ignored; wrap it in brackets to make it a one-entry list")
    )]
    AxisNotASequence { key: String, example: String },

    #[error("pnpm-workspace.yaml: supportedArchitectures must be a mapping of os/cpu/libc lists")]
    #[diagnostic(
        severity(Warning),
        code(pudu::init::supported_architectures_not_a_mapping),
        help("the block is ignored; see https://pnpm.io/settings#supportedarchitectures")
    )]
    SupportedArchitecturesNotAMapping,

    #[error("pnpm-workspace.yaml: ignoring non-string entry in supportedArchitectures.{key}")]
    #[diagnostic(
        severity(Warning),
        code(pudu::init::non_string_axis_entry),
        help("every entry must be a quoted or bare string, e.g. `cpu: [x64, arm64]`")
    )]
    NonStringAxisEntry { key: String },
```

The `example` field keeps the message actionable — `os` suggests `linux`,
`cpu` suggests `x64`, `libc` suggests `glibc`. Adjust the test's
`matches!` pattern to `DeriveWarning::AxisNotASequence { key, .. }` since
it now has two fields.

- [ ] **Step 4: Make `axis` report instead of discarding**

Replace `fn axis` in `src/cli/init.rs` with a version that reports what it
drops, and update its two or three call sites to thread the warnings
through. `axis_present` stays as it is.

```rust
/// Read a `supportedArchitectures` axis into a list of strings.
///
/// Reports rather than silently discarding: a bare scalar
/// (`os: linux` instead of `os: [linux]`) and a non-string entry
/// (`cpu: [123]`) were both dropped in silence before, so a typo cost the
/// user their whole intent with no diagnostic (TD-S0-08, TD-S0-09).
fn axis(v: &serde_norway::Value, key: &str, warnings: &mut Vec<DeriveWarning>) -> Vec<String> {
    let Some(entry) = v.get(key) else { return Vec::new() };

    let Some(seq) = entry.as_sequence() else {
        warnings.push(DeriveWarning::AxisNotASequence {
            key: key.to_string(),
            example: match key {
                "os" => "linux",
                "cpu" => "x64",
                _ => "glibc",
            }
            .to_string(),
        });
        return Vec::new();
    };

    let mut out = Vec::with_capacity(seq.len());
    let mut reported = false;
    for item in seq {
        match item.as_str() {
            Some(s) => out.push(s.to_string()),
            None if !reported => {
                // Report once per axis: a list of ten bad entries is one
                // mistake, not ten.
                reported = true;
                warnings.push(DeriveWarning::NonStringAxisEntry { key: key.to_string() });
            }
            None => {}
        }
    }
    out
}
```

In `derive_platforms`, the `if let Some(map) = sa.as_mapping()` block needs
an `else` arm — the silent-ignore case TD-S0-08 names:

```rust
    if let Some(map) = sa.as_mapping() {
        for k in map.keys().filter_map(|k| k.as_str()) {
            if !matches!(k, "os" | "cpu" | "libc") {
                warnings.push(DeriveWarning::UnknownKey { key: k.to_string() });
            }
        }
    } else {
        warnings.push(DeriveWarning::SupportedArchitecturesNotAMapping);
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test
```

Expected: PASS, whole suite. The existing `init` tests must still pass —
if one now fails because `axis` takes a third argument, thread the
`warnings` vector through rather than changing what the test asserts.

- [ ] **Step 6: Verify the new tests can fail**

Remove the `else` arm added in Step 4 →
`a_non_mapping_supported_architectures_warns` must fail. Restore it. Change
`None if !reported` to `None => {}` → `a_non_string_axis_entry_warns_rather_than_vanishing`
must fail. Restore.

- [ ] **Step 7: Update the tech-debt ledger**

In `docs/superpowers/TECH_DEBT.md`, strike TD-S0-08 and TD-S0-09 in the
same commit as the code — the ledger's own rule. Change their `Target`
cell to `~~closed~~` and append to each Description:

- TD-S0-08: `— ✅ closed in S2 (`DeriveWarning::AxisNotASequence` names the axis and shows the bracket form; `SupportedArchitecturesNotAMapping` replaces the silent ignore)`
- TD-S0-09: `— ✅ closed in S2 (`UnknownCpu` now has a test; `axis()` reports dropped non-string entries via `NonStringAxisEntry` once per axis rather than discarding them)`

Then add the rows S2 opens:

```markdown
| TD-S2-01 | 2026-08-31 | S4 | Pruning has no transitive-reachability sweep: a package that survives its own platform check but whose only parent was pruned stays in the view. Validated as correct against four captured oracles, but all 90 platform-gated keys in the fixture are leaves, so the fixture cannot distinguish this design from one with a sweep. Failure mode if a gated package with a subtree ever appears is an orphan left in the store — fat, not incorrect. Revisit when S4 emits per-package targets. |
| TD-S2-02 | 2026-08-31 | S5 | Pudu does not model `engines`, so its survivor set is a superset of pnpm's by the optional dependencies pnpm skips for node-version reasons. Deliberate (node version is not a platform axis), and the oracle test subtracts them, but it means a package pnpm would never install can reach a generated target. Revisit when S5 makes the node toolchain real. |
| TD-S2-03 | 2026-08-31 | S3 | `libc` pruning is best-effort: npm's abbreviated packument omits the field, so most v9 lockfiles carry no `libc` at all and musl builds are indistinguishable from gnu ones. Pudu matches `libc` when present and never infers it from a package name. A private registry serving full metadata gets stricter pruning than the public one, from the same lockfile shape. |
```

Also re-target the two S1 rows this stage did not close, so the ledger does
not quietly claim they were handled:

- TD-S1-01: change Target from `S2` to `S3` (S2 adds no lockfile parse).
- TD-S1-06: change Target from `S2` to `S4` (`cycles` stays a diagnostic; nothing in S2 depends on its completeness).

And close TD-S1-07 as a decision rather than code, appending to its
Description: `— ✅ closed in S2 as a decision (spec §7.1): `engines` stays parsed and unused because node version is not a platform axis, and the peer-dependency fields are not surfaced by S2. See TD-S2-02 for the measurable consequence.`

- [ ] **Step 8: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
git add -A
git commit -m "fix(init): report what supportedArchitectures parsing discards

Closes TD-S0-08 and TD-S0-09. A bare scalar axis, a non-mapping block, and
non-string entries were all dropped in silence, so a typo cost the user
their whole intent with no diagnostic. Ledger updated in the same commit,
including the three rows S2 opens and the two S1 rows it re-targets rather
than closes."
```

---

## Final Verification

Run before considering the stage complete. Every item is a gate, not a
suggestion.

- [ ] `cargo test` — whole suite green, `platform_fuzz` ignored.
- [ ] `cargo test --test platform_fuzz -- --ignored` — zero disagreements.
- [ ] `cargo clippy --all-targets -- -D warnings` — clean.
- [ ] `cargo fmt --check` — clean.
- [ ] `cargo +1.88 check --all-targets` — MSRV holds.
- [ ] `pudu debug platforms` run twice on the fixture produces identical output:
  ```bash
  cd tests/fixtures/lock/real && \
    diff <(cargo run -q -- debug platforms 2>/dev/null) \
         <(cargo run -q -- debug platforms 2>/dev/null) && echo DETERMINISTIC
  ```
  This needs a `pudu.toml` in the fixture directory; if none exists, create
  one in a temp copy rather than committing one that would confuse the
  lockfile fixtures.
- [ ] Every exit criterion in spec §10 is met, checked one at a time. If one
  is not, say so explicitly rather than reporting the stage complete.
- [ ] The spec and the code agree. Where they diverge, **stop and report the
  divergence** — do not silently amend either. Two such conflicts arose in
  S1 and both needed the human's ruling.
