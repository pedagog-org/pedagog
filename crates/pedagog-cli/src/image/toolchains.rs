//! The toolchains directory: a flat directory of `<id>.toml` definitions.
//! `add`/`delete` are the filesystem primitives (copy a def in, remove one);
//! `register`/`unregister` wrap them with the ledger accounting — the same
//! primitive/accounting split as the `PackageManager` trait.

use std::path::{Path, PathBuf};

use miette::{IntoDiagnostic, Result, WrapErr, miette};
use pedagog_core::image::ledger::Ledger;
use pedagog_core::image::toolchain::{Toolchain, valid_id};

pub use pedagog_core::image::toolchain::DEFAULT_TOOLCHAINS;

/// Path of the definition file for `id` within `dir`. Rejects an id that isn't a
/// legal (filename-safe) toolchain id, so a raw id can't escape `dir`.
fn path_for(dir: &Path, id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        return Err(miette!(
            "invalid toolchain id '{id}' (allowed: alphanumeric, '.', '-', '_')"
        ));
    }
    Ok(dir.join(format!("{id}.toml")))
}

/// Read and parse the definition for `id`, returning `None` if it is not
/// registered.
pub fn resolve(dir: &Path, id: &str) -> Result<Option<Toolchain>> {
    let path = path_for(dir, id)?;
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .into_diagnostic()
        .wrap_err_with(|| format!("reading toolchain {}", path.display()))?;
    let toolchain = text
        .parse::<Toolchain>()
        .into_diagnostic()
        .wrap_err_with(|| format!("parsing toolchain {}", path.display()))?;
    Ok(Some(toolchain))
}

/// Primitive: copy `contents` into the dir as `<id>.toml`, creating the dir if
/// needed. Refuses to replace an existing def unless `overwrite`. No accounting.
pub fn add(dir: &Path, id: &str, contents: &str, overwrite: bool) -> Result<()> {
    let path = path_for(dir, id)?;
    if path.exists() && !overwrite {
        return Err(miette!(
            "toolchain '{id}' is already registered (unregister it, or use --overwrite)"
        ));
    }
    std::fs::create_dir_all(dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("creating toolchains dir {}", dir.display()))?;
    std::fs::write(&path, contents)
        .into_diagnostic()
        .wrap_err_with(|| format!("writing {}", path.display()))
}

/// Primitive: delete the def file for `id`. Errors if it is not present.
pub fn delete(dir: &Path, id: &str) -> Result<()> {
    let path = path_for(dir, id)?;
    if !path.exists() {
        return Err(miette!("toolchain '{id}' is not registered"));
    }
    std::fs::remove_file(&path)
        .into_diagnostic()
        .wrap_err_with(|| format!("removing {}", path.display()))
}

/// Register the def at `file`: validate it, `add` it under its own id, and record
/// it (uninstalled) in `ledger`. Returns the registered id.
pub fn register(dir: &Path, ledger: &mut Ledger, file: &Path, overwrite: bool) -> Result<String> {
    let contents = std::fs::read_to_string(file)
        .into_diagnostic()
        .wrap_err_with(|| format!("reading {}", file.display()))?;
    let toolchain: Toolchain = contents
        .parse()
        .into_diagnostic()
        .wrap_err_with(|| format!("parsing {}", file.display()))?;

    add(dir, &toolchain.id, &contents, overwrite)?;
    ledger.register_toolchain(&toolchain.id);
    Ok(toolchain.id)
}

/// Unregister `id`: refuse if it is installed unless `force`, `delete` the def,
/// and drop it from `ledger`.
pub fn unregister(dir: &Path, ledger: &mut Ledger, id: &str, force: bool) -> Result<()> {
    if ledger.is_installed(id) && !force {
        return Err(miette!(
            "toolchain '{id}' is installed (uninstall it first, or use --force)"
        ));
    }
    delete(dir, id)?;
    ledger.unregister_toolchain(id);
    Ok(())
}

/// List the ids of every registered definition, sorted. A missing toolchains
/// directory is empty.
pub fn list(dir: &Path) -> Result<Vec<String>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    let read = std::fs::read_dir(dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("reading toolchains dir {}", dir.display()))?;
    for entry in read {
        let entry = entry
            .into_diagnostic()
            .wrap_err_with(|| format!("reading entry in {}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEF: &str = "version = \"0.1.0\"\nid = \"rust\"\n";

    /// Write a source def file in `dir` and return its path (its filename differs
    /// from the id, so we exercise id-derived naming).
    fn source(dir: &Path, contents: &str) -> PathBuf {
        let path = dir.join("source.toml");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn add_writes_def() {
        let dir = tempfile::tempdir().unwrap();
        add(dir.path(), "rust", DEF, false).unwrap();
        assert!(path_for(dir.path(), "rust").unwrap().exists());
    }

    #[test]
    fn add_rejects_invalid_id() {
        let dir = tempfile::tempdir().unwrap();
        assert!(add(dir.path(), "../evil", DEF, false).is_err());
    }

    #[test]
    fn add_refuses_existing_without_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        add(dir.path(), "rust", DEF, false).unwrap();
        assert!(add(dir.path(), "rust", DEF, false).is_err());
    }

    #[test]
    fn add_replaces_with_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        add(dir.path(), "rust", DEF, false).unwrap();
        let updated = "version = \"0.1.0\"\nid = \"rust\"\ndescription = \"v2\"\n";
        add(dir.path(), "rust", updated, true).unwrap();
        let tc = resolve(dir.path(), "rust").unwrap().unwrap();
        assert_eq!(tc.description.as_deref(), Some("v2"));
    }

    #[test]
    fn resolve_returns_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve(dir.path(), "rust").unwrap().is_none());
    }

    #[test]
    fn delete_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        add(dir.path(), "rust", DEF, false).unwrap();
        delete(dir.path(), "rust").unwrap();
        assert!(!path_for(dir.path(), "rust").unwrap().exists());
    }

    #[test]
    fn delete_errors_on_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(delete(dir.path(), "rust").is_err());
    }

    #[test]
    fn register_copies_under_id_and_records_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let src = source(dir.path(), DEF);
        let mut led = Ledger::default();

        let id = register(dir.path(), &mut led, &src, false).unwrap();
        assert_eq!(id, "rust");
        assert!(path_for(dir.path(), "rust").unwrap().exists());
        assert!(led.toolchains.contains_key("rust"));
        assert!(!led.is_installed("rust"));
    }

    #[test]
    fn unregister_refuses_installed_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let src = source(dir.path(), DEF);
        let mut led = Ledger::default();
        register(dir.path(), &mut led, &src, false).unwrap();
        led.toolchains.insert("rust".to_owned(), true);

        assert!(unregister(dir.path(), &mut led, "rust", false).is_err());
        // Untouched on refusal.
        assert!(path_for(dir.path(), "rust").unwrap().exists());
        assert!(led.toolchains.contains_key("rust"));
    }

    #[test]
    fn unregister_with_force_drops_installed() {
        let dir = tempfile::tempdir().unwrap();
        let src = source(dir.path(), DEF);
        let mut led = Ledger::default();
        register(dir.path(), &mut led, &src, false).unwrap();
        led.toolchains.insert("rust".to_owned(), true);

        unregister(dir.path(), &mut led, "rust", true).unwrap();
        assert!(!path_for(dir.path(), "rust").unwrap().exists());
        assert!(!led.toolchains.contains_key("rust"));
    }

    #[test]
    fn list_returns_sorted_ids() {
        let dir = tempfile::tempdir().unwrap();
        add(dir.path(), "rust", DEF, false).unwrap();
        add(dir.path(), "python", "version = \"0.1.0\"\nid = \"python\"\n", false).unwrap();
        assert_eq!(list(dir.path()).unwrap(), vec!["python", "rust"]);
    }

    #[test]
    fn list_empty_when_dir_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list(&dir.path().join("nope")).unwrap().is_empty());
    }
}
