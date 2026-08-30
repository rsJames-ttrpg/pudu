use assert_cmd::Command;

fn pudu() -> Command {
    Command::cargo_bin("pudu").expect("binary builds")
}

#[test]
fn help_lists_every_phase_one_verb() {
    let out = pudu().arg("--help").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    for verb in [
        "init", "vendor", "buckify", "fixups", "audit", "unused", "config", "debug",
    ] {
        assert!(text.contains(verb), "--help must list `{verb}`:\n{text}");
    }
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
