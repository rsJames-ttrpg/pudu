//! Registration of verbs that are planned but not yet implemented.
//!
//! Keeping them in `--help` from day one makes the tool's trajectory legible
//! and lets the help snapshot lock verb names before they have behaviour.

use crate::error::CliError;

/// The error every unimplemented verb returns. `main` maps it to exit code 4.
pub fn unimplemented(verb: &str, stage: &str) -> anyhow::Error {
    CliError::Unimplemented {
        verb: verb.to_string(),
        stage: stage.to_string(),
    }
    .into()
}
