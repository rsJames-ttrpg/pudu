const debug = require("debug");

const log = debug("app");

module.exports = function greet() {
  log("greet() called");
  return "pudu";
};
