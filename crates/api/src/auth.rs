//! Who's calling: email login codes, session tokens, and the extractor that
//! turns a request's cookie or bearer token into a user.
//!
//! Codes and tokens are random (`getrandom`) and only their blake3 hashes are
//! stored. Login is behind [`Authenticator`] so UF SSO can be added later.

use axum::extract::FromRequestParts;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::request::Parts;
use pedagog_store::db::accounts::{self as db, CodeCheck, Issued};
use pedagog_store::{SessionKind, User};

use crate::error::{ApiError, unauthorized};
use crate::mail::Mail;
use crate::state::AppState;

pub const COOKIE_NAME: &str = "pedagog_session";
const CODE_WORDS: usize = 3;

static WORDS: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
    include_str!("words.txt")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .collect()
});

/// A uniformly random index below `n` (rejection sampling, no modulo bias).
fn random_below(n: u32) -> anyhow::Result<u32> {
    let limit = u32::MAX - u32::MAX % n;
    loop {
        let mut bytes = [0u8; 4];
        getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
        let v = u32::from_le_bytes(bytes);
        if v < limit {
            return Ok(v % n);
        }
    }
}

/// A login phrase like `tiger-maple-river` (1296³ ≈ 2.2 × 10⁹ possibilities;
/// with 5 tries per code, guessing is hopeless).
pub fn new_code() -> anyhow::Result<String> {
    let n = u32::try_from(WORDS.len())?;
    let words: anyhow::Result<Vec<&str>> = (0..CODE_WORDS)
        .map(|_| Ok(WORDS[random_below(n)? as usize]))
        .collect();
    Ok(words?.join("-"))
}

/// What's typed in may have other separators or capitals.
pub fn hash_code(code: &str) -> String {
    let normalized: Vec<String> = code
        .split(|c: char| c == '-' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    blake3::hash(normalized.join("-").as_bytes())
        .to_hex()
        .to_string()
}

/// A session token: 32 random bytes, hex.
pub fn new_token() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn hash_token(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

/// How someone proves who they are. Email codes for now; SSO later.
#[derive(Clone)]
pub enum Authenticator {
    EmailCode,
}

/// The outcome of finishing a login.
#[derive(Debug)]
pub enum Login {
    Ok(User),
    Wrong {
        left: i32,
    },
    /// No valid code: start again.
    Void,
}

impl Authenticator {
    /// Send a code to `email` if it belongs to a user (silently not otherwise,
    /// and not again within a minute).
    pub async fn start(&self, state: &AppState, email: &str) -> anyhow::Result<()> {
        let Authenticator::EmailCode = self;
        let Some(user) = db::user_by_email(&state.pool, email).await? else {
            tracing::info!("login code requested for an unknown email");
            return Ok(());
        };
        let code = new_code()?;
        let limits = state.config.limits;
        let issued = db::issue_code(
            &state.pool,
            &user.email,
            &hash_code(&code),
            limits.code_ttl.as_secs() as i64,
            limits.code_interval.as_secs() as i64,
        )
        .await?;
        if issued == Issued::TooSoon {
            tracing::info!(user = user.id, "login code requested again too soon");
            return Ok(());
        }
        let minutes = limits.code_ttl.as_secs() / 60;
        state
            .mailer
            .send(Mail {
                to: user.email.clone(),
                subject: format!("Your Pedagog login code: {code}"),
                body: format!(
                    "Your login code is\n\n    {code}\n\nIt works once, for {minutes} minutes.\n\
                     If you didn't ask for it, you can ignore this email.\n\n{}\n",
                    state.config.public_url
                ),
            })
            .await?;
        tracing::info!(user = user.id, "login code sent");
        Ok(())
    }

    pub async fn finish(&self, state: &AppState, email: &str, code: &str) -> anyhow::Result<Login> {
        let Authenticator::EmailCode = self;
        let attempts = state.config.limits.code_attempts;
        Ok(
            match db::check_code(&state.pool, email, &hash_code(code), attempts).await? {
                CodeCheck::Valid => match db::user_by_email(&state.pool, email).await? {
                    Some(user) => Login::Ok(user),
                    None => Login::Void,
                },
                CodeCheck::Wrong { left } => Login::Wrong { left },
                CodeCheck::Void => Login::Void,
            },
        )
    }
}

/// Open a session for `user`; returns its token.
pub async fn open_session(
    state: &AppState,
    user: &User,
    kind: SessionKind,
) -> anyhow::Result<String> {
    let token = new_token()?;
    let ttl = match kind {
        SessionKind::Web => state.config.limits.web_session,
        SessionKind::Cli => state.config.limits.cli_session,
    };
    db::create_session(
        &state.pool,
        &hash_token(&token),
        user.id,
        kind,
        ttl.as_secs() as i64,
    )
    .await?;
    db::record_login(&state.pool, user.id, kind).await?;
    Ok(token)
}

/// The caller's token: `Authorization: Bearer <token>` (hammer) or the
/// session cookie (the web).
pub fn token_of(parts: &Parts) -> Option<String> {
    if let Some(auth) = parts
        .headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        && let Some(token) = auth.strip_prefix("Bearer ")
    {
        return Some(token.trim().to_owned());
    }
    parts
        .headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value.to_owned())
}

/// A logged-in caller; handlers that take it refuse anonymous requests (401).
pub struct CurrentUser {
    pub user: User,
    /// The hash of the session's token (for logout).
    pub token_hash: String,
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = token_of(parts).ok_or_else(|| unauthorized("log in first"))?;
        let token_hash = hash_token(&token);
        let user = db::session_user(&state.pool, &token_hash)
            .await
            .map_err(|e| crate::error::internal("reading the session", e))?
            .ok_or_else(|| unauthorized("your session has ended or expired: log in again"))?;
        Ok(CurrentUser { user, token_hash })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::http::Request;

    #[test]
    fn the_word_list_has_1296_plain_words() {
        assert_eq!(WORDS.len(), 1296);
        assert!(
            WORDS
                .iter()
                .all(|w| w.chars().all(|c| c.is_ascii_lowercase()))
        );
    }

    #[test]
    fn codes_are_three_words_and_vary() {
        let a = new_code().unwrap();
        assert_eq!(a.split('-').count(), 3, "{a}");
        let distinct: std::collections::HashSet<String> =
            (0..20).map(|_| new_code().unwrap()).collect();
        assert!(distinct.len() > 15, "codes repeat too much");
    }

    #[test]
    fn codes_hash_the_same_however_they_are_typed() {
        let h = hash_code("tiger-maple-river");
        assert_eq!(hash_code("Tiger Maple River"), h);
        assert_eq!(hash_code("  tiger -maple- river "), h);
        assert_ne!(hash_code("tiger-maple-rivers"), h);
    }

    #[test]
    fn tokens_are_long_random_hex() {
        let t = new_token().unwrap();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(new_token().unwrap(), t);
    }

    fn parts(headers: &[(&str, &str)]) -> Parts {
        let mut req = Request::builder();
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        req.body(()).unwrap().into_parts().0
    }

    #[test]
    fn the_token_comes_from_a_bearer_header_or_the_cookie() {
        assert_eq!(
            token_of(&parts(&[("authorization", "Bearer abc")])).as_deref(),
            Some("abc")
        );
        assert_eq!(
            token_of(&parts(&[(
                "cookie",
                "theme=dark; pedagog_session=xyz; other=1"
            )]))
            .as_deref(),
            Some("xyz")
        );
        assert_eq!(token_of(&parts(&[("cookie", "theme=dark")])), None);
        assert_eq!(token_of(&parts(&[])), None);
    }
}
