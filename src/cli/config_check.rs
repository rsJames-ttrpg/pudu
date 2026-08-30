//! `pudu config check` — validate pudu.toml with no side effects.

use std::path::Path;

use crate::config::Config;

/// Validate `pudu.toml` in the current directory.
///
/// `format` is `"human"` or `"json"`. JSON goes to stdout in both the ok and
/// error cases so CI can parse it; human-readable errors go to stderr.
pub fn run(format: &str) -> anyhow::Result<()> {
    let path = Path::new("pudu.toml");
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;

    let config = Config::from_str(&text, path)?;
    let base = std::env::current_dir()?;
    let (errors, warnings) = config.validate(&base);

    let json = match format {
        "human" => false,
        "json" => true,
        other => anyhow::bail!("unknown --format `{other}` (expected \"human\" or \"json\")"),
    };

    if json {
        let obj = serde_json::json!({
            "ok": errors.is_empty(),
            "errors": errors.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
            "warnings": warnings,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
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
