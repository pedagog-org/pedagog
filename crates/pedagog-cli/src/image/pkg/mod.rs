//! `pedagog image pkg …` — clap surface for the apk package verbs; the logic
//! lives in [`ops`].

use std::path::PathBuf;

use clap::Subcommand;
use miette::Result;

use crate::image::ledger::DEFAULT_LEDGER;
use crate::image::toolchains::DEFAULT_TOOLCHAINS;

mod ops;

#[derive(Debug, Subcommand)]
pub enum PkgCommand {
    /// List every installed package; toolchain-owned ones show the toolchain(s).
    Installed {
        #[arg(long, default_value = DEFAULT_TOOLCHAINS)]
        toolchains: PathBuf,
        #[arg(long, default_value = DEFAULT_LEDGER)]
        ledger: PathBuf,
    },
    /// Install apk packages and record them in the ledger.
    Install {
        /// Packages to install.
        packages: Vec<String>,
        #[arg(long, default_value = DEFAULT_LEDGER)]
        ledger: PathBuf,
    },
    /// Remove packages and drop them from the ledger. Refused if an installed
    /// toolchain depends on one, unless `--force`.
    Remove {
        /// Packages to remove.
        packages: Vec<String>,
        /// Remove even if an installed toolchain depends on a package.
        #[arg(long)]
        force: bool,
        #[arg(long, default_value = DEFAULT_TOOLCHAINS)]
        toolchains: PathBuf,
        #[arg(long, default_value = DEFAULT_LEDGER)]
        ledger: PathBuf,
    },
}

impl PkgCommand {
    pub fn run(self) -> Result<()> {
        match self {
            PkgCommand::Installed { toolchains, ledger } => ops::installed(&ledger, &toolchains),
            PkgCommand::Install { packages, ledger } => ops::install(&ledger, &packages),
            PkgCommand::Remove {
                packages,
                force,
                toolchains,
                ledger,
            } => ops::remove(&ledger, &toolchains, &packages, force),
        }
    }
}
