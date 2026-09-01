//! `pudu buckify` — the generated Buck2 files.
//!
//! Adds no analysis. The set of packages to emit is exactly the set
//! `pudu vendor` would fetch (the union of what survives pruning on any
//! configured platform), so `vendor::build_plan` computes it and
//! `packages::staleness` insists the table agrees before anything is
//! rendered. Design §4's "vendor is mandatory before buckify" is that check.

use std::collections::BTreeMap;

use anyhow::Result;

use crate::buck;
use crate::cli::context::load_validated;
use crate::cli::vendor::build_plan;
use crate::error::{VendorError, render};
use crate::lock::Graph;
use crate::packages::{self, Loaded};
use crate::platform::prune::prune;

pub fn run(check: bool) -> Result<()> {
    let (config, lockfile) = load_validated()?;

    let graph = Graph::build(&lockfile)?;
    let (matrix, warnings) = prune(&graph, &config.platforms);
    for w in &warnings {
        eprint!("{}", render(w));
    }

    let plan = build_plan(&graph, &matrix, &config)?;

    let base = std::env::current_dir()?;
    let third_party_dir = base.join(&config.third_party_dir);
    let table_path = third_party_dir.join("packages.toml");
    let loaded = packages::load(&table_path)?;

    // Stale or missing table fails here, before a single file is written, so
    // a half-generated tree is not a reachable state.
    let differences = packages::staleness(&plan.expected, &loaded);
    if !differences.is_empty() {
        for d in &differences {
            eprintln!("  {d}");
        }
        return Err(VendorError::Stale {
            differences: differences.iter().map(ToString::to_string).collect(),
        }
        .into());
    }

    // staleness() found no differences against `plan.expected`. When
    // `plan.expected` is non-empty that forces `loaded` to be `Present` with
    // exactly those keys — but `staleness()` iterates `expected`, so an
    // *empty* `expected` reports no differences no matter what `loaded` is,
    // including `Loaded::Absent` (a lockfile with no packages at all, and no
    // packages.toml on disk). That case is reachable, not a bug: there is
    // nothing to vendor and nothing to build, so an empty BUCK is correct.
    let empty = BTreeMap::new();
    let entries = match &loaded {
        Loaded::Present(t) => &t.entries,
        Loaded::Absent | Loaded::WrongVersion(_) => &empty,
    };

    let generated = buck::generate(entries, &config.platforms, &third_party_dir)?;

    if check {
        generated.check(&third_party_dir)?;
        eprintln!(
            "generated files are up to date ({} packages)",
            entries.len()
        );
        return Ok(());
    }

    generated.write(&third_party_dir)?;
    eprintln!(
        "wrote BUCK, pudu.bzl and config/BUCK ({} packages)",
        entries.len()
    );
    Ok(())
}
