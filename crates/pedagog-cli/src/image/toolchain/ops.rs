//! Register, unregister, and list toolchain definitions — the logic behind
//! `pedagog image toolchain …`. The toolchains-directory copy and the ledger
//! accounting live in [`crate::image::toolchains`]; here we load/save the ledger
//! around it and report.

use std::path::Path;

use miette::{Result, miette};

use pedagog_core::image::ledger::Ledger;

use crate::image::apk::Apk;
use crate::image::ledger;
use crate::image::shell::Sh;
use crate::image::toolchains::{self, InstallOutcome};

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

/// Install each target in order. A target that looks like a definition file
/// (path-like) is registered first — only with `--register` — then installed; a
/// bare id must already be registered. Stops at the first failure, persisting the
/// ledger with whatever succeeded.
pub fn install(
    targets: &[String],
    register: bool,
    overwrite: bool,
    dir: &Path,
    ledger_path: &Path,
) -> Result<()> {
    let mut led = ledger::load(ledger_path)?;
    let pm = Apk;
    let sh = Sh;

    let result = install_all(&pm, &sh, &mut led, targets, register, overwrite, dir);
    // Persist successes even on a mid-run failure; surface the install error first.
    let saved = ledger::save(ledger_path, &led);
    result.and(saved)
}

/// Resolve and install each target in order, returning at the first failure.
fn install_all(
    pm: &Apk,
    sh: &Sh,
    led: &mut Ledger,
    targets: &[String],
    register: bool,
    overwrite: bool,
    dir: &Path,
) -> Result<()> {
    for target in targets {
        let id = resolve_target(led, dir, target, register, overwrite)?;
        let tc = toolchains::resolve(dir, &id)?
            .ok_or_else(|| miette!("toolchain '{id}' is not registered (register it first)"))?;
        match toolchains::install(pm, sh, led, &tc)? {
            InstallOutcome::Installed => println!("installed toolchain '{id}'"),
            InstallOutcome::AlreadyInstalled => println!("toolchain '{id}' already installed"),
        }
    }
    Ok(())
}

/// Resolve one target to a registered id. A path-like target must be registered:
/// with `--register` it is copied in (honoring `overwrite`); without it, that is
/// an error. A bare id is returned as-is (it must already be registered).
fn resolve_target(
    led: &mut Ledger,
    dir: &Path,
    target: &str,
    register: bool,
    overwrite: bool,
) -> Result<String> {
    if looks_like_path(target) {
        if !register {
            return Err(miette!(
                "'{target}' looks like a definition file; pass --register to register and install it"
            ));
        }
        toolchains::register(dir, led, Path::new(target), overwrite)
    } else {
        Ok(target.to_owned())
    }
}

/// Whether `target` is a path to a definition file rather than a bare id (a legal
/// id contains no `/` and never carries a `.toml` extension).
fn looks_like_path(target: &str) -> bool {
    target.contains('/') || target.ends_with(".toml")
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
