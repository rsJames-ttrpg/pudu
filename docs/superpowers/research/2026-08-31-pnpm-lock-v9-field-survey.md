# pnpm-lock.yaml v9 field survey

**Date:** 2026-08-31
**Purpose:** Resolve the lockfile assumptions design §12 deferred to S1.
**Corpus:** 21 `pnpm-lock.yaml` files found on the development machine — 18 at
`lockfileVersion: '9.0'`, 3 at `'6.0'`. Sizes 9–8173 lines. Sources include
SvelteKit/Vite apps, VS Code extensions, pnpm's own tooling, and dlx caches.

The v6 files are not targets; they serve only as a contrast set for
establishing which fields v9 dropped.

## Q1 — Did v9 drop `requiresBuild`? **Yes. Confirmed.**

| key | v9 (18 files) | v6 (3 files) |
|---|---|---|
| `requiresBuild` | **0** | 105 |
| `hasBin` | 336 | 80 |
| `bundledDependencies` | 3 | 0 |
| `libc` | 74 | 0 |
| `cpu` / `os` | 716 / 726 | 44 / 46 |
| `deprecated` | 14 | 1 |

`requiresBuild` does not appear in any v9 file. Design §4's premise holds:
install-script detection requires tarball inspection, so the `pudu vendor`
pass stays mandatory.

**But `hasBin` survived into v9**, which the design did not account for. Its
value is always the bare boolean `true` (147/147 occurrences in the sampled
repos) — it records *that* a package has bins, never *which*. The bin map
still requires the tarball. The useful consequence is a cross-check: if the
lockfile says `hasBin: true` and the vendor pass extracts no bin entries,
that is a pudu bug or a corrupt tarball, and should error rather than
silently emit a package with no `.bin` wiring.

`libc` appearing only in v9 confirms the three-axis platform model.

## Q2 — How do `bundledDependencies` surface? **As a name list, already excluded from the graph.**

Three occurrences, all the same shape — a list under a `packages:` entry:

```yaml
'@tailwindcss/oxide-wasm32-wasi@4.1.17':
  resolution: {integrity: sha512-cEytGq...}
  engines: {node: '>=14.0.0'}
  cpu: [wasm32]
  bundledDependencies:
    - '@napi-rs/wasm-runtime'
    - '@emnapi/core'
    - tslib
```

The corresponding `snapshots:` entry carries **no `dependencies` at all**:

```yaml
'@tailwindcss/oxide-wasm32-wasi@4.1.17':
  optional: true
```

pnpm has already excluded the bundled names from the dependency graph — they
ship inside the tarball and are never resolved as separate packages. **Pudu
therefore needs no special handling and has no double-install risk**: the
bundled names simply never become edges. `bundledDependencies` is metadata to
tolerate and ignore. This closes the design §12 item asking for an explicit
S1 decision.

Note `cpu: [wasm32]` — a cpu value outside the `Cpu {X64, Arm64}` enum. Unknown
cpu/os/libc values must parse and prune, never fail.

## Q3 — Which `resolution:` variants occur? **Only `{integrity: …}` in this corpus.**

All 5168 v9 resolutions are `resolution: {integrity: sha512-…}`. No `tarball`,
git, or `directory` variants appear. This is a corpus gap, not evidence of
absence — the corpus has no private registries, git dependencies, or
`link:`/`workspace:` specifiers either. Those variants must be covered by
constructed fixtures, and unknown variants must be rejected by name.

## Structural findings (not asked for, but load-bearing)

**1. `packages:` and `snapshots:` are cleanly split by key form.** In the
sampled 7267-line lockfile, `packages:` keys contain `(` **zero** times;
`snapshots:` keys contain it **124** times. So:

- `packages:` is keyed `name@version` and holds *metadata* — resolution,
  engines, cpu/os/libc, hasBin, deprecated, peerDependencies,
  bundledDependencies.
- `snapshots:` is keyed `name@version(peer)(peer)` and holds *edges* —
  dependencies, optionalDependencies, optional, transitivePeerDependencies.

This validates "one Buck target per snapshot key". Metadata lookup is
snapshot key → strip peer suffix → `packages` entry.

**2. Peer suffixes nest recursively.** Real key from the corpus:

```
eslint-plugin-svelte@3.14.0(eslint@9.39.2(jiti@2.6.1))(svelte@5.49.1)
```

The grammar is balanced parens to arbitrary depth. **A parser that splits on
`(` or takes the first `)` is wrong.** Stripping the peer suffix means
scanning to the first top-level `(` at depth 0 — and `@` cannot be used as a
delimiter either, since scoped names begin with one.

**3. Snapshot keys reach 422 characters.** The three longest in one ordinary
SvelteKit app:

| chars | key (truncated) |
|---|---|
| 422 | `@vite-pwa/sveltekit@1.1.0(@sveltejs/kit@2.50.1(@sveltejs/vite-plugin-svelte@6.2.4(svelte@…` |
| 364 | `@sveltejs/adapter-cloudflare@7.2.6(@sveltejs/kit@2.50.1(…` |
| 277 | `@sveltejs/vite-plugin-svelte-inspector@5.0.2(…` |

The design's >128-char hash-mangling path is therefore **the common case in
ordinary projects, not a rare edge**. Every long-key behaviour — hash
stability, collision handling, and the readability of the generated target
name — is on the main path and must be tested with these real keys.

**4. Dependency edges are `name: <version-with-suffix>`.** An edge is resolved
to a snapshot key by concatenating `name` + `@` + the value verbatim:

```yaml
'@eslint-community/eslint-utils': 4.9.1(eslint@9.39.2(jiti@2.6.1))
#   → key: @eslint-community/eslint-utils@4.9.1(eslint@9.39.2(jiti@2.6.1))
```

Importers use the same encoding under a `version:` field beside `specifier:`.

**5. A top-level `settings:` block exists in all 18 v9 files** and the design
does not mention it:

```yaml
settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false
```

`excludeLinksFromLockfile: true` means `link:` dependencies are omitted from
the lockfile — pudu would emit a graph with silently missing edges. This
warrants at minimum a warning.

Top-level keys observed, and only these: `lockfileVersion`, `settings`,
`importers`, `packages`, `snapshots`. (`packages`/`snapshots` are absent in
the one trivial no-dependency lockfile.)

**6. No `catalogs:`, `overrides:`, or `patchedDependencies:` in the corpus** —
all are real pnpm features that would appear as additional top-level keys.
Their absence here means they are untested, not unsupported-by-format.

## Corpus gaps — must be covered by constructed fixtures

`link:`/`workspace:`/`file:` specifiers · git and tarball resolutions ·
multi-package workspaces beyond two importers · `catalogs:` · `overrides:` ·
`patchedDependencies:` · dependency cycles · musl `libc` variants.

## Late findings — two that change S1's requirements

### 7. Dependency cycles are universal. The roadmap's "reject cycles" criterion is wrong.

A DFS over the snapshot graph of four real lockfiles:

| lockfile | nodes | cycles |
|---|---|---|
| trandox | 740 | 8 |
| main-currents/frontend | 834 | 9 |
| yazi.nvim | 517 | 1 |
| superdesign ext | 517 | 4 |

**Every lockfile has them**, and they sit in the most ordinary packages:

```
@babel/core@7.28.6  ->  @babel/helper-module-transforms@7.28.6(@babel/core@7.28.6)  ->  @babel/core@7.28.6
eslint@9.39.2(jiti@2.6.1)  ->  @eslint-community/eslint-utils@4.9.1(eslint@…)  ->  eslint@…
browserslist@4.28.1  ->  update-browserslist-db@1.2.3(browserslist@4.28.1)  ->  browserslist@4.28.1
es-abstract@1.24.1  ->  string.prototype.trim@1.2.10  ->  es-abstract@1.24.1
```

Roadmap S1 lists a fixture for "cycles (rejected clearly)". **Rejecting cycles
would reject essentially every real project**, including any that uses Babel,
ESLint, or Browserslist. That criterion must be replaced.

Cycles are survivable because of the store design already chosen in §8: the
virtual store is **one `filegroup` mapping paths to tarball artifacts**, and a
package's extracted content never depends on its dependents' targets. The
cycle lives in the symlink wiring, which is data inside one target, not in the
Buck target graph. pnpm itself works the same way.

The consequence is a constraint on S4 rather than a defect in S1: **the store
cannot be decomposed into one Buck target per package that depends on its
dependencies' targets** — that shape would reintroduce the cycle as a Buck
target cycle and fail to load. If S4 ever splits the single `filegroup` for
incrementality (design §12 floats this as the scale fallback), it must split
along tarball-extraction lines, which are acyclic, never along dependency
edges.

S1 should therefore *detect* cycles and expose them as a diagnostic — they are
worth reporting, and the detector is needed to prove the acyclic-extraction
claim — but must not treat them as an error.

### 8. Dependency edge values are not always bare versions — npm aliases appear.

```yaml
'@isaacs/cliui@8.0.2':
  dependencies:
    string-width: 5.1.2                    # bare version
    string-width-cjs: string-width@4.2.3   # ALIAS: name@version
    strip-ansi-cjs: strip-ansi@6.0.1
    wrap-ansi-cjs: wrap-ansi@7.0.0
```

This is npm's alias syntax (`"string-width-cjs": "npm:string-width@^4.2.0"`).
The map key is the **link name** — the directory created under
`node_modules/` — while the value names a *different* package. The naive rule
`key = name + "@" + value` yields `string-width-cjs@string-width@4.2.3`, which
matches no entry; the correct key is `string-width@4.2.3`, which does exist in
both `packages:` and `snapshots:`.

Only 3 distinct aliased edges appear, but they occur in **4 of the 8
substantive v9 lockfiles** — all via `@isaacs/cliui`, a transitive dependency
of `glob` and `rimraf`. A parser that gets this wrong fails on roughly half of
all real projects.

**Rule.** Strip any peer suffix from the value; if what remains still contains
an `@` beyond position 0, the value is already a complete `name@version` key
and the map key is only a link name. Otherwise the value is a bare version and
the key is `name@value`.

**Link name ≠ package name is therefore a property S1 must model on the
edge**, not a detail S4 can reconstruct: the virtual store must symlink the
package's content in under the *alias*, so the edge carries both.

---

## Reuse investigation — 2026-08-31

Question asked: can existing pnpm work be reused to make S1 easier? Two
candidates, one rejected and one adopted wholesale.

### `chaste-pnpm` (crates.io) — rejected as a dependency, kept as corroboration

v0.6.0, Apache-2.0 OR BSD-2-Clause, updated 2026-01-29, ~3.4k downloads, from
the `chaste` lockfile-parser family. Only 586 lines, and it already uses
`serde_norway` — the same YAML crate pudu uses.

It independently implements the recursive peer-suffix grammar
(`snapshot_key_rest`) and npm-alias handling (`is_aliased` / `alias_name`),
which **independently corroborates both findings above**.

It is nonetheless the wrong dependency for pudu:

| need | chaste-pnpm |
|---|---|
| `os` / `cpu` / `libc` on packages | **absent** — its `Package` carries only resolution, version, peerDependencies |
| `engines`, `hasBin`, `deprecated`, `bundledDependencies` | absent |
| deterministic ordering | `HashMap` throughout; pudu's determinism invariant needs `BTreeMap` |
| resolution variants | integrity + tarball only; no git or directory |
| API shape | `parse(root_dir)` reads a directory and wants a `package.json` |

The missing platform fields are decisive: they are exactly what S2's
optional-dependency pruning — pudu's core value — operates on. Adopting
chaste would mean forking it or landing upstream changes, then still working
around `HashMap` ordering.

### `@pnpm/dependency-path` — algorithm adopted, verified byte-exact

The bigger win. Rather than inventing a target-name mangling scheme, pudu
ports pnpm's own `depPathToFilename` (v1001.1.10):

```js
let filename = depPath.replace(/[\\/:*?"<>|#]/g, '+');
if (filename.includes('(')) {
  filename = filename.replace(/\)$/, '').replace(/\)\(|\(|\)/g, '_');
}
if (filename.length > maxLengthWithoutHash ||
    (filename !== filename.toLowerCase() && !filename.startsWith('file+'))) {
  return `${filename.substring(0, maxLengthWithoutHash - 33)}_${sha256hex(filename).slice(0, 32)}`;
}
return filename;
```

`maxLengthWithoutHash` is pnpm's `virtual-store-dir-max-length`, default
**120**. `createShortHash` is `sha256(input).hex().slice(0, 32)`.

**Verification.** Reimplemented and run over every snapshot key in two real
lockfiles, diffed against the actual `node_modules/.pnpm/` directories pnpm
created:

| project | hashed names matched | hashed missed | plain matched | plain missed |
|---|---|---|---|---|
| main-currents/frontend | **24** | **0** | 728 | 82 |
| trandox | **8** | **0** | 603 | 129 |

**Every long/hashed name matched exactly — 32 for 32.** On the cleanly
installed project the hashed set is a perfect bijection: 24 produced, 24 on
disk, 24 matched, no extras.

All 211 plain-name misses are explained and none is an algorithm fault: 208
are optional dependencies pruned at install time for this platform
(`@esbuild/android-arm64`, `@cloudflare/workerd-darwin-64`, …), and the
remaining pair (`@emnapi/runtime`, `tslib`) are reachable only through the
bundled-dependency wasm package that was itself pruned. trandox additionally
has 12 stale store directories left by earlier installs — its `vite` entry on
disk predates `terser` joining the peer set — which is store hygiene, not
naming.

**1363 directory names reproduced exactly.**

### Consequences for the S1 spec

Adopting pnpm's algorithm serves design §5's greppability goal literally
rather than "in spirit" — a generated Buck target name can be pasted straight
into `ls node_modules/.pnpm/`. It also satisfies the approved principle
(readable stem, hash only to disambiguate) *better* than the scheme it
replaces, because short peer sets stay fully readable
(`vite@7.3.1_terser@5.46.0`) instead of being hashed away.

Four details the invented scheme had wrong:

1. **The escape set is `[\\/:*?"<>|#]` → `+`**, not `/` alone. Those are the
   Windows-illegal path characters plus `#`.
2. **Peers flatten to a readable `_` form** when short — trailing `)`
   dropped, then `)(`, `(`, `)` each to `_`. Hashing is the fallback, not the
   rule.
3. **Uppercase forces the hash path** regardless of length (the
   `filename !== filename.toLowerCase()` clause), guarding
   case-insensitive filesystems. Nothing in the corpus exercises this — 0 of
   3224 snapshot keys contain uppercase — so it is untested by real data and
   needs a constructed fixture.
4. **Peers must NOT be sorted.** pnpm hashes the lockfile's own order.
   Design §5's "pudu re-sorts defensively" would produce names that diverge
   from the real store, defeating the entire point. Determinism comes from
   the lockfile being deterministic, which it is.
