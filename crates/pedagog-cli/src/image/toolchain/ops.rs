//! Register, unregister, and list toolchain definitions — the logic behind
//! `pedagog image toolchain …`. The toolchains-directory copy and the ledger
//! accounting live in [`crate::image::toolchains`]; here we load/save the ledger
//! around it and report.

use std::path::Path;

use miette::{Result, miette};

use pedagog_core::image::ledger::Ledger;
use pedagog_core::image::toolchain::Toolchain;

use crate::image::apk::Apk;
use crate::image::ledger;
use crate::image::shell::Sh;
use crate::image::toolchains::{self, InstallOutcome, RemoveOptions};

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

/// Health-check toolchains: confirm each one's packages are present and its verify
/// commands pass. `--all` checks every installed toolchain; otherwise the given
/// ids. Read-only. Checks every target (not fail-fast) and reports each, then
/// errors if any failed.
pub fn verify(ids: &[String], all: bool, dir: &Path, ledger_path: &Path) -> Result<()> {
    let led = ledger::load(ledger_path)?;
    let targets: Vec<String> = if all {
        led.toolchains
            .iter()
            .filter(|(_, installed)| **installed)
            .map(|(id, _)| id.clone())
            .collect()
    } else {
        ids.to_vec()
    };

    let pm = Apk;
    let sh = Sh;
    let mut failed = 0;
    for id in &targets {
        match verify_one(&pm, &sh, dir, id) {
            Ok(()) => println!("{id}: ok"),
            Err(e) => {
                println!("{id}: FAILED: {e}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(miette!("{failed} of {} toolchain(s) failed", targets.len()));
    }
    Ok(())
}

/// Resolve `id`'s definition and health-check it; a missing def is a failure.
fn verify_one(pm: &Apk, sh: &Sh, dir: &Path, id: &str) -> Result<()> {
    let tc = toolchains::resolve(dir, id)?
        .ok_or_else(|| miette!("not registered"))?;
    toolchains::verify(pm, sh, &tc)
}

/// The `remove` CLI flags (clap surface in the parent module).
#[derive(Debug, Clone, Copy)]
pub struct RemoveFlags {
    /// Remove every installed toolchain instead of a given id list.
    pub all: bool,
    /// Keep the toolchains' packages (skip the purge).
    pub no_purge: bool,
    /// Skip the uninstall command.
    pub no_cmd: bool,
    /// Just mark uninstalled — shorthand for `no_cmd` + `no_purge`.
    pub forget: bool,
    /// Print the plan and change nothing.
    pub dry_run: bool,
}

/// Remove each toolchain in order: run its uninstall cmd, dependency-gated purge,
/// then mark it uninstalled (§2). `--forget` is shorthand for `--no-cmd
/// --no-purge` (just mark uninstalled), which is also the only way to remove a
/// toolchain whose def is missing. Stops at the first failure, persisting the
/// ledger with whatever succeeded (skipped entirely under `--dry-run`).
pub fn remove(ids: &[String], flags: RemoveFlags, dir: &Path, ledger_path: &Path) -> Result<()> {
    let mut led = ledger::load(ledger_path)?;
    let targets: Vec<String> = if flags.all {
        led.toolchains
            .iter()
            .filter(|(_, installed)| **installed)
            .map(|(id, _)| id.clone())
            .collect()
    } else {
        ids.to_vec()
    };
    let opts = RemoveOptions {
        no_cmd: flags.no_cmd || flags.forget,
        no_purge: flags.no_purge || flags.forget,
        dry_run: flags.dry_run,
    };

    let result = remove_all(&mut led, &targets, opts, dir);
    if flags.dry_run {
        return result; // changed nothing; nothing to persist
    }
    let saved = ledger::save(ledger_path, &led);
    result.and(saved)
}

/// Remove each id in order, returning at the first failure.
fn remove_all(led: &mut Ledger, ids: &[String], opts: RemoveOptions, dir: &Path) -> Result<()> {
    let pm = Apk;
    let sh = Sh;
    for id in ids {
        match toolchains::resolve(dir, id)? {
            Some(tc) => {
                let others = other_installed(led, dir, id)?;
                let direct = led.additional_packages.clone();
                toolchains::remove(&pm, &sh, led, &tc, &others, &direct, opts)?;
            }
            // No def: we can only mark it uninstalled, and only when nothing is
            // wanted from the def (i.e. --forget / --no-cmd --no-purge).
            None if opts.no_cmd && opts.no_purge => {
                if opts.dry_run {
                    println!("dry-run: remove '{id}' (no def): mark uninstalled");
                } else {
                    led.mark_uninstalled(id);
                }
            }
            None => {
                return Err(miette!(
                    "toolchain '{id}' has no registered def; use --forget to just mark it uninstalled"
                ));
            }
        }
        if !opts.dry_run {
            println!("removed toolchain '{id}'");
        }
    }
    Ok(())
}

/// Resolve the defs of every installed toolchain except `except` (the one being
/// removed) — the requirers that keep a shared package alive. A registered
/// toolchain whose def is missing is skipped.
fn other_installed(led: &Ledger, dir: &Path, except: &str) -> Result<Vec<Toolchain>> {
    let mut defs = Vec::new();
    for (id, installed) in &led.toolchains {
        if !*installed || id == except {
            continue;
        }
        if let Some(def) = toolchains::resolve(dir, id)? {
            defs.push(def);
        }
    }
    Ok(defs)
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
