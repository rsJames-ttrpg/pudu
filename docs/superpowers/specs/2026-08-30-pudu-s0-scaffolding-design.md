# S0 — Scaffolding & Config Spec

**Status:** draft v1 (2026-08-30)
**Companion to:** `2026-08-30-pudu-design.md` (full design), `2026-08-30-pudu-roadmap.md` (roadmap)
**Stage:** S0 (first phase-1 stage)

---

## 1. Scope

Stand up the CLI dispatcher, `pudu.toml` parser, error machinery, `pudu init`, and `pudu config check`, so every later stage plugs into a working tool harness. No business logic — no lockfile parsing, no tarball fetching, no BUCK emission. The deliverable is "the tool runs, validates config, scaffolds a project, and is ready to be extended."

**Narrower than muntjac's S0.** The crate skeleton, `Cargo.toml` with resolved dependencies, MIT LICENSE, `.gitignore`, three-runner CI, and the fmt+clippy pre-commit gate all landed in the initial scaffolding commit. S0 does not redo them; it extends CI only if a new job is needed.

Out of scope (later stages): lockfile parsing (S1), platform matching logic (S2), vendor and `pudu.lock` (S3), BUCK emission (S4), script gate (S6), fixups (S7/S8).

---

## 2. CLI surface (S0 slice)

Implemented:

```
pudu init [--force] [PATH]         # scaffold a project
pudu config check [-C PATH]        # validate pudu.toml in isolation
                  [--format json]
pudu debug                         # subcommand group; no children at S0
pudu --help                        # lists all phase-1 verbs
pudu --version                     # version + build identifier
```

Registered but stubbed — each prints `error: pudu <verb> is not implemented yet (planned for S<n>); see docs/superpowers/specs/` and exits 2:

```
pudu vendor        # UNIMPLEMENTED (S3)
pudu buckify       # UNIMPLEMENTED (S4)
pudu fixups …      # UNIMPLEMENTED (S7/S8)
pudu audit         # UNIMPLEMENTED (Phase 2)
pudu unused        # UNIMPLEMENTED (Phase 2)
```

Registering the full surface from day one keeps `--help` honest about where the project is going, and lets the help snapshot test lock verb names before any of them have behaviour to rename.

Global flags (registered in S0, honored where meaningful):

```
-C <path>          # cd into a pudu project before running
-v, -vv            # log verbosity
--no-network       # forbid network access (no-op until S3)
--check            # assert on-disk artifacts match what would be regenerated
                   #   (accepted on vendor/buckify; no-op until S3)
```

Deliberately **absent**, and why:

- **No `--frozen`.** muntjac has it because it may shell out to `uv lock`. Pudu never runs pnpm (design §2), so there is nothing to freeze. Adding the flag would imply a capability that does not exist.
- **No `--tree`.** Multi-lockfile support is v2 (design §12). pnpm workspaces already cover the common case via `importers`, so unlike muntjac there is no multi-tree schema to parse ahead of time.

---

## 3. `pudu init` behaviour

### Detection algorithm

1. Walk from CWD upward looking for `pnpm-lock.yaml`. Stop at the first match or at filesystem root. **The lockfile is the anchor**, not `package.json` — the lockfile is pudu's actual input, and a repo can hold many `package.json` files but exactly one lockfile per workspace.
2. If found, look for `pnpm-workspace.yaml` in the same directory.
3. Derive the platform matrix (below).
4. If no lockfile is found, fall back to the undetected template (§3.4).

### Platform derivation from `supportedArchitectures`

pnpm lets a workspace declare the architectures it installs for. That is exactly pudu's platform matrix, and it is information the user has already given pnpm — so pudu's targets match what they actually install for, rather than a guess. This mirrors muntjac deriving `python_versions` from `requires-python`.

```yaml
# pnpm-workspace.yaml
supportedArchitectures:
  os: [linux, darwin]
  cpu: [x64, arm64]
  libc: [glibc]
```

Expansion rules:

- Cross-product `os × cpu`.
- **`libc` applies only to `linux`.** A `darwin-musl` or `darwin-glibc` platform is meaningless and must be filtered out, not emitted. macOS platforms carry no libc.
- If `libc` is absent or `[glibc]`, linux platforms get `libc = "glibc"`.
- **`win32` is skipped with a warning** naming Windows as a v1 non-goal (design §1) and pointing at the roadmap. Skipping loudly beats silently emitting a platform that cannot work.
- pnpm's **`current` keyword** resolves to the host's value for that axis.
- If the expansion yields zero usable platforms (e.g. `os: [win32]` only), error rather than writing an empty `[platforms]` table.

Default when `supportedArchitectures` is absent:

```toml
[platforms.linux-x64-gnu]    os = "linux",  cpu = "x64",   libc = "glibc"
[platforms.linux-arm64-gnu]  os = "linux",  cpu = "arm64", libc = "glibc"
[platforms.darwin-arm64]     os = "darwin", cpu = "arm64"
```

Platform naming convention: `<os>-<cpu>` for macOS, `<os>-<cpu>-<libc-short>` for Linux (`gnu` / `musl`), matching the design's examples.

### Files written

`pudu init` is invoked at the repo root (or the directory given by `[PATH]`), and
writes `pudu.toml` there — beside `pnpm-lock.yaml`, matching the layout in design §3:

```
pudu.toml
third-party/js/
├── BUCK              # placeholder: "# Generated by pudu. Run: pudu buckify"
├── toolchains.bzl    # defines system_node_toolchain (pudu-owned)
├── .gitignore        # ignores vendor/ ahead of v2 vendor mode
└── fixups/.gitkeep
toolchains/BUCK       # appended, marker-delimited (§3.3)
```

### Node toolchain handling

The Buck2 prelude ships `system_python_toolchain` but **no Node equivalent**, so pudu supplies one. `.buckconfig` conventionally declares `toolchains` as a cell (`[cells] toolchains = toolchains`), so the target resolves as `toolchains//:node`.

Ownership is split deliberately: the **rule definition** lives in pudu-owned `third-party/js/toolchains.bzl`; only the **instantiation** goes into the user's `toolchains/BUCK`, loaded across the cell boundary with an explicit `root//` prefix:

```python
# --- begin pudu-managed (do not edit inside this block) ---
load("root//third-party/js:toolchains.bzl", "system_node_toolchain")
system_node_toolchain(name = "node", visibility = ["PUBLIC"])
# --- end pudu-managed ---
```

**This is a deliberate divergence from muntjac**, whose fixture states plainly that "the toolchains/ tree is committed but its contents are not regenerated by muntjac — they live alongside .buckconfig in the user's project skeleton." Pudu writes into a user-owned file because there is no prelude Node toolchain to lean on, so declining to write it would push a mandatory manual step onto every single user. The cost is that S0 must carry safety machinery muntjac never needed:

| Situation | Behaviour |
|---|---|
| Markers present | Replace block contents **only** with `--force`; otherwise leave it and report that it is current |
| No markers, but a node toolchain already exists in the file | **Never append.** Report the target found and record it in `pudu.toml` |
| Neither | Append the block |
| `toolchains/BUCK` missing | Create it, containing only the marked block |
| `toolchains/BUCK` unparseable | Do not touch it; print the block for manual pasting and continue |

Invariants:

- **Nothing outside the markers is ever modified.** Detection is textual, and the write path only ever replaces the span between markers or appends after EOF.
- **Idempotent.** Three consecutive `pudu init` runs produce a byte-identical `toolchains/BUCK`. Asserted in tests.
- The resolved label is recorded as `[buck] node_toolchain = "toolchains//:node"` rather than hardcoded in the emitter, which doubles as the escape hatch for a user with their own toolchain.

"A node toolchain already exists" is detected by scanning for a `system_node_toolchain(` call or a target named `node` at the top level of the file. Deliberately conservative: a false positive costs one printed line of manual instruction, while a false negative silently produces a duplicate target and a confusing Buck error.

### Starter `pudu.toml` (detected case)

```toml
# Generated by `pudu init`. Edit freely.
# Full schema: docs/superpowers/specs/2026-08-30-pudu-design.md

lockfile_path   = "pnpm-lock.yaml"          # detected at init time
third_party_dir = "third-party/js"

# Derived from supportedArchitectures in pnpm-workspace.yaml.
[platforms.linux-x64-gnu]
os   = "linux"
cpu  = "x64"
libc = "glibc"

[platforms.darwin-arm64]
os  = "darwin"
cpu = "arm64"

[registry]
default = "https://registry.npmjs.org"

[fixups]
# Community fixup registry. Leave as "none" until a v0.1.0+ release exists.
registry              = "none"
allow_local_overrides = true

[scripts]
# Packages whose lifecycle scripts are acknowledged (not run). See design §6.
allow = []

[buck]
file_name      = "BUCK"
node_toolchain = "toolchains//:node"
```

### Starter `pudu.toml` (undetected case)

Same shape, with `lockfile_path = "TODO: path to your pnpm-lock.yaml"`, the default platform set, and a banner:

```toml
# TODO: pudu init could not find a pnpm-lock.yaml.
# Edit `lockfile_path`, then run `pudu config check`.
```

### Overwrite handling

- `pudu.toml` exists → `error: pudu.toml already exists; pass --force to overwrite`, exit 2.
- `third-party/js/` exists and is non-empty → warn, write nothing inside it, still write `pudu.toml`.
- `toolchains/BUCK` → governed by the table in §3.3, which `--force` also affects.
- `--force` bypasses the first two checks.

---

## 4. `pudu config check` behaviour

Reads `pudu.toml` (from `-C <path>` or CWD) and validates with no side effects:

1. TOML parses. Errors carry line/column.
2. Required keys present: `lockfile_path`, `third_party_dir`, a non-empty `[platforms]`.
3. `lockfile_path` resolves to an existing file; a failure names the resolved absolute path.
4. `third_party_dir` exists or is creatable (write-test: create a temp file, remove it).
5. Every platform is valid: known `os`, known `cpu`, `libc` present only on linux, no duplicate names, and no two platforms resolving to an identical `(os, cpu, libc)` triple.
6. `constraints` entries, where present, look like `cell//path:target`.
7. `[registry]` values are absolute `http(s)` URLs; scope keys start with `@`.
8. `[fixups].registry` is `"none"`, `"file://<path>"`, or matches `^github\.com/[^/]+/[^/]+$`.
9. `[scripts].allow` entries are valid npm package names (including scoped).
10. `[buck].node_toolchain` looks like a Buck target label.

Exit non-zero on any failure. On success:

```
pudu.toml ok: 3 platforms (linux-x64-gnu, linux-arm64-gnu, darwin-arm64)
```

`--format json` emits `{ok: bool, errors: [...], warnings: [...]}` for CI.

**Warnings, not errors:** a single configured platform (legal, but usually a mistake); `[fixups].registry` set with no `registry_rev` pin.

---

## 5. Config types

Typed enums rather than strings, so validation falls out of deserialization and S2's npm-field matching gets exhaustive `match` coverage rather than string comparison:

```rust
pub struct Config {
    pub lockfile_path:   PathBuf,
    pub third_party_dir: PathBuf,
    pub platforms:       BTreeMap<String, Platform>,
    pub registry:        RegistryConfig,
    pub fixups:          FixupsConfig,
    pub scripts:         ScriptsConfig,
    pub buck:            BuckConfig,
}

pub struct Platform {
    pub os:          Os,                     // Linux | Darwin  (Win32 rejected in v1)
    pub cpu:         Cpu,                    // X64 | Arm64
    pub libc:        Option<Libc>,           // Glibc | Musl — Linux only
    pub constraints: Option<Vec<String>>,    // escape hatch, design §7
}

pub struct RegistryConfig {
    pub default: Url,
    pub scopes:  BTreeMap<String, Url>,      // "@myorg" → registry URL
}

pub struct FixupsConfig {
    pub registry:              FixupRegistry,   // None | File(PathBuf) | Github(owner, repo)
    pub registry_rev:          Option<String>,
    pub allow_local_overrides: bool,            // default true
}

pub struct ScriptsConfig { pub allow: BTreeSet<String> }

pub struct BuckConfig {
    pub file_name:      String,              // default "BUCK"
    pub node_toolchain: String,              // default "toolchains//:node"
}
```

`Os`, `Cpu`, and `Libc` live in `src/platform.rs` — S0 defines the types and their serde impls; S2 adds the npm-field matching and constraint-label mapping that consume them.

`BTreeMap`/`BTreeSet` throughout, not `HashMap`, so iteration order is deterministic — a precondition for the byte-stable output the design requires (§5 invariants).

---

## 6. Error machinery

CLI-level errors use `anyhow`; library-internal errors use `thiserror`-derived enums per module so tests can assert on variants rather than message text. `main` renders the final error through `miette` for line/column and source-span display.

S0 implements `ConfigError` in `src/error.rs` with variants covering the §4 validation classes: `Parse(toml::de::Error)`, `MissingField`, `BadPlatform`, `LibcOnNonLinux`, `DuplicatePlatform`, `BadConstraintLabel`, `BadRegistryUrl`, `BadFixupRegistry`, `BadPackageName`, `LockfileNotFound`, `ThirdPartyDirNotWritable`.

**Contract, enforced by snapshot tests:** every error message names a field or a file by path, and carries line/column where the source position is known.

`miette` integration stays minimal at S0 — message plus position. Rich source-span rendering is deferred to S7, where the fixup `cfg()` parser needs it far more.

---

## 7. Module layout (S0 slice)

```
src/
├── main.rs               # entrypoint; wires clap → cli/
├── lib.rs                # pub mod cli, config, error, platform
├── cli/
│   ├── mod.rs            # clap derive structs, dispatcher
│   ├── init.rs           # pudu init (incl. toolchain append logic)
│   ├── config_check.rs   # pudu config check
│   ├── debug.rs          # debug group, no children at S0
│   └── stub.rs           # UNIMPLEMENTED verb registration
├── config.rs             # pudu.toml parsing + Config
├── platform.rs           # Os/Cpu/Libc types (matching logic lands in S2)
└── error.rs              # error types + diagnostic rendering
```

No other modules are created. `lib.rs` re-exports only what S0 needs.

Note `init.rs` will be the largest file, since it carries detection, templating, and the toolchain-append state machine. If it passes ~350 lines, split the toolchain logic into `cli/toolchain.rs` — the append state machine is independently testable and has a clean boundary.

---

## 8. Testing

### Unit tests

- **`config.rs`** — parses a valid config; rejects each §4 validation class; round-trips parse → serialize → parse to an identical `Config`.
- **`platform.rs`** — `Os`/`Cpu`/`Libc` serde round-trips; rejects unknown values with a message listing the valid ones.
- **`cli/init.rs`** — `supportedArchitectures` expansion, specifically: the darwin×libc filter, the `win32` skip-with-warning, `current` resolution, an all-`win32` input erroring, and the absent-key default.
- **toolchain append** — each row of the §3.3 table, plus the idempotency assertion (three runs, byte-identical result) and the never-touch-outside-markers invariant.
- **`error.rs`** — a TOML parse error carries line/column; a missing-field error names the field.

### Integration tests (`tests/`)

- `tests/init.rs` — spawn `pudu init` in a tempdir; verify files and contents; detection from a subdirectory; refusal without `--force`; the `[PATH]` argument.
- `tests/config_check.rs` — good config exits 0; each corrupted field exits non-zero with the expected error class; `--format json` shape.
- `tests/help.rs` — snapshot of `pudu --help`, locking the verb surface.

Shared helpers in `tests/common/mod.rs`.

### CI

The existing three-runner workflow covers S0 unchanged. No new jobs.

---

## 9. Exit criteria

1. `pudu init` in an empty dir writes `pudu.toml` plus the `third-party/js/` skeleton.
2. `pudu init` from a subdirectory of a pnpm repo finds `pnpm-lock.yaml` up-tree and writes a correct relative `lockfile_path`.
3. `supportedArchitectures` expansion is correct, including the darwin-libc filter, the `win32` warning, and `current` resolution.
4. `pudu init` refuses to overwrite `pudu.toml` without `--force`.
5. The toolchain append satisfies every row of §3.3; three consecutive runs leave `toolchains/BUCK` byte-identical; a pre-existing node toolchain is left untouched and recorded in `pudu.toml`.
6. `pudu --help` lists all phase-1 verbs with unimplemented ones marked; `pudu --version` prints version plus a build identifier.
7. `pudu config check` accepts a good config and rejects each of the ten §4 classes with an error naming the field or file.
8. `pudu debug` with no subcommand errors and exits 2; every stubbed verb reports its planned stage and exits 2.
9. CI green on all three runners: build, test, clippy `-D warnings`, fmt `--check`.
10. Snapshot tests pass, and running them twice produces no diff.

---

## 10. Open questions / risks

- **`NodeToolchainInfo` provider shape.** Pudu defines the toolchain rule itself, so the provider is ours to design. S0 mirrors `system_python_toolchain`'s pattern (a `RunInfo`-bearing field resolving a command name or absolute path). The exact field set should be confirmed against `@prelude//toolchains:python.bzl` during implementation, since S5's `node_binary` consumes it and changing it later is a breaking change to generated code.
- **`supportedArchitectures` schema.** This spec assumes the keys are `os`, `cpu`, `libc`, with a `current` sentinel. S0 must confirm against pnpm's documentation rather than assume; an unrecognized key should warn and be ignored rather than error, so a future pnpm addition doesn't break `pudu init`.
- **Node version.** Pudu configures *which node binary* runs, not which version. A package's `engines.node` field is ignored in v1. Worth revisiting if S5's e2e surfaces version-sensitivity.
- **Textual toolchain detection.** Scanning `toolchains/BUCK` as text rather than parsing Starlark is deliberate — a real parser is far out of scope — but it means a `system_node_toolchain` call inside a conditional or a macro is invisible. The conservative bias (§3.3) means the failure mode is a duplicate-target error at Buck time, not silent corruption. Documented, accepted.
- **Relative path fragility.** Both `lockfile_path` and `third_party_dir` are written relative to `pudu.toml`'s own directory (the repo root, per design §3). Moving `pudu.toml` breaks both. `config check` catches this immediately with the resolved absolute path in the message, which is the cheapest available mitigation.

---

## 11. Follow-ups (not S0)

- S1 adds `pudu debug print-graph` under the `debug` namespace.
- S2 adds `pudu debug platforms`, and extends `src/platform.rs` with npm-field matching plus constraint-label mapping.
- S3 makes `--no-network` and `--check` meaningful.
- S9 revisits `--help` formatting and the version string for the v0.1.0 polish pass.
- `pudu init --interactive` is deliberately deferred; revisit only if init's detection proves insufficient in practice.
