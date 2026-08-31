use std::path::Path;

use assert_cmd::Command;

pub fn pudu(dir: &Path) -> Command {
    let mut c = Command::cargo_bin("pudu").expect("binary builds");
    c.current_dir(dir);
    c
}

// Load-bearing, not decoration (TD-S0-07): every file under `tests/`
// compiles as its own crate, and `tests/init.rs` includes this module but
// uses only `pudu()`. Without the allow, that crate fails `-D warnings` with
// `constant GOOD_CONFIG is never used`. Verified by removal.
#[allow(dead_code)]
pub const GOOD_CONFIG: &str = r#"
lockfile_path   = "pnpm-lock.yaml"
third_party_dir = "third-party/js"

[platforms.linux-x64-gnu]
os   = "linux"
cpu  = "x64"
libc = "glibc"

[platforms.darwin-arm64]
os  = "darwin"
cpu = "arm64"
"#;

/// A tempdir containing a lockfile and, optionally, a `pudu.toml`.
///
/// (Unused by the `tests/init.rs` crate — see the note on `GOOD_CONFIG`
/// above for why the allow has to stay.)
#[allow(dead_code)]
pub fn project(config: Option<&str>) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    if let Some(c) = config {
        std::fs::write(d.path().join("pudu.toml"), c).unwrap();
    }
    d
}

/// A tempdir with a `pudu.toml` whose `lockfile_path` points at a copy of
/// `fixture_dir`'s `pnpm-lock.yaml`.
///
/// (Unused by some test crates depending on which of this module's helpers
/// they exercise — see the note on `GOOD_CONFIG` above for why the allow has
/// to stay.)
#[allow(dead_code)]
pub fn scratch_with_config(fixture_dir: &Path) -> tempfile::TempDir {
    let lock_text = std::fs::read_to_string(fixture_dir.join("pnpm-lock.yaml"))
        .expect("fixture lockfile is readable");
    scratch_with_lockfile(&lock_text)
}

/// A tempdir with a `pudu.toml` plus a `pnpm-lock.yaml` containing `text`.
#[allow(dead_code)]
pub fn scratch_with_lockfile(text: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("pnpm-lock.yaml"), text).unwrap();
    std::fs::write(
        d.path().join("pudu.toml"),
        "lockfile_path = \"pnpm-lock.yaml\"\n",
    )
    .unwrap();
    d
}
