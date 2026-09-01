//! CLI surface and dispatch.

pub mod config_check;
pub mod context;
pub mod debug;
pub mod init;
pub mod stub;
pub mod toolchain;
pub mod vendor;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "pudu",
    version,
    about = "Translate pnpm-lock.yaml into Buck2 build rules"
)]
pub struct Cli {
    /// Change to this directory before running.
    #[arg(short = 'C', global = true, value_name = "PATH")]
    pub directory: Option<PathBuf>,

    /// Increase log verbosity (-v, -vv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Forbid all network access.
    #[arg(long, global = true)]
    pub no_network: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Scaffold a pudu project.
    Init {
        /// Overwrite existing files.
        #[arg(long)]
        force: bool,
        /// Directory to scaffold (default: current directory).
        path: Option<PathBuf>,
    },
    /// Inspect pudu.toml.
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Fetch tarballs and write pudu.lock.
    Vendor {
        /// Exit non-zero if pudu.lock is stale.
        #[arg(long)]
        check: bool,
        /// Maximum parallel downloads.
        #[arg(long, value_name = "N", default_value_t = 8)]
        jobs: usize,
    },
    /// Emit BUCK, pudu.bzl, and config/BUCK. [UNIMPLEMENTED — S4]
    Buckify {
        /// Exit non-zero if generated files are stale.
        #[arg(long)]
        check: bool,
    },
    /// Manage the community fixup registry. [UNIMPLEMENTED — S7/S8]
    Fixups,
    /// Cross-check the lockfile against advisories. [UNIMPLEMENTED — Phase 2]
    Audit,
    /// Report unreferenced vendored tarballs. [UNIMPLEMENTED — Phase 2]
    Unused,
    /// Developer inspection commands.
    #[command(hide = true)]
    Debug {
        #[command(subcommand)]
        command: DebugCommands,
    },
}

/// Subcommands of the hidden `pudu debug` surface. No stability promise.
#[derive(Subcommand, Debug)]
pub enum DebugCommands {
    /// Print the instance graph as JSON.
    PrintGraph,
    /// Print the per-platform pruning view as JSON.
    Platforms,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Validate pudu.toml.
    Check {
        /// Output format.
        #[arg(long, value_name = "FORMAT", value_enum, default_value_t = config_check::OutputFormat::Human)]
        format: config_check::OutputFormat,
    },
}

impl Cli {
    pub fn run(self) -> anyhow::Result<()> {
        if let Some(dir) = &self.directory {
            // Typed, so a bad `-C` exits 2 like pudu's other usage refusals
            // (spec §6.1) and prints with a `code` like every other
            // diagnostic, rather than falling through to the unclassified 1.
            std::env::set_current_dir(dir).map_err(|source| {
                crate::error::CliError::BadDirectory {
                    path: dir.clone(),
                    source,
                }
            })?;
        }

        let no_network = self.no_network;
        let verbose = self.verbose > 0;
        match self.command {
            Commands::Init { force, path } => init::run(force, path),
            Commands::Config { command } => match command {
                ConfigCommands::Check { format } => config_check::run(format),
            },
            Commands::Vendor { check, jobs } => vendor::run(check, jobs, no_network, verbose),
            Commands::Buckify { .. } => Err(stub::unimplemented("buckify", "S4")),
            Commands::Fixups => Err(stub::unimplemented("fixups", "S7/S8")),
            Commands::Audit => Err(stub::unimplemented("audit", "Phase 2")),
            Commands::Unused => Err(stub::unimplemented("unused", "Phase 2")),
            Commands::Debug { command } => match command {
                DebugCommands::PrintGraph => debug::print_graph(),
                DebugCommands::Platforms => debug::platforms(),
            },
        }
    }
}
