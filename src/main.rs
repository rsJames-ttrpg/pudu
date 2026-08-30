//! pudu CLI entrypoint.

use clap::Parser;

use pudu::cli::Cli;
use pudu::error::{CliError, ExitCode, exit_code, render_cli};

fn main() {
    // Diagnostics are rendered through miette (spec §6), so `code(...)` and
    // `help(...)` reach the user; the exit code classifies the failure for CI
    // (spec §6.1). Clap exits 2 on its own for a bad command line.
    let code = match Cli::parse().run() {
        Ok(()) => ExitCode::Ok,
        Err(e) => {
            // A subcommand that already printed its own diagnostics returns a
            // summary purely to carry the exit code; rendering it here would
            // repeat what the user just read.
            if !e
                .downcast_ref::<CliError>()
                .is_some_and(CliError::already_reported)
            {
                eprint!("{}", render_cli(&e));
            }
            exit_code(&e)
        }
    };
    std::process::exit(code.as_i32());
}
