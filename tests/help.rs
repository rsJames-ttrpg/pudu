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

#[test]
fn stubbed_verbs_report_their_stage_and_exit_two() {
    for (verb, stage) in [("vendor", "S3"), ("buckify", "S4"), ("audit", "Phase 2")] {
        let out = pudu().arg(verb).output().unwrap();
        let text = String::from_utf8(out.stderr).unwrap();
        assert_eq!(out.status.code(), Some(2), "`{verb}` must exit 2:\n{text}");
        assert!(text.contains("not implemented yet"), "{text}");
        assert!(text.contains(stage), "`{verb}` must name {stage}:\n{text}");
    }
}

#[test]
fn debug_without_subcommand_exits_two() {
    let out = pudu().arg("debug").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
