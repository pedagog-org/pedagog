//! The nightly registry cleanup: delete assignment images nothing uses and
//! Kaniko cache images older than [`CACHE_MAX_AGE`], then (once no build is
//! pushing) reclaim their space with the registry's garbage collection, and
//! check how full its volume is.

use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use pedagog_k8s::maintenance::{DISK_MARKER, Maintenance, RegistryGc};
use pedagog_store::db;
use time::OffsetDateTime;

use crate::disk;
use crate::state::AppState;

const ASSIGNMENTS: &str = "pedagog/assignments/";
const CACHE: &str = "pedagog/cache";
pub const CACHE_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 3600);
/// How long the cleanup waits for builds to finish pushing before skipping GC.
const PUSH_WAIT: Duration = Duration::from_secs(30 * 60);
const GC_DEADLINE: Duration = Duration::from_secs(15 * 60);
const MANIFEST_TYPES: &str = "application/vnd.oci.image.manifest.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json, \
     application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.list.v2+json";

/// A tag in the registry, and the manifest it points to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub repo: String,
    pub tag: String,
    pub digest: String,
}

impl Tag {
    /// `repository:tag`, as images are matched against builds.
    pub fn image(&self) -> String {
        format!("{}:{}", self.repo, self.tag)
    }
}

/// The registry's HTTP API (plain HTTP inside the cluster).
pub struct Registry {
    base: String,
    http: reqwest::Client,
}

impl Registry {
    pub fn new(host: &str) -> Self {
        Self {
            base: format!("http://{host}/v2"),
            http: reqwest::Client::new(),
        }
    }

    pub async fn repositories(&self) -> anyhow::Result<Vec<String>> {
        let mut all = Vec::new();
        let mut last = String::new();
        loop {
            let page: serde_json::Value = self
                .http
                // Repository names are URL-safe ([a-z0-9._/-]).
                .get(format!("{}/_catalog?n=1000&last={last}", self.base))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let repos: Vec<String> =
                serde_json::from_value(page["repositories"].clone()).unwrap_or_default();
            let done = repos.len() < 1000;
            last = repos.last().cloned().unwrap_or_default();
            all.extend(repos);
            if done {
                return Ok(all);
            }
        }
    }

    pub async fn tags(&self, repo: &str) -> anyhow::Result<Vec<String>> {
        let list: serde_json::Value = self
            .http
            .get(format!("{}/{repo}/tags/list", self.base))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(serde_json::from_value(list["tags"].clone()).unwrap_or_default())
    }

    /// A tag's manifest digest and body.
    pub async fn manifest(
        &self,
        repo: &str,
        tag: &str,
    ) -> anyhow::Result<Option<(String, serde_json::Value)>> {
        let reply = self
            .http
            .get(format!("{}/{repo}/manifests/{tag}", self.base))
            .header(reqwest::header::ACCEPT, MANIFEST_TYPES)
            .send()
            .await?;
        if reply.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let reply = reply.error_for_status()?;
        let digest = reply
            .headers()
            .get("Docker-Content-Digest")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("{repo}:{tag} has no digest"))?;
        Ok(Some((digest, reply.json().await?)))
    }

    /// When an image was made, from its config.
    pub async fn created(
        &self,
        repo: &str,
        manifest: &serde_json::Value,
    ) -> anyhow::Result<Option<OffsetDateTime>> {
        let Some(config) = manifest["config"]["digest"].as_str() else {
            return Ok(None);
        };
        let blob: serde_json::Value = self
            .http
            .get(format!("{}/{repo}/blobs/{config}", self.base))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(blob["created"].as_str().and_then(|c| {
            OffsetDateTime::parse(c, &time::format_description::well_known::Rfc3339).ok()
        }))
    }

    pub async fn delete(&self, repo: &str, digest: &str) -> anyhow::Result<()> {
        let reply = self
            .http
            .delete(format!("{}/{repo}/manifests/{digest}", self.base))
            .send()
            .await?;
        if reply.status() != reqwest::StatusCode::NOT_FOUND {
            reply.error_for_status()?;
        }
        Ok(())
    }
}

/// `repository:tag` of a stored image ref (`host[:port]/repository:tag`).
pub fn image_of(image_ref: &str) -> &str {
    image_ref
        .split_once('/')
        .map_or(image_ref, |(_, rest)| rest)
}

/// Assignment tags to delete: those no build in use names, unless a tag in
/// use shares their manifest (deleting a manifest drops every tag on it).
pub fn unused<'a>(tags: &'a [Tag], in_use: &HashSet<String>) -> Vec<&'a Tag> {
    let kept: HashSet<(&str, &str)> = tags
        .iter()
        .filter(|t| in_use.contains(&t.image()))
        .map(|t| (t.repo.as_str(), t.digest.as_str()))
        .collect();
    let mut seen = HashSet::new();
    tags.iter()
        .filter(|t| t.repo.starts_with(ASSIGNMENTS) && !in_use.contains(&t.image()))
        .filter(|t| !kept.contains(&(t.repo.as_str(), t.digest.as_str())))
        .filter(|t| seen.insert((t.repo.as_str(), t.digest.as_str())))
        .collect()
}

/// What a cleanup did.
#[derive(Debug, Default)]
pub struct Cleaned {
    pub images: usize,
    pub cache: usize,
    pub collected: bool,
}

/// Delete what's unused, then collect garbage once no build is pushing.
pub async fn cleanup(state: &AppState) -> anyhow::Result<Cleaned> {
    let registry = Registry::new(&state.config.registry);
    // Tags first, then what's in use: a build that starts in between pushes a
    // tag this listing doesn't have, so it can't be deleted.
    let mut tags = Vec::new();
    let mut cache = Vec::new();
    for repo in registry.repositories().await? {
        if !(repo.starts_with(ASSIGNMENTS) || repo == CACHE) {
            continue;
        }
        for tag in registry.tags(&repo).await? {
            let Some((digest, manifest)) = registry.manifest(&repo, &tag).await? else {
                continue;
            };
            let t = Tag {
                repo: repo.clone(),
                tag,
                digest,
            };
            if repo == CACHE {
                cache.push((t, registry.created(&repo, &manifest).await?));
            } else {
                tags.push(t);
            }
        }
    }
    let in_use: HashSet<String> = db::assignments::image_refs_in_use(&state.pool)
        .await?
        .iter()
        .map(|r| image_of(r).to_owned())
        .collect();

    let mut cleaned = Cleaned::default();
    let mut deleted = Vec::new();
    for t in unused(&tags, &in_use) {
        match registry.delete(&t.repo, &t.digest).await {
            Ok(()) => deleted.push(t.image()),
            Err(e) => tracing::warn!(image = %t.image(), error = %e, "deleting an image failed"),
        }
    }
    db::assignments::images_deleted(&state.pool, &deleted).await?;
    cleaned.images = deleted.len();

    let stale = OffsetDateTime::now_utc() - CACHE_MAX_AGE;
    let digests: BTreeSet<&str> = cache
        .iter()
        .filter(|(_, created)| created.is_some_and(|c| c < stale))
        .map(|(t, _)| t.digest.as_str())
        .collect();
    for digest in digests {
        match registry.delete(CACHE, digest).await {
            Ok(()) => cleaned.cache += 1,
            Err(e) => tracing::warn!(%digest, error = %e, "deleting a cache image failed"),
        }
    }
    tracing::info!(
        images = cleaned.images,
        cache = cleaned.cache,
        "registry: deleted what nothing uses"
    );

    let waited = tokio::time::Instant::now();
    while db::maintenance::pushes_in_progress(&state.pool).await? > 0 {
        if waited.elapsed() > PUSH_WAIT {
            tracing::warn!("registry: builds kept pushing; garbage collection skipped tonight");
            return Ok(cleaned);
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
    let run = state
        .maintenance
        .registry_gc(&RegistryGc {
            image: state.config.registry_image.clone(),
            pvc: state.config.registry_pvc.clone(),
            registry_app: "registry".into(),
            deadline: GC_DEADLINE,
        })
        .await?;
    if !run.succeeded {
        let tail = &run.logs[run.logs.len().saturating_sub(2000)..];
        tracing::error!(logs = %tail, "registry garbage collection failed");
        return Ok(cleaned);
    }
    cleaned.collected = true;
    match run
        .logs
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix(DISK_MARKER))
    {
        Some(line) => match disk::parse_df(line) {
            Some(usage) => state.config.disk.report("registry volume", usage),
            None => tracing::warn!(%line, "registry: unreadable disk usage"),
        },
        None => tracing::warn!("registry: garbage collection didn't report disk usage"),
    }
    Ok(cleaned)
}

/// Postgres's databases against its volume (data only: a hint, not exact).
pub async fn check_postgres(state: &AppState, maintenance: &Maintenance) -> anyhow::Result<()> {
    let Some(capacity) = maintenance
        .pvc_capacity(&state.config.postgres_pvc)
        .await?
        .as_deref()
        .and_then(disk::bytes)
    else {
        return Ok(());
    };
    let used = db::maintenance::database_bytes(&state.pool).await?;
    state.config.disk.report(
        "postgres volume (databases' size)",
        disk::Usage {
            used: used.max(0) as u64,
            capacity,
        },
    );
    Ok(())
}

/// Background: the cleanup once a day at `at` (UTC), on one replica, and
/// Postgres's size every hour.
pub async fn run(state: AppState) {
    let mut last_postgres = tokio::time::Instant::now() - Duration::from_secs(3600);
    loop {
        if last_postgres.elapsed() >= Duration::from_secs(3600) {
            if let Err(e) = check_postgres(&state, &state.maintenance).await {
                tracing::warn!(error = %e, "checking postgres's size failed");
            }
            last_postgres = tokio::time::Instant::now();
        }
        let now = OffsetDateTime::now_utc();
        if let Some(at) = state.config.cleanup_at
            && (now.hour(), now.minute()) >= at
        {
            let day = now.date().to_string();
            match db::maintenance::claim(&state.pool, "registry-cleanup", &day).await {
                Ok(true) => match cleanup(&state).await {
                    Ok(c) => tracing::info!(?c, "registry cleanup done"),
                    Err(e) => tracing::error!(error = %e, "registry cleanup failed"),
                },
                Ok(false) => {}
                Err(e) => tracing::warn!(error = %e, "claiming the registry cleanup failed"),
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

/// `HH:MM` (UTC), or `off`.
pub fn parse_at(text: &str) -> anyhow::Result<Option<(u8, u8)>> {
    if text.trim().eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    let bad = || anyhow::anyhow!("expected HH:MM (UTC) or off, got {text:?}");
    let (h, m) = text.trim().split_once(':').ok_or_else(bad)?;
    let (h, m): (u8, u8) = (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?);
    if h > 23 || m > 59 {
        return Err(bad());
    }
    Ok(Some((h, m)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(repo: &str, tag: &str, digest: &str) -> Tag {
        Tag {
            repo: repo.into(),
            tag: tag.into(),
            digest: digest.into(),
        }
    }

    #[test]
    fn images_nothing_uses_go_unless_they_share_a_kept_manifest() {
        let ll = "pedagog/assignments/cop3502/ll";
        let tags = [
            tag(ll, "aaa", "sha256:1"),
            tag(ll, "bbb", "sha256:2"),
            tag(ll, "ccc", "sha256:1"),
            tag(ll, "ddd", "sha256:3"),
            tag(ll, "eee", "sha256:3"),
            tag("pedagog/ubuntu", "22", "sha256:9"),
        ];
        let in_use: HashSet<String> = [format!("{ll}:aaa")].into();
        let gone: Vec<String> = unused(&tags, &in_use).iter().map(|t| t.image()).collect();
        assert_eq!(gone, [format!("{ll}:bbb"), format!("{ll}:ddd")]);
    }

    #[test]
    fn refs_lose_their_host_and_times_parse() {
        assert_eq!(
            image_of("registry-service.pedagog-data.svc:5000/pedagog/assignments/c/ll:abc"),
            "pedagog/assignments/c/ll:abc"
        );
        assert_eq!(parse_at("03:00").unwrap(), Some((3, 0)));
        assert_eq!(parse_at("off").unwrap(), None);
        assert!(parse_at("25:00").is_err());
        assert!(parse_at("3").is_err());
    }
}
