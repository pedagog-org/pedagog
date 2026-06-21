//! The `pedagog image …` verb group.

mod network;

use clap::Subcommand;
use miette::Result;

use network::NetworkCommand;

#[derive(Debug, Subcommand)]
pub enum ImageCommand {
    /// Student egress policy.
    #[command(subcommand)]
    Network(NetworkCommand),
}

impl ImageCommand {
    pub fn run(self) -> Result<()> {
        match self {
            ImageCommand::Network(cmd) => cmd.run(),
        }
    }
}
