//! `pudu config check` — validate pudu.toml with no side effects.

use std::path::Path;

use miette::Diagnostic;

use crate::config::Config;
use crate::error::{CliError, full_message, render};

/// Output format for `pudu config check`.
///
/// A `ValueEnum` rather than a string so clap rejects unknown formats before
/// any work happens (and documents the valid ones in `--help`): reading
/// `pudu.toml` first made `--format xml` report a missing config instead of
/// the bad format.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

/// Print the JSON envelope CI consumes.
fn emit_json(errors: &[String], warnings: &[String]) -> anyhow::Result<()> {
    let obj = serde_json::json!({
        "ok": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
    });
    println!("{}", serde_json::to_string_pretty(&obj)?);
    Ok(())
}

/// Report a failure that happened before validation could run.
///
/// In JSON mode this still goes out as the envelope: a missing or malformed
/// `pudu.toml` is exactly the case `--format json` exists for, and emitting
/// nothing on stdout there makes `pudu config check --format json | jq -e .ok`
/// a parse error rather than `false`.
///
/// The returned error is what `main` renders and classifies; in JSON mode it
/// is the count summary, since the detail already went out as JSON.
fn fail_early<E>(json: bool, err: E) -> anyhow::Error
where
    E: Diagnostic + Send + Sync + 'static,
{
    if !json {
        return err.into();
    }
    // TD-S0-16: `full_message` joins the cause chain, so an error with no
    // `#[source]` is reported exactly once.
    if let Err(e) = emit_json(&[full_message(&err)], &[]) {
        return e;
    }
    CliError::ConfigInvalid { count: 1 }.into()
}

/// Validate `pudu.toml` in the current directory.
///
/// JSON goes to stdout in both the ok and error cases so CI can parse it;
/// human-readable diagnostics go to stderr, rendered through miette so
/// `code` and `help` reach the user (spec §6).
pub fn run(format: OutputFormat) -> anyhow::Result<()> {
    let json = format == OutputFormat::Json;
    let path = Path::new("pudu.toml");

    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(source) => {
            return Err(fail_early(
                json,
                CliError::ConfigUnreadable {
                    path: path.to_path_buf(),
                    source,
                },
            ));
        }
    };

    let config = match Config::from_str(&text, path) {
        Ok(c) => c,
        Err(e) => return Err(fail_early(json, e)),
    };

    let base = std::env::current_dir()?;
    let (errors, warnings) = config.validate(&base);

    if json {
        let errors: Vec<String> = errors.iter().map(|e| full_message(e)).collect();
        let warnings: Vec<String> = warnings.iter().map(ToString::to_string).collect();
        emit_json(&errors, &warnings)?;
        if errors.is_empty() {
            return Ok(());
        }
        return Err(CliError::ConfigInvalid {
            count: errors.len(),
        }
        .into());
    }

    for w in &warnings {
        eprint!("{}", render(w));
    }

    if !errors.is_empty() {
        for e in &errors {
            eprint!("{}", render(e));
        }
        return Err(CliError::ConfigInvalid {
            count: errors.len(),
        }
        .into());
    }

    let names: Vec<&str> = config.platforms.keys().map(String::as_str).collect();
    println!(
        "pudu.toml ok: {} platform{} ({})",
        names.len(),
        if names.len() == 1 { "" } else { "s" },
        names.join(", ")
    );
    Ok(())
}
