# pudu S3 — `pudu vendor` and the package table

**Date:** 2026-08-31
**Stage:** S3 of the [roadmap](2026-08-30-pudu-roadmap.md)
**Depends on:** S1 (instance graph), S2 (platform pruning)
**Evidence:** [npm tarball, bin, and install-script survey](../research/2026-08-31-npm-tarball-vendor-survey.md)

---

## 1. Scope

`pudu vendor` performs one download pass over every package that survives S2's
pruning on at least one configured platform, verifies each tarball against the
lockfile's sha512, and records into a committed `<third_party_dir>/packages.toml`
the four things `pnpm-lock.yaml` cannot supply: `url`, `sha256`, `size`, and
the results of inspecting the archive (`bin`, `has_install_script`).

`pudu vendor --check` is an offline staleness gate for CI.

### 1.1 Not in S3

- **Authentication of any kind.** No `~/.npmrc`, no tokens, no `Authorization`
  header. A 401 or 403 is an error that names the registry and says so. Filed
  as [TD-S3-01](#11-tech-debt).
- **Unverifiable resolutions.** `resolution: {tarball: …}` with no integrity
  (the `github:` shape), `Git`, and `Directory` are hard errors — see §3.2.
- **Vendor mode.** Committing tarballs under `third-party/js/vendor/` is
  roadmap S10; `packages.toml` is designed to carry what it will need.
- **`prefer_tarball`.** A fixup key, so it lands with fixups in S7.
- **Emitting anything Buck reads.** S3 produces `packages.toml` and nothing else.

---

## 2. Module layout

Six modules, drawn so that exactly one of them touches the network. Every
other module is testable with neither a socket nor a temporary directory.

| File | Responsibility | Needs |
|---|---|---|
| `src/registry.rs` | `(name, version, &RegistryConfig) → Url`; scope overrides | nothing |
| `src/tarball.rs` | bytes → verify sha512, compute sha256/size, inspect archive | nothing |
| `src/cache.rs` | integrity-addressed store under `~/.cache/pudu` | filesystem |
| `src/fetch.rs` | ureq agent, worker pool, retries, `--no-network` gate | network |
| `src/packages.rs` | `packages.toml` render, parse, staleness diff | nothing |
| `src/cli/vendor.rs` | orchestration, diagnostics, exit codes | all of the above |

`src/cli/context.rs` is added alongside: `debug.rs` today owns a private
`load()` that reads `pudu.toml` and the lockfile **without** running
`Config::validate`. `vendor` cannot use that — an invalid registry URL must be
rejected before it is fetched from. `context.rs` exposes both
`load_validated()` and `load_lenient()`; `debug` moves to the lenient one with
its behaviour unchanged, `vendor` uses the validated one.

New dependencies: `ureq` (rustls), `tar`, `flate2`, `base64`, `dirs`; dev-only
`httpmock`. `sha2` is already present from S1.

---

## 3. Deciding what to fetch

### 3.1 The package set

`vendor` builds the S1 graph, runs `platform::prune` against the configured
platforms, and vendors the **union** of surviving nodes —
`matrix.platforms_by_node.keys()`. A package excluded on every configured
platform is not downloaded.

This makes `packages.toml` a function of `pudu.toml` as well as of the lockfile:
adding a platform makes the package table stale, and `--check` catches it. That is
the intended behaviour, not a side effect.

An empty `[platforms]` table means there is nothing to vendor, and `vendor`
errors rather than writing an empty package table. No `vendor`-specific check is
needed for that: `vendor` calls `load_validated()` unconditionally, and
`Config::validate` already rejects an empty `[platforms]` table with
`ConfigError::NoPlatforms` (exit 3). An earlier draft of this section
mandated a separate `NoPlatformsConfigured` variant; it was unreachable by
construction and has been removed. The integration test still asserts the
behaviour, through the `ConfigError` path.

Several snapshot keys can share one tarball — peer-dependency instances of the
same package@version differ only in their edges. The download set is therefore
keyed on `name@version`, deduplicated, so no tarball is fetched twice.

Every key in that set is a genuine `name@version` pair. The one lockfile shape
whose key carries a URL in its version segment — the integrity-less `github:`
resolution — is rejected by §3.2 before the set is built, so no package-table
key ever holds a URL.

### 3.2 Resolution shapes

`Resolution` (from `src/lock/types.rs`) maps to a fetch source as follows.
The survey's §4 establishes that the two `Tarball` shapes are genuinely
different cases, and S1's own type comment already anticipated this split.

| Resolution | Source | Integrity |
|---|---|---|
| `Integrity { integrity }` | URL derived per §3.3 | the recorded sha512 |
| `Tarball { tarball, integrity: Some(i) }` | `tarball` **verbatim** | `i` |
| `Tarball { tarball, integrity: None }` | — | **error** |
| `Git { .. }` | — | **error** |
| `Directory { .. }` | — | **error** |

The second row is the private-registry shape. The third is the `github:`
shape: no hash exists anywhere for those bytes, and a codeload archive is not
npm-shaped (it nests under `<repo>-<sha>/`, not `package/`).

Unsupported resolutions are **collected, not raced**: every offending package
is reported in one `UnsupportedResolution` error naming each package and its
resolution kind, so a repo with four git dependencies learns about all four in
one run.

### 3.3 URL derivation

```
<registry>/<name>/-/<basename>-<version>.tgz
```

where `basename` is `name` after the last `/`, and `<registry>` is
`config.registry.scopes["@scope"]` when `name` begins with `@scope/` and that
exact key is present, else `config.registry.default`. Scope matching is exact
— npm has no notion of nested scopes.

The survey verified this against 400 live registry manifests with zero
mismatches, including 130 scoped names and version shapes from `0.0.0` to
dated prereleases.

**Registries with a path prefix.** `Url::join` is wrong here: joining
`name/-/name-1.0.0.tgz` onto `https://host/repo/npm` discards the `npm`
segment. Derivation appends to the base path with exactly one separating `/`,
so both `https://host` and `https://host/repo/npm/` behave. A configured base
whose path ends in `/` and one that does not must produce identical output.

`RegistryConfig` already exists and is already validated by S0: the scheme is
restricted to http/https and scope keys must begin with `@`. S3 adds no
configuration.

---

## 4. Fetching

### 4.1 The cache

Content-addressed by the integrity the lockfile already records, so a cache
lookup needs no network to compute its key:

```
$PUDU_CACHE_DIR  or  $XDG_CACHE_HOME/pudu  or  ~/.cache/pudu
  └── tarballs/sha512/<hex[0..2]>/<hex>.tgz
```

`<hex>` is the lowercase hex of the digest decoded from the `sha512-<base64>`
integrity string. Writes go to a temporary file in the same directory and are
renamed into place, so a killed run never leaves a truncated entry that a
later run trusts.

Cached bytes are **re-verified on read**. The hash is cheap next to the I/O,
and it is what makes `--no-network` against a warm cache trustworthy rather
than merely fast.

`PUDU_CACHE_DIR` exists so the integration tests are hermetic; it is not
advertised in `--help`.

### 4.2 The client

`ureq` with rustls, a shared agent, and a pool of `--jobs` worker threads
(default 8, clamped to 1..=64). Each worker runs the whole per-package
pipeline — fetch, cache-write, verify, inspect — and discards the bytes before
taking the next package, so peak memory is bounded by `jobs`, not by the size
of the dependency graph.

Results are collected into a `BTreeMap` keyed by package, so output order is
independent of completion order. This is what makes the determinism criterion
hold under parallelism rather than by luck.

Timeouts: 10s connect, 120s per request — `typescript` is roughly 8 MB.
Retries: 3 attempts, backoff 250 ms / 500 ms / 1 s, on transport errors, 408,
429, and 5xx **only**. A 4xx is never retried.

### 4.3 `--no-network`

Already a global flag. With it set, a cache hit proceeds normally and a cache
miss is a `NetworkDisabled` error naming the package and the URL that would
have been fetched. No socket is opened in either case.

---

## 5. Verification and inspection

`tarball::verify_and_inspect(key, bytes, expected_integrity)` is a pure
function: bytes in, `Verified { sha256, size, inspection }` out.

1. **Verify.** Compute sha512 over the bytes as received; compare against the
   decoded integrity. A mismatch aborts with `IntegrityMismatch`, naming the
   package, the URL, and both hashes.
2. **Hash for Buck.** Compute sha256 over the same bytes. Record `size` as the
   compressed byte count — what `http_archive` will download.
3. **Inspect.** Gunzip and walk the archive once, collecting `package.json`
   and the full list of entry names.

The order matters: sha256 is only ever recorded for bytes whose sha512 already
verified. That is the whole trust chain, and it is why `packages.toml` is worth
committing.

**Archive shape.** Every entry must share a **single root directory** — but
that root is not required to be `package`.

An earlier revision of this section required the first path component to be
literally `package`, on the strength of design §8's `strip_prefix =
"package"`. That is false of the real registry: DefinitelyTyped's
types-publisher nests each `@types/*` package under its own display name, so
`@types/estree` unpacks to `estree/` and `@types/node@22.20.x` to `node
v22.20/` — a space and all. The fixture has 18 `@types/*` entries, every one
of them a counterexample. Requiring `package` rejected all of them as
malformed.

What pudu enforces instead is consistency: an archive whose entries disagree
about their root cannot be extracted to one directory, and that is
`MalformedTarball`. The root it settles on is recorded as the package table's
`root` field (§6) and is what design §8 must pass to `strip_prefix`.

Tar metadata members — `pax_global_header`, emitted by GNU tar's default pax
format and by `git archive` — are not part of the directory tree and are
skipped before the consistency check, or a valid archive from a private
registry would be rejected for having two roots.

### 5.1 `has_install_script`

pnpm's rule, verbatim from the survey's §2 — three independent triggers:

1. `scripts.preinstall`, `scripts.install`, or `scripts.postinstall` is present
   and non-empty in `package.json`
2. `package/binding.gyp` exists
3. any entry matches `package/.hooks/…`

Design §4 describes this as "`package.json` inspection". It is not — triggers 2
and 3 are properties of the file list.

Trigger 1 has live instances in the fixture: `esbuild@0.25.12` and
`esbuild@0.28.2`, both `"postinstall": "node install.js"`. **Triggers 2 and 3
have none**, and are covered only by synthetic tarballs in `src/tarball.rs`'s
unit tests.

An earlier revision of this spec named `fsevents@2.3.3` as a live instance of
trigger 2. It is not one: its sha512-verified tarball carries no
`binding.gyp`, no `gypfile` field, and no install-family script at all — it
ships a prebuilt `fsevents.node`. Only the registry packument claims
otherwise, and that metadata is stale; real pnpm reads the extracted tarball,
never the registry API, so it would compute `false` here as pudu does.
`tests/vendor_oracle.rs` asserts exactly that, and TECH_DEBT TD-S3-03 carries
the evidence.

### 5.2 `bin`

`@pnpm/package-bins`, reproduced exactly (survey §3):

- `bin` as a **string** yields one command, named after the package with any
  leading scope stripped — `@babel/parser` gives `parser`.
- `bin` as an **object** yields its keys, each scope-stripped the same way.
- A command whose name is not equal to its own `encodeURIComponent` encoding
  is **dropped**, unless the name is exactly `$`.
- A command whose resolved path escapes the package root is **dropped**.
- With no `bin` field at all, `directories.bin` is walked **recursively**;
  every file becomes a command named after its basename, so nested files
  collapse to bare names.

Recorded paths are relative to the package root, forward-slashed, with any
leading `./` removed. Entries are processed in sorted archive order and a
later entry overwrites an earlier one of the same name, which keeps the result
deterministic; a collision emits a warning.

A `bin` object value that is not a string is dropped with a warning rather
than failing the run.

**Cross-check.** The lockfile's `hasBin` disagreeing with the computed map is
a **warning**, never an error. On the 400-package corpus the two agree exactly
(27 and 27), but `hasBin` is a flag pnpm derives from the packument, and the
archive is the truth.

---

## 6. The package table

`<third_party_dir>/packages.toml`, committed, deterministic, sorted:

```toml
# @generated by pudu. Do not edit by hand.
version = 1

["@babel/parser@7.29.8"]
url = "https://registry.npmjs.org/@babel/parser/-/parser-7.29.8.tgz"
sha512 = "sha512-…"
sha256 = "…"
size = 1948123
root = "package"
bin = { parser = "bin/babel-parser.js" }

["@types/estree@1.0.9"]
url = "https://registry.npmjs.org/@types/estree/-/estree-1.0.9.tgz"
sha512 = "sha512-…"
sha256 = "…"
size = 16145
root = "estree"

["esbuild@0.25.12"]
url = "https://registry.npmjs.org/esbuild/-/esbuild-0.25.12.tgz"
sha512 = "sha512-…"
sha256 = "…"
size = 129712
root = "package"
has_install_script = true
```

Keyed on `name@version`, not the snapshot key: a tarball has no peer
dependencies, so one entry serves every peer instance.

`sha512` is the lockfile's integrity string verbatim, prefix included;
`sha256` is lowercase hex, which is the form `http_archive` expects. `bin` is
omitted when empty and `has_install_script` when false — the common
case is a package with neither, and 373 of 400 fixture packages have no bin.

`root` is the archive's single root directory (§5), with no trailing slash:
what design §8's `http_archive` passes to `strip_prefix`. It is **required**,
never defaulted — every valid archive has exactly one, and it is `package` for
most but not all of them (18 `@types/*` entries in the fixture nest under
their own display name). It is recorded rather than recomputed because the
package table is the only offline input the build-rule pass has; deriving the root
there would mean re-downloading and re-inspecting every tarball, which is the
one thing committing the package table exists to avoid.

`--check` forms no expectation for `root`, because it forms none for anything
the bytes decide. A root that changed did so because the tarball changed,
which the `sha512` comparison already catches.

**Rendering is by hand, not via `toml::to_string`.** Byte-identical output is
an exit criterion, and hand-rendering makes the format the spec rather than a
property of a serializer's table-inlining heuristics. Parsing uses the `toml`
crate normally. Keys are always quoted; the writer escapes `"` and `\`.

The file is written to a temporary file and renamed, so an interrupted run
leaves the previous package table intact rather than a half-written one.

Renamed from `pudu.lock` in S3.5 — see `docs/superpowers/specs/2026-09-01-pudu-s3.5-package-table-design.md`.

---

## 7. `pudu vendor`

```
pudu vendor [--check] [--jobs N]
```

1. `load_validated()` — `pudu.toml` and `pnpm-lock.yaml`.
2. Build the graph; prune; take the union of surviving nodes (§3.1).
3. Resolve each to a fetch source (§3.2); collect unsupported resolutions.
4. Read the existing `packages.toml` if present.
5. **Carry over** every entry whose `url` and `sha512` already match what step
   3 expects. These are not re-downloaded and not re-verified.
6. Fetch, verify, and inspect the remainder (§4, §5).
7. Merge, render, write atomically.

The package table is **rebuilt** from the expected set rather than amended, so an
entry for a package that has left the graph is dropped rather than lingering.
`--jobs` is accepted but has no effect under `--check`, which downloads
nothing.

Step 5 is what makes a one-package version bump cost one download. The
trade-off is explicit: a recorded `sha256` is never re-checked against upstream
once written. That is also precisely what makes `packages.toml` an audit artifact
— the bytes a reviewer approved are the bytes the build consumes, and a
registry that later changes them fails the build instead of silently winning.

**Reporting.** By default one summary line to stderr:
`vendored 316 packages (12 downloaded, 292 cached, 12 unchanged)`. Under `-v`,
one line per download. stdout stays empty — `vendor` produces a file, not a
stream.

### 7.1 `--check`

Steps 1 through 4 only. No socket is opened and no tarball is read. The
expected `(url, sha512)` for every package in the set is compared against the
package table, and every difference is reported:

- a package in the graph with no package-table entry
- a package-table entry for a package no longer in the graph
- a `url` that differs — a changed registry or scope override
- a `sha512` that differs — a republished version
- a `packages.toml` `version` that is not 1

Clean exits 0. Any difference lists every one of them and exits
`ExitCode::Stale`.

This cannot detect a registry that changed a tarball's bytes after vendoring;
detecting that requires downloading, which would turn a sub-second CI gate
into a full download pass. The `sha256` in the committed package table is what
actually defends against it, at build time.

---

## 8. Errors, warnings, exit codes

A new `ExitCode::Stale = 5`, distinct so CI can tell "the package table needs
regenerating" from "the input is invalid".

`VendorError` joins the `typed_errors!` registry, which per S0's design means
every variant renders with a `code` and maps to an exit code by construction:

| Variant | Exit |
|---|---|
| `Stale { differences }` | 5 |
| `UnsupportedResolution { packages }` | 3 |
| `IntegrityMismatch { key, url, expected, actual }` | 3 |
| `MalformedTarball { key, reason }` | 3 |
| `MissingPackageJson { key }` | 3 |
| `TableMalformed { path, reason }` | 3 |
| `NetworkDisabled { key, url }` | 3 |
| `HttpStatus { key, url, status }` | 1 |
| `Transport { key, url, source }` | 1 |

`HttpStatus` carries dedicated help text for 401 and 403 saying that
authentication is not implemented and naming the registry host — the single
most likely first encounter with a private registry.

`VendorWarning`, registered nowhere (warnings have no exit code, per S2):
`HasBinDisagreement`, `BinNameRejected`, `BinPathEscapes`, `BinNameCollision`,
`NonStringBinValue`.

---

## 9. Testing

**Unit — `registry.rs`:** plain and scoped names; a scope override taking
precedence; a scope with no override falling back to default; a base with a
path prefix; bases with and without a trailing slash producing identical URLs;
a version containing characters needing no escaping and one with a prerelease
suffix.

**Unit — `tarball.rs`:** tarballs are built in-process with `tar` + `flate2`,
so every branch is reachable without a network or a fixture file. Covers: a
correct sha512; a mismatch naming both hashes; a non-`package` root; a missing
`package.json`; string `bin` on an unscoped and on a scoped name; object `bin`;
a scoped object key; a name rejected by the `encodeURIComponent` rule; the `$`
exemption; a path escaping the root; `directories.bin` including a nested file;
a bin-name collision; a non-string bin value; each of the three
`has_install_script` triggers **in isolation**, including `binding.gyp` with
no scripts at all.

**Unit — `packages.rs`:** render/parse round-trip; rendering is byte-identical
across runs and independent of insertion order; `bin` and `has_install_script`
omitted when empty/false; each staleness difference in isolation; an
unparseable package table.

**Unit — `cache.rs`:** key derivation from an integrity string; a hit; a miss;
corrupt cached bytes rejected on read; `PUDU_CACHE_DIR` honoured.

**Integration — `httpmock`:** a full vendor pass writing a package table; a second
run downloading nothing; `--check` clean and stale; `--no-network` with a warm
and a cold cache; a 500 retried then succeeding; a 404 not retried; a 401
producing the auth help text; a served tarball whose bytes fail sha512.

**Oracle — network-gated CI job.** The 400 registry manifests are captured to
`tests/fixtures/lock/real/oracle/manifests.json` by a committed script. A test
runs the real vendor pass against registry.npmjs.org for the fixture and
asserts, for every package it vendors, that the derived `url`, computed `bin`,
and `has_install_script` match the oracle — and that the vendored set is
exactly S2's pruned union. It runs in its own CI job, mirroring S2's fuzz job,
and is `#[ignore]`d locally.

Where the tarball and the manifest disagree, the tarball wins and the
disagreement is the finding — the oracle is a cross-check, not the truth.

---

## 10. Exit criteria

1. `pudu vendor` writes a deterministic, sorted `packages.toml` covering every
   package in the pruned union; a second run reproduces it byte-identically.
2. A tarball whose bytes fail the lockfile's sha512 aborts with an error
   naming the package, the URL, and both hashes.
3. `pudu vendor --check` exits 5 when stale, 0 when current, and opens no
   socket in either case.
4. `--no-network` succeeds against a warm cache and errors against a cold one,
   naming the missing package and its URL.
5. A scoped-registry override resolves to the right host and the resolved URL
   is recorded in the package table.
6. `has_install_script` is true for `esbuild@0.25.12`, whose published tarball
   genuinely carries `"postinstall": "node install.js"` and is rooted at
   `package/`. (Not `fsevents@2.3.3`: its tarball carries no install-family
   script and no `binding.gyp`, whatever its registry packument says — see
   §5.1. The `binding.gyp` and `.hooks/` triggers have no live instance in
   this fixture and are pinned by synthetic tarballs instead.)
7. `@babel/parser`'s bin is recorded as `parser`, not `@babel/parser`.

---

## 11. Tech debt

Filed at spec time; the ledger is the authority.

**Carried in from earlier stages.** TD-S1-05 (the last-`@` split mis-parsing a
`git@host` dep path) was targeted at S3 on the assumption that S3 would meet a
real git dependency. It does not: §3.2 rejects git resolutions by key, without
consulting the parsed name or version, so the mis-parse stays unexercised and
the item moves to the stage that supports git dependencies. TD-S1-01 (the
lockfile parsed three times) and TD-S1-08 (`print-graph` echoing the version
constant) are untouched by this stage and are re-targeted rather than closed.

- **TD-S3-01 — no authentication.** `[registry]` supplies hosts only. A
  private registry needing credentials cannot be vendored from. The follow-up
  is reading `~/.npmrc` (`_authToken` lines, scope→registry mappings, env
  interpolation, precedence), chosen explicitly over an ad-hoc env-var scheme
  so that pudu honours what users already have configured.
