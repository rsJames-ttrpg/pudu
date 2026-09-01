# Pudu Design

**Status:** draft v1 (2026-08-30)
**Author:** Jack Mayo
**One-liner:** Pudu is `reindeer` for JavaScript — it translates a `pnpm-lock.yaml` into Buck2 build rules so a Buck monorepo can consume npm packages without running `pnpm install` at build time.

Named for the world's smallest deer, continuing the line: reindeer (Cargo) → muntjac (uv) → pudu (pnpm).

---

## 1. Goals & non-goals

### Goals

- **Sharp scope: `pnpm-lock.yaml → BUCK`.** Only pnpm is supported as the resolver, and only lockfile version `9.0` (pnpm 9 and 10). We do not accept `package-lock.json`, `yarn.lock`, `bun.lock`, or a bare `package.json`. The sharpness is the feature: assuming pnpm lets us trust the lockfile format, the resolver's output, the peer-dependency resolutions it already computed, and the on-disk layout it implies.
- **Solve esbuild on day one.** The credible v1 launch surface is: typescript, esbuild, express, zod, vitest, and `@swc/core` working end-to-end on `linux-x64-gnu` and `darwin-arm64`. Platform-specific `optionalDependencies` are where JavaScript hides its native-code problem; a tool that only handles pure-JS packages is a toy.
- **Fixups as data.** A community `pudu-fixups/` registry, layered with per-repo local overrides, using the same schema shape and `cfg()` grammar as reindeer and muntjac. Cross-pollination between the three tools' contributors is an explicit design goal.
- **Ship in public early.** Repo open from day one. Pivot signal: no PR activity on `pudu-fixups` 90 days post-launch ⇒ rethink.

### Non-goals (v1, explicitly)

- **Windows.** Buck2-on-Windows is immature. Clean v2 deliverable.
- **npm / yarn / bun lockfiles.** pnpm-only. Migration path: `pnpm import`.
- **Running lifecycle scripts at build time.** `preinstall`/`install`/`postinstall` are blocked. A package declaring one errors with a precise message and a fixup escape hatch. See §6. This matches pnpm 10's own default (scripts blocked unless allowlisted via `onlyBuiltDependencies`), so it is not a surprising stance to a pnpm user.
- **Bundling, TypeScript compilation, test runners.** Pudu emits dependency targets and a thin way to run Node. It is not `rules_js`. A `ts_library` or `esbuild_bundle` rule is a different project.
- **The Metro / React Native `js_bundle` bridge.** Prelude's `js_library` and `js_bundle` exist but are source-transform oriented and require a user-supplied worker. Emitting `JsLibraryInfo` adapters is captured in §12, not committed.
- **Bazel output.** Module structure leaves room for a future emitter under `src/bazel/`; no commitment.
- **`link:` / `file:` dependencies pointing outside the workspace.** Workspace-internal `link:` deps are workspace members and handled; escaping the repo root is rejected.

---

## 2. Architecture overview

Pudu is a **thin, offline reimplementation of pnpm's layout rules**, not a wrapper around pnpm. Unlike muntjac (which shells out to `uv` twice), pudu never invokes pnpm. pnpm has already done the hard part — resolution, peer-dependency disambiguation, and integrity recording — and written the answer into `pnpm-lock.yaml`. Pudu:

1. parses `pnpm-lock.yaml`,
2. builds the resolved graph, one instance per snapshot key (§5),
3. prunes `optionalDependencies` per platform using npm's `os` / `cpu` / `libc` fields,
4. fetches each tarball once to record what the lockfile omits (§4),
5. applies the layered fixup graph,
6. emits a deterministic, byte-stable `BUCK` plus a `pudu.bzl` helper.

Implemented as a single Rust crate with `src/lib.rs` + `src/main.rs` split for internal modularity. No public library API commitment in v1.

External crates pinned in `Cargo.toml` **today** (the S0 set): `anyhow`/`thiserror`, `clap`, `miette`, `pathdiff`, `serde`, `serde_json`, `serde_norway` (maintained `serde_yaml` fork; `serde_yaml` itself is unmaintained), `tempfile`, `toml`, `url` — plus dev-dependencies `assert_cmd` and `insta`.

The rest of the toolkit is chosen but **deferred to the stage that needs it**, so a first-time contributor does not compile rustls and gix to run a TOML parser. The mapping (ledger row TD-S0-21 is the authority): `nodejs-semver` → S1/S2 version handling; `reqwest`, `sha2`, `base64`, `tar`, `flate2` → S3 tarball fetching, with `httpmock` for its tests; `toml_edit` → S4, format-preserving `pudu.toml` rewriting; `glob`/`walkdir` → S7 local fixup discovery; `gix` and `dirs` → S8b's git fixup registry and its cache.

---

## 3. User-facing layout

### Canonical pnpm workspace

```
repo/
├── package.json                # workspace root
├── pnpm-workspace.yaml         # declares packages/*
├── pnpm-lock.yaml              # one resolved lockfile, source of truth for pudu
├── pudu.toml                   # platforms, registries, fixup registry pin
├── packages/
│   ├── server/
│   │   ├── package.json        # workspace member — an "importer" in the lockfile
│   │   └── BUCK                # hand-written: node_binary deps = [":node_modules"]
│   └── cli/
└── third-party/js/
    ├── BUCK                    # generated; do not edit
    ├── pudu.bzl                # generated helper macros
    ├── pudu.lock               # generated; committed (see §4)
    ├── config/BUCK             # generated config_setting targets
    ├── fixups/                 # local fixup overrides
    │   └── sharp/fixups.toml
    └── vendor/                 # only in vendor mode (v2)
```

### Workspaces are importers, not trees

One `pnpm-lock.yaml` already describes every workspace member under its `importers:` key. Pudu emits **one `node_modules_tree` target per importer**, all sharing a single package store. muntjac's multi-tree escape hatch (its S11) exists for *multiple lockfiles*; pnpm workspaces make that unnecessary for the common case, so multi-lockfile support is deferred to v2 (§12).

### `pudu.toml`

```toml
lockfile_path   = "pnpm-lock.yaml"
third_party_dir = "third-party/js"

[platforms.linux-x64-gnu]
os   = "linux"
cpu  = "x64"
libc = "glibc"

[platforms.darwin-arm64]
os  = "darwin"
cpu = "arm64"

[registry]
default  = "https://registry.npmjs.org"
"@myorg" = "https://npm.mycorp.example"      # scope-specific override

[fixups]
registry              = "none"                # or "github.com/<user>/pudu-fixups"
registry_rev          = "<git-sha-or-tag>"
allow_local_overrides = true

[scripts]
allow = []                                    # lifecycle-script allowlist; empty = block all

[buck]
file_name = "BUCK"
```

Platform entries carry **two duties**: the `os` / `cpu` / `libc` values are matched literally against the corresponding npm `packages:` fields to prune optional dependencies (§5), and they are translated into Buck constraint labels to generate `config_setting` targets (§7). Both duties are served by one derived mapping — there is no string-keyed join between config and emitted Buck, which is the failure mode discussed in §7.

---

## 4. `pudu vendor` and the `pudu.lock` sidecar

This is pudu's largest departure from muntjac, and it is forced by a hard constraint.

**The constraint.** npm records integrity as `sha512-<base64>`. Buck2's `http_archive` and `http_file` accept `sha1` and `sha256` only (`prelude/http_archive/http_archive.bzl` passes exactly those to `ctx.actions.download_file`). There is no path from the lockfile's hash to a hash Buck can verify.

**The resolution.** `pudu vendor` performs one download pass and writes a committed sidecar at `<third_party_dir>/pudu.lock`. The pass resolves four things the lockfile cannot supply:

| Recorded | Why `pnpm-lock.yaml` can't supply it |
|---|---|
| `sha256`, `size` | Buck cannot verify sha512. The lockfile's sha512 **is** verified during this download, so the trust chain is unbroken: pnpm's hash gates what gets hashed into pudu.lock. |
| `url` | Lockfile v9 stores integrity only for registry packages. The URL is derived from name + version + the `[registry]` config, and must be recorded because scope overrides can change it. |
| `bin` map | Lives in the tarball's `package.json`. Needed to build `node_modules/.bin/` symlinks. The lockfile's `hasBin: true` is only a flag. |
| `hasInstallScript` | Lockfile v9 does not carry a `requiresBuild` field (v5/v6 did). The §6 script gate needs the real `package.json`. |

**Consequences.**

- `pudu vendor` is **mandatory** before `pudu buckify`, unlike muntjac where `vendor` is a convenience. `buckify` errors precisely when `pudu.lock` is missing or stale for any package.
- `pudu.lock` is committed. This is a security improvement over trusting a transitively-derived URL: the sha256 actually consumed by the build is auditable in review, and a registry that changes a tarball's bytes fails the build rather than silently succeeding.
- `pudu vendor --check` is the CI gate for "did someone forget to re-run pudu?"

**Format** (deterministic, sorted by key):

```toml
# @generated by pudu. Do not edit by hand.
version = 1

["esbuild@0.23.0"]
url    = "https://registry.npmjs.org/esbuild/-/esbuild-0.23.0.tgz"
sha512 = "sha512-1lvV17H2bMYda/WaFb2jLPeHU3zml2k4/yagNMG8Q/YtfMjCwEUZa2eXXMgZTVSL5q1n4H7sQ0X6CdJDqqeCFA=="
sha256 = "a5b1e5a25c1a2e0f36b3a55a3d0a5cba8a0f2c9c9f6b3f0c8dd0a5c8f8c1b7a3"
size   = 89341
bin    = { esbuild = "bin/esbuild" }
has_install_script = true
```

Keyed on `name@version` (not the snapshot key) because a tarball has no peer dependencies — the same tarball serves every peer instance, and downloads are never duplicated.

---

## 5. Pipeline

`pudu buckify` flow:

```
Input: pnpm-lock.yaml + pudu.toml + pudu.lock + fixups/ + registry cache
   ↓
1. Parse pnpm-lock.yaml → importers, packages, snapshots
   ↓
2. Build instance graph — one node per snapshot key
   ↓
3. Platform pruning — filter optionalDependencies by os/cpu/libc
   ↓
4. Resolve tarballs from pudu.lock (error if stale)
   ↓
5. Script gate — reject packages with install scripts unless allowlisted
   ↓
6. Fixup application — community ⊕ local, cfg sections evaluated
   ↓
7. BUCK emitter — stable ordering, normalized formatting
```

### Lockfile shape (v9)

```yaml
lockfileVersion: '9.0'
importers:
  packages/server:
    dependencies:
      express: { specifier: ^4.19.2, version: 4.19.2 }
packages:
  express@4.19.2:
    resolution: { integrity: sha512-... }
    engines: { node: '>= 0.10.0' }
  '@esbuild/linux-x64@0.23.0':
    resolution: { integrity: sha512-... }
    cpu: [x64]
    os: [linux]
snapshots:
  express@4.19.2:
    dependencies:
      accepts: 1.3.8
  esbuild@0.23.0:
    optionalDependencies:
      '@esbuild/linux-x64': 0.23.0
      '@esbuild/darwin-arm64': 0.23.0
  'react-dom@18.3.1(react@18.3.1)':
    dependencies:
      react: 18.3.1
```

`packages:` carries version-level metadata (integrity, `engines`, `os`, `cpu`, `libc`, `hasBin`, `deprecated`, `peerDependencies`). `snapshots:` carries the resolved dep edges, keyed by **snapshot key**.

### Peer-dependency instances

pnpm v9 snapshot keys encode peer resolutions in parentheses: `react-dom@18.3.1(react@18.3.1)`. The same `package@version` may appear as several instances with different dep sets. There is no Python or Cargo analog — muntjac and reindeer both key on `(name, version)`.

**Design: one Buck target per snapshot key.** Tarball targets stay keyed on `name@version`, so nothing is downloaded twice; only the dep wiring differs per instance.

Target-name mangling is a **direct port of pnpm's own `depPathToFilename`**
(`@pnpm/dependency-path`), not an invented scheme. Generated Buck target names
are therefore byte-identical to the directory names in a real
`node_modules/.pnpm/`, so a target name can be pasted straight into `ls` there.

In outline: escape `\ / : * ? " < > | #` to `+`; flatten peer parens to `_`;
and if the result exceeds 120 characters — or contains any uppercase, which
guards case-insensitive filesystems — truncate to 87 characters and append
`_` plus the first 32 hex of its sha256. Short peer sets stay fully readable
(`vite@7.3.1_terser@5.46.0`); long ones keep a readable stem and a hash tail.

Peers are **not** sorted: pnpm hashes the lockfile's own order, and re-sorting
would make every hashed name diverge from the real store. Determinism comes
from the lockfile being deterministic. Two distinct keys mangling to one
target name is a hard error naming both.

A reimplementation was verified against real virtual stores — 1363 directory
names reproduced exactly, including 32 of 32 hashed long names (see the
[v9 field survey](../research/2026-08-31-pnpm-lock-v9-field-survey.md)).

Peer suffixes nest to arbitrary depth
(`eslint-plugin-svelte@3.14.0(eslint@9.39.2(jiti@2.6.1))(svelte@5.49.1)`), so
the grammar is recursive and must be parsed with paren-depth tracking. Full
grammar, the npm-alias edge rule, and the exact naming algorithm:
[S1 spec](2026-08-31-pudu-s1-lockfile-design.md).

### Platform pruning

A dependency edge is dropped for a platform when the depended-on package's `packages:` entry declares an `os`, `cpu`, or `libc` list that excludes it. npm's fields support negation (`os: ["!win32"]`), which pudu honours.

This is the **only** axis of variance. pnpm resolves once, platform-independently; muntjac's second axis (`python_version`) has no analog, because a Node version does not fork resolution. So the matrix is platform alone.

`esbuild@0.23.0` optionally depends on ~20 `@esbuild/*` packages. On `linux-x64-gnu`, pudu keeps `@esbuild/linux-x64` and drops the rest. That pruning is what makes the launch surface work.

### Invariants

- **Determinism.** Same inputs ⇒ byte-identical output. Sort order is lexicographic by snapshot key, then platform.
- **Precise errors.** A failure names the exact `(package, version, platform)` tuple and points at `pudu fixups show <package>` as the next action.
- **Pudu never runs pnpm.** No shellout. The lockfile is the contract.
- **Tarball trust chains through sha512.** The sha256 in `pudu.lock` is only ever written after the lockfile's sha512 verified.

---

## 6. Lifecycle scripts

Packages declaring `preinstall`, `install`, or `postinstall` are **blocked by default**. Running them would mean arbitrary package code executing inside every Buck build, usually reaching the network (node-gyp fetching headers, `prebuild-install` fetching binaries) — which forfeits hermeticity, the entire point of buckifying.

The modern ecosystem largely routes around this already: esbuild, `@swc/core`, sharp, rollup, lightningcss, and Prisma all ship prebuilt per-platform packages selected via `optionalDependencies` and `os`/`cpu` fields. Pudu handles those natively, so the launch surface runs zero scripts.

When a package genuinely needs one:

```
error: sharp 0.33.5 declares an `install` script and is not allowlisted.

  Most native packages resolve via platform optionalDependencies
  (@img/sharp-linux-x64), which pudu handles natively.

  If this one genuinely needs a build step, add a fixup at
  third-party/js/fixups/sharp/fixups.toml — see `pudu fixups show sharp`
  for the current community fixup, or use `replace_deps` to point at a
  hand-rolled Buck target.
```

`[scripts] allow = ["..."]` in `pudu.toml` acknowledges a package without running its script (the package is emitted as-is, script ignored) — the common case where the script is an optional telemetry or fallback-build step the prebuilt binary makes unnecessary. Actually *executing* a script inside a Buck rule is a v2 surface; the `[scripts]` fixup block in §8 reserves the schema.

---

## 7. Platform model and Buck configuration

Pudu selects on **the prelude's own constraint settings**, and generates the `config_setting` targets that combine them. It does not invent a constraint universe and does not generate any wiring file.

### Why this differs from muntjac

muntjac generates a parallel `constraint_setting(name = "platform")` with one value per named platform, plus a `wiring.bzl` exporting `MUNTJAC_HOST_MODIFIERS` that the user must load into their root `PACKAGE` and pass to `set_cfg_modifiers`. It needs that indirection because manylinux version baselines and macOS deployment targets are **not expressible** as prelude constraints — there is no `manylinux` constraint setting, so muntjac had to mint an axis and hand-wire the mapping back to `prelude//os` + `prelude//cpu`.

Pudu has no such forcing function. npm's three platform fields map 1:1 onto constraint settings that already ship in the prelude:

| npm field | Prelude constraint setting | Values available |
|---|---|---|
| `os` | `prelude//os/constraints:os` | linux, macos, windows, android, iphoneos, … |
| `cpu` | `prelude//cpu/constraints:cpu` | x86_64, x86_32, arm64, arm64_32, arm32, … |
| `libc` | `prelude//abi/constraints:abi` | gnu, musl, msvc |

### What is generated

`third-party/js/config/BUCK`:

```python
##
## @generated by pudu
## Do not edit by hand.
##

config_setting(
    name = "linux-x64-gnu",
    constraint_values = [
        "prelude//cpu/constraints:x86_64",
        "prelude//os/constraints:linux",
    ],
    visibility = ["PUBLIC"],
)
config_setting(
    name = "darwin-arm64",
    constraint_values = [
        "prelude//cpu/constraints:arm64",
        "prelude//os/constraints:macos",
    ],
    visibility = ["PUBLIC"],
)
```

### The abi constraint is conditional

`prelude//platforms:default` derives its configuration from `host_info()` and sets **only cpu and os** (`prelude/platforms/defs.bzl`: `_host_cpu_configuration`, `_host_os_configuration`). Nothing sets an abi constraint by default.

So pudu emits the abi constraint **only when it is discriminating** — that is, only when two configured platforms share an `os` and `cpu` but differ in `libc`. A glibc-only configuration, which is the overwhelmingly common case, therefore needs **zero user wiring**: os and cpu come free from the default platform, and the generated `config_setting`s match out of the box.

When a user does configure both `linux-x64-gnu` and `linux-x64-musl`, both `config_setting`s gain their abi constraint and `pudu init` prints the one-time instruction for establishing it (a custom target platform, or `--modifier prelude//abi/constraints:musl`). Boilerplate is proportional to a feature the user explicitly asked for, rather than mandatory for everyone.

### Escape hatch

Monorepos with their own constraint universe override the generated labels:

```toml
[platforms.corp-linux]
os  = "linux"
cpu = "x64"
constraints = ["ovr_config//os:linux", "ovr_config//cpu:x86_64"]
```

`os` / `cpu` still drive npm field matching; `constraints` replaces only the emitted `constraint_values`.

### What we deliberately avoid

reindeer takes a different route: `[platform.<name>]` blocks hold cfg predicates, the emitted BUCK carries a `platform = {"linux-x86_64": dict(deps = [...])}` dictionary, and the mapping from Buck labels to those name strings lives in the **prelude** (`DEFAULT_REINDEER_PLATFORMS` in `prelude/rust/cargo_package.bzl`), overridable via `set_reindeer_platforms`. The platform name is an opaque string joining two files that never validate each other.

That has a documented failure mode: [reindeer#50](https://github.com/facebookincubator/reindeer/issues/50) — a config declaring `[platform.macos]` while the prelude's select produced `"macos-arm64"`. The dict key matched nothing, the dependency silently vanished, and the build failed on a missing crate. It was closed without adding validation.

Because pudu generates both `config/BUCK` and `pudu.bzl`, the select keys are real Buck labels checked by Buck itself. That class of drift is structurally impossible.

---

## 8. BUCK output shape

Two principles, inherited from muntjac:

1. **One semver-stable alias per package.** Users write `//third-party/js:express`, not `express-4.19.2`.
2. **Selects at the alias level, not inside package rules.** Each (instance × platform) gets its own concrete target. No internal `select()`.

### Generated `BUCK` (illustrative)

```python
##
## @generated by pudu
## Do not edit by hand.
##

load("//third-party/js:pudu.bzl", "npm_package")

npm_package(
    name    = "esbuild@0.23.0",
    url     = "https://registry.npmjs.org/esbuild/-/esbuild-0.23.0.tgz",
    sha256  = "a5b1e5a2...",
    size    = 89341,
    root    = "package",              # from pudu.lock; `@types/*` differ
    bin     = {"esbuild": "bin/esbuild"},
    visibility = ["PUBLIC"],
)
```

### Generated `pudu.bzl`

Four macros. `npm_package` emits an `http_archive` per package@version and an alias selecting the right per-platform variant:

```python
def npm_package(name, url, sha256, size, root, bin = {}, visibility = None):
    http_archive(
        name = name,
        urls = [url],
        sha256 = sha256,
        size_bytes = size,
        type = "tar.gz",
        strip_prefix = root,          # NOT always "package" — see below
        sub_targets = bin.values(),   # expose each bin script as //...:pkg[bin/foo]
        visibility = visibility or ["PUBLIC"],
    )
```

`root` is **not** a constant. An earlier revision of this file hard-coded
`strip_prefix = "package"` with the comment "npm tarballs universally nest
under package/". That is false: DefinitelyTyped's types-publisher nests each
`@types/*` package under its own display name, so `@types/estree` unpacks to
`estree/` and `@types/node@22.20.x` to `node v22.20/`. The 400-package fixture
has 18 such entries.

The value comes from the `root` field S3 records for every package in
`pudu.lock` (S3 design §6), computed once from the archive itself at vendor
time. This pass must read it from the sidecar and must not re-derive it —
`pudu.lock` is the whole offline input here, and deriving the root would mean
re-downloading every tarball.

A package target carries **no dependency attribute and no `select()`**. Dependency
edges are not wired target-to-target at all: pudu consumes the instance graph at
emit time and expresses the entire dependency structure as *paths in the store
layout* (below). A package is just its extracted tarball. That is what makes the
peer-instance explosion cheap — instances differ only in where they appear in a
tree, never in the artifact they point at.

`node_modules_tree` needs no custom rule at all. `filegroup` with `copy = False` calls `ctx.actions.symlinked_dir`, and its `srcs` accepts a dict of `path → artifact` — which is exactly a pnpm store layout:

```python
filegroup(
    name = "server_node_modules",
    copy = False,
    srcs = {
        "node_modules/express": "//third-party/js:express@4.19.2",
        "node_modules/.pnpm/express@4.19.2/node_modules/express": "//third-party/js:express@4.19.2",
        "node_modules/.pnpm/express@4.19.2/node_modules/accepts": "//third-party/js:accepts@1.3.8",
        "node_modules/.bin/esbuild": "//third-party/js:esbuild@0.23.0[bin/esbuild]",
    },
)
```

The dict is generated flat, one entry per edge in the store graph, and describes the whole layout in a single action.

Platform variance lives here and nowhere else, since pruning changes *which* store
paths exist. `node_modules_tree` emits one `filegroup` per platform plus an alias
selecting between them — keeping the "select at the alias level" principle, and
avoiding any dependence on `select()` working inside a dict-valued attribute:

```python
def node_modules_tree(name, srcs_by_platform, visibility = None):
    for platform, srcs in srcs_by_platform.items():
        native.filegroup(
            name = "{}__{}".format(name, platform),
            copy = False,
            srcs = srcs,
            visibility = [],
        )
    native.alias(
        name = name,
        actual = select({
            "//third-party/js/config:{}".format(p): ":{}__{}".format(name, p)
            for p in srcs_by_platform
        }),
        visibility = visibility or ["PUBLIC"],
    )
``` Node's own resolution algorithm then works unmodified, because the layout **is** pnpm's — a module inside `.pnpm/express@4.19.2/node_modules/express` resolves its siblings by walking up one directory, exactly as under a real `pnpm install`. Scoped names use pnpm's `+` convention (`.pnpm/@scope+name@1.0.0/`).

`node_binary` and `node_test` are thin: a generated launcher script plus `sh_binary`, taking the tree as a resource and the Node executable from a toolchain.

### Toolchain

`pudu init` writes a `system_node_toolchain` into the user's `toolchains/BUCK`, providing `NodeToolchainInfo { node: RunInfo }`. The prelude has no Node toolchain (Python has `system_python_toolchain`; JS has nothing), so pudu supplies one. It lives in the user's repo, so it can be swapped for a hermetic Node without a pudu release.

### First-party consumer (hand-written)

```python
# packages/server/BUCK
load("//third-party/js:pudu.bzl", "node_binary")

node_binary(
    name = "server",
    main = "src/index.js",
    node_modules = "//third-party/js:server_node_modules",
)
```

---

## 9. Fixup schema & the moat

### File layout

```
third-party/js/fixups/          # local (per-repo)
├── sharp/fixups.toml
└── prisma/fixups.toml

pudu-fixups/                    # community registry, separate repo
├── README.md
├── CONTRIBUTING.md
└── packages/
    ├── sharp/fixups.toml
    └── ...
```

### Schema (v1)

```toml
# All top-level keys optional. Apply to all versions × all platforms by default.

# --- Dep overrides ---
extra_deps   = ["//third-party/c:libvips"]
omit_deps    = ["node-gyp"]
replace_deps = { esbuild = "//company/esbuild:esbuild" }

# --- Tarball selection / patching ---
prefer_tarball    = "https://npm.mycorp.example/..."
exclude_platforms = ["linux-x64-musl"]
overlay           = "overlay/"          # dir mirroring the package root

# --- Metadata corrections ---
bin        = { tsc = "bin/tsc.js" }     # override a broken/missing bin map
visibility = ["//some/path/..."]
labels     = ["security-sensitive"]

# --- Runtime env ---
runtime_env = { SHARP_IGNORE_GLOBAL_LIBVIPS = "1" }

# --- Script build (v2 surface; schema is v1) ---
[scripts]
run          = "install"
build_env    = { npm_config_build_from_source = "true" }
build_deps   = ["//third-party/c:libvips"]
extra_native_libs = ["//third-party/c:libjpeg"]

# --- Per-version sections ---
['cfg(version = ">=0.33")']
omit_deps = ["node-addon-api"]

# --- Per-platform sections ---
['cfg(target_os = "linux")']
extra_deps = ["//third-party/c:libvips"]

['cfg(all(target_os = "linux", target_env = "musl"))']
extra_deps = ["//third-party/c:libvips-static"]
```

### Layering rules (community ⊕ local)

Identical to muntjac's, deliberately:

1. **Load order:** community first, local second. Local overrides.
2. **List-valued fields merge:** `extra_deps`, `omit_deps`, `labels`, `exclude_platforms`, `build_deps` → community ∪ local, deduped, community-first order.
3. **Scalar/dict-valued fields replace:** `overlay`, `prefer_tarball`, `bin`, `replace_deps`, `runtime_env`, `[scripts]` subkeys, `visibility` → local wins if set.
4. **Cfg sections evaluate independently** from both layers, applied community → local with the same merge rule.
5. **Escape hatch:** top-level `replace_community = true` in a local fixup disables the community fixup entirely.

### Cfg predicate grammar

Same expression language as reindeer and muntjac: `version = "…"`, `target_os = "…"`, `target_arch = "…"`, `target_env = "…"`, plus `all(...)`, `any(...)`, `not(...)`.

Note the deliberate impedance mismatch: predicates use reindeer's vocabulary (`target_os = "macos"`, `target_env = "musl"`) while `pudu.toml` platforms use npm's (`os = "darwin"`, `libc = "musl"`). Pudu maps between them. Keeping reindeer's vocabulary in fixups is worth the translation cost because fixup authors are the population we most want shared across the three tools.

### Registry pinning & updates

- `pudu.toml` carries `registry_rev = "<git-sha-or-tag>"`. Lockfile-style; no implicit follow-main.
- `pudu fixups update` bumps to `main` (or `--rev <X>`), prints a structured diff, updates the pin.
- Cache at `~/.cache/pudu/fixups/<sha>/`. Content-addressed; multiple revs coexist.
- **Air-gapped:** `registry = "none"` uses only local. `registry = "file:///path"` for a vendored copy.

---

## 10. Rust module layout

```
src/
├── main.rs                 # CLI entrypoint
├── lib.rs                  # internal re-exports
├── cli/
│   ├── mod.rs  init.rs  vendor.rs  buckify.rs
│   ├── audit.rs  fixups.rs  unused.rs
├── config.rs               # pudu.toml parsing
├── lock/
│   ├── mod.rs
│   ├── parser.rs           # pnpm-lock.yaml → typed
│   ├── snapshot_key.rs     # peer-suffix key grammar + mangling
│   ├── types.rs            # Package, Snapshot, Importer, Resolution
│   └── graph.rs            # instance graph construction
├── platform.rs             # os/cpu/libc model; npm field matching; constraint mapping
├── registry.rs             # tarball URL derivation, scope overrides
├── sidecar.rs              # pudu.lock read/write
├── tarball.rs              # fetch, sha512 verify, package.json inspection
├── scripts.rs              # lifecycle-script gate
├── fixup/
│   ├── mod.rs  schema.rs  cfg.rs  layer.rs  loader.rs  registry.rs
├── buck/
│   ├── mod.rs  emit.rs  bzl.rs  config.rs  format.rs
├── cache.rs                # ~/.cache/pudu/
├── advisory.rs             # OSV ingest for `audit` (v2)
└── error.rs                # errors naming (pkg, version, platform) tuples
```

---

## 11. Testing

### Unit tests

Heaviest coverage on the algorithmically fiddly modules:

- `lock/snapshot_key.rs` — peer-suffix parsing, mangling, the >120-char hash path, sort stability.
- `platform.rs` — npm `os`/`cpu`/`libc` matching including negation (`!win32`), constraint-label mapping, conditional-abi logic.
- `fixup/cfg.rs` — predicate parser + evaluator, version comparison edge cases.
- `fixup/layer.rs` — merge rule correctness.
- `registry.rs` — URL derivation for plain and scoped names, scope overrides.

### Snapshot tests (insta)

Each fixture under `tests/fixtures/<scenario>/` holds a minimal `package.json` + `pnpm-lock.yaml` + `pudu.lock` + optional fixups, plus golden output. CI fails on byte mismatch.

v1 fixtures:

1. `01-pure-js` — minimal pure-JS deps, single platform
2. `02-platform-optional` — esbuild's `@esbuild/*` matrix
3. `03-peer-instances` — same package@version at two peer resolutions
4. `04-musl` — a platform pair differing only by libc; asserts the abi constraint appears
5. `05-workspace` — multiple importers sharing one store
6. `06-scoped-registry` — `@myorg` scope override
7. `07-local-fixup` — overlay + omit_deps
8. `08-community-fixup` — registry layering
9. `09-install-script-error` — asserts the error message format
10. `10-determinism` — buckify twice, byte-compare

### End-to-end smoke

`buck2 run //packages/server:server` on the `02-platform-optional` fixture must succeed on Linux x86_64 and macOS arm64, printing output that proves the correct `@esbuild/*` package was selected. Skipped when `buck2` is not on PATH.

### CI matrix

`ubuntu-latest` (x86_64), `ubuntu-24.04-arm` (aarch64), `macos-latest` (arm64).

---

## 12. Open questions & risks

- **~~Lockfile v9 field inventory needs verification.~~ Resolved 2026-08-31.** The [v9 field survey](../research/2026-08-31-pnpm-lock-v9-field-survey.md) confirms `requiresBuild` is absent from all 18 v9 lockfiles examined, so §4's mandatory vendor pass stands. `hasBin` did survive into v9 as a bare boolean — not a bin map, but a useful cross-check against the vendor pass.
- **pnpm lockfile format churn.** v9 has been stable across pnpm 9 and 10, but pnpm moves fast. Mitigation: reject unknown `lockfileVersion` loudly rather than parsing optimistically; CI tests against the min-supported and latest pnpm.
- **Dependency cycles are universal, and constrain how the store may be split.** Every real lockfile surveyed contains cycles (`@babel/core` ↔ `@babel/helper-module-transforms`, `eslint` ↔ `@eslint-community/eslint-utils`). They are harmless here because the store is one `filegroup` whose cycle lives in symlink data, not in the Buck target graph. But S4 must not decompose the store into one target per package depending on its dependencies' targets — that reintroduces the cycle as a Buck target cycle. A split for scale must follow tarball-extraction lines, which are acyclic.
- **`filegroup` scale.** A large workspace's store graph could produce a dict with tens of thousands of entries in one `filegroup`. Unknown whether Buck2's `symlinked_dir` handles that comfortably. S4 must measure on a realistic lockfile; if it degrades, the fallback is per-package `filegroup`s composed into a tree.
- **`.bin` sub-target extraction.** `http_archive` exposes `sub_targets`, but referencing a single file inside the extracted archive for a `.bin` symlink needs confirming — the design assumes `//third-party/js:pkg[bin/foo]` works. S4 validates; fallback is a small genrule per bin entry.
- **Node's symlink realpath behaviour.** Node resolves `node_modules` symlinks to their real paths by default (`--preserve-symlinks` off). Under Buck, the "real path" is buck-out. pnpm relies on the same behaviour and works, so this should hold, but S4's e2e test is the actual proof.
- **~~Bundled dependencies.~~ Resolved 2026-08-31.** pnpm already omits bundled names from the snapshot graph — the bundled package's snapshot carries no `dependencies` at all — so they never become edges and there is no double-install risk. Pudu parses the field and ignores it.
- **Registry URL derivation is a heuristic.** `https://<registry>/<name>/-/<basename>-<version>.tgz` holds for npmjs.org and most mirrors, but private registries (Artifactory, Verdaccio) vary. Mitigation: `pudu.lock` records the resolved URL, so a wrong derivation fails loudly at vendor time and `prefer_tarball` overrides it.
- **Registry repo location undecided.** `github.com/<user>/pudu-fixups` is a placeholder; the real location is a launch-time decision and blocks nothing before the first public release.

---

## 13. Future work (v2+)

Captured, not committed:

- **Vendor mode.** Commit tarballs under `third-party/js/vendor/`; swap `http_archive(urls=…)` for a local source ref. Air-gapped builds. The `pudu.lock` sidecar already carries everything needed.
- **`audit` and `unused`.** OSV cross-check against the GitHub Advisory Database; report vendored tarballs no importer references.
- **Lifecycle-script execution.** Run allowlisted scripts inside a sandboxed Buck rule. Schema hooks already in place (§9 `[scripts]`).
- **Multi-lockfile trees.** muntjac's `[tree.<name>]` model, for repos with genuinely separate lockfiles.
- **Windows.** `win32` os field + Buck2-on-Windows toolchain.
- **`JsLibraryInfo` adapter.** Expose pudu packages to prelude's `js_bundle` for React Native projects.
- **Bazel output backend.** Second emitter under `src/bazel/`.
