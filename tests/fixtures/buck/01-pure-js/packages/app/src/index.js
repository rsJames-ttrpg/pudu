// `./greet` is a plain relative require and proves nothing about the tree —
// it resolves next to whichever file Node actually loaded. What proves the
// copies-not-symlinks property is the *bare* `require("debug")` inside
// greet.js: a bare specifier is resolved by walking up from the requiring
// file's directory looking for `node_modules`, so it only succeeds if the
// copied sources sit under the app directory whose `node_modules` symlink
// that walk finds.
const greet = require("./greet");

// `debug` depends on `ms`, so resolving it at all proves sibling lookup
// works through the virtual store — the case S4 design §1.4 could not make
// work with a filegroup.
require("debug").enable("app");

console.log("hello from", greet());
console.log("debug resolved to", require.resolve("debug"));
console.log("ms resolved to", require.resolve("ms", {
  paths: [require("path").dirname(require.resolve("debug"))],
}));
