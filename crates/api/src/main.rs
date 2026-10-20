//! The public api: login, courses, rosters, and (through `jobs`) assignments,
//! submissions and results, filtered by who's asking.
//!
//! `pedagog-api` serves; `pedagog-api add-admin <email>` makes someone an
//! admin (how the first admin gets in).

mod assignments;
mod auth;
mod config;
mod courses;
mod error;
mod http;
mod mail;
mod members;
mod notices;
mod policy;
mod results;
mod roster;
mod state;
mod submissions;
#[cfg(test)]
mod testing;
mod users;

use std::sync::Arc;
use std::time::Duration;

use pedagog_store::db;
use tokio::net::TcpListener;
use tokio::task::JoinSet;

use crate::auth::Authenticator;
use crate::config::Config;
use crate::mail::Mailer;
use crate::state::AppState;

const CLEANUP_EVERY: Duration = Duration::from_secs(3600);

fn main() -> anyhow::Result<()> {
    // sqlx and lettre both use rustls; pick ring explicitly, as jobs does.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _telemetry = pedagog_telemetry::init(concat!("pedagog-api@", env!("CARGO_PKG_VERSION")))?;
    let args: Vec<String> = std::env::args().collect();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    match args.get(1).map(String::as_str) {
        Some("add-admin") => {
            let [_, _, email] = args.as_slice() else {
                anyhow::bail!("usage: pedagog-api add-admin <email>");
            };
            runtime.block_on(add_admin(email))
        }
        Some(other) => anyhow::bail!("unknown command {other:?} (expected none, or add-admin)"),
        None => runtime.block_on(serve()),
    }
}

async fn add_admin(email: &str) -> anyhow::Result<()> {
    let url = std::env::var(config::DATABASE_URL)
        .map_err(|_| anyhow::anyhow!("{} must be set", config::DATABASE_URL))?;
    let pool = db::connect(&url).await?;
    db::run_migrations(&pool).await?;
    match db::accounts::set_role(&pool, email, pedagog_store::GlobalRole::Admin).await? {
        db::accounts::RoleSet::Set { user, .. } => println!("{} is an admin", user.email),
        db::accounts::RoleSet::LastAdmin => unreachable!("making an admin never removes one"),
    }
    Ok(())
}

async fn serve() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    let pool = db::connect(&config.database_url).await?;
    db::run_migrations(&pool).await?;
    let mailer = Mailer::smtp(&config.smtp)?;
    tracing::info!(
        smtp = %config.smtp.host, port = config.smtp.port, tls = ?config.smtp.tls,
        login = config.smtp.login.is_some(), "starting pedagog-api"
    );
    let state = AppState {
        pool,
        config: Arc::new(config),
        mailer,
        auth: Authenticator::EmailCode,
        jobs: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()?,
    };

    // Background work owned here: if it stops, the service exits and is restarted.
    let mut background = JoinSet::new();
    background.spawn(cleanup(state.clone()));
    background.spawn(notices::run(state.clone()));

    let listener = TcpListener::bind(&state.config.listen_addr).await?;
    tracing::info!(addr = %state.config.listen_addr, "listening");
    tokio::select! {
        served = axum::serve(listener, http::router(state)) => served?,
        Some(stopped) = background.join_next() => {
            anyhow::bail!("a background task stopped: {stopped:?}");
        }
    }
    Ok(())
}

/// Forget expired sessions and login codes, hourly.
async fn cleanup(state: AppState) {
    loop {
        match db::accounts::delete_expired(&state.pool).await {
            Ok(n) if n > 0 => tracing::info!(removed = n, "expired sessions and codes removed"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "removing expired sessions failed"),
        }
        tokio::time::sleep(CLEANUP_EVERY).await;
    }
}
