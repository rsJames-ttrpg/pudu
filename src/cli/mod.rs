//! CLI surface and dispatch.

pub mod config_check;
pub mod init;
pub mod stub;
pub mod toolchain;

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

    /// Forbid all network access. (No effect until S3.)
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
    /// Fetch tarballs and write pudu.lock. [UNIMPLEMENTED — S3]
    Vendor {
        /// Exit non-zero if pudu.lock is stale.
        #[arg(long)]
        check: bool,
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
    // Has no subcommands at S0; S1 adds `print-graph` and S2 adds
    // `platforms`. Modelled as trailing args rather than an empty
    // `#[derive(Subcommand)]` enum, because deriving `Subcommand` on an
    // uninhabited enum does not compile. Kept out of the `///` doc comment
    // so clap does not ship this rationale to users in `--help`.
    Debug {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
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
            std::env::set_current_dir(dir).map_err(|e| {
                anyhow::anyhow!("cannot change directory to {}: {e}", dir.display())
            })?;
        }

        match self.command {
            Commands::Init { force, path } => init::run(force, path),
            Commands::Config { command } => match command {
                ConfigCommands::Check { format } => config_check::run(format),
            },
            Commands::Vendor { .. } => Err(stub::unimplemented("vendor", "S3")),
            Commands::Buckify { .. } => Err(stub::unimplemented("buckify", "S4")),
            Commands::Fixups => Err(stub::unimplemented("fixups", "S7/S8")),
            Commands::Audit => Err(stub::unimplemented("audit", "Phase 2")),
            Commands::Unused => Err(stub::unimplemented("unused", "Phase 2")),
            Commands::Debug { args } => Err(anyhow::anyhow!(
                "pudu debug requires a subcommand (none exist yet; S1 adds \
                 `print-graph`){}",
                if args.is_empty() {
                    String::new()
                } else {
                    format!(": unknown `{}`", args[0])
                }
            )),
        }
    }
}
