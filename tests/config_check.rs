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
fn json_format_reports_errors_on_stdout_and_exits_nonzero() {
    let d = project(Some(
        "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"win32\"\ncpu=\"x64\"\nlibc=\"glibc\"\n",
    ));
    let out = pudu(d.path())
        .args(["config", "check", "--format", "json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("\"ok\""), "{stdout}");
    assert!(stdout.contains("false"), "{stdout}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["ok"], false, "{value}");
    let errors = value["errors"].as_array().expect("errors array");
    assert!(!errors.is_empty(), "{value}");
}

#[test]
fn warnings_go_to_stderr_not_stdout() {
    let single_platform = "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"linux\"\ncpu=\"x64\"\nlibc=\"glibc\"\n";
    let d = project(Some(single_platform));
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("only one platform is configured"),
        "{stderr}"
    );
    assert!(
        !stdout.contains("only one platform is configured"),
        "{stdout}"
    );
}

#[test]
fn missing_config_file_names_the_path() {
    let d = project(None);
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("pudu.toml"), "{stderr}");
}

/// I6: a missing `pudu.toml` is exactly what `--format json` exists to
/// report, so it must come out as the envelope on stdout — not as nothing
/// on stdout and a human sentence on stderr, which makes `| jq -e .ok` a
/// parse error.
#[test]
fn json_format_reports_a_missing_config_as_json() {
    let d = project(None);
    let out = pudu(d.path())
        .args(["config", "check", "--format", "json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["ok"], false, "{value}");
    let errors = value["errors"].as_array().expect("errors array");
    assert_eq!(errors.len(), 1, "{value}");
    assert!(errors[0].as_str().unwrap().contains("pudu.toml"), "{value}");
}

/// I6: same for a config that cannot be parsed.
#[test]
fn json_format_reports_a_malformed_config_as_json() {
    let d = project(Some("lockfile_path = \n"));
    let out = pudu(d.path())
        .args(["config", "check", "--format", "json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["ok"], false, "{value}");
    let errors = value["errors"].as_array().expect("errors array");
    assert!(!errors.is_empty(), "{value}");
    assert!(
        errors[0].as_str().unwrap().contains("pudu.toml"),
        "the parse error must name the file: {value}"
    );
}

/// I6: the format argument is validated before the config is read, so a
/// bad format is reported as a bad format even with no pudu.toml present.
#[test]
fn unknown_format_is_rejected_before_reading_the_config() {
    let d = tempfile::tempdir().unwrap();
    let out = pudu(d.path())
        .args(["config", "check", "--format", "xml"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("xml"), "{stderr}");
    assert!(
        stderr.contains("human") && stderr.contains("json"),
        "the valid formats must be listed: {stderr}"
    );
    assert!(
        !stderr.contains("cannot read"),
        "must not report a missing config instead: {stderr}"
    );
}
