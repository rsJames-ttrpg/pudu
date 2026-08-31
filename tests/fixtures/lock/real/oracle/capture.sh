#!/usr/bin/env bash
# Regenerate the per-platform pruning oracles.
#
# Each oracle is the exact set of directory names pnpm creates in
# `node_modules/.pnpm/` when installing this fixture's lockfile with
# `supportedArchitectures` pinned to one platform. `tests/platform_oracle.rs`
# asserts pudu reproduces each set exactly.
#
# Requires pnpm and node on PATH. Run from anywhere:
#     ./tests/fixtures/lock/real/oracle/capture.sh
#
# IMPORTANT: regenerate ALL files together, including engine-excluded.txt,
# and update the versions recorded in ../README.md. An oracle captured with
# one node version and an exclusion list from another is silently wrong.
#
# Node-dependency note: engine-excluded.mjs needs `@pnpm/package-is-installable`
# and `yaml`, neither of which is a pudu/npm dependency of this fixture. Rather
# than committing a node_modules/package.json under tests/, this script
# installs both packages into a throwaway scratch directory (outside the repo,
# under $TMPDIR) with `npm install --prefix`, passes that directory to
# engine-excluded.mjs, and deletes it afterwards. Nothing under tests/ is
# ever written by this step.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$(dirname "$HERE")"

capture() {
  local out="$1" os="$2" cpu="$3" libc="$4"
  local w
  w="$(mktemp -d)"
  cp -r "$SRC"/. "$w"/
  rm -rf "$w/node_modules" "$w"/packages/*/node_modules "$w/oracle"
  cat > "$w/pnpm-workspace.yaml" <<YAML
packages:
  - "packages/*"
supportedArchitectures:
  os:
    - $os
  cpu:
    - $cpu
  libc:
    - $libc
YAML
  ( cd "$w" && pnpm install --ignore-scripts --frozen-lockfile >/dev/null 2>&1 )
  if ! diff -q "$SRC/pnpm-lock.yaml" "$w/pnpm-lock.yaml" >/dev/null; then
    echo "FATAL: the lockfile drifted capturing $os/$cpu/$libc" >&2
    exit 1
  fi
  ls "$w/node_modules/.pnpm" | grep -vx 'node_modules' | grep -vx 'lock.yaml' \
    | LC_ALL=C sort > "$HERE/$out"
  rm -rf "$w"
  echo "  $out: $(wc -l < "$HERE/$out") directories"
}

echo "pnpm $(pnpm --version), node $(node --version)"
capture linux-x64-gnu.txt   linux  x64   glibc
capture linux-x64-musl.txt  linux  x64   musl
capture linux-arm64-gnu.txt linux  arm64 glibc
capture darwin-arm64.txt    darwin arm64 glibc

# pnpm skips an OPTIONAL dependency that fails `engines`, so the listings
# above are not a pure platform oracle. Pudu does not model `engines`
# (node version is not a platform axis), so the test subtracts this set.
# It depends on the node version used here — regenerate it with the rest.
#
# engine-excluded.mjs needs `@pnpm/package-is-installable` and `yaml`, which
# are not fixture dependencies. Install them into a scratch dir outside the
# repo and point the script's module resolution at it, so nothing under
# tests/ ever gets a node_modules or package.json committed.
DEPS_DIR="$(mktemp -d)"
trap 'rm -rf "$DEPS_DIR"' EXIT
npm install --prefix "$DEPS_DIR" --no-save --silent \
  @pnpm/package-is-installable yaml >/dev/null 2>&1

node "$HERE/engine-excluded.mjs" "$DEPS_DIR" > "$HERE/engine-excluded.txt"
echo "  engine-excluded.txt: $(wc -l < "$HERE/engine-excluded.txt") key(s)"
