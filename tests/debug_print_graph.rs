mod common;

use std::path::Path;

/// The committed fixture: a real pnpm install (see its README).
fn fixture() -> &'static Path {
    Path::new("tests/fixtures/lock/real")
}

#[test]
fn print_graph_emits_json_for_the_real_lockfile() {
    let dir = common::scratch_with_config(fixture());
    let out = common::pudu(dir.path())
        .args(["debug", "print-graph"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(
        v["nodes"].as_object().unwrap().len() > 300,
        "the fixture has 400 keys"
    );
    assert_eq!(v["lockfile_version"], "9.0");
}

/// Byte equality alone is weak: two runs that both fail identically, or both
/// print nothing, would also compare equal. The size and success checks give
/// this test teeth — it fails if determinism is achieved by both runs
/// producing the same *empty or broken* output.
#[test]
fn output_is_byte_identical_across_runs() {
    let dir = common::scratch_with_config(fixture());
    let a = common::pudu(dir.path())
        .args(["debug", "print-graph"])
        .output()
        .unwrap();
    let b = common::pudu(dir.path())
        .args(["debug", "print-graph"])
        .output()
        .unwrap();
    assert!(a.status.success(), "{}", String::from_utf8_lossy(&a.stderr));
    assert!(
        a.stdout.len() > 1000,
        "output must be a real graph, not a trivially-matching empty/short output: {} bytes",
        a.stdout.len()
    );
    assert_eq!(a.stdout, b.stdout, "determinism is an invariant");
}

#[test]
fn cycles_are_reported_for_the_real_lockfile() {
    let dir = common::scratch_with_config(fixture());
    let out = common::pudu(dir.path())
        .args(["debug", "print-graph"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let cycles = v["cycles"].as_array().unwrap();
    assert!(
        !cycles.is_empty(),
        "the fixture has @babel/eslint/browserslist cycles"
    );
}

#[test]
fn aliased_edge_survives_into_the_output() {
    let dir = common::scratch_with_config(fixture());
    let out = common::pudu(dir.path())
        .args(["debug", "print-graph"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let cliui = v["nodes"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(k, _)| k.starts_with("@isaacs/cliui@"))
        .expect("the fixture pulls @isaacs/cliui via glob")
        .1;
    let edge = cliui["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["link_name"] == "string-width-cjs")
        .expect("alias edge present");
    assert!(
        edge["target"]
            .as_str()
            .unwrap()
            .starts_with("string-width@4")
    );
}

#[test]
fn a_v6_lockfile_exits_3() {
    let dir = common::scratch_with_lockfile("lockfileVersion: '6.0'\n");
    let out = common::pudu(dir.path())
        .args(["debug", "print-graph"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("6.0") && stderr.contains('9'), "{stderr}");
}
