//! Shell command execution, behind a trait so the toolchain lifecycle is
//! unit-testable with fakes. The real implementation runs each command through
//! `sh -c`, capturing stdout and streaming stderr. Used for a toolchain def's
//! install/uninstall/verify scripts.

use std::process::{Command, Stdio};

use miette::{IntoDiagnostic, Result};

/// Runs a single shell command, fail-fast. The toolchain lifecycle runs a def's
/// `cmd`/`verify` list one command at a time — the caller loops — so it can
/// report progress between commands; each call returns the command's stdout.
pub trait Shell {
    /// Run one command, erroring if it exits non-zero; returns its stdout.
    fn run(&self, cmd: &str) -> Result<String>;
}

/// `Shell` backed by the real `sh` binary: captures stdout, streams stderr so
/// command progress stays visible.
pub struct Sh;

impl Shell for Sh {
    fn run(&self, cmd: &str) -> Result<String> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .output()
            .into_diagnostic()?;
        if !output.status.success() {
            return Err(miette::miette!("`{cmd}` exited with {}", output.status));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_returns_stdout() {
        assert_eq!(Sh.run("echo hello").unwrap(), "hello\n");
    }

    #[test]
    fn run_errors_on_nonzero_exit() {
        assert!(Sh.run("exit 3").is_err());
    }

    #[test]
    fn run_captures_stdout_not_stderr() {
        // stderr is streamed, not captured, so only stdout comes back.
        assert_eq!(Sh.run("echo out; echo err 1>&2").unwrap(), "out\n");
    }
}
