//! `pedagog image pkg …` — clap surface for the apk package verbs; the logic
//! lives in [`ops`].

use std::path::PathBuf;

use clap::Subcommand;
use miette::Result;

use crate::image::ledger::DEFAULT_LEDGER;

mod ops;

#[derive(Debug, Subcommand)]
pub enum PkgCommand {
    /// List every installed package; toolchain-owned ones show the toolchain(s).
    Installed {
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
    /// Remove packages. Refused if an installed toolchain depends on one.
    Remove {
        /// Packages to remove.
        packages: Vec<String>,
        #[arg(long, default_value = DEFAULT_LEDGER)]
        ledger: PathBuf,
    },
}

impl PkgCommand {
    pub fn run(self) -> Result<()> {
        match self {
            PkgCommand::Installed { ledger } => ops::installed(&ledger),
            PkgCommand::Install { packages, ledger } => ops::install(&ledger, &packages),
            PkgCommand::Remove { packages, ledger } => ops::remove(&ledger, &packages),
        }
    }
}
