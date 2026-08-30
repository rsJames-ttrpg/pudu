mod common;

use common::{GOOD_CONFIG, project, pudu};

#[test]
fn accepts_a_good_config() {
    let d = project(Some(GOOD_CONFIG));
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("2 platforms"), "{stdout}");
}

#[test]
fn rejects_a_missing_lockfile() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("pudu.toml"), GOOD_CONFIG).unwrap();
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("pnpm-lock.yaml"), "{stderr}");
}

#[test]
fn reports_every_error_not_just_the_first() {
    let d = project(Some(
        "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"win32\"\ncpu=\"x64\"\nlibc=\"glibc\"\n",
    ));
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("windows"), "{stderr}");
    assert!(stderr.contains("libc"), "{stderr}");
}

#[test]
fn json_format_is_machine_readable() {
    let d = project(Some(GOOD_CONFIG));
    let out = pudu(d.path())
        .args(["config", "check", "--format", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("\"ok\""), "{stdout}");
    assert!(stdout.contains("true"), "{stdout}");
}

#[test]
fn missing_config_file_names_the_path() {
    let d = project(None);
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("pudu.toml"), "{stderr}");
}
