//! Developer inspection commands.
//!
//! Hidden and unstable: these exist to make the pipeline's intermediate
//! stages testable, and carry no compatibility promise.

use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::error::{CliError, ConfigError, render};
use crate::lock::types::Lockfile;
use crate::lock::{Graph, SUPPORTED_VERSION, parse_lockfile};
use crate::platform::constraints::constraint_labels;
use crate::platform::prune::prune;

/// Load `pudu.toml` and the lockfile it names, printing any lockfile
/// warnings to stderr.
///
/// Shared by every `pudu debug` subcommand: they all start from the same
/// two files, and the not-found/unreadable distinction below is worth
/// stating once.
fn load() -> Result<(Config, Lockfile)> {
    let config_path = Path::new("pudu.toml");
    let config_text =
        std::fs::read_to_string(config_path).map_err(|source| CliError::ConfigUnreadable {
            path: config_path.to_path_buf(),
            source,
        })?;
    let config = Config::from_str(&config_text, config_path)?;

    let base = std::env::current_dir()?;
    let lockfile_path = base.join(&config.lockfile_path);
    // Distinguish "not found" from "found but unreadable" (e.g. permissions):
    // the latter is not a missing-file problem, and telling the user to edit
    // `lockfile_path` when the path is already correct is actively wrong
    // advice.
    let lock_text = std::fs::read_to_string(&lockfile_path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            ConfigError::LockfileNotFound {
                path: lockfile_path.clone(),
            }
        } else {
            ConfigError::LockfileUnreadable {
                path: lockfile_path.clone(),
                source,
            }
        }
    })?;

    let (lockfile, warnings) = parse_lockfile(&lock_text, &lockfile_path)?;
    for w in &warnings {
        eprint!("{}", render(w));
    }
    Ok((config, lockfile))
}

/// Print the instance graph as JSON on stdout.
///
/// Warnings go to stderr via [`render`]; the JSON goes to stdout, so stdout
/// stays machine-parseable.
pub fn print_graph() -> Result<()> {
    let (_config, lockfile) = load()?;
    let graph = Graph::build(&lockfile)?;
    let out = serde_json::json!({
        // The constant, not an observation of the parsed file: `parse_lockfile`
        // already rejected anything but `SUPPORTED_VERSION` above, so this
        // field can never disagree with the binary. A test asserting
        // `== "9.0"` therefore cannot catch a regression here — it would need
        // to instead assert against the gate in `parse_lockfile`/`LockError`.
        "lockfile_version": SUPPORTED_VERSION,
        "settings": lockfile.settings,
        "roots": graph.roots,
        "nodes": graph.nodes,
        "cycles": graph.cycles,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// Print the per-platform pruning view as JSON on stdout.
///
/// Warnings go to stderr via [`render`]; the JSON goes to stdout, so stdout
/// stays machine-parseable.
///
/// Every field here is pudu's own invention rather than an echo of the
/// lockfile, so every key is `snake_case` (S1's key-spelling rule).
pub fn platforms() -> Result<()> {
    let (config, lockfile) = load()?;
    let graph = Graph::build(&lockfile)?;
    let (matrix, warnings) = prune(&graph, &config.platforms);
    for w in &warnings {
        eprint!("{}", render(w));
    }

    let mut out = serde_json::Map::new();
    for (name, platform) in &config.platforms {
        let view = &matrix.views[name];
        out.insert(
            name.clone(),
            serde_json::json!({
                "os": platform.os.as_npm(),
                "cpu": platform.cpu.as_npm(),
                "libc": platform.libc.map(|l| l.as_npm()),
                "constraints": constraint_labels(platform, &config.platforms),
                // Recorded so a user debugging a mis-selected target can see
                // the escape hatch applied without re-reading their config.
                "constraints_overridden": platform.constraints.is_some(),
                "node_count": view.nodes.len(),
                "pruned": view.pruned,
                "dropped_required_edges": view.dropped_required_edges,
            }),
        );
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({ "platforms": out }))?
    );
    Ok(())
}
