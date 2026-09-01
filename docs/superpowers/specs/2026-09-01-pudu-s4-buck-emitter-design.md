# Pudu S4 — First BUCK Emitter (Package Layer) — Design

**Status:** approved 2026-09-01
**Stage:** S4 (roadmap §S4, narrowed — see §2)
**Predecessors:** S3 (`pudu vendor`), S3.5 (the package table)

---

## 1. What a spike established before this spec was written

Every claim in this section was produced by running real `buck2`
(`2026-05-18-3f054b09fb3ddf6e96c8c38f1f21e9420b4215f0`) against real npm
tarballs and real Node v24.6.0, not by reading the prelude. Three of the four
findings contradict the top-level design doc, which is why they lead.

### 1.1 `strip_prefix = root` is broken, and is a shell-injection surface

Design §8 emits `strip_prefix = root`, where `root` is the archive's actual
top-level directory as recorded in `packages.toml`. For `@types/node@22.20.0`
that root is `node v22.20` — it contains a space.

`prelude/http_archive/unarchive.bzl` builds the tar command with
`cmd_args(..., delimiter = " ")` and writes it into a `/bin/sh` script. The
prefix is not quoted. buck2 fails:

```
tar: node: Not found in archive
tar: v22.20: Not found in archive
tar: Exiting with failure status due to previous errors
```

`root` is derived from a package's own tarball (S3, TD-S3-02). Word splitting
is therefore not merely a bug for `@types/*`: it is unquoted, third-party,
attacker-influenced text reaching a shell. The 400-package fixture has 18
archives with a non-`package` root.

### 1.2 The fix is to stop stripping at all

Leave the archive un-stripped and expose the root as a named sub-target:

```python
sub_targets = {"root": ["node v22.20"]}
```

referenced as `//third-party/js:types-node[root]`. This builds clean. No shell
is involved — `unarchive` turns a `sub_targets` path into `output.project(path)`,
a pure artifact projection.

The **dict** form is required. The list form names the sub-target after the
path, and buck2 rejects a provider name containing a space at coercion time:

```
Invalid provider name `node v22.20`. Inner providers names can only contain
non-empty alpha numeric characters, and symbols `,`, `=`, `-`, `/`, `+` and `_`.
```

That message understates what is accepted: `//third-party/js:semver2[bin/dotted.js]`
builds successfully, so `.` is fine in practice. A space is not. §5.4 specifies
what pudu does about names it cannot address.

### 1.3 `.bin` extraction works (design §12 unknown, closed — confirmed)

With no `strip_prefix`, `sub_targets = {"bin/semver": ["package/bin/semver.js"]}`
yields `//third-party/js:semver[bin/semver]`, and a `filegroup` consuming it
produces exactly the symlink `.bin/` needs:

```
node_modules/.bin/semver -> ../../../../__semver2__/semver2/package/bin/semver.js
```

The design's fallback (a genrule per bin entry) is not needed.

### 1.4 Node does **not** resolve through the store (design §12 unknown, closed — refuted)

Design §8 claims:

> Node's own resolution algorithm then works unmodified, because the layout
> **is** pnpm's

It does not. `filegroup(copy = False)` calls `ctx.actions.symlinked_dir`, so
every store leaf is a symlink to its extraction directory. Node resolves
`node_modules` symlinks to their real paths, which lands outside the tree, and
sibling lookup escapes. On a store containing `debug@4.3.4` and its only
dependency `ms@2.1.2`, laid out exactly as §8 prescribes:

```
Error: Cannot find module 'ms'
Require stack:
- .../__app_store__/app_store/node_modules/debug/src/common.js
```

`copy = True` fails too, for the opposite reason: it flattens the symlink that
pnpm's isolation depends on, so `node_modules/debug` becomes a real directory
whose walk-up chain no longer passes through `.pnpm/debug@4.3.4/node_modules/`.

A hand-built genuine pnpm shape — **real directories** at
`.pnpm/<key>/node_modules/<pkg>`, **relative symlinks within the tree** for
siblings and for the top level — resolves correctly. Neither `filegroup` mode
can produce that combination, because copies and intra-tree relative symlinks
must be made by the same action.

`--preserve-symlinks` is not a way out: it makes sibling lookup follow the
*logical* path, which is what the `.pnpm` layout specifically relies on not
happening. Under it the pnpm layout cannot work at all, and the alternative
(a nested npm-style tree) is a different design.

**Consequence: the store layout needs a rule pudu writes, not a `filegroup`.**
That is the reason for §2.

---

## 2. Scope

S4 is narrowed to the **package layer**. It emits the three generated files,
and its exit criterion is that `buck2 build //third-party/js/...` extracts and
verifies every package.

### In scope

- `pudu buckify` and `pudu buckify --check`, replacing the S0 stub.
- `third-party/js/BUCK` — one `npm_package` per `name@version`.
- `third-party/js/pudu.bzl` — the `npm_package` macro.
- `third-party/js/config/BUCK` — one `config_setting` per configured platform.
- Deterministic ordering and formatting, with a test that proves it.
- A gated CI job running `pudu buckify` then `buck2 build` on a fixture.

### Not in scope

| Deferred | To | Why |
|---|---|---|
| `node_modules_tree` and the store layout | S5 | §1.4. Its only real proof is `buck2 run`, which needs `node_binary`, which is S5. Designing it here would commit a shape this stage cannot test. |
| `node_binary`, `node_test` | S5 | Unchanged from the roadmap. |
| The semver-stable bare alias (`//third-party/js:express`) | S5 | Which version a bare name means is a tree question — it depends on what the root importer resolves to. |
| Any `select()` | S5 | `config_setting`s are emitted here; the thing that selects on them is the tree. |
| `filegroup` scale measurement (design §12) | S5 | The question is about a rule S4 no longer emits. |
| The lifecycle-script gate | S6 | Unchanged. `has_install_script` is read from the table and ignored at S4. |
| Fixups | S7 | Unchanged. |

---

## 3. Generated files and ownership

| File | Owner | `buckify` behaviour |
|---|---|---|
| `third-party/js/BUCK` | pudu | overwritten every run |
| `third-party/js/pudu.bzl` | pudu | overwritten every run |
| `third-party/js/config/BUCK` | pudu | overwritten every run |
| `third-party/js/toolchains.bzl` | user | untouched |
| `third-party/js/.gitignore` | user | untouched |
| `third-party/js/fixups/` | user | untouched |

Each of the three generated files opens with the header design §7 and §8
already specify:

```
##
## @generated by pudu
## Do not edit by hand.
##
```

**This is an explicit exception to `init`'s rule.** `src/cli/init.rs` states
that files under `third-party/js/` are "user-owned once they exist — never
overwritten, `--force` or not", and it seeds `BUCK` with the placeholder
`# Generated by pudu. Run: pudu buckify`. That rule was written before any
generated file existed. S4 narrows it: the three files above are pudu's, and
`init`'s comment and the spec text it cites are corrected in the same pass.
`init` continues to seed the placeholder, which `buckify` then replaces.

`config/BUCK` lives in a subdirectory, so `buckify` creates
`third-party/js/config/` if absent.

---

## 4. `pudu.bzl`

Static text — no interpolation of any kind, so it is byte-identical across
every project and carries no escaping risk.

```python
##
## @generated by pudu
## Do not edit by hand.
##

load("@prelude//:rules.bzl", "http_archive")

def npm_package(name, url, sha256, size, root, bin = {}, visibility = None):
    """One registry tarball, extracted and verified by Buck.

    The archive is deliberately NOT stripped. `strip_prefix` is interpolated
    unquoted into a shell command by the prelude, and an archive root is
    third-party data that can contain a space (`@types/node` unpacks to
    `node v22.20`). The root is exposed as the `[root]` sub-target instead,
    which is a pure artifact projection.
    """
    sub_targets = {"root": [root]}
    for bin_name, bin_path in bin.items():
        sub_targets["bin/" + bin_name] = [root + "/" + bin_path]

    http_archive(
        name = name,
        urls = [url],
        sha256 = sha256,
        size_bytes = size,
        type = "tar.gz",
        sub_targets = sub_targets,
        visibility = visibility or ["PUBLIC"],
    )
```

`sha512` is not passed. Buck2 cannot verify it (design §4); it is verified at
vendor time and recorded in `packages.toml` for audit.

---

## 5. `BUCK`

### 5.1 Shape

```python
##
## @generated by pudu
## Do not edit by hand.
##

load("//third-party/js:pudu.bzl", "npm_package")

npm_package(
    name = "@types+node@22.20.0",
    url = "https://registry.npmjs.org/@types/node/-/node-22.20.0.tgz",
    sha256 = "94e07dec077aef58bf4e9837e486c6f63345e1131c69ca294404a740bd9da286",
    size = 447051,
    root = "node v22.20",
)

npm_package(
    name = "semver@7.6.3",
    url = "https://registry.npmjs.org/semver/-/semver-7.6.3.tgz",
    sha256 = "376d2ca2c941fc5a37e9ac3ec65302e5e421e2cc1ee3dee57a854d2bd9bee125",
    size = 27678,
    root = "package",
    bin = {"semver": "bin/semver.js"},
)
```

Every field comes from `packages.toml`. `bin` is omitted when empty rather than
written as `{}`; `visibility` is omitted and left to the macro's `["PUBLIC"]`
default. Facts are inline rather than loaded from a generated data file
(TD-S4-01, decided against in §10).

The `load` label is built from the configured `third_party_dir`, matching how
`init` derives `bzl_label` today.

### 5.2 Target names

The target name is `lock::snapshot_key::target_name(<packages.toml key>)`, not
the key itself. That function already implements pnpm's virtual-store escaping
(`/` → `+`, the Windows-illegal set, the >120-byte hash path), is verified
byte-for-byte against 1363 real `node_modules/.pnpm/` directory names, and
carries a "do not improve this" warning earned by that verification. So
`@types/node@22.20.0` emits as `@types+node@22.20.0`.

buck2 accepts the raw form too — `//third-party/js:@types/node@22.20.0` builds
— but using it would mint a second naming convention one stage before S5 needs
the first one for `.pnpm` paths. Reusing `target_name` keeps the package
targets and the store paths spelled identically, which is the greppability
property that function exists to protect.

### 5.3 Which packages get a target

One target per `name@version` — the `packages.toml` key. Peer instances share a
tarball and therefore collapse onto one target, which is why the table is keyed
this way (design §4).

The set is the **union** of packages surviving platform pruning on any
configured platform. An `http_archive` does not vary by platform; only tree
membership does, and the tree is S5. A macOS-only package must not vanish from
a buckify run on Linux, or the emitted output would depend on the host.

Workspace importers (`link:` / `file:` specifiers) get no target — they are not
packages and have no tarball.

### 5.4 Rendering third-party data into Starlark

`root`, `bin` keys and `bin` values come from a package's own tarball. S4 is the
first stage to write them into generated code, so rendering is a trust
boundary and gets its own module (`src/buck/format.rs`) and its own tests.

- String literals are emitted double-quoted with `\`, `"`, newline, carriage
  return and tab escaped, and non-printable characters escaped numerically.
- A `bin` name that cannot be addressed as a buck2 sub-target — one containing
  a space, a `[`, or a `]` — is a hard error naming the package, the field and
  the value, rather than something emitted for buck2 to reject at parse time
  with no mention of which package caused it. Alphanumerics and
  `, = - / + _ .` are known-good (§1.2).
- The escaper is tested against quotes, backslashes, newlines, non-ASCII, and
  the space-rooted case that motivated it.

This is not hypothetical hardening: §1.1 is the same class of problem one layer
down, and it shipped in the prelude.

---

## 6. `config/BUCK`

One `config_setting` per configured platform, using S2's existing
`constraint_labels`, which already returns sorted labels and already implements
the conditional-abi rule (design §7):

```python
config_setting(
    name = "linux-x64",
    constraint_values = [
        "prelude//cpu/constraints:x86_64",
        "prelude//os/constraints:linux",
    ],
    visibility = ["PUBLIC"],
)
```

The spike built exactly this, plus a musl variant carrying
`prelude//abi/constraints:musl`, confirming all three prelude constraint
settings resolve. No new platform logic is written in S4 — the emitter is a
renderer over `constraint_labels`.

TD-S2-06 (two platforms emitting byte-identical `constraint_values`) does not
bite here: target names come from platform names, which are unique config keys,
so nothing collides. The ambiguity it warns about is a `select()` with two
identical arms, and the `select()` arrives with the tree. Retargeted to S5.

---

## 7. Pipeline, modules, errors

### 7.1 Pipeline

`buckify` adds no new analysis. It reuses S1–S3 whole:

```
context::load_validated  →  Graph::build  →  platform::prune
                                  ↓
                      packages::load + packages::staleness
                                  ↓
                  buck::{config,bzl,emit}  →  three files
```

A missing or stale `packages.toml` fails before anything is emitted, through
S3's existing diagnostic and `ExitCode::Stale` (5). Design §4's "vendor is
mandatory before buckify" becomes literal, with no new error vocabulary for the
case it already contracted.

### 7.2 Modules

```
src/buck/
├── mod.rs      # the Buckify output bundle; ties the three renderers together
├── format.rs   # Starlark literal rendering and sub-target-name validation (§5.4)
├── emit.rs     # BUCK
├── bzl.rs      # pudu.bzl (static text + its header)
└── config.rs   # config/BUCK
src/cli/buckify.rs   # replaces the S0 stub
```

Each renderer returns a `String` and touches no filesystem. `mod.rs` owns
writing, so `--check` compares in memory against what is on disk and shares
every line of rendering with the write path — the two cannot drift.

### 7.3 Errors

| Condition | Diagnostic | Exit |
|---|---|---|
| `packages.toml` missing or stale | existing `VendorError` | 5 |
| `--check` and a generated file differs or is absent | new `BuckError::Stale { path }`, naming the file and `pudu buckify` | 5 |
| A `bin` name is not addressable (§5.4) | new `BuckError::UnrepresentableBinName { package, name }` | 3 |
| Cannot write a generated file | existing I/O error shape | 1 |

`--check` reports the first differing file by path. Both new variants carry a
`code(pudu::buckify::…)` like every other diagnostic. `UnrepresentableBinName`
is `InputInvalid` (3) rather than `Usage` (2): the offending value came from
`packages.toml`, not from the command line.

`ExitCode::Stale`'s doc comment names only `pudu vendor --check`. S4 broadens
the code to `buckify --check`, so that comment is updated in the same pass —
the meaning is unchanged ("regenerate by running pudu"), only its scope.

---

## 8. Testing

### 8.1 Unit

- `format.rs` — escaping (quotes, backslash, newline, tab, non-ASCII), and
  sub-target-name acceptance/rejection including the space case.
- `emit.rs` — ordering; `bin` omitted when empty; the `load` label following
  `third_party_dir`; scoped names emitted through `target_name` (`@types/node`
  → `@types+node`).
- `config.rs` — one `config_setting` per platform; the conditional abi label
  present in a musl pair and absent in a glibc-only config.

### 8.2 The fixture

`tests/fixtures/buck/01-pure-js/` — a real pnpm v9 lockfile with a committed
`packages.toml`, deliberately containing:

| Element | Pins |
|---|---|
| a space-rooted `@types` package | §1.1/§1.2 — without it the whole finding ships untested |
| a package with a `bin` | §1.3 and the `bin/` sub-target path join |
| a scoped package | scope handling in target names and URLs |
| two versions of one package | that targets are keyed `name@version`, not `name` |

It doubles as a buck2 project root (`.buckconfig`, `.buckroot`,
`toolchains/BUCK`, a pinned prelude) so CI builds pudu's actual output rather
than a checked-in copy of it.

### 8.3 Snapshot and integration

- insta snapshots of all three generated files for the fixture.
- **Determinism:** `buckify` twice, byte-compare all three. Not a golden-file
  comparison — a second run of the binary.
- **Scale:** `buckify` over the existing 328-package real lockfile, asserting it
  succeeds and is deterministic, and recording emitted size and wall-clock. That
  fixture holds 18 space-rooted `@types` entries, so it exercises §1.1 at volume
  for free.
- `buckify` with no `packages.toml` → exit 5 through the vendor diagnostic.
- `buckify --check` after hand-editing a generated file → exit 5 naming it.
- `buckify` overwrites `init`'s placeholder `BUCK` (§3).

### 8.4 CI

A gated job, alongside `vendor-oracle` and `platform-fuzz`:

```yaml
  buck2-build:
    name: buck2 build (generated output)
    steps:
      - install buck2 at the facebook/buck2 release tag 2026-08-22
      - vendor the prelude from the same tag (a plain directory, not a submodule)
      - cargo run -- buckify   (in the fixture)
      - buck2 build //third-party/js/...
```

buck2 and its prelude are pinned by the single `facebook/buck2` release tag
`2026-08-22` — one tag for both, not a binary version paired with a
separately-tracked prelude commit — so the binary and the prelude cannot
drift apart. `prelude/` is vendored into the fixture as a plain directory at
CI time (sparse-checkout of that tag), not a git submodule.

This job earned its keep on its first run: `buck::generate` was passing the
*absolute* `third_party_dir` into the `load()` label, so every generated
`BUCK` carried `load("///tmp/.../third-party/js:pudu.bzl", ...)` — a path
buck2's parser rejects outright. The bug survived Tasks 3 and 4 because
`emit.rs`'s unit test supplies a relative label by hand, and no integration
test read the load line back out of the CLI's own output; only a real `buck2
build` against pudu's actual output caught it. Fixed by rejecting an absolute
`third_party_dir` in `buck::generate` with a typed error
(`BuckError::AbsoluteThirdPartyDir`, exit code 3 — a label is cell-relative
and an absolute path cannot be expressed as one at all), with explicit
assertions on the load line in `tests/buckify.rs` and an insta snapshot. Had
§1.1 been a prelude regression rather than a design error, this job is what
would have caught that too; unpinned, either failure mode is what would have
broken mysteriously.

---

## 9. Exit criteria

1. `pudu buckify` on `01-pure-js` emits three files matching golden snapshots.
2. A second `buckify` produces byte-identical output.
3. `buck2 build //third-party/js/...` succeeds on the fixture in CI, including
   the space-rooted `@types` package.
4. `//third-party/js:<pkg>[root]` and `[bin/<name>]` resolve to the package
   directory and the bin script respectively.
5. `buckify` on the 328-package lockfile succeeds, is deterministic, and its
   size and wall-clock are recorded. Measured: 322 packages emitted, 88263
   bytes of `BUCK`, in ~58–61 ms (`cargo test --test buckify_scale --
   --ignored --nocapture`).
6. `buckify --check` exits 5 on drift and 0 on agreement.
7. A missing or stale `packages.toml` fails before any file is written.
8. Sort order is documented and tested.
9. `cargo fmt`, `clippy -D warnings`, `cargo +1.88 check`, and the full suite
   pass.

---

## 10. Documentation and tech-debt fallout

Corrected in the same branch, because a design doc that still asserts §1.4 is
worse than no design doc:

- **design §8** — the `npm_package` snippet is rewritten (no `strip_prefix`,
  dict `sub_targets`). The `node_modules_tree` subsection's claim that Node
  resolution "works unmodified" is replaced by §1.4's evidence and a pointer to
  S5.
- **design §12** — the `.bin` unknown closes as *confirmed* (§1.3); the
  symlink-realpath unknown closes as *refuted* (§1.4); the `filegroup` scale
  unknown moves to S5 with the rule it concerns.
- **roadmap** — S4 narrowed to the package layer, S5 gains the store layout.

Tech debt:

| ID | Action |
|---|---|
| TD-S4-01 | **Closed, decided against.** Facts stay inline in `BUCK`. A `vendor.bzl` dict would put the same several hundred hashes in a third generated file and add a Starlark writer whose escaping must track a reader. `packages.toml` is already the committed, reviewable hash record, so the readability argument is served without it. |
| TD-S3-02 | **Closed.** "S4's codegen must record or recompute each package's actual root — it cannot hardcode `package`" is satisfied: `root` comes from the table, and §1.1 shows hardcoding would have failed loudly rather than silently. |
| TD-S2-06 | Retargeted S4 → S5 (§6). |
| TD-S2-01 | Retargeted S4 → S5. The orphan it describes becomes an unreferenced `http_archive` at S4 — fat, not incorrect — and only matters once a tree references packages. |
| TD-S1-03 | Retargeted S4 → S5. `virtual-store-dir-max-length` governs whether generated names match a real `.pnpm/`, and the tree is what emits those paths. S4 does now call `target_name` (§5.2), so both stages share one convention and the row stays a single decision. |
| TD-S1-06 | Retargeted S4 → S9. Cycle enumeration is a diagnostic; nothing in S4 consumes it. |
| TD-S0-05, -22, -23, -24 | Retargeted S4 → S5. All four are `init`/`pudu.toml` concerns filed at "S4" by proximity rather than dependency. |

New debt is filed as discovered during implementation.
