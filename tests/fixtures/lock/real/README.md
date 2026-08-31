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
