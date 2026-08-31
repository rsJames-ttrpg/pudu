# Real-lockfile fixture

A purpose-built pnpm workspace, installed with real packages from the public
npm registry, used as S1's differential test against pnpm itself.

## Why it exists

`virtual-store-listing.txt` is the **exact set of directory names pnpm created**
in `node_modules/.pnpm/` when installing `pnpm-lock.yaml`. S1's target-name
mangling is a port of pnpm's `depPathToFilename` (S1 spec §5), so pudu must
regenerate every one of these names byte-for-byte. That test catches a
divergence in any of the four naming rules at once, without a runtime
dependency on pnpm.

## Provenance

Generated on 2026-08-31 with **pnpm 10.21.0** by running

```sh
pnpm install --ignore-scripts --frozen-lockfile
```

against the `package.json` files committed beside the lockfile. Every
dependency is a public npm package; nothing here comes from a private project.
`--ignore-scripts` was used, so no package lifecycle script ran during
generation.

At capture: **400 snapshot keys**, **316 virtual-store directories**, and a
perfect bijection — every directory on disk is produced by a snapshot key, and
every non-pruned key produces a directory that exists.

The 84 keys with no directory are optional dependencies pnpm pruned for the
generating platform (linux-x64-gnu) — `@esbuild/android-arm64` and friends.
That is expected, and S2's pruning is what will explain them; the S1 test must
therefore assert over *installed* names, not over all keys.

## What it covers

| property | present |
|---|---|
| workspace importers | 3 (`.`, `packages/app`, `packages/lib`) |
| `workspace:*` specifier | yes (`@fixture/lib`) |
| scoped names | 140 |
| nested peer suffixes | 7 keys |
| **hashed (>120 char) names** | 3 |
| longest snapshot key | 272 chars |
| **npm-aliased edges** | 3, via `@isaacs/cliui` (`string-width-cjs: string-width@4.2.3`) |
| platform-gated packages (`os`/`cpu`) | 26, from `esbuild` |
| dependency cycles | yes (`@babel/core`, `eslint`, `browserslist`) |
| `deprecated` field | yes (`glob@10.4.5`) |

## Regenerating

Re-running the install with a newer pnpm will change both files together.
Regenerate them as a pair, or the differential test is meaningless — and
re-check the coverage table above, since a resolution change can silently drop
the hashed-name or alias cases that make this fixture worth having.

## What this fixture cannot catch

The differential test is strong but not sufficient on its own, and mutation
testing during S1 showed exactly where it stops.

Breaking the hash-truncation stem length (`MAX_LEN_WITHOUT_HASH - 33` → `- 32`)
reddens it immediately, naming the three hashed SvelteKit entries. But
narrowing the escape set from the full `\ / : * ? " < > | #` down to just `/`
**does not fail it** — none of the 400 snapshot keys here contains any of the
other nine characters, because ordinary npm package names never do.

That gap is covered by `escapes_every_illegal_path_character` in
`src/lock/snapshot_key.rs`, a unit test over synthetic inputs, which was
confirmed to fail under the same mutation. The two layers are complementary:

| divergence | caught by |
|---|---|
| truncation length, hash function, hash trigger | this differential test |
| paren flattening, `/` → `+` | this differential test |
| the other nine escaped characters | the unit test |
| uppercase forcing the hash path | the unit test |

**So do not treat a green differential test as proof the port is intact.**
Both layers must stay.

## Per-platform pruning oracles (S2)

`oracle/*.txt` are the same kind of capture as `virtual-store-listing.txt`,
one per platform: the exact directories pnpm created in `node_modules/.pnpm/`
with `supportedArchitectures` pinned. `tests/platform_oracle.rs` asserts pudu
reproduces each set exactly.

| platform | directories |
|---|---|
| linux-x64-gnu | 316 |
| linux-x64-musl | 316 |
| linux-arm64-gnu | 316 |
| darwin-arm64 | 315 |

`oracle/linux-x64-gnu.txt` is byte-identical to `virtual-store-listing.txt`,
which is asserted by a test — S1's fixture was captured on a glibc x86_64
host, and the two must not drift apart.

Captured with **pnpm 10.21.0** and **node v24.6.0** by
`oracle/capture.sh`. Regenerate every file together, including
`engine-excluded.txt`, and update the versions here.

### Why `engine-excluded.txt` exists

pnpm skips an **optional** dependency that fails its `engines` check, so a
virtual-store listing is not a pure *platform* oracle. Pudu deliberately does
not model `engines` — node version is not a platform axis (design §5) — so
its survivor set is a superset of pnpm's by exactly that set, and the test
subtracts it.

At the recorded node version this is one package,
`@napi-rs/lzma-linux-x64-gnu@1.5.1`, which wants
`node: ^22.20 || ^24.12 || >=25`. It is eligible by platform on linux-x64 and
skipped by engines, and it was the single discrepancy when this design was
validated. On node ≥ 24.12 the file is empty — which is correct, not broken.

### What these oracles cannot catch

Every one of the fixture's 90 platform-gated snapshot keys is a **leaf**: none
has dependencies of its own. So these oracles cannot distinguish pudu's
per-package pruning from a design that also sweeps away packages left
unreachable when their only parent is pruned. They show the simpler design is
not wrong here, not that a sweep is unnecessary in general (TD-S2-01).

They also carry no `libc` field and no negation — see the platform matching
survey §2 for why no public-registry install can produce either. That coverage
comes from `tests/platform_fuzz.rs` instead.
