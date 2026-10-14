use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

use super::primitives::{deserialize_one_or_many, Ingredient, OsId, ParamVal, Step, ToolchainId, Version};

#[derive(Debug, Deserialize)]
pub struct ToolchainRecipe {
    pub id: ToolchainId,
    pub version: Version,
    #[serde(deserialize_with = "deserialize_one_or_many")]
    pub os: Vec<OsId>,
    #[serde(default)]
    pub params: HashMap<String, ParamVal>,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Optional extras an assignment may select, with the versions known to be
    /// compatible with this toolchain, e.g. `gdb: ["14", "15"]`.
    #[serde(default)]
    pub addons: BTreeMap<ToolchainId, Vec<Version>>,
    /// Whether this version installs side by side with other versions of the
    /// same toolchain. Two versions in one image require it on every version.
    #[serde(default)]
    pub coexist: bool,
    #[serde(default)]
    pub ingredients: Vec<Ingredient>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addons_parse_grouped_by_id() {
        let yaml = "id: gcc\nversion: \"13\"\nos: ubuntu-22\naddons:\n  gdb: [\"14\", \"15\"]\n  valgrind: [\"3\"]\n";
        let tc: ToolchainRecipe = serde_yaml::from_str(yaml).unwrap();
        let gdb: Vec<&str> = tc.addons[&ToolchainId::try_from("gdb".to_owned()).unwrap()]
            .iter()
            .map(Version::as_str)
            .collect();
        assert_eq!(gdb, ["14", "15"]);
        assert_eq!(tc.addons.len(), 2);
        assert!(!tc.coexist);
    }

    #[test]
    fn old_list_form_addons_are_rejected() {
        let yaml = "id: gcc\nversion: \"13\"\nos: ubuntu-22\naddons:\n  - gdb:14\n";
        assert!(serde_yaml::from_str::<ToolchainRecipe>(yaml).is_err());
    }
}
