//! Developer inspection commands.
//!
//! Hidden and unstable: these exist to make the pipeline's intermediate
//! stages testable, and carry no compatibility promise.

use anyhow::Result;

use crate::cli::context::load_lenient;
use crate::error::render;
use crate::lock::{Graph, SUPPORTED_VERSION};
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
