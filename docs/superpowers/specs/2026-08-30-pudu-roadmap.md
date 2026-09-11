# Pudu Roadmap

**Status:** draft v1 (2026-08-30)
**Companion to:** `2026-08-30-pudu-design.md`
**Purpose:** Decompose the design into ordered, demoable stages. Each stage gets its own spec and implementation plan when its turn comes.

---

## Conventions

- Each stage produces a **demoable, testable artifact**, not just internal scaffolding.
- Each stage's design spec lives at `docs/superpowers/specs/YYYY-MM-DD-pudu-s<n>-<topic>-design.md`.
- Each stage's implementation plan lives at `docs/superpowers/plans/YYYY-MM-DD-pudu-s<n>-<topic>.md`.
- **Specs are written stage-by-stage**, not all upfront. Later specs incorporate what earlier stages learned.
- A stage is "done" when its exit criteria pass in CI and the demo works end-to-end.
- Follow-ups discovered during implementation are filed separately, not retro-edited into the original spec.
- Tech debt accumulated during stage reviews is logged in `../TECH_DEBT.md`.

---

## Phase 1 — v0.1.0 launch path

The credible-launch surface: typescript, esbuild, express, zod, vitest, `@swc/core` working end-to-end on `linux-x64-gnu` and `darwin-arm64`, with local fixups and community registry layering active.

### S0 — Scaffolding & config

**Scope:** CLI skeleton (clap), `pudu init`, `pudu.toml` parser with rich errors (miette), error type machinery, GitHub Actions CI on three platforms.

**Exit criteria:**
- `pudu init` writes a starter `pudu.toml`, a `third-party/js/` skeleton, and a `system_node_toolchain` entry; refuses to overwrite without `--force`.
- `pudu --help` lists all phase-1 verbs, unimplemented ones marked as such.
- A malformed `pudu.toml` produces an error naming the bad field and line.
- Platform validation rejects an unknown `os`/`cpu`/`libc` value with the list of valid ones.
- CI green on `ubuntu-latest`, `ubuntu-24.04-arm`, `macos-latest`.

**Demo:** From an empty repo, `pudu init` produces a config that `pudu config check` accepts.

**Touches:** `src/main.rs`, `src/cli/`, `src/config.rs`, `src/error.rs`, `.github/workflows/`.

**Shipped 2026-08-30:** 18 commits, 78 tests (55 unit + 23 integration), CI gates green on stable. All ten exit criteria verified by hand against the built binary. Three defects were found in the plan itself rather than in the implementations, each caught by adversarial review rather than by the plan's own tests:

- **`toolchain::apply` used first-occurrence marker matching**, so a `toolchains/BUCK` with two BEGIN markers took the replace path under `--force` and destroyed every user rule between the first BEGIN and the END. Fixed by requiring exactly one of each marker.
- **`pudu init --force` silently overwrote hand-edited files under `third-party/js/`**, including a `toolchains.bzl` whose own generated header says "Safe to edit". Resolved by a spec change: `--force` now governs `pudu.toml` and the `toolchains/BUCK` managed block only.
- **The `supportedArchitectures` axis fallback could not distinguish "key absent" from "key present but filtered empty"**, so `os: [win32]` silently substituted the host OS instead of erroring.

Also corrected: the plan wrongly asserted `serde_json` was already a dependency, and declared `rust-version = "1.85"` while using `let`-chains (stabilized in 1.88). Deferred minors are logged in [`../TECH_DEBT.md`](../TECH_DEBT.md).

---

### S1 — pnpm-lock.yaml parser & instance graph

**Scope:** Parse lockfile v9 into typed structures — `importers`, `packages`, `snapshots`. The snapshot-key grammar including peer suffixes. Target-name mangling. Instance graph construction. Reject unknown `lockfileVersion` loudly.

The design's lockfile assumptions are **verified** — see the
[v9 field survey](../research/2026-08-31-pnpm-lock-v9-field-survey.md), which
confirms `requiresBuild` is gone from v9, finds `bundledDependencies` need no
handling, and turns up three things the design missed: recursively nested peer
suffixes, 422-character snapshot keys, and npm-aliased dependency edges.
Stage spec: [S1](2026-08-31-pudu-s1-lockfile-design.md).

**Exit criteria:**
- A hidden `pudu debug print-graph` reads the lockfile + `pudu.toml` and prints the instance graph as JSON, one entry per snapshot key.
- Fixtures cover: dev deps, optional deps, peer-dep instances of one package@version, scoped names, workspace importers, `link:` workspace deps, npm-aliased edges, and cycles.
- **Cycles are detected and reported, not rejected.** The survey found them in every real lockfile examined (`@babel/core`, `eslint`, `browserslist`); rejecting them would reject nearly every real project. This replaces an earlier "cycles (rejected clearly)" criterion.
- Snapshot-key mangling is unit-tested including nested peers, the verbatim 422-char corpus key, sort stability, and collision detection.
- An unsupported `lockfileVersion` errors naming the supported range.

**Demo:** On a real-world lockfile with 500+ packages including peer instances, `pudu debug print-graph` produces a stable JSON snapshot.

**Touches:** `src/lock/`, `src/cli/debug.rs`.

---

### S2 — Platform model & optional-dep pruning

**Scope:** The `os`/`cpu`/`libc` model. Matching against npm's `packages:` fields including negation (`!win32`). Optional-dependency pruning per platform. Mapping platform definitions to prelude constraint labels, including the conditional-abi rule (design §7).

**Exit criteria:**
- A hidden `pudu debug platforms` prints, per platform, which optional deps survive pruning.
- Unit tests cover: negation, multi-value lists, packages with no platform fields, a package excluded on every configured platform (warn, don't fail).
- The conditional-abi rule is tested both ways: a glibc-only config emits no abi constraint; a glibc+musl config emits it on both.
- `constraints = [...]` escape hatch overrides generated labels.

**Demo:** On `esbuild@0.23.0`'s ~20 optional deps, each configured platform resolves to exactly one `@esbuild/*` package.

**Touches:** `src/platform.rs`.

---

### S3 — `pudu vendor` & the package table

**Scope:** Registry URL derivation with scope overrides. Tarball fetch. sha512 verification against the lockfile. `package.json` inspection for `bin` and install scripts. Deterministic `packages.toml` writing. `--check` staleness gate. `~/.cache/pudu/` content-addressed download cache.

**Exit criteria:**
- `pudu vendor` writes a deterministic, sorted `packages.toml` covering every package in the graph.
- A tarball whose bytes fail the lockfile's sha512 aborts with a precise error naming the package.
- `pudu vendor --check` exits non-zero when `packages.toml` is stale, zero when current.
- `--no-network` with a warm cache succeeds; with a cold cache it errors naming the missing package.
- Scoped-registry override resolves to the right host; the resolved URL is recorded.

**Demo:** `pudu vendor` on a 200-package lockfile produces a `packages.toml` that a second run reproduces byte-identically.

**Touches:** `src/registry.rs`, `src/tarball.rs`, `src/packages.rs`, `src/cache.rs`.

---

### S4 — First BUCK emitter (package layer, single platform)

**Scope:** Deterministic BUCK formatter for the package layer only. `npm_package` emission, `config/BUCK` generation, `pudu.bzl`. Snapshot tests via insta. Determinism test. The `node_modules_tree` store-layout generator moved to S5 (see below): it needs real directories and intra-tree symlinks from a single Buck action, which `filegroup` cannot provide, so it is a rule pudu must write rather than one composed from prelude primitives.

This stage resolves two of design §12's three unknowns: `http_archive` `sub_targets` for `.bin`/`root` extraction (confirmed) and Node's symlink realpath behaviour under a `filegroup`-based store (refuted — see the S4 design §1.4). The third — store scale, filed as "`filegroup` scale" — moves with the rule it concerns: to S5, and from there to S5.5, since S5's single-platform fixture is too small to measure.

**Exit criteria:**
- `pudu buckify` on `01-pure-js` produces byte-identical artifacts matching golden files.
- Running buckify twice produces no diff (`10-determinism`).
- Sort order documented and tested: lexicographic by snapshot key, then platform.
- `buck2 build //third-party/js/...` succeeds on the fixture, including `[root]` and `[bin/*]` sub-targets.

**Demo:** A fixture with a handful of pure-JS deps buckifies; the BUCK is human-readable and matches design §8.

**Touches:** `src/buck/`, `src/cli/buckify.rs`, `tests/fixtures/buck/01-pure-js/`.

---

### S5 — Store layout, `node_binary`, and `buck2 run`

**Scope:** The `node_modules_tree` store-layout generator (moved here from S4 — see above), as a rule pudu writes rather than a `filegroup`. The store-path computation feeding it. One tree per importer, emitted into `third-party/js/BUCK`. `node_binary` and `node_test`. Extending `01-pure-js` with a runnable importer, and upgrading the CI job from `buck2 build` to `buck2 run`.

Single platform. Per-platform emission and the `select()` move to S5.5, below. Stage spec: [S5](2026-09-10-pudu-s5-store-layout-design.md).

**Already done, needed no work:** `system_node_toolchain`. S0 writes the rule and its `NodeToolchainInfo { node: RunInfo }` provider into the user's `toolchains/`; S5 is merely the first stage to consume it. The roadmap listed it as S5 scope only because nothing had consumed it before.

**Exit criteria:**
- `pudu buckify` on `01-pure-js` emits one `node_modules_tree` per importer, byte-identically across runs.
- The store paths match pnpm's shape: real directories at `.pnpm/<key>/node_modules/<pkg>`, tree-relative symlinks joining them, `../` depth computed from each link's own path rather than constant.
- No emitted symlink dangles, checked over the whole fixture graph.
- CI runs `buck2 run //packages/app:main` on Linux x86_64 and the program resolves its dependencies through the store.

**Demo:** `buck2 run` on the fixture app starts Node, resolves `require`s through a pnpm-shaped store buck2 built, and prints.

**Touches:** `src/buck/store.rs`, `src/buck/bzl.rs`, `src/buck/emit.rs`, `src/cli/buckify.rs`, `tests/buckify_tree.rs`, CI e2e workflow.

**Shipped 2026-09-11:** 14 commits, 447 tests. What the stage found is more interesting than what it built:

- **The design's premise was refuted before a line was implemented.** Design §8 had the store as a `filegroup` needing "no custom rule at all". S4 §1.4 showed Node cannot resolve through `symlinked_dir`'s output, and the S5 spike then measured the replacement against the CI-pinned buck2 rather than specifying it on faith. The store layout became a rule pudu writes, designed against evidence.
- **The `../` arithmetic was right first time.** `store.rs`'s computed depth — the part most obviously fiddly, and the part `buck2 run` exercises most harshly — turned up no defect when the machinery finally ran. The mutation tests that gate it were written before it was trusted, not after.
- **Two real defects surfaced only when the machinery first ran**, both latent and neither findable by unit test:
  - **Every `toolchains/BUCK` pudu had ever written failed to parse.** `pudu init` emitted the `toolchains.bzl` load spec without the `@` cell sigil, and had done since S0. Nothing noticed because until S5 nothing had asked buck2 to resolve the node toolchain.
  - **`node_modules_tree` targets were emitted private.** It is a `rule()`, not a macro, so it does not inherit the `visibility` default that the `npm_package` macro applies; the omission is invisible until another package tries to depend on a tree.
- **The CI gate had to become `buck2 run`, not `buck2 build`.** A dangling symlink, a missing hidden input and a wrong `../` depth all build clean. `buck2 build` green was never evidence the store worked; only running Node through it is.

Debt filed: TD-S5-01 … TD-S5-07 in [`../TECH_DEBT.md`](../TECH_DEBT.md).

---

### S5.5 — Multi-platform ("esbuild day one")

**Scope:** Per-platform emission of the store, and the alias-with-`select()` over `config/` labels. The semver-stable bare alias (`//third-party/js:express`). The `02-platform-optional`, `03-peer-instances`, `04-musl` and `05-workspace` fixtures. macOS arm64 e2e in CI. The store-scale measurement owed since S4.

`link:` roots between importers arrive here too — `Root::target` is `None` for those today, which is TD-S1-04 — as does TD-S2-06's ambiguous `select()`, which cannot arise until there are per-platform arms to be ambiguous between.

**Exit criteria:**
- `02-platform-optional` buckifies; CI runs `buck2 run //packages/app:main` on Linux x86_64 and macOS arm64, and the output proves the correct `@esbuild/*` was selected.
- `03-peer-instances` produces distinct targets for distinct peer resolutions, both buildable.
- `04-musl` emits the abi constraint on both platforms and buckifies.
- `05-workspace` emits one `node_modules_tree` per importer over a shared store, including a `link:` dependency between importers.
- A store-scale measurement is recorded; if the single-action tree degrades, the split-along-extraction-lines fallback is specced as a follow-up.

**Demo:** `buck2 run //packages/server:server` starts an express server whose TypeScript was compiled by tsc and whose esbuild resolved to the host's platform package.

**Touches:** `src/buck/emit.rs`, `src/buck/store.rs`, `src/platform.rs`, CI e2e workflow.

---

### S6 — Lifecycle-script gate

**Scope:** The script gate using `has_install_script` from `packages.toml`. The `[scripts] allow` acknowledgement list. The precise error message contracted in design §6.

**Exit criteria:**
- `09-install-script-error` asserts the error message byte-for-byte with the tuple substituted.
- A package on the `[scripts] allow` list emits normally with its script ignored, and the fact is reported at `-v`.
- The error names `pudu fixups show <pkg>` as the next action.

**Demo:** A fixture depending on a script-declaring package fails with the design's exact message; adding it to `allow` makes the build pass.

**Touches:** `src/scripts.rs`, `src/cli/buckify.rs`.

---

### S7 — Local fixups

**Scope:** `FixupConfig` schema (design §9). `cfg()` predicate parser + evaluator with the npm↔reindeer vocabulary mapping. Local-only layering. Application in the emitter. Overlay materialization.

**Exit criteria:**
- `07-local-fixup` exercises `extra_deps`, `omit_deps`, `replace_deps`, `bin`, `overlay`, `exclude_platforms`, `visibility`, `labels`, `runtime_env`, plus cfg sections.
- `cfg()` works for `version`, `target_os`, `target_arch`, `target_env`, plus `all`/`any`/`not`.
- The vocabulary mapping is unit-tested: `target_os = "macos"` matches a platform declaring `os = "darwin"`.
- `pudu fixups show <pkg>` prints the merged effective fixup as canonical TOML.

**Demo:** A fixup overlays a marker module into a real registry package; CI greps the built output for it.

**Touches:** `src/fixup/`, `src/buck/emit.rs`, `src/cli/fixups.rs`.

---

### S8 — Community registry

Split into two sub-stages, following muntjac's S7a/S7b precedent — layering is independently demoable without git fetch.

#### S8a — Community layering (no-network modes)

**Scope:** `RegistryConfig` enum (`None`/`FileUrl`/`Git`). Community ⊕ local layering. `replace_community` escape hatch. `allow_local_overrides = false`.

**Exit criteria:** `08-community-fixup` snapshot-tests the layering algorithm; `replace_community` drops the community layer; `pudu fixups show` prints labeled community and local blocks.

#### S8b — Git fetch, cache, and the seed repo

**Scope:** `gix` dependency. `~/.cache/pudu/fixups/<sha>/` content-addressed cache. `pudu fixups update` with structured diff. `--no-network` integration test. New `pudu-fixups` repo seeded with the platform-quirky tier (sharp, prisma, `@swc/core`, better-sqlite3, canvas), README, CONTRIBUTING, MIT LICENSE, schema-only CI.

**Exit criteria:** `pudu fixups update` fetches, caches by sha, prints a diff, and bumps the pin; `--no-network` errors on cache miss; the seed repo is public with CI green and tagged `seed-v0.1.0`.

---

### S9 — Launch polish & v0.1.0

**Scope:** README as canonical entry point. The 60-second demo wired as a CI fixture. Cargo metadata polish. Release workflow. `cargo publish --dry-run` gate. v0.1.0 tag. Launch post drafted.

**Exit criteria:**
- The README quickstart passes as a CI test on all three runners.
- `cargo publish --dry-run` succeeds.
- v0.1.0 tag exists; release notes drafted.
- Launch post drafted — the maintainer presses send.

**Demo:** A new user goes from `cargo install pudu` to a Buck-built Node binary in five minutes by following the README.

---

## Phase 2 — v0.2.0+

Deferred deliberately; sequenced after real-world feedback.

- **S10 — Vendor mode.** Commit tarballs; swap `http_archive` for local source refs. `packages.toml` already carries what's needed.
- **S11 — Audit + unused.** OSV cross-check against the GitHub Advisory Database; report vendored tarballs no importer references. Depends on S10.
- **S12 — Multi-lockfile trees.** muntjac's `[tree.<name>]` model, for repos with genuinely separate lockfiles.
- **S13 — Lifecycle-script execution.** Run allowlisted scripts in a sandboxed Buck rule.
- **S14 — Windows.**

---

## Stage dependency graph

```
                    S0 (scaffolding)
                           │
              ┌────────────┴────────────┐
              ▼                         ▼
       S1 (lockfile)            S2 (platforms)
              │                         │
              └────────────┬────────────┘
                           ▼
                   S3 (vendor + package table)
                           │
                           ▼
                   S4 (first BUCK)
                           │
                           ▼
            S5 (store layout + buck2 run)
                           │
                           ▼
        S5.5 (multi-platform, "esbuild day one")
                           │
                           ▼
                   S6 (script gate)
                           │
                           ▼
                   S7 (local fixups)
                           │
                           ▼
              S8a (community layering)
                           │
                           ▼
           S8b (git registry + seed repo)
                           │
                           ▼
                S9 (README + publish)
                           │
                           ▼
        ────────── v0.1.0 ──────────
```

S1 and S2 can be developed in parallel after S0; both feed S3.

---

## Specs index

| Stage | Spec | Plan | Status |
|---|---|---|---|
| Design | [2026-08-30-pudu-design.md](./2026-08-30-pudu-design.md) | n/a | ✅ committed |
| Roadmap | this document | n/a | ✅ committed |
| S0 | [2026-08-30-pudu-s0-scaffolding-design.md](./2026-08-30-pudu-s0-scaffolding-design.md) | [2026-08-30-pudu-s0-scaffolding.md](../plans/2026-08-30-pudu-s0-scaffolding.md) | ✅ shipped (18 commits, 78 tests) |
| S1 | [2026-08-31-pudu-s1-lockfile-design.md](./2026-08-31-pudu-s1-lockfile-design.md) | [2026-08-31-pudu-s1-lockfile.md](../plans/2026-08-31-pudu-s1-lockfile.md) | ✅ shipped |
| S2 | [2026-08-31-pudu-s2-platforms-design.md](./2026-08-31-pudu-s2-platforms-design.md) | [2026-08-31-pudu-s2-platforms.md](../plans/2026-08-31-pudu-s2-platforms.md) | ✅ shipped (17 commits, 247 tests) |
| S3 | [2026-08-31-pudu-s3-vendor-design.md](./2026-08-31-pudu-s3-vendor-design.md) | [2026-08-31-pudu-s3-vendor.md](../plans/2026-08-31-pudu-s3-vendor.md) | ✅ shipped |
| S3.5 | [2026-09-01-pudu-s3.5-package-table-design.md](./2026-09-01-pudu-s3.5-package-table-design.md) | [2026-09-01-pudu-s3.5-package-table.md](../plans/2026-09-01-pudu-s3.5-package-table.md) | ✅ shipped |
| S4 | [2026-09-01-pudu-s4-buck-emitter-design.md](./2026-09-01-pudu-s4-buck-emitter-design.md) | [2026-09-01-pudu-s4-buck-emitter.md](../plans/2026-09-01-pudu-s4-buck-emitter.md) | ✅ shipped |
| S5 | [2026-09-10-pudu-s5-store-layout-design.md](./2026-09-10-pudu-s5-store-layout-design.md) | [2026-09-11-pudu-s5-store-layout.md](../plans/2026-09-11-pudu-s5-store-layout.md) | ✅ shipped (14 commits, 447 tests) |
| S5.5–S9 | (not yet written) | (not yet written) | ⬜ planned |

---

## Implementation cadence

For each stage:

1. **Brainstorm the stage's spec**, referencing the design spec for shared context.
2. **User reviews and approves the spec.**
3. **Write the implementation plan** via `superpowers:writing-plans`.
4. **User reviews and approves the plan.**
5. **Execute** via `superpowers:executing-plans`.
6. **Verify** exit criteria; review under `superpowers:requesting-code-review`.
7. **Commit, tag the stage ✅**, move on.

The roadmap is updated at the end of each stage with what shipped, what slipped, and any follow-ups filed.
