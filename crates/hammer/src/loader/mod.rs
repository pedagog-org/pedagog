use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pedagog_core::recipe::os::OsDef;
use pedagog_core::recipe::platform::{PlatformKind, PlatformRecipe};
use pedagog_core::recipe::primitives::{OsId, ToolchainId, Version};
use pedagog_core::recipe::toolchain::ToolchainRecipe;
use walkdir::WalkDir;

pub struct RecipeStore {
    os: HashMap<OsId, OsDef>,
    platforms: HashMap<PlatformKind, Vec<PlatformRecipe>>,
    toolchains: HashMap<(ToolchainId, Version), ToolchainRecipe>,
}

#[derive(Debug)]
pub struct LoadError {
    pub path: PathBuf,
    pub message: String,
}

impl RecipeStore {
    pub fn load(dirs: &[PathBuf]) -> Result<Self, Vec<LoadError>> {
        let mut store = RecipeStore {
            os: HashMap::new(),
            platforms: HashMap::new(),
            toolchains: HashMap::new(),
        };
        let mut errors: Vec<LoadError> = Vec::new();

        for dir in dirs {
            collect_dir(dir, &mut store, &mut errors);
        }

        if errors.is_empty() {
            Ok(store)
        } else {
            Err(errors)
        }
    }

    // Primary lookups

    pub fn os(&self, id: &OsId) -> Option<&OsDef> {
        self.os.get(id)
    }

    pub fn platform(&self, kind: PlatformKind, os: &OsId) -> Option<&PlatformRecipe> {
        self.platforms
            .get(&kind)?
            .iter()
            .find(|r| r.os.contains(os))
    }

    pub fn toolchain(&self, id: &ToolchainId, version: &Version, os: &OsId) -> Option<&ToolchainRecipe> {
        let tc = self.toolchains.get(&(id.clone(), version.clone()))?;
        tc.os.contains(os).then_some(tc)
    }

}

// Introspection methods — unused now, available for future subcommands.
#[allow(dead_code)]
impl RecipeStore {
    pub fn list_oses(&self) -> Vec<&OsId> {
        self.os.keys().collect()
    }

    pub fn list_platforms(&self) -> Vec<&PlatformRecipe> {
        self.platforms.values().flatten().collect()
    }

    pub fn list_toolchains(&self) -> Vec<&ToolchainRecipe> {
        self.toolchains.values().collect()
    }

    pub fn platforms_for_os(&self, os: &OsId) -> Vec<&PlatformRecipe> {
        self.platforms
            .values()
            .flatten()
            .filter(|r| r.os.contains(os))
            .collect()
    }

    pub fn toolchains_for_os(&self, os: &OsId) -> Vec<&ToolchainRecipe> {
        self.toolchains
            .values()
            .filter(|tc| tc.os.contains(os))
            .collect()
    }

    pub fn oses_for_platform(&self, kind: PlatformKind) -> Vec<&OsId> {
        self.platforms
            .get(&kind)
            .map(|recipes| recipes.iter().flat_map(|r| r.os.iter()).collect())
            .unwrap_or_default()
    }

    pub fn oses_for_toolchain(&self, id: &ToolchainId, version: &Version) -> Vec<&OsId> {
        self.toolchains
            .get(&(id.clone(), version.clone()))
            .map(|tc| tc.os.iter().collect())
            .unwrap_or_default()
    }
}

fn yaml_error_message(src: &str, err: &serde_yaml::Error) -> String {
    let msg = err.to_string();
    let Some(loc) = err.location() else { return msg };
    let line = loc.line();
    let col  = loc.column();

    let lines: Vec<&str> = src.lines().collect();
    let mut out = msg.clone();
    out.push('\n');

    let start = line.saturating_sub(2);
    let end = (line + 1).min(lines.len());
    for i in start..end {
        let lineno = i + 1;
        out.push_str(&format!("  {:>4} | {}\n", lineno, lines[i]));
        if lineno == line {
            out.push_str(&format!("       | {}^\n", " ".repeat(col.saturating_sub(1))));
        }
    }
    out
}

trait Loadable: serde::de::DeserializeOwned {
    fn register(self, store: &mut RecipeStore, path: &Path, errors: &mut Vec<LoadError>);
}

impl Loadable for OsDef {
    fn register(self, store: &mut RecipeStore, path: &Path, errors: &mut Vec<LoadError>) {
        if store.os.contains_key(&self.id) {
            errors.push(LoadError {
                path: path.to_owned(),
                message: format!("duplicate OS id {:?}: ignoring this file", self.id.as_str()),
            });
            return;
        }
        store.os.insert(self.id.clone(), self);
    }
}

impl Loadable for PlatformRecipe {
    fn register(self, store: &mut RecipeStore, _path: &Path, _errors: &mut Vec<LoadError>) {
        store.platforms.entry(self.platform.clone()).or_default().push(self);
    }
}

impl Loadable for ToolchainRecipe {
    fn register(self, store: &mut RecipeStore, path: &Path, errors: &mut Vec<LoadError>) {
        let key = (self.id.clone(), self.version.clone());
        if store.toolchains.contains_key(&key) {
            errors.push(LoadError {
                path: path.to_owned(),
                message: format!(
                    "duplicate toolchain {:?} version {:?}: ignoring this file",
                    self.id.as_str(), self.version.as_str()
                ),
            });
            return;
        }
        store.toolchains.insert(key, self);
    }
}

fn load_yaml<T: Loadable>(src: &str, path: PathBuf, store: &mut RecipeStore, errors: &mut Vec<LoadError>) {
    match serde_yaml::from_str::<T>(src) {
        Ok(r) => r.register(store, &path, errors),
        Err(e) => errors.push(LoadError { path, message: yaml_error_message(src, &e) }),
    }
}

fn collect_dir(dir: &Path, store: &mut RecipeStore, errors: &mut Vec<LoadError>) {
    for subdir in ["os", "platforms", "toolchains"] {
        let base = dir.join(subdir);
        if !base.exists() {
            continue;
        }
        for entry in WalkDir::new(&base)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |x| x == "yaml"))
        {
            let path = entry.path().to_owned();
            let src = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    errors.push(LoadError { path, message: e.to_string() });
                    continue;
                }
            };
            match subdir {
                "os" => load_yaml::<OsDef>(&src, path, store, errors),
                "platforms" => load_yaml::<PlatformRecipe>(&src, path, store, errors),
                "toolchains" => load_yaml::<ToolchainRecipe>(&src, path, store, errors),
                _ => {}
            }
        }
    }
}
