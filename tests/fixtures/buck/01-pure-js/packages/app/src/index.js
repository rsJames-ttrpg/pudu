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
