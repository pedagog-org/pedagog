//! Ingredients in Garage, once per content (`ingredients/sha256/<hex>`), shared by
//! every recipe and assignment that pins the same file. Jobs downloads what its
//! recipes pin (checking each sha256) at startup and every ten minutes, and a
//! build that needs one still missing fetches it then. What no recipe pins any
//! more is deleted at once, unless a build in flight still declared it. Builds
//! fetch only what they declare, by checksum (`fetch-ingredients`), so they never
//! reach GitHub.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use pedagog_core::recipe::primitives::release::{Release, release_api_url, select_asset};
use pedagog_core::recipe::primitives::{Ingredient, IngredientSource};
use pedagog_core::recipe::render::Needed;
use pedagog_core::recipe::store::RecipeStore;
use pedagog_k8s::build::IngredientFetch;
use pedagog_store::db::assignments as db;
use pedagog_store::objects::{INGREDIENTS_PREFIX, ingredient_key};
use sha2::{Digest, Sha256};

use crate::state::AppState;

/// How often the pins are synced again (missing downloads retried, unpinned
/// files deleted).
const SYNC_EVERY: Duration = Duration::from_secs(600);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// Every pinned ingredient in the recipes, by sha256 (the first recipe naming it
/// says where to download it from), with what to call it in messages.
pub fn pinned(store: &RecipeStore) -> BTreeMap<String, (String, &Ingredient)> {
    let mut out = BTreeMap::new();
    for os in store.list_oses() {
        if let Some(def) = store.os(os) {
            for i in &def.ingredients {
                out.entry(i.sha256.clone())
                    .or_insert((format!("os/{os}/{}", i.output), i));
            }
        }
    }
    for p in store.list_platforms() {
        for i in &p.ingredients {
            out.entry(i.sha256.clone())
                .or_insert((format!("platform/{}/{}", p.platform, i.output), i));
        }
    }
    for t in store.list_toolchains() {
        for i in &t.ingredients {
            out.entry(i.sha256.clone())
                .or_insert((format!("toolchain/{}/{}/{}", t.id, t.version, i.output), i));
        }
    }
    out
}

/// What to delete: stored files no recipe pins and no build in flight declared.
pub fn unpinned(
    stored: &[String],
    pinned: &BTreeSet<String>,
    in_flight: &BTreeSet<String>,
) -> Vec<String> {
    stored
        .iter()
        .filter(|sha| !pinned.contains(*sha) && !in_flight.contains(*sha))
        .cloned()
        .collect()
}

/// Where to download an ingredient from (a GitHub release resolves to its asset).
async fn source_url(client: &reqwest::Client, i: &Ingredient) -> anyhow::Result<String> {
    match &i.source {
        IngredientSource::Url(url) => Ok(url.to_string()),
        IngredientSource::Github(gh) => {
            let mut req = client
                .get(release_api_url(gh))
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28");
            if let Some(token) = ["GH_TOKEN", "GITHUB_TOKEN"]
                .iter()
                .filter_map(|v| std::env::var(v).ok())
                .find(|t| !t.is_empty())
            {
                req = req.bearer_auth(token);
            }
            let release: Release = req
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .with_context(|| format!("reading {} release {}", gh.repo, gh.tag))?
                .json()
                .await
                .with_context(|| format!("reading {} release {}", gh.repo, gh.tag))?;
            Ok(select_asset(&release.assets, &gh.asset)
                .map_err(|e| anyhow!("{} release {}: {e}", gh.repo, gh.tag))?
                .browser_download_url
                .clone())
        }
    }
}

/// Stream an ingredient from its source into Garage, hashing as it goes; it's
/// kept only if it's the pinned file (a 200 MB `.deb` never sits in memory).
async fn transfer(
    state: &AppState,
    client: &reqwest::Client,
    i: &Ingredient,
) -> anyhow::Result<()> {
    let url = source_url(client, i).await?;
    let mut response = client
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("downloading {url}"))?;
    let mut upload = state.objects.upload(&ingredient_key(&i.sha256)).await?;
    let mut hasher = Sha256::new();
    let streamed: anyhow::Result<()> = async {
        while let Some(chunk) = response
            .chunk()
            .await
            .with_context(|| format!("downloading {url}"))?
        {
            hasher.update(&chunk);
            upload.write(&chunk).await?;
        }
        Ok(())
    }
    .await;
    let got = format!("{:x}", hasher.finalize());
    match streamed {
        Ok(()) if got == i.sha256 => {
            upload.finish().await?;
            Ok(())
        }
        Ok(()) => {
            let _ = upload.abort().await;
            bail!(
                "{url} isn't the pinned file: sha256 {got}, expected {}",
                i.sha256
            )
        }
        Err(e) => {
            let _ = upload.abort().await;
            Err(e)
        }
    }
}

fn client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent(concat!("pedagog-jobs/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("building the HTTP client")
}

/// Store `i` in Garage unless it's there.
async fn store_one(
    state: &AppState,
    client: &reqwest::Client,
    i: &Ingredient,
) -> anyhow::Result<bool> {
    if state.objects.exists(&ingredient_key(&i.sha256)).await? {
        return Ok(false);
    }
    transfer(state, client, i).await?;
    Ok(true)
}

#[derive(Debug, Default)]
pub struct Synced {
    pub stored: Vec<String>,
    pub missing: Vec<String>,
    pub deleted: Vec<String>,
}

/// One pass: download what's pinned and missing, delete what's unpinned.
pub async fn sync(state: &AppState) -> anyhow::Result<Synced> {
    let store = crate::dispatch::load_store(&state.config.recipes_dir)?;
    let pinned = pinned(&store);
    let client = client()?;
    let mut done = Synced::default();
    for (label, i) in pinned.values() {
        match store_one(state, &client, i).await {
            Ok(true) => done.stored.push(label.clone()),
            Ok(false) => {}
            Err(e) => done.missing.push(format!("{label}: {e:#}")),
        }
    }
    let stored: Vec<String> = state
        .objects
        .keys_under(INGREDIENTS_PREFIX)
        .await?
        .into_iter()
        .filter_map(|k| k.rsplit('/').next().map(str::to_owned))
        .collect();
    let in_flight: BTreeSet<String> = db::ingredients_in_flight(&state.pool)
        .await?
        .into_iter()
        .collect();
    let keys: BTreeSet<String> = pinned.keys().cloned().collect();
    for sha in unpinned(&stored, &keys, &in_flight) {
        state.objects.delete(&ingredient_key(&sha)).await?;
        done.deleted.push(sha);
    }
    Ok(done)
}

/// Background: sync at startup and every ten minutes. Missing ingredients are
/// errors (GlitchTip); builds needing one fail until a pass stores it.
pub async fn run(state: AppState) {
    loop {
        match sync(&state).await {
            Ok(s) => {
                if !s.stored.is_empty() || !s.deleted.is_empty() {
                    tracing::info!(stored = ?s.stored, deleted = ?s.deleted, "ingredients synced");
                }
                for m in &s.missing {
                    tracing::error!(ingredient = %m, "an ingredient couldn't be stored");
                }
            }
            Err(e) => tracing::error!(error = %format!("{e:#}"), "syncing ingredients failed"),
        }
        tokio::time::sleep(SYNC_EVERY).await;
    }
}

/// Before a build: make sure every ingredient it declares is in Garage (storing
/// one a recipe pins but no pass has stored yet).
pub async fn ensure(state: &AppState, needed: &[Needed]) -> anyhow::Result<()> {
    let mut missing: Vec<&Needed> = Vec::new();
    for n in needed {
        if !state.objects.exists(&ingredient_key(&n.sha256)).await? {
            missing.push(n);
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let store = crate::dispatch::load_store(&state.config.recipes_dir)?;
    let pinned = pinned(&store);
    let client = client()?;
    for n in missing {
        let Some((_, i)) = pinned.get(&n.sha256) else {
            bail!(
                "ingredient {} isn't available yet (no recipe pins it)",
                n.path
            );
        };
        store_one(state, &client, i)
            .await
            .with_context(|| format!("ingredient {} isn't available yet", n.path))?;
    }
    Ok(())
}

/// A build's ingredients, made sure of and presigned for its fetch (valid `ttl`).
pub async fn fetches(
    state: &AppState,
    needed: &[Needed],
    ttl: Duration,
) -> anyhow::Result<Vec<IngredientFetch>> {
    ensure(state, needed).await?;
    let mut out = Vec::new();
    for n in needed {
        let url = state
            .objects
            .presign_get(&ingredient_key(&n.sha256), ttl)
            .await?;
        out.push(IngredientFetch {
            path: n.path.clone(),
            sha256: n.sha256.clone(),
            url: url.to_string(),
        });
    }
    Ok(out)
}

/// The checksums a build declares (recorded so they aren't deleted under it).
pub fn checksums(needed: &[Needed]) -> Vec<String> {
    needed.iter().map(|n| n.sha256.clone()).collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const RECIPES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../recipes");

    #[test]
    fn every_pinned_ingredient_is_found_once_by_checksum() {
        let store = crate::dispatch::load_store(Path::new(RECIPES)).unwrap();
        let pinned = pinned(&store);
        let labels: Vec<&str> = pinned.values().map(|(l, _)| l.as_str()).collect();
        assert!(
            labels
                .iter()
                .any(|l| l.starts_with("platform/workspace/code-server.deb")),
            "{labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|l| l.starts_with("toolchain/pedagog-pytest/")),
            "{labels:?}"
        );
        assert!(pinned.keys().all(|k| k.len() == 64));
    }

    #[test]
    fn only_unpinned_files_no_build_needs_are_deleted() {
        let set = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>();
        let stored = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        assert_eq!(unpinned(&stored, &set(&["a"]), &set(&["c"])), ["b"]);
        assert!(unpinned(&stored, &set(&["a", "b", "c"]), &set(&[])).is_empty());
    }
}
