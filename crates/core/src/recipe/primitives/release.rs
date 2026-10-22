//! GitHub release assets, as ingredients name them (`repo`, `tag`, an `asset`
//! pattern): the API's reply, and picking the one asset that matches. The
//! download itself is the caller's (hammer runs curl; jobs uses its client).

use serde::Deserialize;

use super::GithubSource;

/// The release's API URL (public repositories need no token).
pub fn release_api_url(gh: &GithubSource) -> String {
    format!(
        "https://api.github.com/repos/{}/releases/tags/{}",
        gh.repo, gh.tag
    )
}

#[derive(Deserialize)]
pub struct Release {
    pub assets: Vec<Asset>,
}

#[derive(Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

pub fn select_asset<'a>(assets: &'a [Asset], pattern: &str) -> Result<&'a Asset, String> {
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
pub fn wildcard_match(pattern: &str, name: &str) -> bool {
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
