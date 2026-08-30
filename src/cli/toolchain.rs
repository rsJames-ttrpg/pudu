//! The `toolchains/BUCK` append state machine.
//!
//! The Buck2 prelude ships no Node toolchain, so pudu supplies one. Declining
//! to write it would force a manual step on every user, so pudu writes into a
//! user-owned file — which muntjac deliberately refused to do. The safety
//! machinery that requires lives here (spec §3.3):
//!
//! * nothing outside the markers is ever modified;
//! * an existing node toolchain is never overwritten;
//! * repeated runs converge (idempotent).
//!
//! [`apply`] is a pure function over file contents so every case is unit
//! testable without touching the filesystem.

pub const BEGIN: &str = "# --- begin pudu-managed (do not edit inside this block) ---";
pub const END: &str = "# --- end pudu-managed ---";

/// The marker-delimited block pudu owns inside `toolchains/BUCK`.
pub fn managed_block(toolchain_bzl_label: &str) -> String {
    format!(
        "{BEGIN}\nload(\"{toolchain_bzl_label}\", \"system_node_toolchain\")\n\
         system_node_toolchain(name = \"node\", visibility = [\"PUBLIC\"])\n{END}\n"
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum AppendOutcome {
    /// The file did not exist; it was created containing only the block.
    Created,
    /// The block was appended after existing content.
    Appended,
    /// A current managed block is already present; nothing to do.
    AlreadyManaged,
    /// `--force` replaced the contents of an existing managed block.
    Replaced,
    /// A node toolchain the user owns was found; pudu did not write.
    /// `parsed` is false when the target name could not be read out of the
    /// call and the fallback `"node"` is being reported instead.
    ExistingToolchain { name: String, parsed: bool },
    /// Markers are unbalanced; pudu refuses to guess.
    Unparseable,
}

/// Does the file already declare a node toolchain outside pudu's block?
///
/// Returns the target name from the call's `name = "..."` argument, and
/// whether that name was actually parsed out (`false` means the fallback
/// `"node"` is being reported, and the caller should say so).
///
/// Deliberately conservative and textual: a false positive costs one printed
/// line of manual instruction, while a false negative produces a duplicate
/// target and a confusing Buck error.
fn existing_node_toolchain(text: &str) -> Option<(String, bool)> {
    const NAME: &str = "system_node_toolchain";
    for (offset, line) in line_offsets(text) {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        // Look for the call anywhere on the line (e.g. `x =
        // system_node_toolchain(...)`), tolerating whitespace before the
        // opening paren (e.g. `system_node_toolchain (...)`).
        let Some(idx) = line.find(NAME) else {
            continue;
        };
        let after = &line[idx + NAME.len()..];
        if !after.trim_start().starts_with('(') {
            continue;
        }
        // Arguments may wrap over several lines, so scan the rest of the
        // file from the opening paren rather than just the rest of the line.
        let paren = offset + idx + NAME.len() + (after.len() - after.trim_start().len());
        let rest = &text[paren + 1..];
        let args = match rest.find(')') {
            Some(close) => &rest[..close],
            None => rest,
        };
        return Some(match parse_name_argument(args) {
            Some(name) => (name, true),
            None => ("node".to_string(), false),
        });
    }
    None
}

/// Line contents paired with their byte offset in `text`.
fn line_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    text.split_inclusive('\n').map(move |raw| {
        let start = offset;
        offset += raw.len();
        (start, raw.trim_end_matches(['\n', '\r']))
    })
}

/// Pull `"..."` out of a `name = "..."` keyword argument.
fn parse_name_argument(args: &str) -> Option<String> {
    let mut rest = args;
    while let Some(idx) = rest.find("name") {
        let before_is_boundary = rest[..idx]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let after = rest[idx + "name".len()..].trim_start();
        if before_is_boundary && let Some(value) = after.strip_prefix('=') {
            let value = value.trim_start();
            for quote in ['"', '\''] {
                if let Some(v) = value.strip_prefix(quote)
                    && let Some(end) = v.find(quote)
                    && !v[..end].is_empty()
                {
                    return Some(v[..end].to_string());
                }
            }
            return None;
        }
        rest = &rest[idx + "name".len()..];
    }
    None
}

/// Compute the new contents of `toolchains/BUCK`.
///
/// Returns `(None, outcome)` when nothing should be written.
pub fn apply(existing: Option<&str>, block: &str, force: bool) -> (Option<String>, AppendOutcome) {
    let text = match existing {
        None => return (Some(block.to_string()), AppendOutcome::Created),
        // An empty file is the same logical state as an absent one: produce
        // byte-identical output either way.
        Some("") => return (Some(block.to_string()), AppendOutcome::Created),
        Some(text) => text,
    };

    let begin_count = text.matches(BEGIN).count();
    let end_count = text.matches(END).count();
    let begin = text.find(BEGIN);
    let end = text.find(END);

    match (begin_count, end_count, begin, end) {
        (1, 1, Some(b), Some(e)) if e > b => {
            if !force {
                return (None, AppendOutcome::AlreadyManaged);
            }
            // Replace exactly the marked span, including END's trailing
            // newline. Using the FIRST occurrence's offsets is only sound
            // because the `(1, 1, ..)` gate above proves each marker occurs
            // exactly once, so first == only.
            let mut tail = e + END.len();
            if text[tail..].starts_with('\n') {
                tail += 1;
            }
            let mut out = String::with_capacity(text.len() + block.len());
            out.push_str(&text[..b]);
            out.push_str(block);
            out.push_str(&text[tail..]);
            (Some(out), AppendOutcome::Replaced)
        }
        (0, 0, None, None) => {
            if let Some((name, parsed)) = existing_node_toolchain(text) {
                return (None, AppendOutcome::ExistingToolchain { name, parsed });
            }
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
            out.push_str(block);
            (Some(out), AppendOutcome::Appended)
        }
        // Anything else is unbalanced: not exactly one BEGIN and one END, or
        // END before BEGIN. Refuse to guess — a false Unparseable costs one
        // printed instruction; a false Replaced/Appended can destroy content.
        _ => (None, AppendOutcome::Unparseable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block() -> String {
        managed_block("root//third-party/js:toolchains.bzl")
    }

    #[test]
    fn creates_the_file_when_absent() {
        let (written, outcome) = apply(None, &block(), false);
        assert!(matches!(outcome, AppendOutcome::Created));
        let text = written.unwrap();
        assert!(text.contains(BEGIN) && text.contains(END));
        assert!(text.contains("system_node_toolchain"));
    }

    #[test]
    fn appends_to_a_file_without_a_node_toolchain() {
        let existing = "system_python_toolchain(name = \"python\")\n";
        let (written, outcome) = apply(Some(existing), &block(), false);
        assert!(matches!(outcome, AppendOutcome::Appended));
        let text = written.unwrap();
        assert!(
            text.starts_with(existing),
            "existing content must be preserved verbatim"
        );
        assert!(text.contains(BEGIN));
    }

    /// M4: exact-content check. `contains`-based assertions can't see a
    /// missing (or extra) blank line before the appended block.
    #[test]
    fn append_produces_exact_expected_bytes() {
        let existing = "system_python_toolchain(name = \"python\")\n";
        let (written, outcome) = apply(Some(existing), &block(), false);
        assert!(matches!(outcome, AppendOutcome::Appended));
        let expected = format!("{existing}\n{}", block());
        assert_eq!(written.unwrap(), expected);
    }

    #[test]
    fn never_appends_over_an_existing_node_toolchain() {
        let existing = "system_node_toolchain(name = \"node\")\n";
        let (written, outcome) = apply(Some(existing), &block(), false);
        assert!(written.is_none(), "must not rewrite a user's own toolchain");
        match outcome {
            AppendOutcome::ExistingToolchain { name, .. } => assert_eq!(name, "node"),
            other => panic!("expected ExistingToolchain, got {other:?}"),
        }
    }

    /// MINOR 5: false negatives are the expensive direction (a duplicate
    /// Buck target and a confusing error), so unusual-but-plausible spacing
    /// and a bound-to-a-variable call must still be detected.
    #[test]
    fn detects_node_toolchain_with_unusual_spacing_or_assignment() {
        for existing in [
            "system_node_toolchain (name = \"node\")\n",
            "x = system_node_toolchain(name = \"node\")\n",
        ] {
            let (written, outcome) = apply(Some(existing), &block(), false);
            assert!(written.is_none(), "must not write over {existing:?}");
            assert!(
                matches!(outcome, AppendOutcome::ExistingToolchain { .. }),
                "expected ExistingToolchain for {existing:?}, got {outcome:?}"
            );
        }
    }

    /// M2: a commented-out declaration must not count as a real one — the
    /// doc comment on `existing_node_toolchain` calls this out explicitly.
    #[test]
    fn commented_out_node_toolchain_is_not_detected() {
        let existing = "# system_node_toolchain(name = \"node\")\n";
        let (written, outcome) = apply(Some(existing), &block(), false);
        assert!(matches!(outcome, AppendOutcome::Appended));
        assert!(written.is_some());
    }

    #[test]
    fn managed_block_present_is_left_alone_without_force() {
        let existing = format!("x = 1\n{}", block());
        let (written, outcome) = apply(Some(&existing), &block(), false);
        assert!(written.is_none());
        assert!(matches!(outcome, AppendOutcome::AlreadyManaged));
    }

    #[test]
    fn force_replaces_only_the_managed_span() {
        let existing = format!("before = 1\n{}after = 2\n", block());
        let stale = format!("{BEGIN}\nstale content\n{END}\n");
        let existing = existing.replace(&block(), &stale);

        let (written, outcome) = apply(Some(&existing), &block(), true);
        assert!(matches!(outcome, AppendOutcome::Replaced));
        let text = written.unwrap();
        assert!(
            text.contains("before = 1"),
            "content before the block must survive"
        );
        assert!(
            text.contains("after = 2"),
            "content after the block must survive"
        );
        assert!(!text.contains("stale content"));
        assert!(text.contains("system_node_toolchain"));

        // M3: exact-content check. `contains` alone can't see an extra
        // blank line left behind if END's trailing newline isn't consumed.
        let expected = format!("before = 1\n{}after = 2\n", block());
        assert_eq!(text, expected);
    }

    /// CRITICAL 1 (reviewer probe): two BEGIN markers and one END is
    /// unbalanced. Presence-only detection (`find`, first occurrence) would
    /// route this to the replace path and destroy everything between the
    /// first BEGIN and the END — including content the user owns that sits
    /// between the two BEGIN lines. Must be Unparseable, and must not write.
    #[test]
    fn two_begin_markers_and_one_end_are_unparseable() {
        let existing = format!(
            "{BEGIN}\n\
             load(\"old\", \"system_node_toolchain\")\n\
             MY_IMPORTANT_RULE = 1\n\
             system_python_toolchain(name = \"python\")\n\
             {BEGIN}\n\
             load(\"new\", \"system_node_toolchain\")\n\
             {END}\n\
             after = 2\n"
        );
        let (written, outcome) = apply(Some(&existing), &block(), true);
        assert!(
            written.is_none(),
            "must not write when markers are unbalanced"
        );
        assert!(matches!(outcome, AppendOutcome::Unparseable));
    }

    /// IMPORTANT 2: two *complete* managed blocks. Buck would reject the
    /// duplicate `system_node_toolchain` target; pudu must refuse to guess
    /// which one is authoritative rather than silently picking one.
    #[test]
    fn two_complete_managed_blocks_are_unparseable() {
        let existing = format!("{}middle = 1\n{}", block(), block());
        let (written, outcome) = apply(Some(&existing), &block(), false);
        assert!(written.is_none());
        assert!(matches!(outcome, AppendOutcome::Unparseable));

        let (written, outcome) = apply(Some(&existing), &block(), true);
        assert!(
            written.is_none(),
            "force must not pick one of two blocks to rewrite"
        );
        assert!(matches!(outcome, AppendOutcome::Unparseable));
    }

    /// M1: END appearing before BEGIN, with exactly one of each, must not
    /// slip past the marker-count check into the replace arm.
    #[test]
    fn end_marker_before_begin_marker_is_unparseable() {
        let existing = format!("{END}\nmiddle\n{BEGIN}\n");
        let (written, outcome) = apply(Some(&existing), &block(), true);
        assert!(written.is_none());
        assert!(matches!(outcome, AppendOutcome::Unparseable));
    }

    /// IMPORTANT 4: the original version of this test cloned `first` from
    /// `current` and then never reassigned `current`, so the final
    /// `assert_eq!` compared a value against itself and could not fail by
    /// construction. This version drives three real `apply` calls off a
    /// fixed baseline and asserts each of the latter two actually reports
    /// "no change needed" rather than just not panicking.
    #[test]
    fn is_idempotent_across_three_runs() {
        let (first_written, first_outcome) = apply(None, &block(), false);
        assert!(matches!(first_outcome, AppendOutcome::Created));
        let stable = first_written.unwrap();

        for _ in 0..2 {
            let (written, outcome) = apply(Some(&stable), &block(), false);
            assert!(written.is_none(), "content must not change on repeat runs");
            assert!(matches!(outcome, AppendOutcome::AlreadyManaged));
        }
    }

    /// IMPORTANT 4: the force path is the one that could grow the file on
    /// every run (each run replaces the span with a fresh copy of `block`);
    /// assert it converges rather than accumulating bytes.
    #[test]
    fn is_idempotent_across_three_forced_runs() {
        let mut current: Option<String> = None;
        let mut lengths = Vec::new();
        for _ in 0..3 {
            let (written, _) = apply(current.as_deref(), &block(), true);
            current = written;
            lengths.push(current.as_ref().unwrap().len());
        }
        assert_eq!(
            lengths[1], lengths[2],
            "forced re-apply must not grow the file run over run"
        );
        let stable = current.clone().unwrap();

        let (written, outcome) = apply(current.as_deref(), &block(), true);
        assert!(matches!(outcome, AppendOutcome::Replaced));
        assert_eq!(
            written.unwrap(),
            stable,
            "forced runs must converge to identical bytes"
        );
    }

    /// MINOR 6: an empty existing file and an absent file are the same
    /// logical state; they must produce byte-identical output.
    #[test]
    fn empty_existing_file_matches_absent_file() {
        let (from_absent, absent_outcome) = apply(None, &block(), false);
        let (from_empty, empty_outcome) = apply(Some(""), &block(), false);
        assert_eq!(from_absent, from_empty);
        assert_eq!(absent_outcome, empty_outcome);
    }

    /// I1: the reported target name must be the one actually declared in
    /// the file — `pudu.toml`'s `node_toolchain` label is derived from it,
    /// so a hardcoded "node" would point at a target that does not exist.
    #[test]
    fn reports_the_parsed_target_name() {
        for (text, want) in [
            ("system_node_toolchain(name = \"my_node\")\n", "my_node"),
            (
                "system_node_toolchain (name=\"tight\", visibility = [])\n",
                "tight",
            ),
            (
                "x = system_node_toolchain(\n    name = \"multi\",\n)\n",
                "multi",
            ),
            (
                "system_node_toolchain(\n    node = \"/opt/node\",\n    name = \"after\",\n)\n",
                "after",
            ),
        ] {
            let (written, outcome) = apply(Some(text), &block(), false);
            assert!(written.is_none(), "must not write: {text}");
            assert_eq!(
                outcome,
                AppendOutcome::ExistingToolchain {
                    name: want.to_string(),
                    parsed: true,
                },
                "for {text}"
            );
        }
    }

    /// I1: when the name cannot be read out of the call, fall back to
    /// "node" but record that it was a fallback so the caller can say so.
    #[test]
    fn unparseable_target_name_falls_back_to_node() {
        for text in [
            "system_node_toolchain(name = NAME)\n",
            "system_node_toolchain(**kwargs)\n",
            "system_node_toolchain(name = \"\")\n",
        ] {
            let (written, outcome) = apply(Some(text), &block(), false);
            assert!(written.is_none());
            assert_eq!(
                outcome,
                AppendOutcome::ExistingToolchain {
                    name: "node".to_string(),
                    parsed: false,
                },
                "for {text}"
            );
        }
    }

    #[test]
    fn unbalanced_markers_are_treated_as_unparseable() {
        let existing = format!("{BEGIN}\nno end marker\n");
        let (written, outcome) = apply(Some(&existing), &block(), true);
        assert!(written.is_none());
        assert!(matches!(outcome, AppendOutcome::Unparseable));
    }
}
