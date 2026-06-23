//! The `pedagog image …` verb group. Each verb is a folder (`mod.rs` = clap
//! surface, `ops.rs` = logic); `apk`/`ledger`/`manifest`/`shell`/`toolchains`
//! are shared helpers.

mod apk;
mod ledger;
mod manifest;
mod network;
mod pkg;
mod shell;
mod toolchain;
mod toolchains;

use clap::Subcommand;
use miette::Result;

use network::NetworkCommand;
use pkg::PkgCommand;
use toolchain::ToolchainCommand;

#[derive(Debug, Subcommand)]
pub enum ImageCommand {
    /// Student egress policy.
    #[command(subcommand)]
    Network(NetworkCommand),
    /// apk packages, tracked in the build ledger.
    #[command(subcommand)]
    Pkg(PkgCommand),
    /// Register and list toolchain definitions.
    #[command(subcommand)]
    Toolchain(ToolchainCommand),
}

impl ImageCommand {
    pub fn run(self) -> Result<()> {
        match self {
            ImageCommand::Network(cmd) => cmd.run(),
            ImageCommand::Pkg(cmd) => cmd.run(),
            ImageCommand::Toolchain(cmd) => cmd.run(),
        }
    }
}
