//! The kind cluster, from the outside: port-forwards to its services, Mailpit
//! (where login codes and notices land), personas (each a hammer session of
//! its own), kubectl and SQL. Shared by the end-to-end tests and `cargo xtask`.
//!
//! Everything is blocking: these drive external tools and wait on a cluster.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

pub const CONTEXT: &str = "kind-pedagog";
pub const API_PORT: u16 = 18082;
pub const MAILPIT_PORT: u16 = 18025;
pub const GLITCHTIP_PORT: u16 = 18000;

pub type Result<T> = std::result::Result<T, String>;

/// The repository's root (this crate is `crates/e2e`).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// Run `cmd`, returning its output; fails only if it couldn't start.
pub fn run(cmd: &mut Command) -> Result<Output> {
    cmd.output()
        .map_err(|e| format!("couldn't run {:?}: {e}", cmd.get_program()))
}

/// Run `cmd` and require it to succeed; its stdout.
pub fn ok(cmd: &mut Command) -> Result<String> {
    let out = run(cmd)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "{:?} failed: {}{}",
            cmd.get_program(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// `kubectl --context kind-pedagog ARGS`.
pub fn kubectl(args: &[&str]) -> Command {
    let mut cmd = Command::new("kubectl");
    cmd.args(["--context", CONTEXT]).args(args);
    cmd
}

/// One SQL query against the cluster's Postgres, its rows as `|`-separated text.
pub fn sql(query: &str) -> Result<String> {
    ok(&mut kubectl(&[
        "-n",
        "pedagog-data",
        "exec",
        "deploy/postgres",
        "--",
        "psql",
        "-U",
        "pedagog",
        "-d",
        "pedagog",
        "-tAc",
        query,
    ]))
    .map(|s| s.trim().to_owned())
}

/// Whether anything answers HTTP on `localhost:port`.
pub fn answers(port: u16) -> bool {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .and_then(|c| c.get(format!("http://localhost:{port}/")).send())
        .is_ok()
}

/// A `kubectl port-forward` to a Service, stopped when dropped. Reuses a
/// forward that already answers.
pub struct Forward(Option<Child>);

impl Forward {
    pub fn open(namespace: &str, service: &str, local: u16, remote: u16) -> Result<Self> {
        if answers(local) {
            return Ok(Self(None));
        }
        // A forward opened during a rollout can attach to a terminating pod.
        ok(&mut kubectl(&[
            "-n",
            namespace,
            "rollout",
            "status",
            &format!("deploy/{service}"),
            "--timeout=180s",
        ]))?;
        for _ in 0..30 {
            let child = kubectl(&[
                "-n",
                namespace,
                "port-forward",
                &format!("svc/{service}"),
                &format!("{local}:{remote}"),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start kubectl port-forward: {e}"))?;
            let forward = Self(Some(child));
            if wait_for(Duration::from_secs(5), || answers(local)) {
                return Ok(forward);
            }
        }
        Err(format!("cannot reach {service} through a port-forward"))
    }

    /// The api and Mailpit.
    pub fn api_and_mail() -> Result<(Self, Self)> {
        Ok((
            Self::open("pedagog-system", "pedagog-api", API_PORT, 8080)?,
            Self::open("pedagog-data", "mailpit", MAILPIT_PORT, 8025)?,
        ))
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Poll `cond` every 250 ms for up to `within`.
pub fn wait_for(within: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + within;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// A message in Mailpit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mail {
    pub id: String,
    pub subject: String,
}

/// The newest message to `to` whose subject has `words`.
pub fn latest_mail(to: &str, words: &str) -> Result<Option<Mail>> {
    let query: String =
        url::form_urlencoded::byte_serialize(format!("to:{to} subject:\"{words}\"").as_bytes())
            .collect();
    let text = reqwest::blocking::get(format!(
        "http://localhost:{MAILPIT_PORT}/api/v1/search?query={query}&limit=1"
    ))
    .and_then(|r| r.text())
    .map_err(|e| format!("Mailpit: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Mailpit: {e}: {text}"))?;
    Ok(v["messages"].get(0).map(|m| Mail {
        id: m["ID"].as_str().unwrap_or_default().to_owned(),
        subject: m["Subject"].as_str().unwrap_or_default().to_owned(),
    }))
}

/// Wait up to `within` for a message to `to` with `words`, newer than `before`.
pub fn mail_after(to: &str, words: &str, before: Option<&Mail>, within: Duration) -> Result<Mail> {
    let mut found = None;
    wait_for(within, || {
        found = latest_mail(to, words)
            .ok()
            .flatten()
            .filter(|m| Some(m) != before);
        found.is_some()
    });
    found.ok_or_else(|| format!("no \"{words}\" mail to {to} within {}s", within.as_secs()))
}

/// The hammer binary (built by the caller: `cargo build -p hammer`).
pub fn hammer_bin() -> PathBuf {
    root().join("target/debug/hammer")
}

/// Someone using hammer against kind, with a session of their own under
/// `target/kind/<name>`.
#[derive(Clone, Debug)]
pub struct Persona {
    pub name: String,
    pub email: String,
}

impl Persona {
    pub fn new(name: &str, email: &str) -> Self {
        Self {
            name: name.to_owned(),
            email: email.to_owned(),
        }
    }

    pub fn dir(&self) -> PathBuf {
        root().join("target/kind").join(&self.name)
    }

    /// hammer as this persona, with the checkout's recipes.
    pub fn hammer(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(hammer_bin());
        cmd.args(args)
            .env("XDG_CONFIG_HOME", self.dir())
            .env("HAMMER_RECIPES", root().join("recipes"))
            .env("NO_COLOR", "1");
        cmd
    }

    /// Run hammer as this persona: (succeeded, stdout and stderr together).
    pub fn says(&self, args: &[&str]) -> Result<(bool, String)> {
        let out = run(&mut self.hammer(args))?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Ok((out.status.success(), text))
    }

    /// Log in with the code mailed to them (they must be registered).
    pub fn login(&self) -> Result<()> {
        std::fs::create_dir_all(self.dir()).map_err(|e| e.to_string())?;
        let before = latest_mail(&self.email, "login code")?;
        let mut child = self
            .hammer(&[
                "login",
                "--api",
                &format!("http://localhost:{API_PORT}"),
                "--email",
                &self.email,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("couldn't start hammer: {e}"))?;
        let code = mail_after(
            &self.email,
            "login code",
            before.as_ref(),
            Duration::from_secs(30),
        )
        .map(|m| m.subject.rsplit(' ').next().unwrap_or_default().to_owned());
        if let Some(stdin) = &mut child.stdin {
            let _ = writeln!(stdin, "{}", code.as_deref().unwrap_or_default());
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        code?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{}: login as {} failed: {}",
                self.name,
                self.email,
                String::from_utf8_lossy(&out.stderr)
            ))
        }
    }

    /// Log in only if their saved session doesn't work.
    pub fn session(&self) -> Result<()> {
        let (ok, text) = self.says(&["whoami"])?;
        if ok && text.starts_with(&format!("{} ", self.email)) {
            return Ok(());
        }
        self.login()
    }
}

/// Hammer's error report wraps long lines, draws a frame and colours it: one
/// line of plain text.
pub fn flat(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // An escape sequence: ESC [ … letter.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if !matches!(c, '│' | '×' | '╰' | '─' | '╭') {
            plain.push(c);
        }
    }
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hammer_reports_flatten() {
        assert_eq!(
            flat("  × the api refused the submission (403\n  │ Forbidden): it's closed\n"),
            "the api refused the submission (403 Forbidden): it's closed"
        );
        assert_eq!(
            flat("Error: \u{1b}[31m\u{1b}[0m missing \u{1b}[31m\u{1b}[0m required files: a.py"),
            "Error: missing required files: a.py"
        );
    }

    #[test]
    fn the_root_holds_the_workspace() {
        assert!(root().join("Cargo.toml").exists());
        assert!(hammer_bin().ends_with("target/debug/hammer"));
    }
}
