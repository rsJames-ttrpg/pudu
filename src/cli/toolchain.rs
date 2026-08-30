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
    ExistingToolchain(String),
    /// Markers are unbalanced; pudu refuses to guess.
    Unparseable,
}

/// Does the file already declare a node toolchain outside pudu's block?
///
/// Deliberately conservative and textual: a false positive costs one printed
/// line of manual instruction, while a false negative produces a duplicate
/// target and a confusing Buck error.
fn existing_node_toolchain(text: &str) -> Option<String> {
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('#') {
            continue;
        }
        if l.starts_with("system_node_toolchain(") {
            return Some("node".to_string());
        }
    }
    None
}

/// Compute the new contents of `toolchains/BUCK`.
///
/// Returns `(None, outcome)` when nothing should be written.
pub fn apply(existing: Option<&str>, block: &str, force: bool) -> (Option<String>, AppendOutcome) {
    let Some(text) = existing else {
        return (Some(block.to_string()), AppendOutcome::Created);
    };

    let begin = text.find(BEGIN);
    let end = text.find(END);

    match (begin, end) {
        (Some(b), Some(e)) if e > b => {
            if !force {
                return (None, AppendOutcome::AlreadyManaged);
            }
            // Replace exactly the marked span, including END's trailing newline.
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
        (None, None) => {
            if let Some(name) = existing_node_toolchain(text) {
                return (None, AppendOutcome::ExistingToolchain(name));
            }
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
            out.push_str(block);
            (Some(out), AppendOutcome::Appended)
        }
        // Exactly one marker, or END before BEGIN: refuse to guess.
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

    #[test]
    fn never_appends_over_an_existing_node_toolchain() {
        let existing = "system_node_toolchain(name = \"node\")\n";
        let (written, outcome) = apply(Some(existing), &block(), false);
        assert!(written.is_none(), "must not rewrite a user's own toolchain");
        match outcome {
            AppendOutcome::ExistingToolchain(t) => assert_eq!(t, "node"),
            other => panic!("expected ExistingToolchain, got {other:?}"),
        }
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
    }

    #[test]
    fn is_idempotent_across_three_runs() {
        let mut current: Option<String> = None;
        for _ in 0..3 {
            let (written, _) = apply(current.as_deref(), &block(), false);
            if let Some(t) = written {
                current = Some(t);
            }
        }
        let first = current.clone().unwrap();
        let (written, outcome) = apply(current.as_deref(), &block(), false);
        assert!(written.is_none());
        assert!(matches!(outcome, AppendOutcome::AlreadyManaged));
        assert_eq!(first, current.unwrap(), "content must be stable");
    }

    #[test]
    fn unbalanced_markers_are_treated_as_unparseable() {
        let existing = format!("{BEGIN}\nno end marker\n");
        let (written, outcome) = apply(Some(&existing), &block(), true);
        assert!(written.is_none());
        assert!(matches!(outcome, AppendOutcome::Unparseable));
    }
}
