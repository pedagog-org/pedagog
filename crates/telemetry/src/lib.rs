//! The services' logs and error reports: log lines filtered by `RUST_LOG`
//! (default `info`), and, when `PEDAGOG_SENTRY_DSN` is set, every warning and
//! error (and panic) reported to GlitchTip, with the info lines before it as
//! breadcrumbs.

use sentry::integrations::tracing::EventFilter;
use sentry::types::Dsn;
use tracing::{Level, Metadata};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub const SENTRY_DSN: &str = "PEDAGOG_SENTRY_DSN";

/// Keep it for the life of the process: dropping it sends what's pending.
#[must_use]
pub struct Guard(#[allow(dead_code)] Option<sentry::ClientInitGuard>);

/// Set up logging and reporting for `release` (e.g. `pedagog-api@0.0.0`).
/// Call it before starting the tokio runtime, so its threads see the client.
pub fn init(release: &str) -> anyhow::Result<Guard> {
    let client = dsn(std::env::var(SENTRY_DSN).ok())?.map(|dsn| {
        let mut options = sentry::ClientOptions::default();
        options.dsn = Some(dsn);
        options.release = Some(release.to_owned().into());
        sentry::init(options)
    });
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .with(client.is_some().then(layer))
        .init();
    if client.is_some() {
        tracing::info!(
            release,
            "reporting warnings and errors to {SENTRY_DSN}'s host"
        );
    }
    Ok(Guard(client))
}

/// The DSN, if one is set; a malformed one stops the service from starting.
fn dsn(value: Option<String>) -> anyhow::Result<Option<Dsn>> {
    match value.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) => v
            .parse()
            .map(Some)
            .map_err(|e| anyhow::anyhow!("{SENTRY_DSN} is not a DSN: {e}")),
    }
}

fn layer<S>() -> sentry::integrations::tracing::SentryLayer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    sentry::integrations::tracing::layer().event_filter(filter)
}

fn filter(md: &Metadata<'_>) -> EventFilter {
    match *md.level() {
        Level::ERROR | Level::WARN => EventFilter::Event,
        Level::INFO => EventFilter::Breadcrumb,
        _ => EventFilter::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dsn_is_optional_but_must_parse() {
        assert!(dsn(None).unwrap().is_none());
        assert!(dsn(Some(" ".into())).unwrap().is_none());
        let d = dsn(Some("http://k@glitchtip.pedagog-data.svc:8000/2".into()))
            .unwrap()
            .unwrap();
        assert_eq!(
            (d.host(), d.project_id().to_string()),
            ("glitchtip.pedagog-data.svc", "2".into())
        );
        assert!(dsn(Some("glitchtip".into())).is_err());
    }

    #[test]
    fn warnings_and_errors_are_reported_with_info_as_breadcrumbs() {
        let events = sentry::test::with_captured_events(|| {
            let subscriber = tracing_subscriber::registry().with(layer());
            tracing::subscriber::with_default(subscriber, || {
                tracing::debug!("ignored");
                tracing::info!("context");
                tracing::warn!(notice = 7, "sending a notice failed; will retry");
                tracing::error!("gave up");
            });
        });
        let messages: Vec<_> = events
            .iter()
            .map(|e| e.message.clone().unwrap_or_default())
            .collect();
        assert_eq!(messages, ["sending a notice failed; will retry", "gave up"]);
        assert_eq!(events[0].level, sentry::Level::Warning);
        let crumbs: Vec<_> = events[0]
            .breadcrumbs
            .iter()
            .filter_map(|b| b.message.clone())
            .collect();
        assert_eq!(crumbs, ["context"]);
    }
}
