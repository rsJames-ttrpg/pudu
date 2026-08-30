//! Registration of verbs that are planned but not yet implemented.
//!
//! Keeping them in `--help` from day one makes the tool's trajectory legible
//! and lets the help snapshot lock verb names before they have behaviour.

/// The error every unimplemented verb returns. `main` maps it to exit code 2.
pub fn unimplemented(verb: &str, stage: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "pudu {verb} is not implemented yet (planned for {stage}); \
         see https://github.com/rsJames-ttrpg/pudu"
    )
}
