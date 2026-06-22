//! Install/remove apk packages directly, tracked in the build ledger so removal
//! can be dependency-gated — the logic behind `pedagog image pkg …` (clap surface
//! in the parent module).

use std::path::Path;

use miette::Result;

use crate::image::apk::{Apk, PackageManager};
use crate::image::ledger;

/// Print every installed package, annotating toolchain-owned ones.
pub fn installed(ledger: &Path) -> Result<()> {
    let state = ledger::load(ledger)?;
    for (pkg, toolchains) in state.installed_packages() {
        if toolchains.is_empty() {
            println!("{pkg}");
        } else {
            println!("{pkg} ({})", toolchains.join(", "));
        }
    }
    Ok(())
}

/// `apk add` the packages, recording them in the ledger.
pub fn install(ledger: &Path, packages: &[String]) -> Result<()> {
    let mut state = ledger::load(ledger)?;
    Apk.install(&mut state, packages)?;
    ledger::save(ledger, &state)
}

/// `apk del` the packages (dependency-gated), dropping them from the ledger.
pub fn remove(ledger: &Path, packages: &[String]) -> Result<()> {
    let mut state = ledger::load(ledger)?;
    Apk.remove(&mut state, packages)?;
    ledger::save(ledger, &state)
}
