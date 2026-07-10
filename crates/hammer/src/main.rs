mod build;
mod cli;
mod plan;
mod recipe;
mod resolve;

use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    cli::Cli::parse().run()
}
