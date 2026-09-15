//! Developer inspection commands.
//!
//! Hidden and unstable: these exist to make the pipeline's intermediate
//! stages testable, and carry no compatibility promise.

use anyhow::Result;

use crate::cli::context::load_lenient;
use crate::error::render;
use crate::lock::Graph;
use crate::platform::constraints::constraint_labels;
use crate::platform::prune::prune;

/// Print the instance graph as JSON on stdout.
///
/// Warnings go to stderr via [`render`]; the JSON goes to stdout, so stdout
/// stays machine-parseable.
pub fn print_graph() -> Result<()> {
    let (_config, lockfile) = load_lenient()?;
    let graph = Graph::build(&lockfile)?;
    let out = serde_json::json!({
        // The version `parse_lockfile` actually observed in the file (set
        // on `Lockfile::lockfile_version`), not the `SUPPORTED_VERSION`
        // constant — today the two can never disagree, since anything but
        // `SUPPORTED_VERSION` is rejected before this point, but reporting
        // the observation rather than the constant means this field (and a
        // test asserting its value) is capable of catching a regression
        // once/if a second lockfile version is ever supported.
        "lockfile_version": lockfile.lockfile_version,
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
    let (config, lockfile) = load_lenient()?;
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
