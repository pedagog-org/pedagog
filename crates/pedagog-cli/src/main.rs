//! pedagog — the in-image admin/authoring CLI (`pedagog image …`).

mod cli;
mod image;

use clap::Parser;

fn main() -> miette::Result<()> {
    cli::Cli::parse().run()
}
