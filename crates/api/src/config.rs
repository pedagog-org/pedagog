//! api configuration, from the environment.

use std::time::Duration;

pub const DATABASE_URL: &str = "DATABASE_URL";
pub const LISTEN_ADDR: &str = "PEDAGOG_LISTEN_ADDR";
pub const PUBLIC_URL: &str = "PEDAGOG_PUBLIC_URL";
pub const COOKIE_SECURE: &str = "PEDAGOG_COOKIE_SECURE";
pub const SMTP_HOST: &str = "PEDAGOG_SMTP_HOST";
pub const SMTP_PORT: &str = "PEDAGOG_SMTP_PORT";
pub const SMTP_TLS: &str = "PEDAGOG_SMTP_TLS";
pub const SMTP_USER: &str = "PEDAGOG_SMTP_USER";
pub const SMTP_PASSWORD: &str = "PEDAGOG_SMTP_PASSWORD";
pub const MAIL_FROM_NAME: &str = "PEDAGOG_MAIL_FROM_NAME";
pub const MAIL_FROM_ADDRESS: &str = "PEDAGOG_MAIL_FROM_ADDRESS";
pub const MAIL_REPLY_TO: &str = "PEDAGOG_MAIL_REPLY_TO";

const DEFAULT_LISTEN: &str = "0.0.0.0:8080";

pub struct Config {
    pub database_url: String,
    pub listen_addr: String,
    /// Where people reach the platform; used in emails.
    pub public_url: String,
    /// Mark the session cookie `Secure` (off only for plain-HTTP local dev).
    pub cookie_secure: bool,
    pub smtp: Smtp,
    pub limits: AuthLimits,
}

/// How mail leaves: a relay that may or may not want a login, with or
/// without TLS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Smtp {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    /// `(user, password)` when the relay wants a login.
    pub login: Option<(String, String)>,
    pub from_name: String,
    pub from_address: String,
    pub reply_to: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    /// Plain SMTP (a trusted relay, or Mailpit).
    None,
    /// Upgrade with STARTTLS (submission, usually 587).
    StartTls,
    /// TLS from the start (usually 465).
    Tls,
}

impl TlsMode {
    fn parse(s: &str) -> anyhow::Result<Self> {
        match s {
            "none" => Ok(TlsMode::None),
            "starttls" => Ok(TlsMode::StartTls),
            "tls" => Ok(TlsMode::Tls),
            other => anyhow::bail!("{SMTP_TLS} must be none, starttls or tls, not {other:?}"),
        }
    }

    fn default_port(self) -> u16 {
        match self {
            TlsMode::None => 25,
            TlsMode::StartTls => 587,
            TlsMode::Tls => 465,
        }
    }
}

/// Login-code and session lifetimes.
#[derive(Clone, Copy, Debug)]
pub struct AuthLimits {
    pub code_ttl: Duration,
    pub code_attempts: i32,
    pub code_interval: Duration,
    pub web_session: Duration,
    pub cli_session: Duration,
}

impl Default for AuthLimits {
    fn default() -> Self {
        Self {
            code_ttl: Duration::from_secs(10 * 60),
            code_attempts: 5,
            code_interval: Duration::from_secs(60),
            web_session: Duration::from_secs(12 * 3600),
            cli_session: Duration::from_secs(30 * 24 * 3600),
        }
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let required = |k: &str| var(k).ok_or_else(|| anyhow::anyhow!("{k} must be set"));
        let tls = TlsMode::parse(&var(SMTP_TLS).unwrap_or_else(|| "none".to_owned()))?;
        let port = match var(SMTP_PORT) {
            Some(p) => p
                .parse()
                .map_err(|_| anyhow::anyhow!("{SMTP_PORT} must be a port number, got {p:?}"))?,
            None => tls.default_port(),
        };
        let login = match (var(SMTP_USER), var(SMTP_PASSWORD)) {
            (Some(u), Some(p)) if !u.is_empty() => Some((u, p)),
            (Some(u), None) if !u.is_empty() => {
                anyhow::bail!("{SMTP_USER} is set without {SMTP_PASSWORD}")
            }
            _ => None,
        };
        Ok(Self {
            database_url: required(DATABASE_URL)?,
            listen_addr: var(LISTEN_ADDR).unwrap_or_else(|| DEFAULT_LISTEN.to_owned()),
            public_url: required(PUBLIC_URL)?.trim_end_matches('/').to_owned(),
            cookie_secure: var(COOKIE_SECURE).as_deref() != Some("false"),
            smtp: Smtp {
                host: required(SMTP_HOST)?,
                port,
                tls,
                login,
                from_name: var(MAIL_FROM_NAME).unwrap_or_else(|| "Pedagog".to_owned()),
                from_address: required(MAIL_FROM_ADDRESS)?,
                reply_to: var(MAIL_REPLY_TO).filter(|r| !r.is_empty()),
            },
            limits: AuthLimits::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    fn config(pairs: &[(&str, &str)]) -> anyhow::Result<Config> {
        let vars: HashMap<String, String> = [
            (DATABASE_URL, "postgres://x"),
            (PUBLIC_URL, "https://pedagog.example/"),
            (SMTP_HOST, "relay.ufl.edu"),
            (MAIL_FROM_ADDRESS, "pedagog@ufl.edu"),
        ]
        .iter()
        .chain(pairs)
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
        Config::from_vars(|k| vars.get(k).cloned())
    }

    #[test]
    fn a_trusted_relay_needs_only_a_host_and_sender() {
        let c = config(&[]).unwrap();
        assert_eq!(
            (c.smtp.port, c.smtp.tls, c.smtp.login.clone()),
            (25, TlsMode::None, None)
        );
        assert_eq!(c.public_url, "https://pedagog.example");
        assert!(c.cookie_secure);
        assert_eq!(c.smtp.from_name, "Pedagog");
    }

    #[test]
    fn tls_modes_pick_their_usual_port() {
        assert_eq!(config(&[(SMTP_TLS, "starttls")]).unwrap().smtp.port, 587);
        assert_eq!(config(&[(SMTP_TLS, "tls")]).unwrap().smtp.port, 465);
        assert_eq!(
            config(&[(SMTP_TLS, "tls"), (SMTP_PORT, "2465")])
                .unwrap()
                .smtp
                .port,
            2465
        );
        assert!(config(&[(SMTP_TLS, "ssl")]).is_err());
    }

    #[test]
    fn a_login_needs_both_halves() {
        let c = config(&[(SMTP_USER, "pedagog"), (SMTP_PASSWORD, "secret")]).unwrap();
        assert_eq!(c.smtp.login, Some(("pedagog".into(), "secret".into())));
        assert!(config(&[(SMTP_USER, "pedagog")]).is_err());
    }

    #[test]
    fn required_settings_are_named() {
        let err = Config::from_vars(|_| None).err().unwrap().to_string();
        assert!(err.contains(DATABASE_URL), "{err}");
    }
}
