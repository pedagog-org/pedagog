//! The apk package manager, behind a trait so the verb orchestration is
//! unit-testable with fakes. The real implementation shells out to `apk`.

use std::process::Command;

use miette::{IntoDiagnostic, Result, WrapErr};
use pedagog_core::image::build::BuildState;

/// The package manager (`apk`). Implementations supply the `add`/`del` primitives;
/// the higher-level `install`/`remove` keep the build ledger in sync — they mutate
/// the `BuildState` they're handed — and are shared across implementations.
pub trait PackageManager {
    /// Install packages (`apk add`). Empty list is a no-op.
    fn add(&self, packages: &[String]) -> Result<()>;
    /// Remove packages (`apk del`). Empty list is a no-op.
    fn del(&self, packages: &[String]) -> Result<()>;

    /// Install `packages` and record them as directly-installed. Records only
    /// after `add` succeeds; re-recording an already-listed package is a no-op.
    fn install(&self, state: &mut BuildState, packages: &[String]) -> Result<()> {
        self.add(packages)?;
        for pkg in packages {
            if !state.additional_packages.iter().any(|p| p == pkg) {
                state.additional_packages.push(pkg.clone());
            }
        }
        Ok(())
    }

    /// Remove `packages`, all-or-nothing: every removal is dependency-gated first
    /// (on a copy), so if any is refused nothing is touched — neither the ledger
    /// nor apk.
    fn remove(&self, state: &mut BuildState, packages: &[String]) -> Result<()> {
        let mut next = state.clone();
        for pkg in packages {
            next.remove_package(pkg).into_diagnostic()?;
        }
        self.del(packages)?;
        *state = next;
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
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use pedagog_core::image::build::ToolchainRecord;

    use super::*;

    /// Records the packages it was asked to add/remove; never shells out.
    #[derive(Default)]
    struct FakePackageManager {
        added: RefCell<Vec<String>>,
        removed: RefCell<Vec<String>>,
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
    }

    fn pkgs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn install_records_and_calls_apk() {
        let mut state = BuildState::default();
        let pm = FakePackageManager::default();
        pm.install(&mut state, &pkgs(&["ripgrep", "jq"])).unwrap();
        assert_eq!(state.additional_packages, vec!["ripgrep", "jq"]);
        assert_eq!(*pm.added.borrow(), vec!["ripgrep", "jq"]);
    }

    #[test]
    fn install_dedups() {
        let mut state = BuildState::default();
        let pm = FakePackageManager::default();
        pm.install(&mut state, &pkgs(&["jq"])).unwrap();
        pm.install(&mut state, &pkgs(&["jq"])).unwrap();
        assert_eq!(state.additional_packages, vec!["jq"]);
    }

    #[test]
    fn remove_drops_and_calls_apk() {
        let mut state = BuildState {
            additional_packages: pkgs(&["jq", "ripgrep"]),
            ..Default::default()
        };
        let pm = FakePackageManager::default();
        pm.remove(&mut state, &pkgs(&["jq"])).unwrap();
        assert_eq!(state.additional_packages, vec!["ripgrep"]);
        assert_eq!(*pm.removed.borrow(), vec!["jq"]);
    }

    #[test]
    fn remove_refused_when_toolchain_depends_touches_nothing() {
        let mut state = BuildState {
            additional_packages: pkgs(&["curl"]),
            ..Default::default()
        };
        state.toolchains.insert(
            "rust".to_owned(),
            ToolchainRecord {
                packages: pkgs(&["curl"]),
            },
        );
        let pm = FakePackageManager::default();
        assert!(pm.remove(&mut state, &pkgs(&["curl"])).is_err());
        // Neither the ledger nor apk was touched.
        assert_eq!(state.additional_packages, vec!["curl"]);
        assert!(pm.removed.borrow().is_empty());
    }
}
