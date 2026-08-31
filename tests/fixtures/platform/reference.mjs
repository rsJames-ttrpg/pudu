// Reference oracle for `pudu::platform::admits`, backed by pnpm's own
// matcher. Reads one JSON object per line: {"i": 0, "list": [...] | null,
// "current": "linux", "axis": "os"}. Writes "<i>\ttrue"/"<i>\tfalse" per
// line, echoing the input index back so the Rust side can catch a dropped
// or reordered line even if the line counts happen to still match.
//
// pnpm exposes the rule only through `checkPlatform`, which evaluates three
// axes at once, so each case is asked as a package declaring ONLY the axis
// under test — the other two are left absent and admit everything.
//
// `checkPlatform.js` is not in the package's `exports` map, so it cannot be
// resolved via the package-subpath specifier (`.../lib/checkPlatform.js`
// throws ERR_PACKAGE_PATH_NOT_EXPORTED). Resolve the package's main entry
// point instead, then reach the sibling file by filesystem path — that
// bypasses `exports` entirely, since it's no longer a bare specifier. Same
// pattern as tests/fixtures/lock/real/oracle/engine-excluded.mjs used for
// checkEngine.js.
import readline from 'node:readline';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, 'package.json'));

const mainEntry = require.resolve('@pnpm/package-is-installable');
const checkPlatformPath = path.join(path.dirname(mainEntry), 'checkPlatform.js');
const { checkPlatform } = await import(checkPlatformPath);

// TD-S2-05: pnpm reads the host's libc once at module load, via
// `detect-libc`. On a host it cannot classify (`unknown` — macOS, at time
// of writing) it skips the libc axis entirely: `checkPlatform` never rejects
// on `libc` there, no matter what the case asks for. Left unguarded, that
// makes roughly a third of the corpus (every libc case) disagree, which
// reads as "`admits` is broken" rather than "run this on Linux". Fail loudly
// instead of producing a wall of misleading disagreements.
const { familySync } = await import('detect-libc');
const hostLibc = familySync() ?? 'unknown';
if (hostLibc === 'unknown') {
  process.stderr.write(
    "reference.mjs: this host's libc is not detectable (detect-libc reports " +
      "'unknown', e.g. macOS). pnpm skips the libc axis entirely on such a host, " +
      'so every libc case below would appear to disagree with pudu even though ' +
      "pudu is correct. Run this on a Linux host instead.\n"
  );
  process.exit(1);
}

const rl = readline.createInterface({ input: process.stdin, terminal: false });
const out = [];
for await (const line of rl) {
  if (!line.trim()) continue;
  const { i, list, current, axis } = JSON.parse(line);

  // `supportedArchitectures` pins the "current" value for each axis. The
  // libc axis is additionally gated on the HOST having a detectable libc,
  // so this harness must run on Linux for libc cases to be meaningful.
  const sa = { os: ['linux'], cpu: ['x64'], libc: ['glibc'] };
  sa[axis] = [current];

  const wanted = { os: ['any'], cpu: ['any'], libc: ['any'] };
  wanted[axis] = list === null ? ['any'] : list;

  const verdict = checkPlatform('probe', wanted, sa) === null ? 'true' : 'false';
  out.push(`${i}\t${verdict}`);
}
process.stdout.write(out.join('\n') + '\n');
