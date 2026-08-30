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
    // TD-S0-19: a validation failure is exit 3.
    assert_eq!(out.status.code(), Some(3), "{out:?}");
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
    assert_eq!(out.status.code(), Some(3), "{out:?}");
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
    // TD-S0-19: a missing pudu.toml is a configuration failure (3), not an
    // internal error (1).
    assert_eq!(out.status.code(), Some(3), "{out:?}");
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

/// TD-S0-18: the `#[diagnostic(help(...))]` strings were unreachable while
/// `main` printed `error: {e:#}`. They must now reach the user, along with
/// the diagnostic `code`.
#[test]
fn validation_errors_render_their_code_and_help_text() {
    let d = project(Some(
        "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"darwin\"\ncpu=\"arm64\"\nlibc=\"glibc\"\n[platforms.b]\nos=\"linux\"\ncpu=\"x64\"\n",
    ));
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("pudu::config::libc_on_non_linux"),
        "the diagnostic code must be shown:\n{stderr}"
    );
    assert!(
        stderr.contains("remove `libc`, or change `os` to \"linux\""),
        "the help text must be shown:\n{stderr}"
    );
}

/// TD-S0-16: `source_message()` fell back to `to_string()`, so an error with
/// no `#[source]` was printed as `{msg}: {msg}`.
#[test]
fn a_sourceless_error_is_reported_once() {
    let d = project(Some(
        "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"linux\"\ncpu=\"x64\"\n[fixups]\nregistry=\"file://\"\n",
    ));
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    let msg = "`[fixups].registry` value `file://` has no absolute path after `file://`";
    assert_eq!(
        stderr.matches(msg).count(),
        1,
        "the message must appear exactly once:\n{stderr}"
    );

    // Same in the JSON envelope, which builds its strings independently.
    let out = pudu(d.path())
        .args(["config", "check", "--format", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let errors = value["errors"].as_array().expect("errors array");
    assert_eq!(errors.len(), 1, "{value}");
    assert_eq!(errors[0].as_str().unwrap(), msg, "{value}");
}

/// TD-S0-17: warnings are typed, and still render with their own text and
/// help rather than as bare strings.
#[test]
fn warnings_render_as_diagnostics() {
    let single = "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.only]\nos=\"linux\"\ncpu=\"x64\"\nlibc=\"glibc\"\n";
    let d = project(Some(single));
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("pudu::config::single_platform"), "{stderr}");
    assert!(
        stderr.contains("`only`"),
        "the warning names the platform:\n{stderr}"
    );
}

/// The count summary is only worth printing when the count is not already
/// obvious: one rendered diagnostic needs no "1 error(s)" echo after it, and
/// `main` must not re-render the summary error either.
#[test]
fn a_single_error_gets_no_count_summary_but_several_do() {
    let one = project(Some(
        "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"darwin\"\ncpu=\"arm64\"\nlibc=\"glibc\"\n[platforms.b]\nos=\"linux\"\ncpu=\"x64\"\n",
    ));
    let out = pudu(one.path()).args(["config", "check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        !stderr.contains("error(s) in pudu.toml") && !stderr.contains("errors in pudu.toml"),
        "a lone error needs no summary:\n{stderr}"
    );
    assert_eq!(
        stderr.matches("pudu::config::").count(),
        1,
        "exactly one diagnostic, not an echoed summary:\n{stderr}"
    );

    let many = project(Some(
        "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"darwin\"\ncpu=\"arm64\"\nlibc=\"glibc\"\n[platforms.b]\nos=\"win32\"\ncpu=\"x64\"\n",
    ));
    let out = pudu(many.path())
        .args(["config", "check"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("2 errors in pudu.toml"),
        "several errors are worth counting:\n{stderr}"
    );
}

/// `--format json` puts everything CI needs on stdout, so stderr must stay
/// clean rather than carrying a second, differently-shaped report.
#[test]
fn json_format_says_nothing_on_stderr() {
    let d = project(Some(
        "lockfile_path=\"pnpm-lock.yaml\"\n[platforms.a]\nos=\"win32\"\ncpu=\"x64\"\nlibc=\"glibc\"\n",
    ));
    let out = pudu(d.path())
        .args(["config", "check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.is_empty(), "stderr must be empty:\n{stderr}");
}

/// A missing `pudu.toml` must not be reported as "1 error(s) in pudu.toml" —
/// a count of errors in a file that does not exist.
#[test]
fn a_missing_config_is_not_reported_as_a_count_of_errors_in_it() {
    let d = project(None);
    let out = pudu(d.path()).args(["config", "check"]).output().unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("cannot read"), "{stderr}");
    assert!(!stderr.contains("error(s) in pudu.toml"), "{stderr}");
}
