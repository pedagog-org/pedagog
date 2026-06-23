//! Install/remove apk packages directly, tracked in the build ledger — the logic
//! behind `pedagog image pkg …` (clap surface in the parent module).

use std::path::Path;

use miette::{Result, miette};
use pedagog_core::image::ledger::Ledger;
use pedagog_core::image::toolchain::{Toolchain, package_dependencies, toolchains_requiring};

use crate::image::apk::{Apk, PackageManager};
use crate::image::ledger;
use crate::image::toolchains;

/// List every installed package; toolchain-owned ones show the toolchain(s).
pub fn installed(ledger: &Path, toolchains_dir: &Path) -> Result<()> {
    let state = ledger::load(ledger)?;
    let toolchains = installed_toolchains(&state, toolchains_dir)?;

    for (name, owners) in package_dependencies(&state.additional_packages, &toolchains) {
        if owners.is_empty() {
            println!("{name}");
        } else {
            let owners: Vec<&str> = owners.into_iter().collect();
            println!("{name} ({})", owners.join(", "));
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

/// `apk del` the packages, dropping them from the ledger. Unless `force`, refuse
/// if any package is required by an installed toolchain.
pub fn remove(ledger: &Path, toolchains_dir: &Path, packages: &[String], force: bool) -> Result<()> {
    let mut state = ledger::load(ledger)?;
    if !force {
        let installed = installed_toolchains(&state, toolchains_dir)?;
        for pkg in packages {
            let dependents = toolchains_requiring(pkg, &installed);
            if !dependents.is_empty() {
                let dependents: Vec<&str> = dependents.into_iter().collect();
                return Err(miette!(
                    "package '{pkg}' is required by installed toolchain(s): {} (use --force)",
                    dependents.join(", ")
                ));
            }
        }
    }
    Apk.remove(&mut state, packages)?;
    ledger::save(ledger, &state)
}

/// Resolve the definitions of every installed toolchain in the ledger. A
/// registered toolchain whose def is missing is skipped — it can't gate.
fn installed_toolchains(state: &Ledger, toolchains_dir: &Path) -> Result<Vec<Toolchain>> {
    let mut defs = Vec::new();
    for (id, installed) in &state.toolchains {
        if !*installed {
            continue;
        }
        if let Some(def) = toolchains::resolve(toolchains_dir, id)? {
            defs.push(def);
        }
    }
    Ok(defs)
}
