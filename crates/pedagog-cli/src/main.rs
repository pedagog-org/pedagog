//! pedagog — the in-image admin/authoring CLI (`pedagog image …`).

mod cli;
mod image;
mod manifest;

use clap::Parser;

fn main() -> miette::Result<()> {
    cli::Cli::parse().run()
}
