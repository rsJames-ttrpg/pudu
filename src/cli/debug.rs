//! Developer inspection commands.
//!
//! Hidden and unstable: these exist to make the pipeline's intermediate
//! stages testable, and carry no compatibility promise.

use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::error::{CliError, ConfigError, render};
use crate::lock::{Graph, SUPPORTED_VERSION, parse_lockfile};

/// Print the instance graph as JSON on stdout.
///
/// Warnings go to stderr via [`render`]; the JSON goes to stdout, so stdout
/// stays machine-parseable.
pub fn print_graph() -> Result<()> {
    let config_path = Path::new("pudu.toml");
    let config_text =
        std::fs::read_to_string(config_path).map_err(|source| CliError::ConfigUnreadable {
            path: config_path.to_path_buf(),
            source,
        })?;
    let config = Config::from_str(&config_text, config_path)?;

    let base = std::env::current_dir()?;
    let lockfile_path = base.join(&config.lockfile_path);
    // `ConfigError` has no dedicated "lockfile unreadable" variant; reuse
    // `LockfileNotFound`, since an unreadable lockfile is not usefully
    // distinguishable from a missing one at the CLI boundary.
    let lock_text =
        std::fs::read_to_string(&lockfile_path).map_err(|_| ConfigError::LockfileNotFound {
            path: lockfile_path.clone(),
        })?;

    let (lockfile, warnings) = parse_lockfile(&lock_text, &lockfile_path)?;
    for w in &warnings {
        eprint!("{}", render(w));
    }

    let graph = Graph::build(&lockfile)?;
    let out = serde_json::json!({
        "lockfile_version": SUPPORTED_VERSION,
        "settings": lockfile.settings,
        "roots": graph.roots,
        "nodes": graph.nodes,
        "cycles": graph.cycles,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
