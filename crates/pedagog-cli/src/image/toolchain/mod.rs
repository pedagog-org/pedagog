//! `pedagog image toolchain …` — clap surface for managing toolchain
//! definitions; the logic lives in [`ops`].

use std::path::PathBuf;

use clap::Subcommand;
use miette::Result;

use crate::image::ledger::DEFAULT_LEDGER;
use crate::image::toolchains::DEFAULT_TOOLCHAINS;

mod ops;

#[derive(Debug, Subcommand)]
pub enum ToolchainCommand {
    /// Register a toolchain definition, copying it into the toolchains directory.
    Register {
        /// Path to the toolchain definition (`<id>.toml`).
        file: PathBuf,
        /// Replace an already-registered toolchain.
        #[arg(long)]
        overwrite: bool,
        #[arg(long, default_value = DEFAULT_TOOLCHAINS)]
        toolchains: PathBuf,
        #[arg(long, default_value = DEFAULT_LEDGER)]
        ledger: PathBuf,
    },
    /// Unregister a toolchain, deleting its definition. Refused if it is installed
    /// unless `--force`.
    Unregister {
        /// Toolchain id.
        id: String,
        /// Unregister even if it is installed.
        #[arg(long)]
        force: bool,
        #[arg(long, default_value = DEFAULT_TOOLCHAINS)]
        toolchains: PathBuf,
        #[arg(long, default_value = DEFAULT_LEDGER)]
        ledger: PathBuf,
    },
    /// Install toolchains by id (packages, install commands, then verify). Path
    /// arguments are registered first with `--register`.
    Install {
        /// Toolchain ids, or paths to definitions (the latter need `--register`).
        #[arg(required = true)]
        targets: Vec<String>,
        /// Register any path arguments before installing them.
        #[arg(long)]
        register: bool,
        /// When registering, replace an already-registered toolchain.
        #[arg(long, requires = "register")]
        overwrite: bool,
        #[arg(long, default_value = DEFAULT_TOOLCHAINS)]
        toolchains: PathBuf,
        #[arg(long, default_value = DEFAULT_LEDGER)]
        ledger: PathBuf,
    },
    /// Health-check toolchains: confirm their packages are present and their
    /// verify commands pass. Read-only.
    Verify {
        /// Toolchain ids to verify (omit with `--all`).
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        ids: Vec<String>,
        /// Verify every installed toolchain.
        #[arg(long, short)]
        all: bool,
        #[arg(long, default_value = DEFAULT_TOOLCHAINS)]
        toolchains: PathBuf,
        #[arg(long, default_value = DEFAULT_LEDGER)]
        ledger: PathBuf,
    },
    /// List registered toolchains and whether each is installed.
    List {
        #[arg(long, default_value = DEFAULT_TOOLCHAINS)]
        toolchains: PathBuf,
        #[arg(long, default_value = DEFAULT_LEDGER)]
        ledger: PathBuf,
    },
}

impl ToolchainCommand {
    pub fn run(self) -> Result<()> {
        match self {
            ToolchainCommand::Register {
                file,
                overwrite,
                toolchains,
                ledger,
            } => ops::register(&file, overwrite, &toolchains, &ledger),
            ToolchainCommand::Unregister {
                id,
                force,
                toolchains,
                ledger,
            } => ops::unregister(&id, force, &toolchains, &ledger),
            ToolchainCommand::Install {
                targets,
                register,
                overwrite,
                toolchains,
                ledger,
            } => ops::install(&targets, register, overwrite, &toolchains, &ledger),
            ToolchainCommand::Verify {
                ids,
                all,
                toolchains,
                ledger,
            } => ops::verify(&ids, all, &toolchains, &ledger),
            ToolchainCommand::List { toolchains, ledger } => ops::list(&toolchains, &ledger),
        }
    }
}
