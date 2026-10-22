//! The Kaniko pod's initContainers. `fetch-context <url> <dir> <tree-hash>`
//! downloads an assignment archive through a presigned GET and unpacks it as
//! the build context, refusing anything whose files don't match the tree hash
//! recorded at upload. `fetch-ingredients <dir> (<sha256> <path> <url>)...`
//! downloads the ingredients a build declares to where `vend` looks, refusing
//! any whose checksum isn't the pinned one.

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

/// Writes to a file and hashes what passes through.
struct Hashing<W> {
    file: W,
    hasher: sha2::Sha256,
}

impl<W: std::io::Write> std::io::Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use sha2::Digest;
        let n = self.file.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Each `(sha256, path, url)`: download, check, write to `dir/path`.
pub fn fetch_ingredients(dir: &Path, triples: &[String]) -> anyhow::Result<()> {
    use sha2::{Digest, Sha256};
    if !triples.len().is_multiple_of(3) {
        bail!("usage: pedagog-jobs fetch-ingredients <dir> (<sha256> <path> <url>)...");
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .context("building the HTTP client")?;
    for t in triples.chunks(3) {
        let [sha256, path, url] = t else {
            unreachable!()
        };
        // A path from the recipes, not from student code; still, stay under `dir`.
        if path.split('/').any(|seg| seg == ".." || seg.is_empty()) {
            bail!("unsafe ingredient path {path:?}");
        }
        let mut response = client
            .get(url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|e| anyhow::anyhow!("downloading {path} failed: {}", e.without_url()))?;
        let dest = dir.join(path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        // Straight to disk, hashing on the way (the files can be hundreds of MB).
        let file =
            std::fs::File::create(&dest).with_context(|| format!("writing {}", dest.display()))?;
        let mut writer = Hashing {
            file,
            hasher: Sha256::new(),
        };
        let size = std::io::copy(&mut response, &mut writer)
            .map_err(|e| anyhow::anyhow!("downloading {path} failed: {e}"))?;
        let got = format!("{:x}", writer.hasher.finalize());
        if &got != sha256 {
            let _ = std::fs::remove_file(&dest);
            bail!("{path} isn't the pinned file (sha256 {got}, expected {sha256})");
        }
        println!("fetched {path} ({size} bytes)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingredient_fetches_stay_under_their_dir_and_come_in_threes() {
        let dir = std::env::temp_dir().join(format!("fetch-ingredients-{}", std::process::id()));
        let args = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let err = fetch_ingredients(&dir, &args(&["ab", "../etc/x", "http://127.0.0.1:1/x"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unsafe"), "{err}");
        let err = fetch_ingredients(&dir, &args(&["ab", "x"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("usage"), "{err}");
        assert!(fetch_ingredients(&dir, &[]).is_ok(), "nothing to fetch");
    }

    #[test]
    fn what_is_written_is_what_is_hashed() {
        use sha2::Digest;
        let mut w = Hashing {
            file: Vec::new(),
            hasher: sha2::Sha256::new(),
        };
        std::io::copy(&mut &b"hello"[..], &mut w).unwrap();
        assert_eq!(w.file, b"hello");
        assert_eq!(
            format!("{:x}", w.hasher.finalize()),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
