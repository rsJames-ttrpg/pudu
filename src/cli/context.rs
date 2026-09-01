//! Loading the two files every command starts from.
//!
//! Two variants deliberately. `pudu debug` predates config validation and
//! reads whatever parses, so a developer can inspect a half-finished config;
//! `pudu vendor` fetches over the network from `[registry]`, so an invalid
//! registry URL has to be rejected before it is used.

use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::error::{CliError, ConfigError, render};
use crate::lock::parse_lockfile;
use crate::lock::types::Lockfile;

fn read_config() -> Result<Config> {
    let config_path = Path::new("pudu.toml");
    let config_text =
        std::fs::read_to_string(config_path).map_err(|source| CliError::ConfigUnreadable {
            path: config_path.to_path_buf(),
            source,
        })?;
    Ok(Config::from_str(&config_text, config_path)?)
}

fn read_lockfile(config: &Config) -> Result<Lockfile> {
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
    Ok(lockfile)
}

/// Load without validating `pudu.toml`. Used by `pudu debug`.
pub fn load_lenient() -> Result<(Config, Lockfile)> {
    let config = read_config()?;
    let lockfile = read_lockfile(&config)?;
    Ok((config, lockfile))
}

/// Load and validate. Validation errors are printed here, so the returned
/// `CliError::ConfigInvalid` is `already_reported` and `main` does not repeat
/// them.
pub fn load_validated() -> Result<(Config, Lockfile)> {
    let config = read_config()?;

    let base = std::env::current_dir()?;
    let (errors, warnings) = config.validate(&base);
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

    let lockfile = read_lockfile(&config)?;
    Ok((config, lockfile))
}
