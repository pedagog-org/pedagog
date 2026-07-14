use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Ingredient {
    pub output: String,
    #[serde(flatten)]
    pub source: IngredientSource,
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
        let yaml = "output: code-server.deb\ngithub:\n  repo: org/repo\n  asset: '*.deb'\n  tag: v1.0.0\n";
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
    fn ingredient_url_rejects_invalid() {
        let yaml = "output: tool.tar.xz\nurl: 'not a url'\n";
        assert!(serde_yaml::from_str::<Ingredient>(yaml).is_err());
    }

    #[test]
    fn ingredients_default_empty() {
        use crate::recipe::os::OsDef;
        let yaml = "id: test-os\nupstream: ubuntu:22.04\nimage: test/os:1\nhooks:\n  build:\n    init: []\n  pkg:\n    install:\n      steps: []\n    remove:\n      steps: []\n  network:\n    transcribe:\n      steps: []\n    enable:\n      steps: []\n    disable:\n      steps: []\n";
        let def: OsDef = serde_yaml::from_str(yaml).unwrap();
        assert!(def.ingredients.is_empty());
    }
}
