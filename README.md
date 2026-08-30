# pudu

> Translate `pnpm-lock.yaml` into [Buck2](https://buck2.build/) build rules.

## What is pudu?

**Problem.** pnpm is the dependency manager JavaScript monorepos converge on — fast, strict, and correct about peer dependencies. But pnpm doesn't speak Buck. If your monorepo uses Buck2, you've had to either hand-write rules per dependency or shell out to `pnpm install` and give up hermeticity.

**Solution.** pudu reads `pnpm-lock.yaml` plus a small `pudu.toml` and emits `BUCK`, `pudu.bzl`, and `config/BUCK`. Peer-dependency instances, platform-specific `optionalDependencies`, scoped registries, and the pnpm virtual-store layout are all handled.

**Moat.** A community fixup registry, sharing its schema and `cfg()` grammar with [reindeer](https://github.com/facebookincubator/reindeer) and [muntjac](https://github.com/rsJames-ttrpg/muntjac) so fixup authors carry knowledge between all three.

pudu is the world's smallest deer. reindeer does Cargo, muntjac does uv, pudu does pnpm.

## Status

**Pre-implementation.** The design and roadmap are written; code starts at S0. See the [design spec](docs/superpowers/specs/2026-08-30-pudu-design.md) and [roadmap](docs/superpowers/specs/2026-08-30-pudu-roadmap.md).

## Planned quickstart

```sh
cargo install pudu
cd my-pnpm-repo           # contains pnpm-lock.yaml
pudu init                 # writes pudu.toml + third-party/js/ skeleton
pudu vendor               # fetch tarballs, verify integrity, write pudu.lock
pudu buckify              # emit BUCK + pudu.bzl + config/BUCK
buck2 run //packages/server:server
```

Re-run `pudu vendor && pudu buckify` whenever `pnpm-lock.yaml` changes.

## How it works

pudu never runs pnpm. pnpm has already done the hard part — resolution, peer-dependency disambiguation, integrity recording — and written the answer into the lockfile. pudu reimplements pnpm's *layout* rules offline:

1. **Parse** `pnpm-lock.yaml` (lockfileVersion `9.0`) — `importers`, `packages`, `snapshots`.
2. **Build the instance graph**, one node per snapshot key. pnpm encodes peer resolutions in the key itself (`react-dom@18.3.1(react@18.3.1)`), so the same package at one version can appear as several instances with different dependency sets. Each gets its own Buck target.
3. **Prune per platform** using npm's `os` / `cpu` / `libc` fields. This is the only axis of variance — pnpm resolves once, platform-independently. `esbuild` optionally depends on ~20 `@esbuild/*` packages; exactly one survives per platform.
4. **Fetch tarballs once** (`pudu vendor`) to record what the lockfile omits, into a committed `pudu.lock` sidecar.
5. **Emit** a deterministic `BUCK`. Same inputs, byte-identical output.

### Why `pudu vendor` is mandatory

npm records integrity as `sha512-<base64>`. Buck2's `http_archive` accepts `sha1` and `sha256` only. There's no path from one to the other, so pudu does a download pass and records a Buck-usable `sha256` — after verifying the lockfile's `sha512`, so the trust chain holds. That same pass picks up three other things the lockfile doesn't carry: the resolved tarball URL, the package's `bin` map, and whether it declares an install script.

The result is committed and reviewable: the exact hash your build consumes is visible in a diff.

### node_modules

pudu generates the pnpm virtual-store layout directly, as a `filegroup(copy = False)` whose `srcs` dict maps every path in the store to its package artifact:

```python
"node_modules/express": "//third-party/js:express@4.19.2",
"node_modules/.pnpm/express@4.19.2/node_modules/accepts": "//third-party/js:accepts@1.3.8",
```

Node's resolution algorithm then works unmodified, because the layout *is* pnpm's.

### Platforms

npm's three platform fields map 1:1 onto constraint settings that already ship in the Buck2 prelude:

| npm field | Prelude constraint |
|---|---|
| `os` | `prelude//os/constraints:os` |
| `cpu` | `prelude//cpu/constraints:cpu` |
| `libc` | `prelude//abi/constraints:abi` |

So pudu selects on the prelude's own constraints and generates the `config_setting` targets that combine them — no invented constraint universe, no wiring file, nothing to paste into your root `PACKAGE`.

Because `prelude//platforms:default` already sets `os` and `cpu` from the host, a glibc-only configuration needs **zero setup**. The `abi` constraint is emitted only when it's actually discriminating — when you've configured two platforms differing solely by libc, i.e. you actually want Alpine.

## Configuration

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
"@myorg" = "https://npm.mycorp.example"

[fixups]
registry              = "none"
allow_local_overrides = true

[scripts]
allow = []      # lifecycle-script allowlist; empty = block all
```

## Lifecycle scripts

`preinstall` / `install` / `postinstall` are blocked by default — the same stance pnpm 10 takes. Running them means arbitrary package code executing in every build, usually reaching the network, which forfeits the hermeticity you buckified for.

In practice this costs little: esbuild, `@swc/core`, sharp, rollup, lightningcss, and Prisma all ship prebuilt per-platform packages via `optionalDependencies`, which pudu handles natively. When a package genuinely needs a build step, a fixup is the escape hatch.

## Scope

**Supported:** pnpm lockfile v9 (pnpm 9 and 10), Linux x64/arm64 (glibc and musl), macOS arm64, workspaces, peer-dependency instances, scoped registries.

**Not in v1:** Windows; npm/yarn/bun lockfiles; running lifecycle scripts; bundling, TypeScript compilation, or test-runner rules (pudu emits dependency targets, it is not `rules_js`); Bazel output.

## Contributing

Issues and PRs welcome. See [`docs/superpowers/specs/`](docs/superpowers/specs/) for the design specs and [`docs/superpowers/TECH_DEBT.md`](docs/superpowers/TECH_DEBT.md) for the open ledger.

## License

MIT — see [LICENSE](LICENSE).
