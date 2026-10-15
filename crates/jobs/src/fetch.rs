//! `pedagog-jobs fetch-context <url> <dir> <tree-hash>`: the Kaniko pod's
//! initContainer. Downloads an assignment archive through a presigned GET and
//! unpacks it as the build context, refusing anything whose files don't match
//! the tree hash recorded at upload.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, bail};

use crate::archive;
use crate::config::DEFAULT_LIMITS;

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

pub fn fetch_context(url: &str, dir: &Path, expected_tree: &str) -> anyhow::Result<()> {
    // The URL carries a signature: keep it out of errors and logs.
    let client = reqwest::blocking::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .context("building the HTTP client")?;
    let bytes = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .and_then(reqwest::blocking::Response::bytes)
        .map_err(|e| anyhow::anyhow!("downloading the archive failed: {}", e.without_url()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tree = archive::extract(&bytes, &DEFAULT_LIMITS, dir)
        .map_err(|e| anyhow::anyhow!("unpacking the archive failed: {e}"))?;
    if tree != expected_tree {
        bail!("the archive's files don't match the upload (tree {tree}, expected {expected_tree})");
    }
    println!("fetched the build context ({tree})");
    Ok(())
}
