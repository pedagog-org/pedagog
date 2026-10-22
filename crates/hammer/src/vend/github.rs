//! Downloads GitHub release assets through the public REST API and `curl`, so
//! vendoring needs neither the `gh` CLI nor a login for public repositories.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use pedagog_core::recipe::primitives::GithubSource;
use pedagog_core::recipe::primitives::release::{Release, release_api_url, select_asset};

/// Optional token env vars (first non-empty wins). Only raises the anonymous
/// API rate limit; never required for public repositories.
const TOKEN_VARS: [&str; 2] = ["GH_TOKEN", "GITHUB_TOKEN"];

pub fn download(gh: &GithubSource, out_path: &Path) -> Result<(), String> {
    let api = release_api_url(gh);
    let body = fetch_api(&api)?;
    let release: Release = serde_json::from_slice(&body)
        .map_err(|e| format!("unexpected response from {api}: {e}"))?;
    let asset = select_asset(&release.assets, &gh.asset)
        .map_err(|e| format!("{} release {}: {e}", gh.repo, gh.tag))?;
    curl_to_file(&asset.browser_download_url, out_path)
}

fn token() -> Option<String> {
    TOKEN_VARS
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|t| !t.is_empty())
}

/// GETs a GitHub API URL. The token header goes through stdin (`-H @-`) so it
/// never appears in the process list.
fn fetch_api(url: &str) -> Result<Vec<u8>, String> {
    let token = token();
    let mut cmd = Command::new("curl");
    cmd.args([
        "-fsSL",
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        "X-GitHub-Api-Version: 2022-11-28",
    ]);
    if token.is_some() {
        cmd.args(["-H", "@-"]);
    }
    let mut child = cmd
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run curl: {e}"))?;
    if let (Some(t), Some(mut stdin)) = (token, child.stdin.take()) {
        writeln!(stdin, "Authorization: Bearer {t}")
            .map_err(|e| format!("failed to pass token to curl: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("curl failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "GitHub API request failed: {url} (rate-limited? set GH_TOKEN)"
        ));
    }
    Ok(out.stdout)
}

fn curl_to_file(url: &str, out_path: &Path) -> Result<(), String> {
    let status = Command::new("curl")
        .args(["-fsSL", url, "-o", &out_path.to_string_lossy()])
        .status()
        .map_err(|e| format!("failed to run curl: {e}"))?;
    if !status.success() {
        return Err(format!("download failed: {url}"));
    }
    Ok(())
}
