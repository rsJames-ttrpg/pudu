# Pudu S2 — Platform model & optional-dependency pruning

Stage spec. Parent: [Pudu Design](2026-08-30-pudu-design.md) §5 (platform
pruning) and §7 (platform model and Buck configuration). Predecessor:
[S1 — lockfile parser and instance graph](2026-08-31-pudu-s1-lockfile-design.md).

Every behavioural rule below was verified against pnpm's own
`@pnpm/package-is-installable` before this spec was written. The evidence,
including four captured per-platform oracles and the two rules that
contradict intuition, is in
[the platform matching survey](../research/2026-08-31-pnpm-platform-matching-survey.md).
Where this spec states a rule, the survey states why we believe it.

---

## 1. Scope

S2 turns the platform-independent instance graph S1 produces into a
per-platform view, and derives the Buck constraint labels each configured
platform selects on.

**In scope**

- Matching npm's `os` / `cpu` / `libc` fields against a configured platform,
  including negation, mixed lists, and unknown tokens.
- Pruning nodes and edges per platform, with diagnostics.
- Mapping a `[platforms.<name>]` block to prelude constraint labels,
  including the conditional-abi rule.
- A hidden `pudu debug platforms` that prints the result as JSON.

**Out of scope**

- Emitting `config/BUCK`, `pudu.bzl`, or any Buck file — S4 owns emission.
  S2 produces the labels; nothing writes them to disk.
- Modelling `engines` (§7.3).
- Fixups overriding platform metadata — S7.

---

## 2. Module layout

```
src/platform/
    mod.rs           re-exports; the Os/Cpu/Libc types moved from platform.rs
    matching.rs      the checkList port: does a field list admit a value?
    prune.rs         per-platform node/edge pruning over the S1 graph
    constraints.rs   platform -> Buck constraint labels, conditional abi
```

`src/platform.rs` becomes `src/platform/mod.rs` unchanged in content: the
`Os`, `Cpu`, `Libc` enums and their `as_npm()` / `short()` accessors already
exist from S0 and keep their current semantics and tests. This is a file
move, not a rewrite.

---

## 3. The matcher

```rust
/// Does a package's npm platform field admit `current`?
///
/// `field` is the raw list from the lockfile, negation intact; `None` is an
/// absent field. `current` is the platform's npm spelling for that axis.
pub fn admits(field: Option<&[String]>, current: &str) -> bool
```

A faithful port of pnpm's `checkList`, evaluated for a single `current`
value because pudu considers one platform at a time.

1. An absent field (`None`) admits everything.
2. A list of exactly one entry equal to `"any"` admits everything. `"any"`
   in any other position is an ordinary token with no special meaning.
3. Otherwise, walk the list accumulating two facts: whether any positive
   entry equals `current` (`matched`), and how many entries are negations
   (`negations`). A negation whose body equals `current` returns `false`
   immediately.
4. Return `matched || negations == list.len()`.

Rule 4 is the whole subtlety and carries two consequences that an
implementation written from intuition gets wrong:

- **A mixed list requires a positive hit.** `["!win32", "darwin"]` on linux
  yields `matched = false`, `negations = 1`, `len = 2` → **excluded**.
  Negations only ever subtract; they never widen.
- **An empty list admits everything**, since `0 == 0`.

Unknown tokens (`wasm32`, `openharmony`, `aix`) need no special handling:
they are ordinary positives that match nothing. They must never error —
the fixture alone contains seven `os` values (`aix`, `android`, `freebsd`,
`netbsd`, `openbsd`, `openharmony`, `sunos`) and seven `cpu` values (`arm`,
`ia32`, `loong64`, `mips64el`, `ppc64`, `riscv64`, `s390x`) outside pudu's
enums.

pnpm additionally discards non-string list entries before matching. YAML
gives pudu `Vec<String>`, so a non-string entry cannot reach `admits`;
`serde` rejects it at parse time. This is a deliberate, documented
divergence in an unreachable direction, noted so a reader comparing the two
implementations is not left wondering.

### Axis selection

```rust
pub fn admits_platform(meta: &PackageMeta, platform: &Platform) -> bool
```

- `os` is matched against `platform.os.as_npm()`.
- `cpu` is matched against `platform.cpu.as_npm()`.
- `libc` is matched against `platform.libc`'s npm spelling **only when the
  platform declares one**. A platform with `libc: None` skips the axis
  entirely, whatever the package declares.

All three must admit for the package to survive.

The libc rule reproduces pnpm's real behaviour on a machine with no
detectable libc — a Mac — where the axis is never checked. It also means a
darwin oracle captured from a Linux host disagrees with pudu on
libc-declaring packages, in pudu's favour (survey §1).

---

## 4. Pruning

```rust
pub struct PlatformView {
    /// Snapshot keys that survive on this platform, sorted.
    pub nodes: BTreeSet<String>,
    /// Edges dropped because their target did not survive.
    pub dropped_edges: Vec<DroppedEdge>,
}

pub struct Matrix {
    /// One view per configured platform, keyed by platform name.
    pub views: BTreeMap<String, PlatformView>,
    /// Transpose: for each surviving snapshot key, the platforms it is on.
    /// S4 turns this into `select()` keys.
    pub platforms_by_node: BTreeMap<String, BTreeSet<String>>,
}

pub fn prune(graph: &Graph, platforms: &BTreeMap<String, Platform>)
    -> (Matrix, Vec<PlatformWarning>);
```

Every collection is a `BTreeMap`/`BTreeSet` so output ordering is total and
determinism needs no separate sort pass — the same discipline S1 used.

### 4.1 The rule

A node survives on a platform iff `admits_platform(&node.meta, platform)`.
This is a **per-package** decision that consults only that package's own
fields — not its parents, not its dependencies.

An edge is dropped on a platform when its target did not survive there.

There is **no reachability sweep**: a node that survives but has become
unreachable from every root stays in the view. Per-package matching alone
reproduced pnpm's install set exactly on all four captured oracles (survey
§5), so a sweep would be unverifiable extra machinery.

The limit of that evidence is recorded honestly: all 90 platform-gated keys
in the fixture are leaves, so the fixture cannot distinguish the two
designs. If a gated package with its own subtree ever appears, the failure
mode is an orphan left in the store — fat, not incorrect. Tracked as tech
debt rather than pre-solved.

### 4.2 Diagnostics

Two findings, both warnings. S2 introduces **no new hard errors**: every
condition below is a property of somebody's dependency tree, not of pudu's
input being malformed, and none of them makes the remaining output wrong.

| condition | warning |
|---|---|
| a **non-optional** edge's target is pruned | `PlatformWarning::RequiredDependencyExcluded` naming the depending package, the target `name@version`, and the platform |
| a package survives on **no** configured platform | `PlatformWarning::ExcludedEverywhere` naming `name@version` and listing the configured platform names |

An **optional** edge whose target is pruned is the normal case — every
`@esbuild/*` on every platform but one — and is silent.

`ExcludedEverywhere` is the roadmap's "excluded on every configured
platform (warn, don't fail)" criterion. It fires constantly on real input:
the fixture has 90 gated packages and the four platforms cover a fraction of
them, so `@esbuild/openharmony-arm64` and dozens of siblings are excluded
everywhere. The warning is therefore **aggregated into one diagnostic
listing the affected packages**, not emitted 60 times.

### 4.3 The divergence from pnpm

pnpm **warns and installs** a non-optional dependency whose platform
excludes the host; only optional dependencies are skipped (survey §4).
Pudu warns and **drops** the edge for that platform.

This is deliberate. Pudu generates build rules for a platform it is being
asked to configure; emitting a dependency on a package that cannot run there
serves nobody, and the warning preserves the signal that pnpm would have
printed. It is recorded here because it is a divergence from the reference
implementation, and any future comparison against pnpm's behaviour must
expect it.

---

## 5. Constraint mapping

```rust
/// The Buck constraint labels a platform selects on, sorted.
pub fn constraint_labels(
    platform: &Platform,
    all: &BTreeMap<String, Platform>,
) -> Vec<String>;
```

### 5.1 Base mapping

| axis | value | label |
|---|---|---|
| `os` | linux | `prelude//os/constraints:linux` |
| `os` | darwin | `prelude//os/constraints:macos` |
| `os` | win32 | `prelude//os/constraints:windows` |
| `cpu` | x64 | `prelude//cpu/constraints:x86_64` |
| `cpu` | arm64 | `prelude//cpu/constraints:arm64` |
| `libc` | glibc | `prelude//abi/constraints:gnu` |
| `libc` | musl | `prelude//abi/constraints:musl` |

Note that npm's vocabulary and the prelude's differ on two of the seven:
npm's `darwin` is the prelude's `macos`, and npm's `glibc` is the prelude's
`gnu`. Neither is a pass-through, and both are worth a test that would fail
if someone "simplified" the mapping into a lowercase of the npm name.

Labels are returned sorted, so the emitted `constraint_values` list is
deterministic without the caller sorting.

`win32` is mapped although Windows is a v1 non-goal: `Os::Win32` is
representable (a package may declare it), and config validation rejects a
win32 *platform* elsewhere. The mapping is total so no `unreachable!()` is
needed.

### 5.2 The abi constraint is conditional

The abi label is emitted **only when it discriminates**: when some other
configured platform shares this platform's `os` and `cpu` but declares a
different `libc`.

Parent design §7 gives the reason. `prelude//platforms:default` derives its
configuration from `host_info()` and sets only cpu and os; nothing sets an
abi constraint. A glibc-only configuration — the overwhelmingly common case
— therefore needs zero user wiring, and gains none.

When a user configures both `linux-x64-gnu` and `linux-x64-musl`, **both**
gain their abi label. The condition is a property of the platform *set*,
not of either platform alone, which is why `constraint_labels` takes `all`.
The platform need not exclude itself from that scan: it shares its own `os`,
`cpu` **and** `libc`, so it can never be its own discriminator.

A platform whose `libc` is `None` never gains an abi label regardless.

### 5.3 Escape hatch

When `[platforms.<name>]` sets `constraints = [...]`, that list replaces the
generated labels wholesale — including any abi label §5.2 would have added.
`os` / `cpu` / `libc` continue to drive npm field matching unchanged; only
emission is overridden. This is parent design §7's escape hatch, already
parsed by S0's `Platform::constraints`.

An empty `constraints = []` is honoured as written — an explicit request for
a platform with no constraint values — rather than treated as absent.

---

## 6. `pudu debug platforms`

Hidden, unstable, no compatibility promise, exactly like
`pudu debug print-graph`. Same shape: JSON on stdout, warnings rendered to
stderr, so stdout stays machine-parseable.

```json
{
  "platforms": {
    "linux-x64-gnu": {
      "os": "linux",
      "cpu": "x64",
      "libc": "glibc",
      "constraints": [
        "prelude//cpu/constraints:x86_64",
        "prelude//os/constraints:linux"
      ],
      "constraints_overridden": false,
      "node_count": 316,
      "pruned": ["@esbuild/aix-ppc64@0.25.12", "..."],
      "dropped_required_edges": []
    }
  }
}
```

`pruned` lists the snapshot keys excluded on that platform, sorted — this is
what makes the roadmap's demo checkable by eye and what the oracle test
consumes. `constraints_overridden` records whether §5.3's escape hatch
applied, so a user debugging a mis-selected target can see it without
re-reading their config.

Key spelling follows the rule S1 settled: invented fields are snake_case,
fields echoing the lockfile keep pnpm's spelling. Every field here is
invented, so all are snake_case.

Two runs on identical inputs must produce byte-identical stdout.

---

## 7. What S2 does not model

### 7.1 `engines`

Pudu does not prune on `engines`. Node version is not a platform axis
(parent design §5: platform is the only axis of variance), and a Buck build
gets its node from the toolchain, which is configuration rather than a
resolution input.

The consequence is measurable and must be handled by the oracle test rather
than hidden: pnpm silently skips an **optional** dependency failing
`engines`, so pudu's survivor set is a superset of pnpm's by exactly that
set (survey §3). At the fixture's capture node version this is one package,
`@napi-rs/lzma-linux-x64-gnu@1.5.1`.

### 7.2 `libc` when the lockfile omits it

pnpm's default abbreviated packument does not carry `libc`, so most v9
lockfiles have no `libc:` fields at all and musl builds are
indistinguishable from gnu ones (survey §2). Pudu matches `libc` when it is
present and does not synthesise it when absent.

Specifically, pudu does **not** infer libc from a package's name. That
`lightningcss-linux-x64-musl` is a musl build is obvious to a human and
unavailable to a correct implementation; guessing it would make pudu prune
packages pnpm installs, breaking the oracle agreement that is S2's whole
correctness argument.

### 7.3 Transitive reachability

§4.1. Not needed against four oracles; the leaf caveat is recorded there.

---

## 8. Errors and warnings

No new `Error` variants, so nothing is added to `error.rs`'s `typed_errors!`
registry — that macro maps errors to exit codes, and warnings have none.
`PlatformWarning` joins `LockWarning` / `DeriveWarning` / `ConfigWarning` as
a typed warning enum deriving `Diagnostic`, printed through the existing
`error::render`:

```rust
pub enum PlatformWarning {
    RequiredDependencyExcluded { dependent: String, target: String, platform: String },
    ExcludedEverywhere { packages: Vec<String>, platforms: Vec<String> },
}
```

Each carries `severity(Warning)`, a `code(pudu::platform::…)`, and a `help`
naming the next action — for `RequiredDependencyExcluded`, that the package
may need a fixup; for `ExcludedEverywhere`, that the listed packages will
appear in no generated target, which is expected for the platform-specific
binaries of a package like `esbuild` and suspicious for anything else.

There is deliberately no "this platform pruned everything" warning: a
package declaring no `os`/`cpu`/`libc` survives every platform, so a view
can only be empty when the graph itself is, which S1 already reports.

### 8.1 Tech debt folded in

Two S0 rows targeted at S2 are closed here, since S2 is the stage that makes
`supportedArchitectures` parsing load-bearing:

- **TD-S0-08** — a non-sequence axis (`os: linux` rather than `os: [linux]`)
  errors misleadingly, and a non-mapping `supportedArchitectures` block is
  ignored silently. Both get a precise diagnostic.
- **TD-S0-09** — the unknown-`cpu` warning arm has no test, and `axis()`
  silently drops non-string entries. Both get tests; the dropped-entry case
  gets a warning.

Two S1 rows targeted at S2 are **not** closed here, with reasons recorded in
the ledger rather than silently skipped:

- **TD-S1-01** (triple YAML parse) — a performance concern; S2 adds no
  parse. Re-target S3.
- **TD-S1-06** (non-exhaustive cycle detection) — `cycles` remains a
  diagnostic in S2; nothing added here depends on its completeness.

**TD-S1-07** is decided by this spec: `engines` stays parsed and unused
(§7.1), and `transitive_peer_dependencies`, `peer_dependencies`, and
`peer_dependencies_meta` are not surfaced by S2. The row closes as a
decision, not as code.

---

## 9. Testing

### 9.1 Differential fuzz against pnpm

The primary correctness argument for §3, and the mechanism that gives libc
and negation the coverage no fixture can (survey §2).

A test generates several thousand `(field lists × platform)` pairs — drawn
from a token pool of real npm values, unknown tokens, `any`, negations,
mixed lists, empty lists, and multi-entry lists across all three axes — and
compares pudu's `admits` against pnpm's real `checkPlatform` running in
Node.

Following S1's precedent for its naming fuzz: the test is `#[ignore]`d by
default so the suite needs neither Node nor a network, and is run
explicitly during development and in a CI job that installs the reference
module. The committed record of a run goes in the fixture README, as S1's
did.

### 9.2 The per-platform oracles

Four captured listings are committed under
`tests/fixtures/lock/real/oracle/`, one per platform, alongside:

- `capture.sh`, the script that regenerates them, so the provenance is
  executable rather than prose;
- `engine-excluded.txt`, the optional dependencies pnpm skipped for
  `engines` rather than platform, which the test **subtracts** before
  comparing (§7.1);
- the node and pnpm versions used, recorded in the README, because
  `engine-excluded.txt` is only valid for that node version.

The test asserts an exact set equality per platform, over target names
produced by S1's `target_name`. Validated in advance: this comparison passes
on all four platforms (survey §5).

`linux-x64-gnu.txt` must be byte-identical to the existing
`virtual-store-listing.txt`; a test asserts it, which keeps S1's fixture and
S2's oracles from drifting apart silently.

### 9.3 Unit tests

Every row of survey §1's verified-behaviour table becomes a unit test, named
for the rule rather than the input — the mixed-list and singleton-`any`
rules especially, since those are the ones a future "simplification" would
break.

Beyond those: the npm→prelude renames (`darwin`→`macos`, `glibc`→`gnu`); the
conditional-abi rule both ways, gnu-only emitting no abi label and gnu+musl
emitting it on **both** platforms; the escape hatch replacing labels
including a suppressed abi label; `constraints = []` honoured as written;
the aggregated `ExcludedEverywhere` warning firing once rather than per
package; and `pudu debug platforms` producing byte-identical output twice.

### 9.4 The esbuild demo

The roadmap's demo criterion — each configured platform resolves esbuild's
~20 optional deps to exactly one `@esbuild/*` — becomes an explicit test
rather than a manual check. The fixture carries two esbuild versions, so the
assertion is one surviving `@esbuild/*` **per version** per platform.

---

## 10. Exit criteria

1. `pudu debug platforms` prints, per platform, the surviving node count and
   the pruned keys, deterministically.
2. `admits` agrees with pnpm's `checkPlatform` across a multi-thousand-case
   differential fuzz, including negation, mixed lists, `any`, empty lists,
   and unknown tokens.
3. Pruning reproduces all four captured oracles exactly, modulo the
   documented engine-excluded set.
4. The conditional-abi rule is tested both ways; `constraints = [...]`
   overrides generated labels.
5. On the fixture, each configured platform resolves esbuild's optional deps
   to exactly one `@esbuild/*` per esbuild version.
6. A package excluded on every configured platform warns once, aggregated,
   and does not fail the run.
7. TD-S0-08 and TD-S0-09 are closed, in the same commit as the code.
