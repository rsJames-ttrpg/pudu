mod common;

use std::fs;

use common::pudu;

fn workspace(with_lockfile: bool) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    if with_lockfile {
        fs::write(d.path().join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    }
    d
}

#[test]
fn writes_the_project_skeleton() {
    let d = workspace(true);
    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    for f in [
        "pudu.toml",
        "third-party/js/BUCK",
        "third-party/js/toolchains.bzl",
        "third-party/js/.gitignore",
        "third-party/js/fixups/.gitkeep",
        "toolchains/BUCK",
    ] {
        assert!(d.path().join(f).exists(), "missing {f}");
    }

    let cfg = fs::read_to_string(d.path().join("pudu.toml")).unwrap();
    assert!(
        cfg.contains("lockfile_path   = \"pnpm-lock.yaml\""),
        "{cfg}"
    );
    assert!(
        cfg.contains("node_toolchain = \"toolchains//:node\""),
        "{cfg}"
    );
}

#[test]
fn the_generated_config_passes_config_check() {
    let d = workspace(true);
    pudu(d.path()).arg("init").output().unwrap();
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn derives_platforms_from_supported_architectures() {
    let d = workspace(true);
    fs::write(
        d.path().join("pnpm-workspace.yaml"),
        "supportedArchitectures:\n  os: [linux]\n  cpu: [x64, arm64]\n",
    )
    .unwrap();
    pudu(d.path()).arg("init").output().unwrap();

    let cfg = fs::read_to_string(d.path().join("pudu.toml")).unwrap();
    assert!(cfg.contains("[platforms.linux-x64-gnu]"), "{cfg}");
    assert!(cfg.contains("[platforms.linux-arm64-gnu]"), "{cfg}");
    assert!(!cfg.contains("darwin"), "{cfg}");
}

#[test]
fn refuses_to_overwrite_without_force() {
    let d = workspace(true);
    pudu(d.path()).arg("init").output().unwrap();
    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("--force"), "{stderr}");
}

#[test]
fn force_overwrites() {
    let d = workspace(true);
    pudu(d.path()).arg("init").output().unwrap();
    let out = pudu(d.path()).args(["init", "--force"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn toolchain_append_is_idempotent() {
    let d = workspace(true);
    pudu(d.path()).arg("init").output().unwrap();
    let first = fs::read_to_string(d.path().join("toolchains/BUCK")).unwrap();
    pudu(d.path()).args(["init", "--force"]).output().unwrap();
    pudu(d.path()).args(["init", "--force"]).output().unwrap();
    let third = fs::read_to_string(d.path().join("toolchains/BUCK")).unwrap();
    assert_eq!(first, third, "toolchains/BUCK must be stable across runs");
}

#[test]
fn preserves_an_existing_toolchains_buck() {
    let d = workspace(true);
    fs::create_dir_all(d.path().join("toolchains")).unwrap();
    let original = "system_python_toolchain(name = \"python\")\n";
    fs::write(d.path().join("toolchains/BUCK"), original).unwrap();

    pudu(d.path()).arg("init").output().unwrap();

    let text = fs::read_to_string(d.path().join("toolchains/BUCK")).unwrap();
    assert!(
        text.starts_with(original),
        "existing content must be preserved: {text}"
    );
    assert!(text.contains("system_node_toolchain"), "{text}");
}

#[test]
fn never_overwrites_a_user_node_toolchain() {
    let d = workspace(true);
    fs::create_dir_all(d.path().join("toolchains")).unwrap();
    let original = "system_node_toolchain(name = \"node\", node = \"/opt/node/bin/node\")\n";
    fs::write(d.path().join("toolchains/BUCK"), original).unwrap();

    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let text = fs::read_to_string(d.path().join("toolchains/BUCK")).unwrap();
    assert_eq!(
        text, original,
        "a user's own node toolchain must be untouched"
    );
}

#[test]
fn undetected_project_writes_a_todo_template() {
    let d = workspace(false);
    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cfg = fs::read_to_string(d.path().join("pudu.toml")).unwrap();
    assert!(cfg.contains("TODO"), "{cfg}");
}
