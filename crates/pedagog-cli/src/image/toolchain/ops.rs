//! Register, unregister, and list toolchain definitions — the logic behind
//! `pedagog image toolchain …`. The toolchains-directory copy and the ledger
//! accounting live in [`crate::image::toolchains`]; here we load/save the ledger
//! around it and report.

use std::path::Path;

use miette::Result;

use crate::image::ledger;
use crate::image::toolchains;

/// Register the definition at `file`: copy it into the toolchains directory under
/// its own id and record it (uninstalled) in the ledger.
pub fn register(file: &Path, overwrite: bool, dir: &Path, ledger_path: &Path) -> Result<()> {
    let mut led = ledger::load(ledger_path)?;
    let id = toolchains::register(dir, &mut led, file, overwrite)?;
    ledger::save(ledger_path, &led)?;

    println!("registered toolchain '{id}'");
    Ok(())
}

/// Unregister `id`: refuse if it is installed unless `force`, then delete its
/// definition and drop it from the ledger.
pub fn unregister(id: &str, force: bool, dir: &Path, ledger_path: &Path) -> Result<()> {
    let mut led = ledger::load(ledger_path)?;
    toolchains::unregister(dir, &mut led, id, force)?;
    ledger::save(ledger_path, &led)?;

    println!("unregistered toolchain '{id}'");
    Ok(())
}

/// List registered toolchains, each annotated with whether it is installed.
pub fn list(dir: &Path, ledger_path: &Path) -> Result<()> {
    let ids = toolchains::list(dir)?;
    let led = ledger::load(ledger_path)?;
    for id in ids {
        let status = if led.is_installed(&id) {
            "installed"
        } else {
            "registered"
        };
        println!("{id} ({status})");
    }
    Ok(())
}
