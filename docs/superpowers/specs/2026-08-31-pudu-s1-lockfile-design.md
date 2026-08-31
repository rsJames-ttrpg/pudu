# S1 — Lockfile Parser & Instance Graph Spec

> Stage spec for pudu S1. Parent: [design](2026-08-30-pudu-design.md) ·
> [roadmap](2026-08-30-pudu-roadmap.md) ·
> evidence: [v9 field survey](../research/2026-08-31-pnpm-lock-v9-field-survey.md)

**Date:** 2026-08-31
**Status:** approved, not yet implemented

---

## 1. Scope

S1 turns a `pnpm-lock.yaml` into a typed, validated **instance graph** — one
node per snapshot key — and exposes it through a hidden `pudu debug
print-graph` command. It is pure parsing and graph construction. No network,
no filesystem writes, no Buck output.

**In scope:** lockfile deserialization · the snapshot-key grammar ·
target-name mangling · edge resolution including npm aliases · importer roots
· cycle detection · the `lockfileVersion` gate · unsupported-feature
rejection · `pudu debug print-graph`.

**Out of scope, and deliberately so:** platform pruning by `os`/`cpu`/`libc`
(S2 — S1 *carries* those fields on the node but never acts on them) ·
tarball fetch, URLs, `pudu.lock` (S3) · BUCK emission (S4) · fixups (S6).

---

## 2. What the survey settled

S1's first obligation under design §12 was to check the design's lockfile
assumptions against real lockfiles. That work is done and recorded in the
[field survey](../research/2026-08-31-pnpm-lock-v9-field-survey.md); this
spec depends on its conclusions:

- **`requiresBuild` is absent from v9** (0 occurrences across 18 v9 files,
  105 across 3 v6 files). Design §4's mandatory `pudu vendor` pass stands.
- **`hasBin` survived into v9** as a bare boolean. S1 records it; S3 uses it
  to cross-check the vendor pass.
- **`bundledDependencies` needs no handling.** pnpm already omits bundled
  names from the snapshot graph, so they never become edges and there is no
  double-install risk. S1 tolerates and ignores the field.
- **Only `{integrity: …}` resolutions occur in the corpus.** Other variants
  must be covered by constructed fixtures and unknown ones rejected by name.
- **Peer suffixes nest recursively** to arbitrary depth.
- **Real snapshot keys reach 422 characters**, so long-key handling is the
  main path, not an edge case. Naming is settled by porting pnpm's own
  `depPathToFilename`, verified byte-exact against 1363 real store
  directories (§5).
- **Cycles are universal** — every lockfile surveyed has them. They must be
  detected and reported, never rejected (§7).
- **Edge values may be npm aliases**, so link name ≠ package name (§6.2).

---

## 3. Lockfile types

`src/lock/types.rs`. All types derive `Debug`, `Clone`, `PartialEq`,
`Deserialize`, and `Serialize` (the latter for `print-graph` and snapshot
tests). Deserialization is via `serde_norway`.

```rust
pub struct Lockfile {
    pub lockfile_version: LockfileVersion,
    pub settings: Settings,                      // default if absent
    pub importers: BTreeMap<String, Importer>,   // path -> importer
    pub packages: BTreeMap<String, PackageMeta>, // "name@version" -> metadata
    pub snapshots: BTreeMap<String, SnapshotEntry>, // snapshot key -> edges
}
```

`BTreeMap` throughout: ordering is an invariant (design §5), and it must come
from the data structure rather than a sort at the emit site.

```rust
pub struct Settings {
    pub auto_install_peers: bool,             // default true
    pub exclude_links_from_lockfile: bool,    // default false
}

pub struct Importer {
    pub dependencies: BTreeMap<String, ImporterDep>,
    pub dev_dependencies: BTreeMap<String, ImporterDep>,
    pub optional_dependencies: BTreeMap<String, ImporterDep>,
}

pub struct ImporterDep {
    pub specifier: String,   // "^4.19.2", "workspace:*", "catalog:"
    pub version: String,     // same encoding as a snapshot edge value
}

pub struct PackageMeta {
    pub resolution: Resolution,
    pub engines: BTreeMap<String, String>,
    pub os: Option<Vec<String>>,     // raw npm strings, negation intact
    pub cpu: Option<Vec<String>>,    // S2 interprets these; S1 only carries them
    pub libc: Option<Vec<String>>,
    pub has_bin: bool,
    pub deprecated: Option<String>,
    pub peer_dependencies: BTreeMap<String, String>,
    pub peer_dependencies_meta: BTreeMap<String, PeerMeta>,
    pub bundled_dependencies: Vec<String>,  // parsed, never acted on
}

pub enum Resolution {
    Integrity { integrity: String },        // sha512-<base64>
    Tarball   { tarball: String },
    Directory { directory: String },
    Git       { repo: String, commit: String },
}

pub struct SnapshotEntry {
    pub dependencies: BTreeMap<String, String>,          // link name -> edge value
    pub optional_dependencies: BTreeMap<String, String>,
    pub optional: bool,
    pub transitive_peer_dependencies: Vec<String>,
}
```

**`os`/`cpu`/`libc` stay as raw `Vec<String>`.** The survey found
`cpu: [wasm32]` — a value outside pudu's `Cpu` enum. Parsing must never fail
on an unknown platform token; S2 interprets these strings and prunes, and an
unrecognised token simply matches no configured platform.

`Resolution` is an untagged-by-key enum: the variant is chosen by which key
is present. A `resolution:` map with none of the four known keys is an error
naming the keys it did carry (§9).

**Unknown fields are tolerated.** No `deny_unknown_fields` on these structs —
pnpm adds fields between minor releases, and failing on one pudu does not read
would break users for no benefit. Unknown *top-level* keys are different and
do get a warning (§5), because those signal whole features.

---

## 4. Snapshot key grammar

`src/lock/snapshot_key.rs`. This is the core of S1 and the place a naive
implementation goes wrong.

```
key     := name "@" version peers?
name    := ("@" scope "/")? ident
peers   := "(" key ")" peers?
```

A peer is **itself a full key**, so the grammar is recursive and the
parentheses are balanced to arbitrary depth. From the corpus:

```
eslint-plugin-svelte@3.14.0(eslint@9.39.2(jiti@2.6.1))(svelte@5.49.1)
```

### Parsing algorithm

1. Scan the string tracking paren depth. The **first `(` at depth 0** begins
   the peer suffix; everything before it is the head. No `(` at depth 0 means
   no peers.
2. In the head, the split between name and version is the **last `@` at index
   > 0**. Index 0 is excluded because a scoped name starts with `@`.
3. Split the suffix into peer groups at depth-0 parens; parse each
   recursively.

Two rules that a shortcut violates, both stated so the tests can target them:

- **Never split on the first `(`** without depth tracking — nested peers make
  that wrong.
- **Never split the head on the first `@`** — `@scope/name@1.0.0` breaks.

Unbalanced parens, an empty name, an empty version, or a missing `@` are
parse errors naming the offending key and the byte offset.

```rust
pub struct SnapshotKey {
    pub name: String,       // "@sveltejs/kit"
    pub version: String,    // "2.50.1"
    pub peers: Vec<SnapshotKey>,
}

impl SnapshotKey {
    pub fn parse(s: &str) -> Result<Self, KeyParseError>;
    pub fn base(&self) -> String;         // "name@version", no peers
    pub fn canonical(&self) -> String;    // lockfile form, peer order preserved
    pub fn target_name(&self) -> String;  // §5
}
```

`canonical()` renders the key back to its lockfile form. It **does not sort
peers** — see §5 rule 4: pnpm's naming hashes the lockfile's own peer order,
so re-sorting would make every hashed target name diverge from the real
virtual store. The lockfile's own ordering is already deterministic, so
nothing is lost.

---

## 5. Target-name mangling — port pnpm's algorithm

**Pudu does not invent a naming scheme. It ports pnpm's `depPathToFilename`
exactly**, from `@pnpm/dependency-path` v1001.1.10.

Design §5 wants generated names greppable against a real
`node_modules/.pnpm/` directory. Matching pnpm byte-for-byte serves that goal
literally rather than approximately: a Buck target name can be pasted
straight into `ls node_modules/.pnpm/`. It also satisfies the approved
principle — readable stem, hash only to disambiguate — better than the scheme
it replaces, because short peer sets stay fully readable rather than hashed.

```
fn target_name(dep_path: &str, max_len: usize /* 120 */) -> String {
    // 1. escape the Windows-illegal characters plus '#'
    let mut s = replace_any(dep_path, r#"\/:*?"<>|#"#, '+');

    // 2. flatten peer parens, if any
    if s.contains('(') {
        s = s.strip_suffix(')').unwrap_or(&s).to_string();
        s = s.replace(")(", "_").replace('(', "_").replace(')', "_");
    }

    // 3. hash when too long, OR when any uppercase is present
    if s.len() > max_len || (s != s.to_lowercase() && !s.starts_with("file+")) {
        let h = &sha256_hex(&s)[..32];
        return format!("{}_{}", &s[..max_len - 33], h);
    }
    s
}
```

```
svelte@5.49.1                                    -> svelte@5.49.1
@babel/core@7.28.6                               -> @babel+core@7.28.6
vite@7.3.1(@types/node@22.19.7)(terser@5.46.0)   -> vite@7.3.1_@types+node@22.19.7_terser@5.46.0
@sveltejs/kit@2.50.1(…422 chars…)                -> @sveltejs+kit@2.50.1_@sveltejs+vite-plugin-svelte@6.2.4_svelte@…_5908f9fc12a8139630f640243ef0c4e3
MyPkg@1.0.0                                      -> MyPkg@1.0.0_fa093f5301680d2c9a22ce22952dfea8
```

### Four rules that are easy to get wrong

1. **The escape set is `\ / : * ? " < > | #` → `+`**, not `/` alone.
2. **Peers flatten readably when short.** Trailing `)` is dropped first, then
   `)(`, `(`, and `)` each become `_`. Hashing is the fallback, not the rule.
3. **Uppercase forces the hash path at any length** — the
   `s != s.to_lowercase()` clause, which guards case-insensitive filesystems.
   No corpus lockfile exercises this (0 of 3224 snapshot keys contain
   uppercase), so it needs a constructed fixture and is a likely site for a
   silent divergence.
4. **Peers must NOT be sorted.** pnpm hashes the lockfile's own peer order.
   Sorting would produce names that diverge from the real store and defeat
   the point. Determinism comes from the lockfile being deterministic.
   *This reverses `SnapshotKey::canonical()`'s defensive re-sort as originally
   drafted, and supersedes design §5's "pudu re-sorts defensively" line.*

`max_len` is pnpm's `virtual-store-dir-max-length`, default **120**. Pudu
hardcodes 120 for v0.1.0 and does not expose it; a project that has changed
the pnpm setting gets names that do not match its store, which is a
cosmetic-only divergence and a documented limitation, not a correctness bug.

### Verification

This is not adopted on faith. A reimplementation was diffed against the real
`node_modules/.pnpm/` directories of two projects
([survey](../research/2026-08-31-pnpm-lock-v9-field-survey.md)):
**1363 directory names reproduced exactly, including 32 of 32 hashed long
names**, with a perfect bijection on the cleanly installed project. Every
miss was an optional dependency pruned at install time for this platform.

S1 reproduces that check as a test: `tests/fixtures/lock/real/` ships the
lockfile beside a **captured listing of the `.pnpm` directory names** it
produced, and a test asserts pudu regenerates every non-pruned one. That is a
differential test against the real implementation with no runtime dependency
on pnpm — and it is the single highest-value test in the stage, because it
catches any divergence in all four rules at once.

### Collisions

pnpm's scheme can in principle collide, since the hash covers the full
filename and the prefix is truncated. Two distinct snapshot keys mangling to
one target name is a hard error naming both keys and the shared name — a
correctness guard, tested with injected collisions rather than left to
chance.

---

## 6. Instance graph

`src/lock/graph.rs`.

```rust
pub struct Graph {
    pub nodes: BTreeMap<String, Node>,   // canonical snapshot key -> node
    pub roots: Vec<Root>,
    pub cycles: Vec<Vec<String>>,
}

pub struct Node {
    pub key: SnapshotKey,
    pub target_name: String,
    pub meta: PackageMeta,          // resolved via key.base()
    pub edges: Vec<Edge>,           // sorted by link_name
    pub optional: bool,
}

pub struct Edge {
    pub link_name: String,          // directory name under node_modules/
    pub target: String,             // canonical snapshot key of the dependency
    pub kind: EdgeKind,             // Prod | Optional
}

pub struct Root {
    pub importer: String,           // "." or "packages/server"
    pub link_name: String,
    pub target: String,
    pub kind: RootKind,             // Prod | Dev | Optional
}
```

### 6.1 Metadata lookup

A node's metadata comes from `packages[key.base()]` — the peer suffix is
stripped, because `packages:` is never peer-suffixed (survey §1: zero `(` in
`packages` keys against 124 in `snapshots` keys). A snapshot with no matching
`packages` entry is an error naming both the snapshot key and the base it
looked for.

### 6.2 Edge resolution — the alias rule

An edge is a map entry `link_name: value`. Resolving it to a snapshot key:

1. Strip any peer suffix from `value` (depth-0 scan, as §4).
2. If what remains **still contains `@` beyond position 0**, the value is
   already a complete `name@version…` key. Use `value` verbatim.
3. Otherwise the value is a bare version; the key is `link_name + "@" + value`.

```yaml
string-width:     5.1.2                  # -> string-width@5.1.2
string-width-cjs: string-width@4.2.3     # -> string-width@4.2.3   (alias)
eslint:           9.39.2(jiti@2.6.1)     # -> eslint@9.39.2(jiti@2.6.1)
```

**`link_name` is kept on the edge even when it differs from the package
name.** The virtual store must symlink the package's content in under the
alias, so this is graph data S4 consumes, not a detail it can reconstruct.
Importer `version:` fields use the identical encoding and the identical rule.

An edge resolving to a key with no `snapshots:` entry is an error naming the
source node, the link name, and the resolved key.

### 6.3 Roots

Every importer contributes one root per entry in `dependencies`,
`devDependencies`, and `optionalDependencies`, tagged with the originating
kind. S1 keeps all three; which are reachable is a later stage's filter.

A `link:`/`file:`/`workspace:` specifier resolves to another importer rather
than a package. S1 records the root with its raw specifier and does not
attempt to resolve it into a node; workspace linking is S5's problem. It is
noted here so the fixture exists and the shape is not a surprise later.

---

## 7. Cycles are detected, never rejected

The roadmap's S1 exit criterion "cycles (rejected clearly)" is **withdrawn**.
The survey found cycles in every real lockfile examined, in `@babel/core`,
`eslint`, and `browserslist` among others; rejecting them would reject
essentially every real project.

S1 detects cycles with an iterative DFS — recursion would risk stack overflow
on an 800-node graph with deep chains — and records each as a
`Vec<String>` of canonical keys in `Graph.cycles`. They appear in
`print-graph` output. They are **not** a warning: they are normal, and
warning on every run would be noise.

The reason this is safe belongs in the spec rather than only in the survey:
the virtual store is a single `filegroup` mapping paths to tarball artifacts
(design §8), and a package's extracted content never depends on its
dependencies' targets. The cycle lives in symlink data inside one target, not
in the Buck target graph.

**Constraint this places on S4:** the store must not be decomposed into one
Buck target per package depending on its dependencies' targets — that shape
reintroduces the cycle as a Buck target cycle and fails to load. A split for
scale must follow tarball-extraction lines, which are acyclic.

---

## 8. Version gate and unsupported features

### 8.1 `lockfileVersion`

Accepted: **`9.0` only.** The value may be a YAML string (`'9.0'`) or a bare
number (`9.0`); both parse to the same thing, since the corpus contains
quoted forms and the unquoted form is legal YAML.

Anything else is an error that names the version found and the supported
range, and — for v5/v6/v7 — says the lockfile can be upgraded by running
`pnpm install` with pnpm 9 or newer. A missing `lockfileVersion` is the same
error, reported as "absent".

### 8.2 Feature keys

| key | behaviour | why |
|---|---|---|
| `patchedDependencies` | **error** | A patch changes tarball content. Ignoring it emits a build that silently does not match the source — the exact failure pudu exists to prevent. |
| `settings.excludeLinksFromLockfile: true` | **error** | `link:` deps are omitted from the lockfile, so the graph would be silently incomplete — and pudu cannot detect whether any existed, because they are precisely what was excluded. |
| `catalogs` / `catalog` | tolerate | Already resolved to concrete versions by the time they reach the lockfile. |
| `overrides` | tolerate | Likewise already applied to the resolution. |
| any other unknown top-level key | **warning** naming the key | Signals a pnpm feature pudu has not been taught; the run continues. |

Both errors name the key, explain the consequence, and give the remedy
(`excludeLinksFromLockfile`: set it false in `.npmrc` and re-run `pnpm
install`).

---

## 9. Errors and warnings

S1 adds a `sha2` dependency for §5's naming hash — the first new runtime
dependency since the S0 trim, and required by the ported algorithm rather
than chosen.

S1 adds `LockError` and `LockWarning` to the existing machinery in
`src/error.rs`, registered through the `typed_errors!` macro so exit code and
diagnostic stay in one place. Malformed lockfiles are **input errors: exit
code 3**, alongside a malformed `pudu.toml` — the reason `ExitCode::InputInvalid`
carries that name.

Every variant names the specific thing that failed. Following design §5's
precise-errors invariant, a lockfile error carries the snapshot key (and,
where meaningful, the link name) rather than only a line number.

```rust
pub enum LockError {
    UnsupportedVersion { found: Option<String> },
    Yaml { source: serde_norway::Error },
    KeyParse { key: String, offset: usize, reason: KeyParseError },
    MissingPackageMeta { snapshot: String, base: String },
    UnresolvedEdge { from: String, link_name: String, resolved: String },
    UnknownResolution { key: String, found_keys: Vec<String> },
    TargetNameCollision { a: String, b: String, target: String },
    PatchedDependencies,
    ExcludedLinks,
}

pub enum LockWarning {
    UnknownTopLevelKey { key: String },
    DeprecatedPackage { key: String, message: String },
}
```

`DeprecatedPackage` fires from the `deprecated` field (14 occurrences in the
corpus) — useful signal that costs nothing to surface.

---

## 10. `pudu debug print-graph`

A hidden command (`#[command(hide = true)]`) under a `debug` subcommand
group. It is a development and testing surface, not a supported interface,
and carries no stability promise.

```
pudu debug print-graph [--config <path>] [-C <dir>]
```

Reads `pudu.toml` for `lockfile_path`, parses, builds the graph, prints JSON
to stdout, exits 0. Errors go through `error::render` like every other
command.

Output is deterministic — `BTreeMap` ordering throughout, no `HashMap`
anywhere in the serialization path — so it can be an `insta` snapshot:

```json
{
  "lockfile_version": "9.0",
  "settings": { "auto_install_peers": true, "exclude_links_from_lockfile": false },
  "roots": [
    { "importer": ".", "link_name": "svelte", "target": "svelte@5.49.1", "kind": "dev" }
  ],
  "nodes": {
    "@babel/core@7.28.6": {
      "name": "@babel/core", "version": "7.28.6", "peers": [],
      "target_name": "@babel+core@7.28.6",
      "optional": false,
      "meta": { "integrity": "sha512-…", "has_bin": false, "os": null, "cpu": null, "libc": null },
      "edges": [
        { "link_name": "@babel/helper-module-transforms",
          "target": "@babel/helper-module-transforms@7.28.6(@babel/core@7.28.6)",
          "kind": "prod" }
      ]
    }
  },
  "cycles": [
    ["@babel/core@7.28.6", "@babel/helper-module-transforms@7.28.6(@babel/core@7.28.6)", "@babel/core@7.28.6"]
  ]
}
```

---

## 11. Module layout (S1 slice)

```
src/lock/
├── mod.rs            # pub use; parse_lockfile() entry point
├── types.rs          # §3 — serde types, no logic
├── snapshot_key.rs   # §4, §5 — grammar, canonical form, mangling
└── graph.rs          # §6, §7 — construction, alias rule, cycle detection
src/cli/debug.rs      # §10 — print-graph
```

`types.rs` holds no logic and `snapshot_key.rs` no I/O, so both are testable
in isolation. Design §10's `parser.rs` is folded into `mod.rs`: after
`serde_norway` does the deserialization, what remains is a version gate and a
feature check, which is not a module's worth of code.

---

## 12. Testing

### Unit tests

**`snapshot_key.rs` — the highest-value tests in the stage.**

- bare `svelte@5.49.1`; scoped `@babel/core@7.28.6`
- single peer `react-dom@18.3.1(react@18.3.1)`
- **nested peers** `eslint-plugin-svelte@3.14.0(eslint@9.39.2(jiti@2.6.1))(svelte@5.49.1)`
- the **verbatim 422-character key** from the corpus, round-tripped
- peer order is **preserved, not sorted**: `(a@1)(b@2)` and `(b@2)(a@1)` are
  distinct keys producing distinct target names (§5 rule 4)
- the escape set: each of `\ / : * ? " < > | #` maps to `+`
- **uppercase forces the hash path** even on a short name — the branch no
  corpus lockfile exercises
- prerelease and build-metadata versions: `1.0.0-rc.1`, `1.0.0+build.5`
- rejects: unbalanced `(`, trailing `)`, empty name, empty version, no `@`,
  bare `@scope/name` with no version
- short peer sets flatten readably (`vite@7.3.1_terser@5.46.0`), not hashed
- an injected collision produces `TargetNameCollision`, not silent overwrite
- **the differential test**: every non-pruned name in the captured `.pnpm`
  listing beside the real fixture is regenerated exactly

**`graph.rs`**

- alias edge `string-width-cjs: string-width@4.2.3` resolves to
  `string-width@4.2.3` **and** keeps `link_name` as `string-width-cjs` —
  both halves asserted, since the resolution alone would pass with the link
  name dropped
- peer-suffixed edge value resolves to the suffixed key
- two peer instances of one `name@version` are two nodes sharing one
  `packages` entry
- unresolved edge and missing metadata each produce their named error
- a two-node cycle is recorded and does **not** error
- a self-edge is recorded as a cycle of length one
- cycle detection is iterative: a 10 000-node chain does not overflow the stack

**`types.rs` / version gate**

- `'9.0'` quoted and `9.0` unquoted both accepted
- `'6.0'` errors naming the version and the upgrade path; absent errors as
  "absent"
- each `Resolution` variant deserializes; an unrecognised one errors naming
  the keys present
- `cpu: [wasm32]` and an unknown `os` token parse without error
- `bundledDependencies` parses and produces no edges
- `patchedDependencies` and `excludeLinksFromLockfile: true` each error;
  `catalogs` and `overrides` are tolerated silently; an unknown top-level key
  warns and continues

### Fixtures

`tests/fixtures/lock/` — hand-written and minimal, one concern each:
dev/optional deps · peer instances of one package · scoped names · aliases ·
a two-importer workspace · `link:` and `workspace:` specifiers · a cycle · a
`tarball` and a `git` resolution · musl `libc` · v6 (rejection) · empty
lockfile with no `packages`/`snapshots`.

Plus **one real-world lockfile** vendored into
`tests/fixtures/lock/real/`: a 700+ node lockfile with nested peer instances,
aliases, and cycles — the roadmap's demo criterion. Its provenance and
licence are recorded beside it.

### Integration tests

`tests/debug_print_graph.rs` — `insta` snapshot of `print-graph` on the real
lockfile, plus assertions that a second run is byte-identical (the
determinism invariant) and that error paths exit 3.

---

## 13. Exit criteria

1. `pudu debug print-graph` reads `pudu.toml` + lockfile and prints one JSON
   entry per snapshot key, deterministically.
2. The snapshot-key parser handles nested peers, scoped names, and the
   verbatim 422-char corpus key; peer order does not affect the target name.
3. Target names are byte-identical to pnpm's own virtual-store directory
   names, proven by the differential test against the captured `.pnpm`
   listing; a collision is a named error.
4. Aliased edges resolve to the aliased package while retaining the link
   name, proven by a test asserting both.
5. Cycles are detected, reported in the output, and do not error — proven
   against the real-world fixture.
6. A v6 lockfile, `patchedDependencies`, and `excludeLinksFromLockfile: true`
   each error with a named remedy; unknown top-level keys warn.
7. Unknown `os`/`cpu`/`libc` tokens parse without error.
8. Two runs on the real lockfile produce byte-identical output.
9. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` clean;
   MSRV 1.88 `cargo check --all-targets` clean.
10. Design §5's mangling paragraph and roadmap S1's cycle criterion are
    amended to match this spec.

---

## 14. Open questions and follow-ups

- **`link:`/`workspace:` roots are recorded, not resolved.** S1 proves the
  shape parses; S5 makes workspace importers real targets. Deferred
  deliberately, since resolving them requires the workspace layout S5 owns.
- **`catalogs` and `overrides` are tolerated untested** — no corpus lockfile
  has either. The reasoning that they are pre-resolved is sound but
  unverified; a fixture should be constructed when one is available.
- **Non-integrity resolutions are untested against reality.** The `tarball`,
  `git`, and `directory` variants are modelled from the pnpm source and
  covered only by constructed fixtures. First contact with a real private
  registry may correct this.
- **`virtual-store-dir-max-length` is hardcoded to 120.** A project that has
  changed pnpm's setting gets target names that do not match its own store.
  The names stay internally consistent and correct, so this is cosmetic —
  greppability is lost, nothing breaks. Exposing it is a config addition if
  anyone asks.
- **The uppercase branch of pnpm's naming is untested by real data** — 0 of
  3224 corpus snapshot keys contain an uppercase character. It is covered by
  a constructed fixture, but a real uppercase package is the likeliest source
  of a silent divergence from the store.
- **`chaste-pnpm` was evaluated and rejected** as a dependency (it drops the
  platform fields S2 needs; see the survey). If it ever grows them, revisit —
  its peer grammar and alias handling already agree with this spec.
