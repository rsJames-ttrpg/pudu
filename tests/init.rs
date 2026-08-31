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
    // TD-S0-19: refusing to clobber pudu.toml is a usage error (2), not a
    // config failure (3) or an internal one (1).
    assert_eq!(out.status.code(), Some(2), "{out:?}");
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

/// I1: an existing user toolchain must be RECORDED in pudu.toml, under the
/// name the file actually declares — not the hardcoded `:node` (exit
/// criterion 5). A wrong label here becomes a reference to a nonexistent
/// Buck target at S4.
#[test]
fn records_an_existing_user_toolchain_under_its_real_name() {
    let d = workspace(true);
    fs::create_dir_all(d.path().join("toolchains")).unwrap();
    fs::write(
        d.path().join("toolchains/BUCK"),
        "system_node_toolchain(name = \"my_node\", node = \"/opt/node/bin/node\")\n",
    )
    .unwrap();

    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cfg = fs::read_to_string(d.path().join("pudu.toml")).unwrap();
    assert!(
        cfg.contains("node_toolchain = \"toolchains//:my_node\""),
        "the existing toolchain must be recorded: {cfg}"
    );
    assert!(
        !cfg.contains("toolchains//:node\""),
        "must not record the default label: {cfg}"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("my_node"), "{stderr}");

    // And the generated config must still validate.
    let check = pudu(d.path()).args(["config", "check"]).output().unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

/// I1: when the target name cannot be parsed out of the call, fall back to
/// `node` and say so, rather than silently recording a guess.
#[test]
fn unparseable_toolchain_name_falls_back_and_says_so() {
    let d = workspace(true);
    fs::create_dir_all(d.path().join("toolchains")).unwrap();
    fs::write(
        d.path().join("toolchains/BUCK"),
        "system_node_toolchain(**MY_KWARGS)\n",
    )
    .unwrap();

    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(out.status.success());
    let cfg = fs::read_to_string(d.path().join("pudu.toml")).unwrap();
    assert!(
        cfg.contains("node_toolchain = \"toolchains//:node\""),
        "{cfg}"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("could not read the target name"),
        "the fallback must be announced: {stderr}"
    );
    // I4: announced as a diagnostic, not a bare `warning:` line.
    assert!(
        stderr.contains("pudu::init::toolchain_name_unparsed"),
        "{stderr}"
    );
    assert!(!stderr.contains("warning:"), "{stderr}");
}

/// I8: the `root//` load label is anchored at the Buck cell root, not at
/// init's own directory. Running below the lockfile directory must prefix
/// the label with the path from that directory, and warn that the cell root
/// is being guessed.
#[test]
fn load_label_is_relative_to_the_lockfile_directory() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    let nested = d.path().join("apps/web");
    fs::create_dir_all(&nested).unwrap();

    let out = pudu(&nested).arg("init").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let buck = fs::read_to_string(nested.join("toolchains/BUCK")).unwrap();
    assert!(
        buck.contains("load(\"root//apps/web/third-party/js:toolchains.bzl\""),
        "the load label must resolve to the real directory: {buck}"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("cell root"),
        "an ambiguous cell root must be warned about: {stderr}"
    );
    assert!(
        stderr.contains("pudu::init::cell_root_guess"),
        "I4: the guess is a typed diagnostic:\n{stderr}"
    );
}

/// I8: at the lockfile directory itself there is no prefix and no warning.
#[test]
fn load_label_has_no_prefix_at_the_lockfile_directory() {
    let d = workspace(true);
    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(out.status.success());
    let buck = fs::read_to_string(d.path().join("toolchains/BUCK")).unwrap();
    assert!(
        buck.contains("load(\"root//third-party/js:toolchains.bzl\""),
        "{buck}"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(!stderr.contains("cell root"), "{stderr}");
}

/// M4: "wrote third-party/js" must not be printed when every file in it was
/// skipped as already present.
#[test]
fn does_not_claim_to_write_third_party_js_when_everything_was_skipped() {
    let d = workspace(true);
    pudu(d.path()).arg("init").output().unwrap();

    let out = pudu(d.path()).args(["init", "--force"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        !stdout.contains("third-party/js"),
        "nothing under third-party/js was written: {stdout}"
    );
}

/// M6: `pudu init <nonexistent-dir>` must not fail with a bare
/// "No such file or directory" naming pudu.toml.
#[test]
fn init_creates_a_missing_target_directory() {
    let d = workspace(true);
    let out = pudu(d.path()).args(["init", "newdir"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(d.path().join("newdir/pudu.toml").is_file());
}

/// TD-S0-17/19: an unusable `supportedArchitectures` is a typed
/// `DeriveError`, so it exits 3 (configuration invalid) — and the warnings
/// explaining *why* every candidate was dropped ride along on the error
/// rather than being lost.
#[test]
fn no_usable_platforms_exits_three_and_surfaces_the_warnings() {
    let d = workspace(true);
    std::fs::write(
        d.path().join("pnpm-workspace.yaml"),
        "supportedArchitectures:\n  os: [win32, solaris]\n  cpu: [x64]\n",
    )
    .unwrap();
    let out = pudu(d.path()).arg("init").output().unwrap();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("no supported platforms"), "{stderr}");
    assert!(
        stderr.contains("win32"),
        "the win32 skip must survive onto the error:\n{stderr}"
    );
    assert!(
        stderr.contains("solaris"),
        "the unknown-os warning must survive onto the error:\n{stderr}"
    );
    assert!(
        !d.path().join("pudu.toml").exists(),
        "nothing is written when derivation fails"
    );
}

/// I4: spec §6 promises every diagnostic pudu prints has the same shape.
/// `init`'s scaffolding warnings used to be raw `eprintln!("warning: ...")`,
/// so an idempotent `--force` re-run — the common case — printed only the
/// un-typed shape. They are `InitWarning`s rendered through `error::render`
/// now.
#[test]
fn init_warnings_render_as_diagnostics_on_the_file_exists_path() {
    let d = workspace(true);
    pudu(d.path()).arg("init").output().unwrap();

    let out = pudu(d.path()).args(["init", "--force"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("pudu::init::third_party_file_exists"),
        "the file-exists warning must carry its diagnostic code:\n{stderr}"
    );
    assert!(stderr.contains("exists; leaving it alone"), "{stderr}");
    assert!(
        !stderr.contains("warning:"),
        "no raw `warning:` line may survive alongside the typed shape:\n{stderr}"
    );
}

/// A `DeriveWarning` on the success path renders as the same diagnostic it
/// does when it rides along on `DeriveError::NoUsablePlatforms` — one
/// implementation of what a warning looks like, not two.
#[test]
fn success_path_warnings_render_as_diagnostics() {
    let d = workspace(true);
    std::fs::write(
        d.path().join("pnpm-workspace.yaml"),
        "supportedArchitectures:\n  os: [linux, win32]\n  cpu: [x64]\n",
    )
    .unwrap();
    let out = pudu(d.path()).arg("init").output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("pudu::init::win32_skipped"),
        "the diagnostic code must be shown:\n{stderr}"
    );
}
