//! `pudu buckify` end to end, against a mock registry.
//!
//! Hermetic: tarballs are built in-process, so the space-rooted `@types`
//! case is exercised with no network and no dependence on the buck2 job.

mod common;

use std::io::Write as _;

use httpmock::prelude::*;

/// A gzipped tar whose entries all nest under `root`.
///
/// `root` is a parameter because the whole point of spec §1.1 is that it is
/// not always `package`: `@types/node` unpacks to `node v22.20`, and that
/// space is what broke `strip_prefix`.
fn tarball_rooted(root: &str, files: &[(&str, &str)]) -> Vec<u8> {
    let mut ar = tar::Builder::new(Vec::new());
    for (path, body) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        ar.append_data(&mut h, format!("{root}/{path}"), body.as_bytes())
            .unwrap();
    }
    let bytes = ar.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&bytes).unwrap();
    gz.finish().unwrap()
}

/// `\x20` in the lockfile literals below is a literal space: a `\` at the end
/// of a Rust string line strips the newline *and* the next line's leading
/// whitespace, and YAML indentation is load-bearing.
fn integrity_of(bytes: &[u8]) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha512};
    let mut h = Sha512::new();
    h.update(bytes);
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(h.finalize())
    )
}

struct Fixture {
    dir: tempfile::TempDir,
    cache: tempfile::TempDir,
    #[allow(dead_code)]
    server: MockServer,
}

impl Fixture {
    /// A project depending on four packages: `left-pad@1.3.0` (root
    /// `package`, no bin), `tool@2.0.0` (root `package`, bin `cli.js`),
    /// `@types/node@22.20.0` (root `node v22.20`, the space case) and
    /// `mac-only@3.0.0` (`os: [darwin]`, surviving only on the second
    /// configured platform).
    fn new() -> Self {
        let server = MockServer::start();
        let plain = tarball_rooted("package", &[("package.json", r#"{"name":"left-pad"}"#)]);
        let tool = tarball_rooted(
            "package",
            &[
                (
                    "package.json",
                    r#"{"name":"tool","bin":"cli.js","scripts":{"install":"node x"}}"#,
                ),
                ("cli.js", "#!/usr/bin/env node\n"),
            ],
        );
        let types_node = tarball_rooted(
            "node v22.20",
            &[("package.json", r#"{"name":"@types/node"}"#)],
        );
        // `os: [darwin]`, so S2 prunes it away on `linux-x64-gnu` and it
        // survives only because a second platform is configured.
        let mac = tarball_rooted("package", &[("package.json", r#"{"name":"mac-only"}"#)]);

        server.mock(|when, then| {
            when.method(GET).path("/left-pad/-/left-pad-1.3.0.tgz");
            then.status(200).body(plain.clone());
        });
        server.mock(|when, then| {
            when.method(GET).path("/tool/-/tool-2.0.0.tgz");
            then.status(200).body(tool.clone());
        });
        server.mock(|when, then| {
            when.method(GET).path("/@types/node/-/node-22.20.0.tgz");
            then.status(200).body(types_node.clone());
        });
        server.mock(|when, then| {
            when.method(GET).path("/mac-only/-/mac-only-3.0.0.tgz");
            then.status(200).body(mac.clone());
        });

        let dir = tempfile::tempdir().unwrap();
        // Two platforms, deliberately — mirrors tests/vendor.rs: mac-only
        // must survive into BUCK even though it is pruned on the first
        // platform, since an http_archive does not vary by platform.
        let config = format!(
            "lockfile_path = \"pnpm-lock.yaml\"\n\
             third_party_dir = \"third-party/js\"\n\n\
             [platforms.linux-x64-gnu]\n\
             os = \"linux\"\ncpu = \"x64\"\nlibc = \"glibc\"\n\n\
             [platforms.darwin-arm64]\n\
             os = \"darwin\"\ncpu = \"arm64\"\n\n\
             [registry]\ndefault = \"{}\"\n",
            server.base_url()
        );
        std::fs::write(dir.path().join("pudu.toml"), config).unwrap();

        let lock = format!(
            "lockfileVersion: '9.0'\n\n\
             importers:\n\n  .:\n    dependencies:\n\
             \x20     left-pad:\n        specifier: 1.3.0\n        version: 1.3.0\n\
             \x20     tool:\n        specifier: 2.0.0\n        version: 2.0.0\n\
             \x20     '@types/node':\n        specifier: 22.20.0\n        version: 22.20.0\n\
             \x20   optionalDependencies:\n\
             \x20     mac-only:\n        specifier: 3.0.0\n        version: 3.0.0\n\n\
             packages:\n\n\
             \x20 left-pad@1.3.0:\n    resolution: {{integrity: {}}}\n\n\
             \x20 tool@2.0.0:\n    resolution: {{integrity: {}}}\n    hasBin: true\n\n\
             \x20 '@types/node@22.20.0':\n    resolution: {{integrity: {}}}\n\n\
             \x20 mac-only@3.0.0:\n    resolution: {{integrity: {}}}\n    os: [darwin]\n\n\
             snapshots:\n\n  left-pad@1.3.0: {{}}\n\n  tool@2.0.0: {{}}\n\n\
             \x20 '@types/node@22.20.0': {{}}\n\n\
             \x20 mac-only@3.0.0: {{}}\n",
            integrity_of(&plain),
            integrity_of(&tool),
            integrity_of(&types_node),
            integrity_of(&mac),
        );
        std::fs::write(dir.path().join("pnpm-lock.yaml"), lock).unwrap();

        Fixture {
            dir,
            cache: tempfile::tempdir().unwrap(),
            server,
        }
    }

    /// A project whose lockfile has no `packages:` section at all.
    fn empty_lockfile() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = "lockfile_path = \"pnpm-lock.yaml\"\n\
             third_party_dir = \"third-party/js\"\n\n\
             [platforms.linux-x64-gnu]\n\
             os = \"linux\"\ncpu = \"x64\"\nlibc = \"glibc\"\n\n\
             [platforms.darwin-arm64]\n\
             os = \"darwin\"\ncpu = \"arm64\"\n\n\
             [registry]\ndefault = \"https://registry.npmjs.org\"\n";
        std::fs::write(dir.path().join("pudu.toml"), config).unwrap();
        std::fs::write(
            dir.path().join("pnpm-lock.yaml"),
            "lockfileVersion: '9.0'\n",
        )
        .unwrap();
        Fixture {
            dir,
            cache: tempfile::tempdir().unwrap(),
            server: MockServer::start(),
        }
    }

    fn cmd(&self) -> assert_cmd::Command {
        let mut c = common::pudu(self.dir.path());
        c.env("PUDU_CACHE_DIR", self.cache.path());
        c
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.dir.path().join("third-party/js").join(rel)
    }
    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).unwrap()
    }
    /// All three generated files, for byte-comparison across runs.
    fn generated(&self) -> Vec<String> {
        ["BUCK", "pudu.bzl", "config/BUCK"]
            .iter()
            .map(|n| self.read(n))
            .collect()
    }
}

#[test]
fn buckify_writes_three_files_and_a_second_run_is_byte_identical() {
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();
    f.cmd().arg("buckify").assert().success();

    let first = f.generated();
    f.cmd().arg("buckify").assert().success();
    assert_eq!(first, f.generated(), "buckify must be deterministic");
}

#[test]
fn the_space_rooted_package_emits_its_real_root_and_no_strip_prefix() {
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();
    f.cmd().arg("buckify").assert().success();

    let buck = f.read("BUCK");
    assert!(buck.contains(r#"name = "@types+node@22.20.0","#), "{buck}");
    assert!(buck.contains(r#"root = "node v22.20","#), "{buck}");
    assert!(
        !f.read("pudu.bzl").contains("strip_prefix ="),
        "spec §1.1: strip_prefix reaches a shell unquoted"
    );
}

#[test]
fn a_package_surviving_on_only_one_platform_still_gets_a_target() {
    // The emitted set is the union across platforms: an http_archive does not
    // vary by platform, and a macOS-only package must not vanish from a
    // buckify run on Linux.
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();
    f.cmd().arg("buckify").assert().success();
    assert!(f.read("BUCK").contains(r#"name = "mac-only@3.0.0","#));
}

#[test]
fn buckify_without_a_package_table_fails_before_writing_anything() {
    let f = Fixture::new();
    let out = f.cmd().arg("buckify").output().unwrap();
    assert_eq!(out.status.code(), Some(5));
    assert!(
        !f.path("BUCK").exists(),
        "no file may be written on failure"
    );
    assert!(!f.path("config/BUCK").exists());
}

#[test]
fn check_passes_on_fresh_output_and_fails_after_an_edit() {
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();
    f.cmd().arg("buckify").assert().success();
    f.cmd().args(["buckify", "--check"]).assert().success();

    std::fs::write(f.path("BUCK"), "# hand-edited\n").unwrap();
    let out = f.cmd().args(["buckify", "--check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(5));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&f.path("BUCK").display().to_string()),
        "the diagnostic must name the file that differs: {stderr}"
    );
}

#[test]
fn check_fails_when_a_generated_file_is_missing_entirely() {
    // A tree that has never been buckified must fail --check, not pass it.
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();
    f.cmd().arg("buckify").assert().success();
    std::fs::remove_file(f.path("config/BUCK")).unwrap();
    let out = f.cmd().args(["buckify", "--check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(5));
}

#[test]
fn buckify_replaces_inits_placeholder_buck() {
    // `init` seeds `# Generated by pudu. Run: pudu buckify` and its own
    // comment says files under third-party/js are never overwritten. Spec §3
    // makes the three generated files an explicit exception.
    let f = Fixture::new();
    std::fs::create_dir_all(f.path("BUCK").parent().unwrap()).unwrap();
    std::fs::write(f.path("BUCK"), "# Generated by pudu. Run: pudu buckify\n").unwrap();
    f.cmd().arg("vendor").assert().success();
    f.cmd().arg("buckify").assert().success();
    assert!(f.read("BUCK").contains("npm_package("));
}

#[test]
fn the_generated_files_carry_the_generated_banner() {
    let f = Fixture::new();
    f.cmd().arg("vendor").assert().success();
    f.cmd().arg("buckify").assert().success();
    for name in ["BUCK", "pudu.bzl", "config/BUCK"] {
        assert!(
            f.read(name)
                .starts_with("##\n## @generated by pudu\n## Do not edit by hand.\n##\n"),
            "{name} must open with the generated banner"
        );
    }
}

#[test]
fn a_lockfile_with_no_packages_emits_an_empty_buck() {
    // staleness() reports nothing when there is nothing expected, so this
    // path reaches the emitter with no package table at all. It must emit an
    // empty BUCK rather than panicking.
    let f = Fixture::empty_lockfile();
    f.cmd().arg("buckify").assert().success();
    let buck = f.read("BUCK");
    // The `load(...)` line still names the macro (it is always emitted, used
    // or not); what must be absent is an actual rule instance.
    assert!(!buck.contains("npm_package("), "{buck}");
    assert!(buck.starts_with("##\n## @generated by pudu\n"));
}
