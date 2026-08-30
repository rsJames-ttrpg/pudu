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
    // The lockfile_path *value* itself, not just the banner comment, must
    // carry the TODO placeholder.
    assert!(
        cfg.contains("lockfile_path   = \"TODO: path to your pnpm-lock.yaml\""),
        "{cfg}"
    );
}

/// CRITICAL: `--force` narrows to `pudu.toml` and the `toolchains/BUCK`
/// managed block only (spec change, commit f4c5b0c) — files under
/// third-party/js/ are user-owned once they exist and must never be
/// overwritten, `--force` or not.
#[test]
fn force_never_overwrites_hand_edited_third_party_js_contents() {
    let d = workspace(true);
    pudu(d.path()).arg("init").output().unwrap();

    let bzl_path = d.path().join("third-party/js/toolchains.bzl");
    let buck_path = d.path().join("third-party/js/BUCK");
    let hand_edited_bzl = "# hand-edited by a user\nsystem_node_toolchain = 1\n";
    let hand_edited_buck = "# hand-edited BUCK\n";
    fs::write(&bzl_path, hand_edited_bzl).unwrap();
    fs::write(&buck_path, hand_edited_buck).unwrap();

    let out = pudu(d.path()).args(["init", "--force"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(
        fs::read_to_string(&bzl_path).unwrap(),
        hand_edited_bzl,
        "third-party/js/toolchains.bzl must survive --force untouched"
    );
    assert_eq!(
        fs::read_to_string(&buck_path).unwrap(),
        hand_edited_buck,
        "third-party/js/BUCK must survive --force untouched"
    );
}

/// IMPORTANT: a relative `[PATH]` argument must detect the same lockfile a
/// bare `pudu init` run from the target directory would find, ascending
/// above the process cwd when necessary.
///
/// The lockfile sits TWO levels above the `[PATH]` argument (workspace root
/// -> proj -> proj/sub), not one: with only one level, `detect("sub")`'s
/// upward walk terminates at `""`, and `"".join("pnpm-lock.yaml")` resolves
/// against the process cwd -- which in a one-level layout happens to BE the
/// directory holding the lockfile, masking the bug this regression test
/// exists to catch. Process cwd is `proj`; `[PATH]` is `sub`, so `detect`
/// must ascend past `proj` to the workspace root to find the lockfile.
#[test]
fn relative_path_argument_detects_lockfile_above_it() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    let proj = d.path().join("proj");
    fs::create_dir_all(proj.join("sub")).unwrap();

    let out = pudu(&proj).args(["init", "sub"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cfg = fs::read_to_string(proj.join("sub/pudu.toml")).unwrap();
    assert!(
        cfg.contains("lockfile_path   = \"../../pnpm-lock.yaml\""),
        "{cfg}"
    );
}

/// IMPORTANT: without `--force`, a stale-but-parseable managed block already
/// present in `toolchains/BUCK` (exactly one BEGIN/END pair, contents
/// diverged from what pudu would generate today) must be left byte-for-byte
/// alone. `pudu.toml` must not already exist, since that gate short-circuits
/// `run()` before the toolchain logic runs at all — so this managed block is
/// created directly rather than via a prior `pudu init`.
#[test]
fn non_force_run_leaves_a_stale_managed_block_alone() {
    let d = workspace(true);
    fs::create_dir_all(d.path().join("toolchains")).unwrap();
    let stale_block = "# --- begin pudu-managed (do not edit inside this block) ---\n\
         load(\"root//third-party/js:toolchains.bzl\", \"system_node_toolchain\")\n\
         system_node_toolchain(name = \"node\", visibility = [\"//:x\"])\n\
         # --- end pudu-managed ---\n";
    fs::write(d.path().join("toolchains/BUCK"), stale_block).unwrap();

    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(
        fs::read_to_string(d.path().join("toolchains/BUCK")).unwrap(),
        stale_block,
        "a non-force run must not refresh a stale managed block"
    );
}
