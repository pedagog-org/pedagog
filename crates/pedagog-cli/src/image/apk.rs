//! The apk package manager, behind a trait so the verb orchestration is
//! unit-testable with fakes. The real implementation shells out to `apk`.

use std::process::{Command, Stdio};

use miette::{IntoDiagnostic, Result, WrapErr};
use pedagog_core::image::ledger::Ledger;

/// The package manager (`apk`). Implementations supply the `add`/`del` primitives;
/// the higher-level `install`/`remove` keep the build ledger in sync — they mutate
/// the `Ledger` they're handed — and are shared across implementations.
pub trait PackageManager {
    /// Install packages (`apk add`). Empty list is a no-op.
    fn add(&self, packages: &[String]) -> Result<()>;
    /// Remove packages (`apk del`). Empty list is a no-op.
    fn del(&self, packages: &[String]) -> Result<()>;
    /// Whether `package` is currently installed (`apk info -e`). Used by
    /// `verify` and by the dependency-gated purge.
    // wired in by `toolchain install`/`verify` (next increment)
    #[allow(dead_code)]
    fn is_installed(&self, package: &str) -> Result<bool>;

    /// Install `packages` and record them as directly-installed. Records only
    /// after `add` succeeds; re-recording an already-listed package is a no-op.
    fn install(&self, ledger: &mut Ledger, packages: &[String]) -> Result<()> {
        self.add(packages)?;
        for pkg in packages {
            ledger.add_package(pkg);
        }
        Ok(())
    }

    /// Remove `packages` and drop them from the ledger. Records only after `del`
    /// succeeds.
    fn remove(&self, ledger: &mut Ledger, packages: &[String]) -> Result<()> {
        self.del(packages)?;
        for pkg in packages {
            ledger.remove_package(pkg);
        }
        Ok(())
    }
}

/// `PackageManager` backed by the real `apk` binary.
pub struct Apk;

impl Apk {
    /// Run `apk <verb> <packages…>`, streaming output. No-op for an empty list.
    fn run(verb: &str, packages: &[String]) -> Result<()> {
        if packages.is_empty() {
            return Ok(());
        }
        let status = Command::new("apk")
            .arg(verb)
            .args(packages)
            .status()
            .into_diagnostic()
            .wrap_err("spawning apk (is it installed?)")?;
        if !status.success() {
            return Err(miette::miette!("apk {verb} exited with {status}"));
        }
        Ok(())
    }
}

impl PackageManager for Apk {
    fn add(&self, packages: &[String]) -> Result<()> {
        Self::run("add", packages)
    }

    fn del(&self, packages: &[String]) -> Result<()> {
        Self::run("del", packages)
    }

    fn is_installed(&self, package: &str) -> Result<bool> {
        // `apk info -e <pkg>` exits 0 (and echoes the name) if installed, 1 if
        // not; silence stdout and read the status.
        let status = Command::new("apk")
            .args(["info", "-e", package])
            .stdout(Stdio::null())
            .status()
            .into_diagnostic()
            .wrap_err("spawning apk (is it installed?)")?;
        Ok(status.success())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// Records the packages it was asked to add/remove; never shells out.
    #[derive(Default)]
    struct FakePackageManager {
        added: RefCell<Vec<String>>,
        removed: RefCell<Vec<String>>,
        present: Vec<String>,
    }

    impl PackageManager for FakePackageManager {
        fn add(&self, packages: &[String]) -> Result<()> {
            self.added.borrow_mut().extend_from_slice(packages);
            Ok(())
        }
        fn del(&self, packages: &[String]) -> Result<()> {
            self.removed.borrow_mut().extend_from_slice(packages);
            Ok(())
        }
        fn is_installed(&self, package: &str) -> Result<bool> {
            Ok(self.present.iter().any(|p| p == package))
        }
    }

    fn pkgs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn install_records_and_calls_apk() {
        let mut ledger = Ledger::default();
        let pm = FakePackageManager::default();
        pm.install(&mut ledger, &pkgs(&["ripgrep", "jq"])).unwrap();
        assert_eq!(ledger.additional_packages, vec!["ripgrep", "jq"]);
        assert_eq!(*pm.added.borrow(), vec!["ripgrep", "jq"]);
    }

    #[test]
    fn install_dedups() {
        let mut ledger = Ledger::default();
        let pm = FakePackageManager::default();
        pm.install(&mut ledger, &pkgs(&["jq"])).unwrap();
        pm.install(&mut ledger, &pkgs(&["jq"])).unwrap();
        assert_eq!(ledger.additional_packages, vec!["jq"]);
    }

    #[test]
    fn remove_drops_and_calls_apk() {
        let mut ledger = Ledger {
            additional_packages: pkgs(&["jq", "ripgrep"]),
            ..Default::default()
        };
        let pm = FakePackageManager::default();
        pm.remove(&mut ledger, &pkgs(&["jq"])).unwrap();
        assert_eq!(ledger.additional_packages, vec!["ripgrep"]);
        assert_eq!(*pm.removed.borrow(), vec!["jq"]);
    }

    #[test]
    fn is_installed_reflects_present_set() {
        let pm = FakePackageManager {
            present: pkgs(&["jq"]),
            ..Default::default()
        };
        assert!(pm.is_installed("jq").unwrap());
        assert!(!pm.is_installed("ripgrep").unwrap());
    }
}
