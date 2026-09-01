#!/usr/bin/env node
// Capture the registry's own view of every package in the real fixture:
// the tarball URL, the bin map, and whether an install script runs.
//
// The expectations are computed here, in JavaScript, by a port of
// @pnpm/package-bins and @pnpm/building.pkg-requires-build. That makes the
// oracle an independent implementation rather than a recording of pudu's
// output. See docs/superpowers/research/2026-08-31-npm-tarball-vendor-survey.md.
//
// Two limits, both deliberate:
//   * `directories.bin` cannot be resolved from a manifest — it needs the
//     archive. Such packages are recorded with bin: null and skipped by the
//     test. None exist in this fixture.
//   * `pkgRequiresBuild`'s file-list trigger (a root `binding.gyp`, or
//     anything under `.hooks/`) is likewise invisible to a manifest fetch,
//     since it needs the archive's file listing. There is no manifest field
//     that reliably substitutes for it: an earlier version of this script
//     used the manifest's `gypfile` flag as a proxy, but `fsevents@2.3.3`
//     disproves that equivalence — see the note on that package below. So
//     this oracle answers `has_install_script` from `scripts` alone and
//     under-counts any package that relies solely on the file-list trigger.
//     None are known to exist in this fixture, but the possibility is
//     unclosed, unlike the `directories.bin` case, which the test can detect
//     and skip.
//
// `fsevents@2.3.3` is a known, verified exception, kept in the output as
// captured rather than patched: the registry manifest at
// https://registry.npmjs.org/fsevents/2.3.3 reports `scripts.install:
// "node-gyp rebuild"` and `gypfile: true`, but the published tarball at
// `dist.tarball` (sha512 verified against `dist.integrity`) contains
// *neither* an `install` script nor a `binding.gyp` file — it ships a
// prebuilt `fsevents.node` binary instead, so no build step actually runs.
// pnpm's real `pkgRequiresBuild(manifest, filesIndex)` (@pnpm/building.pkg-
// requires-build, called from pnpm11/worker/src/start.ts with a manifest and
// files index both read back from the extracted tarball, never the registry
// API) would see the tarball's package.json, not this manifest, and would
// therefore compute `false`. The registry's own packument is simply stale or
// wrong for this one field on this one version; `tests/vendor_oracle.rs`
// documents and special-cases it rather than treating pudu's tarball-derived
// `false` as a bug.
//
// Usage: node capture-manifests.mjs > manifests.json

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const lock = fs.readFileSync(path.join(here, '..', 'pnpm-lock.yaml'), 'utf8');

const keys = [];
let inPackages = false;
for (const line of lock.split('\n')) {
  if (line === 'packages:') { inPackages = true; continue; }
  if (line === 'snapshots:') { inPackages = false; continue; }
  if (!inPackages) continue;
  const m = line.match(/^ {2}'?([^ ']+)'?:$/);
  if (m) keys.push(m[1]);
}

function commandName(raw) {
  return raw[0] === '@' ? raw.slice(raw.indexOf('/') + 1) : raw;
}

function normalize(rel) {
  const out = [];
  for (const part of rel.split('/')) {
    if (part === '' || part === '.') continue;
    if (part === '..') { if (out.pop() === undefined) return null; continue; }
    out.push(part);
  }
  return out.length ? out.join('/') : null;
}

function bins(manifest) {
  if (manifest.bin === undefined || manifest.bin === null) {
    return manifest.directories?.bin ? null : {};
  }
  const pairs = typeof manifest.bin === 'string'
    ? [[manifest.name, manifest.bin]]
    : (typeof manifest.bin === 'object' ? Object.entries(manifest.bin) : []);
  const out = {};
  for (const [rawName, rawPath] of pairs) {
    if (typeof rawPath !== 'string') continue;
    const name = commandName(rawName);
    if (name !== encodeURIComponent(name) && name !== '$') continue;
    const p = normalize(rawPath);
    if (p === null) continue;
    out[name] = p;
  }
  return out;
}

const out = [];
let next = 0;
async function worker() {
  while (next < keys.length) {
    const key = keys[next++];
    const at = key.lastIndexOf('@');
    const name = key.slice(0, at);
    const version = key.slice(at + 1);
    const res = await fetch(
      `https://registry.npmjs.org/${name.replace('/', '%2f')}/${version}`
    );
    if (!res.ok) throw new Error(`${key}: HTTP ${res.status}`);
    const m = await res.json();
    const s = m.scripts ?? {};
    out.push({
      key,
      url: m.dist.tarball,
      bin: bins(m),
      has_install_script: Boolean(s.preinstall || s.install || s.postinstall),
    });
  }
}
await Promise.all(Array.from({ length: 12 }, worker));
out.sort((a, b) => (a.key < b.key ? -1 : 1));
process.stdout.write(JSON.stringify(out, null, 1) + '\n');
