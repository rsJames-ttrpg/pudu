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
