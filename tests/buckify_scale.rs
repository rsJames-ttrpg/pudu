//! Buckify over the 328-package real lockfile.
//!
//! `#[ignore]`d: it needs the network, like `tests/vendor_oracle.rs`. CI runs
//! it in the vendor-oracle job. Its value is scale and determinism at scale —
//! the hermetic fixture in `tests/buckify.rs` has four packages. Space-rooted
//! `@types` archives are covered there and by the buck2 job, which builds
//! them; counting them here only ever produced a number that disagreed with
//! reality.

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
    // Exact, not a floor: both inputs are pinned (the committed lockfile in
    // tests/fixtures/lock/real, and CONFIG's two platforms above), and
    // pruning is deterministic, so the package count is a fixed number, not
    // a range. A looser bound like `> 300` would not catch a 20-package
    // pruning regression.
    assert_eq!(
        packages, 322,
        "expected the full store (322 packages for this pinned lockfile/platform pair), got {packages}"
    );

    // Spec exit criterion 5 asks for these numbers; `--nocapture` prints them.
    eprintln!(
        "buckify scale: {packages} packages, BUCK {} bytes, {elapsed:?}",
        buck.len()
    );

    assert!(
        !buck.contains("strip_prefix"),
        "spec §1.1: strip_prefix must never be emitted"
    );
}
