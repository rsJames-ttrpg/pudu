# Pudu S5 — Store layout, `node_binary`, and `buck2 run`

**Status:** draft v1 (2026-09-10)
**Companion to:** [`2026-08-30-pudu-design.md`](./2026-08-30-pudu-design.md), [`2026-08-30-pudu-roadmap.md`](./2026-08-30-pudu-roadmap.md)
**Predecessor:** [S4 — first BUCK emitter](./2026-09-01-pudu-s4-buck-emitter-design.md)

S4 emitted the package layer: one `http_archive` per package@version, with the
archive root and each bin exposed as sub-targets. Nothing consumes those
targets yet. S5 makes them runnable.

---

## 1. Spike findings

Roadmap S5 was written when design §8 claimed the store layout needed
"no custom rule at all" — a `filegroup` with a `path → artifact` dict. S4 §1.4
refuted that: `filegroup(copy = False)` symlinks every store leaf out of the
tree, Node realpaths through those symlinks, and sibling lookup escapes.

That left the replacement unverified. Before specifying a rule, the following
was measured against the CI-pinned buck2 (`2026-08-22`, the same tag CI
installs, not the older binary on the development machine) and Node v24.6.0.
The spike project is throwaway; what it established is below.

### 1.1 buck2 preserves intra-tree relative symlinks in an action output

A rule that declares `ctx.actions.declare_output("node_modules", dir = True)`
and builds the tree itself keeps every symlink verbatim:

```
drwxr-xr-x .pnpm
lrwxrwxrwx debug -> .pnpm/debug@4.3.4/node_modules/debug
```

```
drwxr-xr-x debug
lrwxrwxrwx ms -> ../../ms@2.1.2/node_modules/ms
```

Real directories at `.pnpm/<key>/node_modules/<pkg>`, relative symlinks within
the tree. This is the combination §1.4 established that no `filegroup` mode can
produce, and it is exactly pnpm's shape.

### 1.2 Node resolves through it

The store above is §1.4's failing case — `debug@4.3.4` whose only dependency is
`ms@2.1.2` — rebuilt with the new rule. `require("debug")` succeeds, and
`debug`'s own `require("ms")` resolves through the sibling symlink:

```
2026-09-08T15:04:02.207Z spike resolved through the store
OK debug at .../__tree__/node_modules/.pnpm/debug@4.3.4/node_modules/debug/src/index.js
```

Against §1.4's:

```
Error: Cannot find module 'ms'
Require stack:
- .../__app_store__/app_store/node_modules/debug/src/common.js
```

### 1.3 `node_binary` composes as a second action, and `buck2 run` works

First-party sources live in the repo; the tree lives in buck-out. Node finds
`node_modules` by walking up from the *main script's* directory, so the two
must meet somewhere.

What works: a second action producing an app directory of copied first-party
sources plus a single `node_modules` symlink pointing out of the output into
the tree artifact. Node realpaths that one link *into* the genuine tree, where
`.pnpm` isolation holds, and relative `require`s between first-party files
still work because those files are real:

```
2026-09-08T15:06:29.671Z app hello from buck2 run
OK: .../__tree__/node_modules/.pnpm/debug@4.3.4/node_modules/debug/src/index.js
```

**That escaping symlink is the fragile part of this design.** It is a
dependency edge buck2 does not track structurally, and it failed twice during
the spike:

- the tree was absent from the `RunInfo` hidden set, so buck2 never
  materialized it and the link dangled;
- `relative_to`'s parent count was off by one, producing
  `../__tree__/node_modules` where the tree was at `../../__tree__/node_modules`.

Both produce a dangling symlink. **Both build clean.** The failure surfaces
only when Node runs, as a `MODULE_NOT_FOUND` naming the *first-party* file that
did the `require`, which reads like an application bug rather than a wiring
bug. §7 turns this into a gating requirement.

### 1.4 Hardlinks work, and make the tree affordable

Materializing store entries with `fs.linkSync` rather than `fs.cpSync`
succeeds: link count 2, Node still resolves, `buck2 run` still passes. This is
pnpm's own strategy, and it makes a tree cost O(entries) rather than O(bytes) —
which is what makes a tree per importer, and later per platform, affordable.

The cost: tree files share inodes with the `http_archive` extraction output, so
anything that writes into `node_modules` corrupts a cached buck-out artifact.
The main writer — lifecycle install scripts — is already forbidden by S6's
gate. See §8 for the residual risk and §4.3 for where hardlinking is *not*
used.

### 1.5 The builder reads a manifest, so no package name reaches a shell

Inputs arrive as a JSON file written by `ctx.actions.write_json`. No package
name, archive root or bin path is ever interpolated into a command line. S4
§1.4's hazard — unquoted third-party text reaching `/bin/sh`, which is what
ruled out `strip_prefix` — cannot arise here by construction rather than by
correct escaping.

### 1.6 `--preserve-symlinks` remains unusable

Confirmed as reasoning, not measurement. Under `--preserve-symlinks` Node does
not realpath, so `node_modules/debug` stays at its logical path and `debug`'s
walk-up looks for a *top-level* `ms` that the pnpm layout deliberately does not
provide. The flag makes the `.pnpm` layout unworkable; the alternative is a
nested npm-style tree, which is a different design.

### 1.7 What the spike did not close

- **RE/CAS round-trip.** No remote-execution backend was available, so whether
  an action output containing symlinks *and* hardlinks survives upload and
  download is untested. The prelude's own `symlinked_dir` outputs suggest
  relative symlinks are a supported CAS concept; hardlinks are indistinguishable
  from files at that layer. Filed as debt (§10), not assumed.
- **Scale.** No measurement on a realistic store. The roadmap owed one for
  `filegroup`; the obligation transfers to the rule that replaced it, and lands
  in S5.5 with the multi-platform trees that make it matter.

---

## 2. Scope

S5 is the vertical slice on a **single platform**: enough to run.

**In:**

- The `node_modules_tree` rule (§4) and the store-path computation feeding it (§3).
- One tree per importer, emitted into `third-party/js/BUCK`.
- `node_binary` and `node_test` (§5).
- Extending `01-pure-js` with a runnable importer, and upgrading the CI job
  from `buck2 build` to `buck2 run` (§7).

**Out, to S5.5 ("esbuild day one"):**

- Per-platform emission, the `select()` over `config/` labels, and the
  semver-stable bare alias.
- The `02-platform-optional`, `03-peer-instances`, `04-musl`, `05-workspace`
  fixtures and macOS e2e CI.
- `link:` roots between importers — `Root::target` is `None` for those today,
  and TD-S1-04 lands with them.
- TD-S2-06's ambiguous `select()`, which cannot arise until there are
  per-platform arms to be ambiguous between.
- The scale measurement (§1.7).

**Already done, needs no work:** `system_node_toolchain`. S0 writes the rule
and its `NodeToolchainInfo { node: RunInfo }` provider into the user's
`toolchains/`. S5 consumes it; the roadmap listed it as S5 scope only because
nothing had consumed it before.

---

## 3. Store-path computation

This is pudu's own logic — the part Buck does not do. For one importer, over
the pruned instance graph, it produces two flat maps.

Let `key(k)` be a snapshot key, `tn(k) = target_name(k)` its mangled virtual-store
directory name (S1), and `dir(k)` the package's directory name under
`node_modules/` — which for a scoped package is two segments, `@scope/name`.

**Copies**, one per reachable node:

```
.pnpm/<tn(k)>/node_modules/<dir(k)>   ←   //third-party/js:<key(k)>[root]
```

**Top-level links**, one per root of this importer with a resolved target:

```
<root.link_name>   →   .pnpm/<tn(k)>/node_modules/<dir(k)>
```

**Sibling links**, one per edge of each reachable node:

```
.pnpm/<tn(k)>/node_modules/<edge.link_name>   →   <up>/<tn(t)>/node_modules/<dir(t)>
```

`edge.link_name` is the directory name, not the target package's name, so npm
aliases fall out for free: S1 already separates the two, and
`string-width-cjs → string-width@4.2.3` needs no special case here.

### 3.1 Relative depth is computed, never constant

`<up>` above is **not** `../../`. It is derived from the segment count of the
link's own path relative to `.pnpm/`. A sibling link at

```
.pnpm/<tn(k)>/node_modules/ms
```

needs `../../`, but one at

```
.pnpm/<tn(k)>/node_modules/@types/node
```

sits one directory deeper and needs `../../../`. The same applies to top-level
links: `node_modules/express` needs no prefix, `node_modules/@types/node` needs
one `../`.

Getting this wrong yields a dangling symlink, which — per §1.3 — **builds
clean**. §7 makes it a mutation-tested requirement.

### 3.2 Dev dependencies are included

`RootKind` distinguishes `Prod`/`Dev`/`Optional`, and the tree includes all
three. A tree without dev dependencies cannot run `tsc`, `vitest` or `eslint`,
which is most of what a JS build graph is for. Filtering by root kind is a
later flag, not an S5 default.

### 3.3 `.bin` entries

pnpm creates `node_modules/.bin/<name>` for an importer's direct dependencies,
and `.pnpm/<key>/node_modules/.bin/<name>` for a package's own dependencies'
bins. Both are symlinks to the owning package's bin path, and both follow §3.1's
depth rule.

**Stated assumption, not a measured fact:** the executable bit may need setting
explicitly. pnpm chmods bin files at install time, and npm tarballs frequently
ship them non-executable; the `[bin/*]` sub-targets S4 emits carry whatever the
tarball contained. The implementation plan carries a task to check this against
a real package — `semver@7.6.3` is already in the fixture — and to chmod in the
builder if it holds. It is written here as an open question so that a passing
build is not mistaken for a settled one.

---

## 4. The `node_modules_tree` rule

A real `rule()` in `pudu.bzl`, not a macro. S4's `pudu.bzl` contained only
macros over prelude rules; from S5 it also defines rule implementations.

### 4.1 Interface

```python
node_modules_tree = rule(
    impl = _node_modules_tree_impl,
    attrs = {
        "copies": attrs.dict(attrs.string(), attrs.dep(), default = {}),
        "links": attrs.dict(attrs.string(), attrs.string(), default = {}),
        "_node_toolchain": attrs.toolchain_dep(default = "toolchains//:node"),
    },
)
```

`NodeToolchainInfo` is defined in `toolchains.bzl`, which S0 writes into the
same directory as `pudu.bzl`, so the load is a same-package one. Note what that
makes it: **a contract between a generated file and a user-owned one.**
`toolchains.bzl` carries the header "Safe to edit" and explicitly invites
replacing the rule to get a hermetic Node. A user who does so must keep the
provider's name and its `node` field, or the generated `pudu.bzl` stops
loading. The plan resolves whether that is worth an error message; today it
would surface as a Starlark load failure.

`copies` maps a destination path to the `[root]` sub-target of a package;
`links` maps a destination path to a relative symlink target. Both are
generated flat, sorted, one entry per edge — pudu holds them in `BTreeMap`s so
the emitted Starlark and the resulting manifest are deterministic.

### 4.2 Implementation

The implementation declares a directory output, writes the manifest with
`write_json(..., with_inputs = True)` so artifact paths are substituted and
tracked as inputs, and runs the builder under the node toolchain:

```python
def _node_modules_tree_impl(ctx):
    out = ctx.actions.declare_output("node_modules", dir = True)
    builder = ctx.actions.write("build_tree.js", _BUILDER)
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
```

The spike ran `node` from `PATH`; the shipped rule takes it from the toolchain,
which is the reason S0 wrote one.

### 4.3 Materialization strategy

The builder hardlinks store entries, falling back to a copy when `linkSync`
fails — across a filesystem boundary, most likely. Directories are recreated
rather than linked; symlinks encountered inside an extracted archive are
resolved and their target linked.

**First-party sources are always copied, never hardlinked.** A hardlink from
buck-out to a file in the user's working tree shares an inode with a file the
user edits. Most editors replace the inode on save and would be harmless, but
an in-place write would mutate a build output directly. The store case is
different in kind: those inodes belong to immutable buck-out extractions.

### 4.4 Emission

One tree per importer, at `//third-party/js:<importer>_node_modules` — the
label design §8 already uses. Trees live in the generated `third-party/js/BUCK`
alongside the package targets; pudu generates nothing outside its own
directory, so a first-party `BUCK` referring to a tree is hand-written by the
user, as design §8 shows.

---

## 5. `node_binary` and `node_test`

### 5.1 `node_binary`

Two outputs: an app directory built by the same builder (first-party sources
copied in, one `node_modules` symlink out to the tree) and a launcher script
that execs the toolchain's node on the main module inside it.

Requirements taken directly from §1.3's failures, both of which build clean and
therefore need explicit coverage:

1. The tree must appear in **both** `DefaultInfo.other_outputs` **and** the
   `RunInfo` hidden set. Absent from either, buck2 does not materialize it and
   the `node_modules` link dangles.
2. The `relative_to` parent count must place the link correctly. In the spike
   the app output is at `<pkg>/__<name>__/app` and the tree at
   `<pkg>/__<tree>__/node_modules`, making `relative_to = (app, 0)` correct and
   `(app, 1)` silently wrong.

### 5.2 `node_test`

`node_binary` plus `ExternalRunnerTestInfo`, so `buck2 test` runs it. No
separate app-dir mechanism.

---

## 6. Pipeline and modules

`pudu buckify` gains a store-layout pass between graph pruning and emission:

| Module | Responsibility |
|---|---|
| `src/buck/store.rs` | Store-path computation (§3). Pure: graph + importer → `(copies, links)`. No I/O, no Starlark. |
| `src/buck/bzl.rs` | Extended with the `node_modules_tree`, `node_binary`, `node_test` rule bodies and the embedded builder. |
| `src/buck/emit.rs` | Extended to render tree targets after package targets. |

`store.rs` is deliberately separable from Starlark rendering: the depth
arithmetic of §3.1 is the error-prone part and is worth testing without a
formatter in the way.

---

## 7. Testing

Unit tests over `store.rs`: scoped and unscoped siblings, the npm-alias
`link_name`/target split, top-level links, and `.bin` entries — each asserting
the exact `../` count.

Snapshot tests (insta) over the emitted tree targets, and a determinism test
that a second `buckify` produces no diff.

**The CI job must run, not just build.** S4's `buck2-build` job is upgraded to
`buck2 run` with an assertion on stdout. This is not a nicety: §1.3 established
that a dangling `node_modules` symlink, a missing hidden input and a wrong
`relative_to` depth all produce a **green `buck2 build`**. A build-only gate
would ship every bug in that class.

---

## 8. Exit criteria

1. `buck2 run` on the extended `01-pure-js` prints a line proving a dependency
   resolved through the store.
2. A first-party module `require`-ing another first-party module works — the
   copies-not-symlinks requirement of §1.3.
3. A transitive sibling resolves (`debug` → `ms`): S4 §1.4's refuted case,
   green.
4. Both importers' trees build, and a second `buckify` is byte-identical.
5. **A deliberate mutation fails the suite:** a wrong `../` depth on a scoped
   link. Since a dangling link builds clean, this mutation is the only evidence
   that the gate in §7 actually gates.
6. `cargo test`, clippy and fmt clean; MSRV 1.88 builds.

---

## 9. Errors

No new error variants are expected. `format::validate_bin_name` already rejects
bin names Buck cannot spell as provider names, and store paths are rendered as
Starlark strings through `starlark_string`, which S4 established as the trust
boundary for third-party text.

If store-path computation can fail — a root whose target is missing from the
pruned graph is the candidate — it gets a typed variant rather than a panic.
The plan resolves whether that case is reachable; §2 defers `link:` roots,
which are the obvious source of `None` targets.

---

## 10. Doc and tech-debt fallout

- Design §8's `filegroup` example and its "needs no custom rule" framing are
  superseded by §4 and must be corrected, not merely annotated.
- The roadmap's S5 entry is split into S5 and S5.5, and its exit criteria
  rewritten: they were written against the refuted `filegroup` design.
- **New debt:** the RE/CAS round-trip of an output containing symlinks and
  hardlinks is untested (§1.7).
- **New debt:** hardlinked store entries share inodes with buck-out
  extractions, so a write into `node_modules` corrupts a cached artifact
  (§1.4). Mitigated by S6's script gate; worth revisiting when S13 considers
  running scripts at all.
- **Retargeted to S5.5:** TD-S1-04 (`file:` specifiers treated as importer
  links), TD-S2-06 (two platforms emitting identical constraint labels).
