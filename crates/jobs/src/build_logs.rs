//! Each phase's pod log of a build attempt, whole, in Garage
//! (`logs/builds/{id}/{build,validation}.log.gz`) for the staff build page; the
//! attempt's row keeps only a failure's short reason and the log's length.
//! The nightly cleanup deletes logs [`KEEP_DAYS`] after their attempt ended.

use std::io::Write;

use flate2::Compression;
use flate2::write::GzEncoder;
use pedagog_store::db::assignments::{self as db, LogPhase};
use pedagog_store::objects::build_log_key;

use crate::state::AppState;

pub const KEEP_DAYS: i32 = 180;
const MAX_REASON_CHARS: usize = 500;

/// A failure in a line: "timed out", else the log's last non-empty line.
pub fn reason(log: &str) -> String {
    if log.starts_with("timed out") {
        return "timed out".to_owned();
    }
    let last = log
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("failed (no log)");
    last.chars().take(MAX_REASON_CHARS).collect()
}

fn gzip(text: &str) -> std::io::Result<Vec<u8>> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(text.as_bytes())?;
    gz.finish()
}

/// Store a phase's log (best effort: a build never fails over its log).
pub async fn keep(state: &AppState, build: i64, phase: LogPhase, log: &str) {
    if log.is_empty() {
        return;
    }
    let lines = i32::try_from(log.lines().count()).unwrap_or(i32::MAX);
    let stored = async {
        state
            .objects
            .put(&build_log_key(build, phase.as_str()), gzip(log)?)
            .await?;
        db::log_stored(&state.pool, build, phase, lines).await?;
        anyhow::Ok(())
    };
    if let Err(e) = stored.await {
        tracing::warn!(build, phase = phase.as_str(), error = %e, "keeping a build log failed");
    }
}

/// Delete the logs of attempts that ended more than [`KEEP_DAYS`] ago.
pub async fn expire(state: &AppState) -> anyhow::Result<usize> {
    let builds = db::expired_logs(&state.pool, KEEP_DAYS).await?;
    for &build in &builds {
        for phase in [LogPhase::Build, LogPhase::Validation] {
            // Absent already (a phase that never ran) is fine.
            let _ = state
                .objects
                .delete(&build_log_key(build, phase.as_str()))
                .await;
        }
        db::logs_forgotten(&state.pool, build).await?;
    }
    Ok(builds.len())
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    #[test]
    fn a_failure_is_its_last_line_or_a_timeout() {
        assert_eq!(
            reason("Step 7/9\nERROR: no matching numpy\nbuild failed: exit status 1\n\n"),
            "build failed: exit status 1"
        );
        assert_eq!(reason("timed out\nStep 3/9\n"), "timed out");
        assert_eq!(reason(""), "failed (no log)");
        assert_eq!(reason(&"é".repeat(600)).chars().count(), 500);
    }

    #[test]
    fn logs_are_gzipped_whole() {
        let log = "line 1\nline 2\n";
        let mut back = String::new();
        flate2::read::GzDecoder::new(gzip(log).unwrap().as_slice())
            .read_to_string(&mut back)
            .unwrap();
        assert_eq!(back, log);
    }
}
