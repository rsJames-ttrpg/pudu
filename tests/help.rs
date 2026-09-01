use assert_cmd::Command;

fn pudu() -> Command {
    Command::cargo_bin("pudu").expect("binary builds")
}

/// The verb surface, locked. A substring loop cannot see a renamed verb
/// whose old name still appears elsewhere in the help, cannot see
/// reordering, and never checks the `[UNIMPLEMENTED — <stage>]` markers
/// (exit criterion 6). A snapshot sees all three: any change to the verbs,
/// their order, their one-line descriptions, or their stage labels has to be
/// reviewed and re-accepted.
#[test]
fn help_output_is_stable() {
    let out = pudu().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    insta::assert_snapshot!(text);
}

/// I5: clap turns the doc comment into long help, so an implementation
/// note left on the variant ships to users. Snapshotted so it cannot come
/// back.
#[test]
fn debug_long_help_is_stable() {
    let out = pudu().args(["debug", "--help"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        !text.contains("uninhabited"),
        "internal rationale must not reach users:\n{text}"
    );
    insta::assert_snapshot!(text);
}

#[test]
fn version_prints_the_crate_version() {
    let out = pudu().arg("--version").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
}

/// TD-S0-19: an unimplemented verb exits 4, distinct from a usage error (2)
/// and from a config failure (3), so CI can branch on it.
///
/// `buckify` left this list in S4 — it now has real behaviour, covered by
/// `tests/buckify.rs` — leaving `audit` as the sole remaining stub.
#[test]
fn stubbed_verbs_report_their_stage_and_exit_four() {
    let (verb, stage) = ("audit", "Phase 2");
    let out = pudu().arg(verb).output().unwrap();
    let text = String::from_utf8(out.stderr).unwrap();
    assert_eq!(out.status.code(), Some(4), "`{verb}` must exit 4:\n{text}");
    assert!(text.contains("not implemented yet"), "{text}");
    assert!(text.contains(stage), "`{verb}` must name {stage}:\n{text}");
}

/// A usage error, not an unimplemented verb: exit 2.
#[test]
fn debug_without_subcommand_exits_two() {
    let out = pudu().arg("debug").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// TD-S0-19: clap's own code for a bad command line stays 2, so pudu's
/// usage errors and clap's agree.
#[test]
fn an_unknown_flag_exits_two() {
    let out = pudu().arg("--nonsense").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// M8: `-C <nonexistent>` is a usage refusal (spec §6.1 code 2), and prints
/// with a `code` header like every other diagnostic — it used to be a bare
/// `anyhow!` that exited 1 with no code.
#[test]
fn a_bad_change_directory_is_a_usage_error() {
    let out = pudu()
        .args(["-C", "no/such/dir", "config", "check"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("pudu::usage::bad_directory"), "{stderr}");
    assert!(stderr.contains("no/such/dir"), "{stderr}");
}
