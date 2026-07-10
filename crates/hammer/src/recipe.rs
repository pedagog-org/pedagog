use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct OsDef {
    pub id: String,
    pub upstream: String,
    pub image: String,
    pub pkg_manager: PkgManager,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PkgManager {
    Apt,
    Apk,
    Dnf,
}

impl PkgManager {
    pub fn install_cmd(&self) -> &'static str {
        match self {
            PkgManager::Apt => "apt-get install -y",
            PkgManager::Apk => "apk add --no-cache",
            PkgManager::Dnf => "dnf install -y",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ToolchainRecipe {
    pub id: String,
    pub version: String,
    pub os: String,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub addons: Vec<AddonRef>,
}

#[derive(Debug, Deserialize)]
pub struct PlatformRecipe {
    pub id: String,
    pub os: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
pub struct Step {
    pub name: String,
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub run: Vec<String>,
}

/// A reference to an addon toolchain, e.g. "gdb:14" or "gdb"
#[derive(Debug, Clone)]
pub struct AddonRef {
    pub id: String,
    pub version: Option<String>,
}

impl<'de> Deserialize<'de> for AddonRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.split_once(':') {
            Some((id, ver)) => AddonRef { id: id.to_owned(), version: Some(ver.to_owned()) },
            None => AddonRef { id: s, version: None },
        })
    }
}
