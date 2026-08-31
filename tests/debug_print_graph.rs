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

/// Byte equality alone is weak: two runs that both fail identically, both
/// print nothing, or both emit 1000+ bytes of malformed/wrong-shaped JSON
/// would also compare equal. Parsing `a.stdout` and asserting on a known node
/// count and a known key gives this test teeth: it would itself have caught
/// a JSON-shape regression (e.g. wrong key casing), not just non-determinism.
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

    let v: serde_json::Value =
        serde_json::from_slice(&a.stdout).expect("stdout must be valid JSON, not just consistent");
    assert_eq!(v["lockfile_version"], "9.0");
    assert!(
        v["nodes"].as_object().unwrap().len() > 300,
        "the fixture has 400 keys"
    );
    assert!(
        v["nodes"].as_object().unwrap().contains_key("glob@10.4.5"),
        "expected a known node from the fixture"
    );

    assert_eq!(a.stdout, b.stdout, "determinism is an invariant");
}

/// Pins the JSON contract's key-spelling rule (spec §10, as amended): fields
/// pudu invents are snake_case (`target_name`, `link_name`,
/// `lockfile_version`); fields echoed straight from the lockfile keep pnpm's
/// own camelCase spelling (`autoInstallPeers`, `hasBin`), so a reader can
/// grep a key from this output straight into `pnpm-lock.yaml`. Nothing else
/// in this file checks key spelling at all — a `rename_all` change on
/// `Settings` or `PackageMeta` would otherwise pass every other test here.
#[test]
fn json_keys_follow_the_invented_vs_echoed_spelling_rule() {
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
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    // Invented fields: snake_case.
    assert!(v.get("lockfile_version").is_some());
    let nodes = v["nodes"].as_object().unwrap();
    let glob = &nodes["glob@10.4.5"];
    assert!(glob.get("target_name").is_some(), "{glob}");
    let edges = glob["edges"].as_array().unwrap();
    assert!(
        edges.iter().all(|e| e.get("link_name").is_some()),
        "{edges:?}"
    );

    // Echoed-from-lockfile fields: pnpm's own camelCase.
    let settings = &v["settings"];
    assert!(settings.get("autoInstallPeers").is_some(), "{settings}");
    assert!(
        settings.get("excludeLinksFromLockfile").is_some(),
        "{settings}"
    );
    assert!(settings.get("auto_install_peers").is_none());

    let meta = &glob["meta"];
    assert!(meta["resolution"].get("integrity").is_some(), "{meta}");
    assert!(meta.get("hasBin").is_some(), "{meta}");
    assert!(meta.get("has_bin").is_none());
    assert!(meta.get("os").is_some() || meta["os"].is_null(), "{meta}");
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
