//! pudu CLI entrypoint.

use clap::Parser;

use pudu::cli::Cli;

fn main() {
    if let Err(e) = Cli::parse().run() {
        eprintln!("error: {e:#}");
        std::process::exit(2);
    }
}
