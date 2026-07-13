use serde::Deserialize;

use super::id::PkgName;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Step {
    Install {
        #[serde(default)]
        name: Option<String>,
        packages: Vec<PkgName>,
    },
    Run {
        #[serde(default)]
        name: Option<String>,
        run: Vec<String>,
    },
}
