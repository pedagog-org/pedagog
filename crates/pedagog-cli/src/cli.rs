//! The top-level `pedagog` command surface (clap) and its dispatch.

use clap::{Parser, Subcommand};
use miette::Result;

use crate::image::ImageCommand;

/// Instructor/admin authoring CLI for a pedagog assignment image.
#[derive(Debug, Parser)]
#[command(name = "pedagog", version, about)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Author and inspect the assignment image.
    #[command(subcommand)]
    Image(ImageCommand),
}

impl Cli {
    pub fn run(self) -> Result<()> {
        match self.command {
            Command::Image(cmd) => cmd.run(),
        }
    }
}
