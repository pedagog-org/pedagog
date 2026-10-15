use serde::{Deserialize, Deserializer};

#[derive(Debug, Deserialize)]
pub struct Ingredient {
    pub output: String,
    /// Expected SHA-256 of the file (hex); `hammer vend` refuses a download that differs.
    #[serde(default, deserialize_with = "sha256_hex")]
    pub sha256: Option<String>,
    #[serde(flatten)]
    pub source: IngredientSource,
}

fn sha256_hex<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let Some(hex) = Option::<String>::deserialize(d)? else {
        return Ok(None);
    };
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(serde::de::Error::custom(format!(
            "sha256 must be 64 hex digits, got {hex:?}"
        )));
    }
    Ok(Some(hex.to_ascii_lowercase()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IngredientSource {
    Github(GithubSource),
    Url(url::Url),
}

#[derive(Debug, Deserialize)]
pub struct GithubSource {
    pub repo: String,
    pub asset: String,
    pub tag: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingredient_github_source() {
        let yaml =
            "output: code-server.deb\ngithub:\n  repo: org/repo\n  asset: '*.deb'\n  tag: v1.0.0\n";
        let i: Ingredient = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(i.output, "code-server.deb");
        assert!(matches!(i.source, IngredientSource::Github(ref g) if g.tag == "v1.0.0"));
    }

    #[test]
    fn ingredient_url_source() {
        let yaml = "output: tool.tar.xz\nurl: 'https://example.com/tool.tar.xz'\n";
        let i: Ingredient = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(i.output, "tool.tar.xz");
        assert!(matches!(i.source, IngredientSource::Url(_)));
    }

    #[test]
    fn sha256_is_optional_and_normalized() {
        let yaml = "output: a\nurl: 'https://example.com/a'\n";
        assert!(
            serde_yaml::from_str::<Ingredient>(yaml)
                .unwrap()
                .sha256
                .is_none()
        );
        let digest = "AB".repeat(32);
        let yaml = format!("output: a\nurl: 'https://example.com/a'\nsha256: {digest}\n");
        let i: Ingredient = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(i.sha256.as_deref(), Some("ab".repeat(32).as_str()));
    }

    #[test]
    fn malformed_sha256_is_rejected() {
        for bad in ["abc", &"g".repeat(64), &"a".repeat(65)] {
            let yaml = format!("output: a\nurl: 'https://example.com/a'\nsha256: '{bad}'\n");
            assert!(serde_yaml::from_str::<Ingredient>(&yaml).is_err(), "{bad}");
        }
    }

    #[test]
    fn ingredient_url_rejects_invalid() {
        let yaml = "output: tool.tar.xz\nurl: 'not a url'\n";
        assert!(serde_yaml::from_str::<Ingredient>(yaml).is_err());
    }

    #[test]
    fn ingredients_default_empty() {
        use crate::recipe::os::OsDef;
        let yaml = "id: test-os\nupstream: ubuntu:22.04\nimage: test/os:1\nhooks:\n  build:\n    init: []\n  pkg:\n    install:\n      steps: []\n    remove:\n      steps: []\n";
        let def: OsDef = serde_yaml::from_str(yaml).unwrap();
        assert!(def.ingredients.is_empty());
    }
}
