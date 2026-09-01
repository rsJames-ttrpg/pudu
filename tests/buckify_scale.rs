//! Buckify over the 328-package real lockfile.
//!
//! `#[ignore]`d: it needs the network, like `tests/vendor_oracle.rs`. CI runs
//! it in the vendor-oracle job. Its value is scale — the fixture in
//! `tests/buckify.rs` has four packages, and 18 of this lockfile's entries
//! are space-rooted `@types` packages, so this exercises spec §1.1 at volume
//! for free.

mod common;

const CONFIG: &str = r#"
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

#[test]
#[ignore = "downloads several hundred tarballs from registry.npmjs.org"]
fn buckify_on_the_real_lockfile_is_deterministic() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lock/real");
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        fixture.join("pnpm-lock.yaml"),
        dir.path().join("pnpm-lock.yaml"),
    )
    .unwrap();
    std::fs::write(dir.path().join("pudu.toml"), CONFIG).unwrap();
    let cache = tempfile::tempdir().unwrap();

    let out = common::pudu(dir.path())
        .env("PUDU_CACHE_DIR", cache.path())
        .arg("vendor")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "vendor failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let read = |rel: &str| std::fs::read_to_string(dir.path().join("third-party/js").join(rel));

    let started = std::time::Instant::now();
    common::pudu(dir.path()).arg("buckify").assert().success();
    let elapsed = started.elapsed();

    let first: Vec<String> = ["BUCK", "pudu.bzl", "config/BUCK"]
        .iter()
        .map(|n| read(n).unwrap())
        .collect();

    common::pudu(dir.path()).arg("buckify").assert().success();
    let second: Vec<String> = ["BUCK", "pudu.bzl", "config/BUCK"]
        .iter()
        .map(|n| read(n).unwrap())
        .collect();
    assert_eq!(first, second, "buckify must be deterministic at scale");

    let buck = &first[0];
    let packages = buck.matches("npm_package(").count();
    assert!(
        packages > 300,
        "expected the full store, got {packages} packages"
    );

    // The 18 space-rooted `@types` entries are the reason this test exists at
    // scale rather than only in the four-package fixture.
    let space_rooted = buck
        .lines()
        .filter(|l| l.trim_start().starts_with("root = ") && l.contains(' ') && l.contains("v"))
        .count();

    // Spec exit criterion 5 asks for these numbers; `--nocapture` prints them.
    eprintln!(
        "buckify scale: {packages} packages, BUCK {} bytes, {elapsed:?}, \
         {space_rooted} candidate space-rooted entries",
        buck.len()
    );

    assert!(
        !buck.contains("strip_prefix"),
        "spec §1.1: strip_prefix must never be emitted"
    );
}
