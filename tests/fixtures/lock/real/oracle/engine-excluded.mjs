// Print the snapshot keys pnpm skips for `engines` rather than platform:
// optional dependencies whose `engines` the running node does not satisfy.
//
// Needs `@pnpm/package-is-installable` and `yaml`, resolved from a directory
// passed as argv[2] (capture.sh installs them there with `npm install
// --prefix` into a scratch dir — nothing is committed under tests/).
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

const here = path.dirname(fileURLToPath(import.meta.url));
const resolveRoot = process.argv[2] ?? here;
const require = createRequire(path.join(resolveRoot, 'package.json'));

const YAML = await import(require.resolve('yaml'));
// `checkEngine.js` is not in the package's `exports` map, so it cannot be
// resolved via the package-subpath specifier. Resolve the package's main
// entry point instead, then reach the sibling file by filesystem path —
// that bypasses `exports` entirely, since it's no longer a bare specifier.
const mainEntry = require.resolve('@pnpm/package-is-installable');
const checkEnginePath = path.join(path.dirname(mainEntry), 'checkEngine.js');
const { checkEngine } = await import(checkEnginePath);

const lock = YAML.parse(fs.readFileSync(path.join(here, '..', 'pnpm-lock.yaml'), 'utf8'));
const base = (k) => { const i = k.indexOf('('); return i < 0 ? k : k.slice(0, i); };

const out = [];
for (const [key, snap] of Object.entries(lock.snapshots)) {
  if (snap?.optional !== true) continue;             // only optional deps are skipped
  const meta = lock.packages[base(key)];
  if (!meta?.engines) continue;
  if (checkEngine(key, meta.engines, { node: process.version.slice(1), pnpm: '10.21.0' }) != null) {
    out.push(key);
  }
}
out.sort();
for (const k of out) console.log(k);
