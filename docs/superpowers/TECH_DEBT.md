# Pudu Tech Debt Ledger

Items discovered during stage reviews that were deliberately deferred. Each
stage's brainstorm should skim this for items targeted at the upcoming stage;
the planner folds them in as explicit task items.

| ID | Opened | Target | Description |
|---|---|---|---|
| TD-S0-01 | 2026-08-30 | S9 | `ConfigError::NoPlatforms`'s message ("no platforms configured") names neither a field nor a file, a literal miss against the error-message contract stated in `src/error.rs`'s own doc comment. |
| TD-S0-02 | 2026-08-30 | S2 | `Libc::as_npm()` and `Libc::short()` have no direct unit test. A swapped `gnu`/`musl` mapping is currently caught only indirectly, via Task 7's platform-name assertions. |
| TD-S0-03 | 2026-08-30 | S8 | `[fixups] registry = "file://"` parses to `File("")` and `file://relative` to `File("relative")`; validation should reject an empty or relative registry path. |
| TD-S0-04 | 2026-08-30 | S9 | Because `RawRegistry` uses `#[serde(flatten)]`, a typo'd `[registry]` key (`defualt = …`) is silently collected as a scope. `BadRegistryScope` (every scope must start with `@`) is the only backstop. |
| TD-S0-05 | 2026-08-30 | S4 | Neither `Platform` nor `Config` derives `Serialize`, and `FixupRegistry` has no `Display` back to its `github.com/owner/repo` form. `pudu init` hand-renders `pudu.toml`; a round-trip serializer would be sturdier. |
| TD-S0-06 | 2026-08-30 | S1 | Stub stage labels for `fixups` (S7/S8) and `unused` (Phase 2) are asserted by no test; only `vendor`, `buckify`, and `audit` are covered. |
| TD-S0-07 | 2026-08-30 | S1 | `#[allow(dead_code)]` on the three `tests/common/mod.rs` helpers is inert today — all three are used. Drop it if it stays unnecessary. |
| TD-S0-08 | 2026-08-30 | S2 | A non-sequence `supportedArchitectures` axis (`os: linux` rather than `os: [linux]`) errors with a misleading message and no warning; a non-mapping `supportedArchitectures` block is ignored silently. |
| TD-S0-09 | 2026-08-30 | S2 | The unknown-`os`/`cpu`/`libc` **value** warning arms have no test, and `axis()` silently drops non-string entries (`os: [123]`). |
| TD-S0-10 | 2026-08-30 | S5 | CRLF `toolchains/BUCK` files gain one stray blank line on the first `--force` (the `\r` is not consumed by the newline check). Self-converges on the second run. |
| TD-S0-11 | 2026-08-30 | S5 | `AppendOutcome::ExistingToolchain(String)` always carries the hardcoded `"node"` rather than the user's actual target name, so `pudu init`'s guidance message always prints `:node`. |
| TD-S0-12 | 2026-08-30 | S5 | Toolchain detection widened to find-then-check-remainder now false-positives on `not_system_node_toolchain(` (no left-boundary check). Safe direction — it refuses to write — but closable by checking the preceding character. |
| TD-S0-13 | 2026-08-30 | S5 | `AlreadyManaged` reports on marker presence, not block-content equality, so a stale block from an older pudu is reported as present until someone passes `--force`. |
| TD-S0-14 | 2026-08-30 | S5 | Spec §3.3 says toolchain detection covers "a `system_node_toolchain(` call **or a target named `node`**"; only the first half is implemented. |
| TD-S0-15 | 2026-08-30 | S5 | The replace path's span still uses first-occurrence `find` offsets, safe only because the marker-count gate guarantees uniqueness. Add a comment if that gate is ever relaxed. |
