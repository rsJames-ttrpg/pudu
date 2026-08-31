//! `pudu debug platforms` — the hidden per-platform pruning view.

use std::process::Command;

/// A tempdir holding a `pudu.toml` and a `pnpm-lock.yaml`.
///
/// `tests/common`'s `project` takes only a config and writes a stub
/// lockfile, and `scratch_with_lockfile` writes a `pudu.toml` with no
/// platforms — this test needs both halves to be its own, so it builds the
/// directory here rather than widening a helper three other test crates
/// share.
fn project(config: &str, lockfile: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    std::fs::write(d.path().join("pudu.toml"), config).expect("write pudu.toml");
    std::fs::write(d.path().join("pnpm-lock.yaml"), lockfile).expect("write lockfile");
    d
}

/// A lockfile with one ungated package and two platform-gated ones.
const LOCK: &str = r#"lockfileVersion: '9.0'

importers:

  .:
    dependencies:
      app:
        specifier: 1.0.0
        version: 1.0.0

packages:

  app@1.0.0:
    resolution: {integrity: sha512-app}

  '@esbuild/linux-x64@0.25.12':
    resolution: {integrity: sha512-lin}
    cpu: [x64]
    os: [linux]

  '@esbuild/darwin-arm64@0.25.12':
    resolution: {integrity: sha512-dar}
    cpu: [arm64]
    os: [darwin]

snapshots:

  app@1.0.0:
    optionalDependencies:
      '@esbuild/linux-x64': 0.25.12
      '@esbuild/darwin-arm64': 0.25.12

  '@esbuild/linux-x64@0.25.12':
    optional: true

  '@esbuild/darwin-arm64@0.25.12':
    optional: true
"#;

const CONFIG: &str = r#"lockfile_path   = "pnpm-lock.yaml"
third_party_dir = "third-party/js"

[platforms.linux-x64-gnu]
os   = "linux"
cpu  = "x64"
libc = "glibc"

[platforms.darwin-arm64]
os  = "darwin"
cpu = "arm64"
"#;

fn run() -> (serde_json::Value, String) {
    let dir = project(CONFIG, LOCK);
    let out = Command::new(env!("CARGO_BIN_EXE_pudu"))
        .args(["debug", "platforms"])
        .current_dir(dir.path())
        .output()
        .expect("run pudu");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        serde_json::from_slice(&out.stdout).expect("stdout is JSON"),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn prints_one_entry_per_configured_platform() {
    let (json, _) = run();
    let p = json["platforms"]
        .as_object()
        .expect("platforms is an object");
    assert_eq!(p.len(), 2);
    assert!(p.contains_key("linux-x64-gnu"));
    assert!(p.contains_key("darwin-arm64"));
}

#[test]
fn each_platform_keeps_its_own_gated_package_and_prunes_the_other() {
    let (json, _) = run();
    let lin = &json["platforms"]["linux-x64-gnu"];
    assert_eq!(lin["node_count"], 2);
    assert_eq!(
        lin["pruned"].as_array().unwrap(),
        &vec![serde_json::json!("@esbuild/darwin-arm64@0.25.12")]
    );

    let mac = &json["platforms"]["darwin-arm64"];
    assert_eq!(
        mac["pruned"].as_array().unwrap(),
        &vec![serde_json::json!("@esbuild/linux-x64@0.25.12")]
    );
}

#[test]
fn reports_the_platform_axes_and_generated_constraints() {
    let (json, _) = run();
    let lin = &json["platforms"]["linux-x64-gnu"];
    assert_eq!(lin["os"], "linux");
    assert_eq!(lin["cpu"], "x64");
    assert_eq!(lin["libc"], "glibc");
    // Sorted, and with no abi constraint: only one libc is configured, so
    // abi does not discriminate.
    assert_eq!(
        lin["constraints"].as_array().unwrap(),
        &vec![
            serde_json::json!("prelude//cpu/constraints:x86_64"),
            serde_json::json!("prelude//os/constraints:linux"),
        ]
    );
    assert_eq!(lin["constraints_overridden"], false);

    let mac = &json["platforms"]["darwin-arm64"];
    assert!(mac["libc"].is_null(), "darwin configures no libc");
}

#[test]
fn an_excluded_optional_dependency_is_not_a_dropped_required_edge() {
    let (json, _) = run();
    for name in ["linux-x64-gnu", "darwin-arm64"] {
        assert!(
            json["platforms"][name]["dropped_required_edges"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{name}"
        );
    }
}

/// stdout must stay machine-parseable, so diagnostics go to stderr.
///
/// As originally written this test only asserted `json.is_object()`, which
/// `run()` already establishes by successfully parsing `out.stdout` as JSON
/// — if a warning leaked into stdout, `run()`'s own `.expect("stdout is
/// JSON")` would already fail every other test in this file, so this test
/// could never fail on its own. To give it teeth, it forces a real lockfile
/// warning (an unrecognised top-level key) and checks both sides: the
/// warning text lands on stderr, and stdout still parses as the expected
/// JSON shape untouched by it.
#[test]
fn stdout_is_pure_json_and_warnings_go_to_stderr() {
    let noisy_lock = LOCK.replacen(
        "lockfileVersion: '9.0'\n",
        "lockfileVersion: '9.0'\nfutureTopLevelKey: true\n",
        1,
    );
    let dir = project(CONFIG, &noisy_lock);
    let out = Command::new(env!("CARGO_BIN_EXE_pudu"))
        .args(["debug", "platforms"])
        .current_dir(dir.path())
        .output()
        .expect("run pudu");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("futureTopLevelKey"),
        "expected the unknown-key warning on stderr: {stderr}"
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be pure JSON, not warning text");
    let p = json["platforms"]
        .as_object()
        .expect("platforms is an object");
    assert_eq!(p.len(), 2);
}

#[test]
fn output_is_byte_identical_across_runs() {
    let dir = project(CONFIG, LOCK);
    let a = Command::new(env!("CARGO_BIN_EXE_pudu"))
        .args(["debug", "platforms"])
        .current_dir(dir.path())
        .output()
        .expect("run pudu");
    let b = Command::new(env!("CARGO_BIN_EXE_pudu"))
        .args(["debug", "platforms"])
        .current_dir(dir.path())
        .output()
        .expect("run pudu");
    assert_eq!(a.stdout, b.stdout);
}
