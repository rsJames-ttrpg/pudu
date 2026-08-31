# pnpm platform matching — empirical survey

Written 2026-08-31, before the S2 spec. Every claim here was produced by
running pnpm's own code or by installing real packages, not by reading
documentation. Where a finding contradicts an assumption in the roadmap or
the parent design, it is called out.

Reference implementation: `@pnpm/package-is-installable@1000.0.21`
(`lib/checkPlatform.js`, `lib/checkEngine.js`), the module pnpm 10.21.0 uses
to decide whether a package is installable. Probes ran on node v24.6.0,
Linux x86_64, glibc.

---

## 1. The matching algorithm

`checkPlatform` compares three independent axes — `os`, `cpu`, `libc` — each
through one shared helper, `checkList(current, list)`. Reduced to its
observable rules:

```js
list = list.filter(v => typeof v === 'string')   // non-strings dropped first
if (list.length === 1 && list[0] === 'any') return true
for (const item of list) {
  if (item[0] === '!') { if (item.slice(1) === current) return false; blc++ }
  else match ||= (item === current)
}
return match || blc === list.length
```

The absent-field case never reaches `checkList`: the caller substitutes
`['any']` for a missing `os`/`cpu`/`libc`.

### Verified behaviour

Each row was executed against the real `checkPlatform`.

| package fields | platform | result |
|---|---|---|
| *(no fields)* | linux-x64-glibc | install |
| `os: [linux]` | linux-x64-glibc | install |
| `os: [darwin]` | linux-x64-glibc | prune |
| `os: [!win32]` | linux-x64-glibc | install |
| `os: [!win32]` | win32-x64 | prune |
| `os: [!win32, !darwin]` | linux-x64-glibc | install |
| **`os: [!win32, darwin]`** | linux-x64-glibc | **prune** |
| `os: [!win32, linux]` | linux-x64-glibc | install |
| `os: [any]` | linux-x64-glibc | install |
| **`os: [any, darwin]`** | linux-x64-glibc | **prune** |
| `os: []` | linux-x64-glibc | install |
| `cpu: [123]` *(non-string)* | linux-x64-glibc | install |
| `cpu: [123, arm64]` | linux-x64-glibc | prune |
| `cpu: [wasm32]` | linux-x64-glibc | prune |
| `libc: [glibc]` | linux-x64-glibc | install |
| `libc: [musl]` | linux-x64-glibc | prune |
| `libc: [musl]` | linux-x64-musl | install |

Two rules are counter-intuitive and are the ones an implementation written
from intuition gets wrong:

- **A mixed list needs an explicit positive hit.** `["!win32", "darwin"]`
  does *not* mean "anything but win32, and darwin especially". Because
  `blc (1) !== list.length (2)`, the result is `match`, which is false on
  linux. Negations only ever subtract.
- **`any` is special only as a singleton.** `["any", "darwin"]` is treated as
  an ordinary two-element positive list, so it prunes everywhere except
  darwin.

Both also mean an empty list, and a list of entirely non-string entries,
match everything: `match=false, blc=0, list.length=0` satisfies
`blc === list.length`.

### The libc axis is conditionally skipped

```js
if (wantedPlatform.libc && currentLibc !== 'unknown') { … }
```

`currentLibc` comes from `detect-libc`'s `familySync()`, evaluated once at
module load **against the host**, and is `'unknown'` on macOS. So on a real
Mac the libc axis is never checked at all, whatever a package declares.

This is a live trap for oracle capture. Capturing a darwin oracle *from a
Linux host* leaves `currentLibc = 'glibc'`, so libc **is** checked, and a
`libc: [musl]` package is pruned — which a real Mac would have installed.
Verified: `libc:[musl]` with `supportedArchitectures.os = [darwin]` prunes on
this host.

Pudu's rule — skip the libc axis when the configured platform declares no
libc — reproduces real-macOS semantics, and is therefore correct where a
Linux-captured darwin oracle would be wrong.

---

## 2. `libc` is usually absent from v9 lockfiles

The npm registry serves two documents. pnpm requests the **abbreviated**
packument (`Accept: application/vnd.npm.install-v1+json`) by default.

For `lightningcss-linux-x64-musl@1.33.0`:

| document | `os` | `cpu` | `libc` |
|---|---|---|---|
| abbreviated | `["linux"]` | `["x64"]` | **absent** |
| full | `["linux"]` | `["x64"]` | `["musl"]` |

The abbreviated document carries exactly `cpu dist engines funding name os
version`. `libc` is not in it.

Consequences, all confirmed against the committed fixture:

- The fixture's `pnpm-lock.yaml` contains **zero** `libc:` fields, across 804
  package entries, despite including `lightningcss-linux-x64-musl` and
  `@rollup/rollup-linux-x64-musl` — both of which declare `libc` in the
  registry's full document.
- Those musl builds are therefore indistinguishable from their gnu siblings
  in the lockfile: both read `cpu: [x64], os: [linux]`.
- **pnpm installs both on a glibc platform.** The captured
  `linux-x64-gnu` oracle contains `@rollup+rollup-linux-x64-musl` and
  `lightningcss-linux-x64-musl`. pnpm under-prunes here, and pudu
  reproducing pnpm exactly means pudu under-prunes identically.

The earlier v9 field survey counted 74 `libc` occurrences across the
author's private lockfiles. That is consistent with a private registry
returning full metadata for the abbreviated request — so pudu must handle
`libc` when present while never depending on it.

**This is why S2 cannot get libc coverage from a public-registry fixture.**
No ordinary `pnpm install` against registry.npmjs.org produces a lockfile
with a `libc` line. Coverage comes from the differential fuzz against
`checkPlatform` instead.

Negation is likewise absent from the fixture (0 occurrences) and rare in the
registry: `fsevents`, the canonical platform-specific package, uses positive
`os: ["darwin"]`. Negation is a real part of the npm spec that pudu must
honour, but no realistic fixture will exercise it.

---

## 3. Engines contaminate the oracle

`packageIsInstallable` checks platform **and** engines, and for an
**optional** dependency a failure of either causes a silent skip:

```js
const warn = checkPackage(pkgId, pkg, options)   // platform, then engines
if (warn == null) return true
if (options.optional) { …log skipped…; return false }
if (options.engineStrict) throw warn
return null                                       // non-optional: install anyway
```

So the set of directories in `node_modules/.pnpm` is *not* a pure platform
oracle. Measured against the fixture:

| node version | optional deps failing engines |
|---|---|
| 24.6.0 *(capture host)* | `@napi-rs/lzma-linux-x64-gnu@1.5.1` |
| ≥ 24.12.0 | none |

`@napi-rs/lzma-linux-x64-gnu@1.5.1` wants
`node: ^22.20 || ^24.12 || >=25`. On node 24.6.0 it is eligible by platform
on linux-x64 and skipped by engines — and it was the **single** discrepancy
in the first end-to-end validation run, on the one platform where it is
otherwise installable.

(`svelte-eslint-parser@1.8.1` also fails engines on every node version, via
a `pnpm: 10.34.5` constraint against pnpm 10.21.0. It is *not* optional, so
pnpm warns and installs it, and it does not affect the oracle.)

The oracle must therefore be committed together with the capture's node and
pnpm versions and the computed engine-excluded set, and the differential
test must subtract that set. Pudu deliberately does not model `engines`:
node version is not a platform axis (parent design §5), so pudu's survivor
set is a superset of pnpm's by exactly the engine-excluded optional
dependencies.

---

## 4. Non-optional dependencies are installed, not rejected

Contrary to the assumption that pnpm hard-fails on a platform mismatch
(`EBADPLATFORM` is npm's behaviour), pnpm **warns and installs** a
non-optional dependency whose platform excludes the host, unless
`engineStrict` is set. Only optional dependencies are skipped.

S2 deliberately diverges: pudu warns and **drops** the edge for that
platform. Emitting a Buck dependency on a package that cannot run on the
platform being configured serves nobody, and the warning preserves the
signal. The divergence is recorded in the spec because it is a divergence,
not an oversight.

---

## 5. Per-platform oracles: capture and validation

Captured by copying the fixture, writing `supportedArchitectures` into
`pnpm-workspace.yaml`, and running
`pnpm install --ignore-scripts --frozen-lockfile`, then listing
`node_modules/.pnpm`. The lockfile did not drift on any capture.

| platform | directories |
|---|---|
| linux-x64-gnu | 316 |
| linux-x64-musl | 316 |
| linux-arm64-gnu | 316 |
| darwin-arm64 | 315 |

The `linux-x64-gnu` capture reproduces the committed
`virtual-store-listing.txt` **byte-for-byte**, which retroactively confirms
that S1's fixture was generated on a glibc x86_64 host and that the two
files stay meaningful together.

The platforms differ exactly where expected — `@esbuild/*`, `@rollup/*`,
`lightningcss-*` swap variants — and `fsevents@2.3.3` (`os: [darwin]`)
appears only in the darwin capture.

### End-to-end validation of the S2 model

A model implementing *only* per-package field matching, minus the
engine-excluded optional set, was compared against all four oracles, using
S1's verified `depPathToFilename` port for name mangling:

```
linux-x64-gnu     model=316 oracle=316  EXACT MATCH
linux-x64-musl    model=316 oracle=316  EXACT MATCH
linux-arm64-gnu   model=316 oracle=316  EXACT MATCH
darwin-arm64      model=315 oracle=315  EXACT MATCH
```

**No transitive reachability pass is needed.** Per-package field matching
reproduces pnpm's install set exactly on every platform tested.

### The boundary of that claim

All **90** platform-gated snapshot keys in the fixture have **zero**
dependencies of their own. Every one is a leaf. The fixture therefore cannot
distinguish "field matching suffices" from "field matching plus a
reachability sweep" — it can only show that the simpler design is not yet
wrong.

That is a property of the modern ecosystem rather than a coincidence: the
prebuilt-binary packages that carry `os`/`cpu` (`@esbuild/*`, `@rollup/*`,
`lightningcss-*`, `@img/sharp-*`) are single-artifact leaves by
construction. Should a gated package with its own subtree ever appear, the
failure mode is an orphaned package left in the store — fat, not incorrect.

---

## 6. What this survey changed

| assumption | outcome |
|---|---|
| The fixture could be extended to cover `libc` | **Withdrawn.** Abbreviated packuments make it unreachable from the public registry; coverage moves to the differential fuzz. |
| The virtual-store listing is a pure platform oracle | **Withdrawn.** Engines silently skip optional deps; the exclusion set must be captured and subtracted. |
| pnpm hard-fails a non-optional platform mismatch | **False.** It warns and installs. Pudu's warn-and-drop is a deliberate divergence. |
| Pruning may need a reachability pass | **Not needed**, verified against four oracles — with the leaf caveat recorded above. |
| `!x` and `any` behave intuitively in a list | **False** in two ways: mixed lists need a positive hit, and `any` is special only as a singleton. |
