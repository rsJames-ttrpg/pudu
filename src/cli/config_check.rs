//! `pudu config check` — validate pudu.toml with no side effects.

use std::path::Path;

use crate::config::Config;

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
fn fail_early(json: bool, message: String) -> anyhow::Result<()> {
    if json {
        emit_json(std::slice::from_ref(&message), &[])?;
        anyhow::bail!("1 error(s) in pudu.toml");
    }
    Err(anyhow::anyhow!(message))
}

/// Validate `pudu.toml` in the current directory.
///
/// JSON goes to stdout in both the ok and error cases so CI can parse it;
/// human-readable errors go to stderr.
pub fn run(format: OutputFormat) -> anyhow::Result<()> {
    let json = format == OutputFormat::Json;
    let path = Path::new("pudu.toml");

    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return fail_early(json, format!("cannot read {}: {e}", path.display())),
    };

    let config = match Config::from_str(&text, path) {
        Ok(c) => c,
        Err(e) => return fail_early(json, format!("{e}: {}", e.source_message())),
    };

    let base = std::env::current_dir()?;
    let (errors, warnings) = config.validate(&base);

    if json {
        let errors: Vec<String> = errors.iter().map(|e| format!("{e:#}")).collect();
        emit_json(&errors, &warnings)?;
        if errors.is_empty() {
            return Ok(());
        }
        anyhow::bail!("{} error(s) in pudu.toml", errors.len());
    }

    for w in &warnings {
        eprintln!("warning: {w}");
    }

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("error: {e:#}");
        }
        anyhow::bail!("{} error(s) in pudu.toml", errors.len());
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
