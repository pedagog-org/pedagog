//! Downloads GitHub release assets through the public REST API and `curl`, so
//! vendoring needs neither the `gh` CLI nor a login for public repositories.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use pedagog_core::recipe::primitives::GithubSource;
use serde::Deserialize;

/// Optional token env vars (first non-empty wins). Only raises the anonymous
/// API rate limit; never required for public repositories.
const TOKEN_VARS: [&str; 2] = ["GH_TOKEN", "GITHUB_TOKEN"];

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub fn download(gh: &GithubSource, out_path: &Path) -> Result<(), String> {
    let api = format!(
        "https://api.github.com/repos/{}/releases/tags/{}",
        gh.repo, gh.tag
    );
    let body = fetch_api(&api)?;
    let release: Release = serde_json::from_slice(&body)
        .map_err(|e| format!("unexpected response from {api}: {e}"))?;
    let asset = select_asset(&release.assets, &gh.asset)
        .map_err(|e| format!("{} release {}: {e}", gh.repo, gh.tag))?;
    curl_to_file(&asset.browser_download_url, out_path)
}

fn select_asset<'a>(assets: &'a [Asset], pattern: &str) -> Result<&'a Asset, String> {
    let matches: Vec<&Asset> = assets
        .iter()
        .filter(|a| wildcard_match(pattern, &a.name))
        .collect();
    let names = |list: &mut dyn Iterator<Item = &Asset>| {
        list.map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
    };
    match matches.as_slice() {
        [only] => Ok(only),
        [] => Err(format!(
            "no asset matches {pattern:?} (assets: {})",
            names(&mut assets.iter())
        )),
        many => Err(format!(
            "{} assets match {pattern:?} ({}); make the pattern more specific",
            many.len(),
            names(&mut many.iter().copied())
        )),
    }
}

/// `*` matches any run of characters (including none); everything else is literal.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let [first, middle @ .., last] = parts.as_slice() else {
        return pattern == name;
    };
    if name.len() < first.len() + last.len() || !name.starts_with(first) || !name.ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for part in middle {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
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

#[cfg(test)]
mod tests {
    use super::*;

    fn assets(names: &[&str]) -> Vec<Asset> {
        names
            .iter()
            .map(|n| Asset {
                name: (*n).to_owned(),
                browser_download_url: format!("https://example.com/{n}"),
            })
            .collect()
    }

    #[test]
    fn wildcard_matches_like_a_glob_star() {
        assert!(wildcard_match(
            "code-server_*_arm64.deb",
            "code-server_4.127.0_arm64.deb"
        ));
        assert!(!wildcard_match(
            "code-server_*_arm64.deb",
            "code-server_4.127.0_amd64.deb"
        ));
        assert!(wildcard_match("exact.tar.gz", "exact.tar.gz"));
        assert!(!wildcard_match("exact.tar.gz", "exact.tar.gz.sha256"));
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("a*b*c", "a-x-b-y-c"));
        assert!(!wildcard_match("a*b*c", "a-c-b"));
        // Prefix and suffix may not overlap: "ab" is too short for "ab*b".
        assert!(!wildcard_match("ab*b", "ab"));
    }

    #[test]
    fn selects_the_single_matching_asset() {
        let list = assets(&["tool_1.0_amd64.deb", "tool_1.0_arm64.deb", "checksums.txt"]);
        let picked = select_asset(&list, "tool_*_arm64.deb").unwrap();
        assert_eq!(picked.name, "tool_1.0_arm64.deb");
    }

    #[test]
    fn no_match_lists_available_assets() {
        let list = assets(&["a.deb", "b.deb"]);
        let err = select_asset(&list, "c*.deb").err().unwrap();
        assert!(
            err.contains("no asset matches") && err.contains("a.deb, b.deb"),
            "{err}"
        );
    }

    #[test]
    fn several_matches_is_an_error() {
        let list = assets(&["tool_amd64.deb", "tool_arm64.deb"]);
        let err = select_asset(&list, "tool_*.deb").err().unwrap();
        assert!(err.contains("2 assets match"), "{err}");
    }

    #[test]
    fn parses_release_api_response() {
        let json = r#"{"tag_name":"v1","assets":[{"name":"x.deb","browser_download_url":"https://github.com/o/r/releases/download/v1/x.deb","size":1}]}"#;
        let release: Release = serde_json::from_str(json).unwrap();
        assert_eq!(release.assets[0].name, "x.deb");
    }
}
