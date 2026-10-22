use serde::{Deserialize, Deserializer};

#[derive(Debug, Deserialize)]
pub struct Ingredient {
    pub output: String,
    /// The file's SHA-256 (hex), required: builds get exactly this file (stored
    /// by it), and a download that differs is refused.
    #[serde(deserialize_with = "sha256_hex")]
    pub sha256: String,
    #[serde(flatten)]
    pub source: IngredientSource,
}

/// An ingredient as a build needs it: where `vend` finds it, and which file.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pin {
    pub output: String,
    pub sha256: String,
}

/// The pins of a recipe's ingredients.
pub fn pins(ingredients: &[Ingredient]) -> Vec<Pin> {
    ingredients
        .iter()
        .map(|i| Pin {
            output: i.output.clone(),
            sha256: i.sha256.clone(),
        })
        .collect()
}

fn sha256_hex<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let hex = String::deserialize(d)?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(serde::de::Error::custom(format!(
            "sha256 must be 64 hex digits, got {hex:?}"
        )));
    }
    Ok(hex.to_ascii_lowercase())
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

    const SHA: &str = "sha256: 0000000000000000000000000000000000000000000000000000000000000000\n";

    #[test]
    fn ingredient_github_source() {
        let yaml = format!(
            "output: code-server.deb\ngithub:\n  repo: org/repo\n  asset: '*.deb'\n  tag: v1.0.0\n{SHA}"
        );
        let i: Ingredient = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(i.output, "code-server.deb");
        assert!(matches!(i.source, IngredientSource::Github(ref g) if g.tag == "v1.0.0"));
    }

    #[test]
    fn ingredient_url_source() {
        let yaml = format!("output: tool.tar.xz\nurl: 'https://example.com/tool.tar.xz'\n{SHA}");
        let i: Ingredient = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(i.output, "tool.tar.xz");
        assert!(matches!(i.source, IngredientSource::Url(_)));
    }

    #[test]
    fn sha256_is_required_and_normalized() {
        let yaml = "output: a\nurl: 'https://example.com/a'\n";
        let err = serde_yaml::from_str::<Ingredient>(yaml)
            .unwrap_err()
            .to_string();
        assert!(err.contains("sha256"), "{err}");
        let digest = "AB".repeat(32);
        let yaml = format!("output: a\nurl: 'https://example.com/a'\nsha256: {digest}\n");
        let i: Ingredient = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(i.sha256, "ab".repeat(32));
        assert_eq!(
            pins(&[i]),
            [Pin {
                output: "a".into(),
                sha256: "ab".repeat(32)
            }]
        );
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
