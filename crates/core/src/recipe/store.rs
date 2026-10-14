use std::collections::HashMap;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::recipe::os::OsDef;
use crate::recipe::platform::{PlatformKind, PlatformRecipe};
use crate::recipe::primitives::{ArgHook, OsId, Step, ToolchainId, Version};
use crate::recipe::resolve::params::token_roots;
use crate::recipe::toolchain::ToolchainRecipe;

pub struct RecipeStore {
    os: HashMap<OsId, OsDef>,
    platforms: HashMap<PlatformKind, Vec<PlatformRecipe>>,
    toolchains: HashMap<(ToolchainId, Version), ToolchainRecipe>,
    toolchain_paths: HashMap<(ToolchainId, Version), PathBuf>,
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
            toolchain_paths: HashMap::new(),
        };
        let mut errors: Vec<LoadError> = Vec::new();

        for dir in dirs {
            Self::collect_dir(dir, &mut store, &mut errors);
        }
        store.check_addon_declarations(&mut errors);

        if errors.is_empty() {
            Ok(store)
        } else {
            Err(errors)
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
                .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
            {
                let path = entry.path().to_owned();
                let src = match std::fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(e) => {
                        errors.push(LoadError {
                            path,
                            message: e.to_string(),
                        });
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

    /// Every declared addon version must be a real toolchain recipe that shares
    /// at least one OS with the toolchain declaring it.
    fn check_addon_declarations(&self, errors: &mut Vec<LoadError>) {
        for (key, tc) in &self.toolchains {
            let mut problems = Vec::new();
            for (addon_id, versions) in &tc.addons {
                if addon_id == &tc.id {
                    problems.push(format!("{} lists itself as an addon", tc.id));
                    continue;
                }
                for version in versions {
                    match self.toolchains.get(&(addon_id.clone(), version.clone())) {
                        None => problems.push(format!(
                            "declares addon {addon_id}:{version}, but no such toolchain recipe exists"
                        )),
                        Some(addon) if !addon.os.iter().any(|os| tc.os.contains(os)) => {
                            problems.push(format!(
                                "declares addon {addon_id}:{version}, which supports none of this toolchain's os ({})",
                                tc.os.iter().map(|o| o.as_str()).collect::<Vec<_>>().join(", ")
                            ));
                        }
                        Some(_) => {}
                    }
                }
            }
            let path = self.toolchain_paths.get(key).cloned().unwrap_or_default();
            errors.extend(problems.into_iter().map(|message| LoadError {
                path: path.clone(),
                message,
            }));
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

    pub fn toolchain(
        &self,
        id: &ToolchainId,
        version: &Version,
        os: &OsId,
    ) -> Option<&ToolchainRecipe> {
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
    let Some(loc) = err.location() else {
        return msg;
    };
    let line = loc.line();
    let col = loc.column();

    let lines: Vec<&str> = src.lines().collect();
    let mut out = msg.clone();
    out.push('\n');

    let start = line.saturating_sub(2);
    let end = (line + 1).min(lines.len());
    for (i, text) in lines.iter().enumerate().take(end).skip(start) {
        let lineno = i + 1;
        out.push_str(&format!("  {:>4} | {}\n", lineno, text));
        if lineno == line {
            out.push_str(&format!(
                "       | {}^\n",
                " ".repeat(col.saturating_sub(1))
            ));
        }
    }
    out
}

trait Loadable: serde::de::DeserializeOwned {
    fn register(self, store: &mut RecipeStore, path: &Path, errors: &mut Vec<LoadError>);
}

impl Loadable for OsDef {
    fn register(self, store: &mut RecipeStore, path: &Path, errors: &mut Vec<LoadError>) {
        let pkg = &self.hooks.pkg;
        let mut problems = check_arg_hook("pkg.install", &pkg.install);
        if let Some(remove) = &pkg.remove {
            problems.extend(check_arg_hook("pkg.remove", remove));
        }
        errors.extend(problems.into_iter().map(|message| LoadError {
            path: path.to_owned(),
            message,
        }));
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
        store
            .platforms
            .entry(self.platform.clone())
            .or_default()
            .push(self);
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
                    self.id.as_str(),
                    self.version.as_str()
                ),
            });
            return;
        }
        store.toolchain_paths.insert(key.clone(), path.to_owned());
        store.toolchains.insert(key, self);
    }
}

/// An arg hook may only run commands, and may only reference its declared args.
fn check_arg_hook<A: Ord>(hook_name: &str, hook: &ArgHook<A>) -> Vec<String>
where
    for<'x> &'x A: Into<&'static str>,
{
    let declared: Vec<&'static str> = hook.args.iter().map(Into::into).collect();
    let mut problems = Vec::new();
    for step in &hook.steps {
        let Step::Run { run, .. } = step else {
            problems.push(format!(
                "{hook_name}: arg hook steps may not install packages"
            ));
            continue;
        };
        for cmd in run {
            match token_roots(cmd) {
                Ok(roots) => {
                    for root in roots.iter().filter(|r| !declared.contains(&r.as_str())) {
                        problems.push(format!(
                            "{hook_name}: {{{root}}} is not a declared arg (declared: {declared:?})"
                        ));
                    }
                }
                Err(e) => problems.push(format!("{hook_name}: {e}")),
            }
        }
    }
    problems
}

fn load_yaml<T: Loadable>(
    src: &str,
    path: PathBuf,
    store: &mut RecipeStore,
    errors: &mut Vec<LoadError>,
) {
    match serde_yaml::from_str::<T>(src) {
        Ok(r) => r.register(store, &path, errors),
        Err(e) => errors.push(LoadError {
            path,
            message: yaml_error_message(src, &e),
        }),
    }
}

/// Writes recipe files into a fresh temp directory and loads them.
#[cfg(test)]
pub(crate) fn fixture_store(files: &[(&str, &str)]) -> Result<RecipeStore, Vec<LoadError>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pedagog-fixture-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    for (rel, src) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, src).unwrap();
    }
    let store = RecipeStore::load(std::slice::from_ref(&dir));
    let _ = std::fs::remove_dir_all(&dir);
    store
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::os::PkgArg;

    const OS: &str = "id: os-a\nupstream: base:1\nimage: pedagog/a:1\nhooks:\n  build:\n    init: []\n  pkg:\n    install:\n      args: [packages]\n      steps: []\n";

    fn toolchain(id: &str, version: &str, os: &str, addons: &str) -> String {
        format!("id: {id}\nversion: \"{version}\"\nos: {os}\n{addons}")
    }

    fn load_errors(files: &[(&str, &str)]) -> Vec<String> {
        match fixture_store(files) {
            Ok(_) => vec![],
            Err(errs) => errs.into_iter().map(|e| e.message).collect(),
        }
    }

    #[test]
    fn declared_addon_that_exists_on_shared_os_loads() {
        let gcc = toolchain("gcc", "13", "os-a", "addons:\n  gdb: [\"14\"]\n");
        let gdb = toolchain("gdb", "14", "[os-a, os-b]", "");
        let errs = load_errors(&[
            ("os/a.yaml", OS),
            ("toolchains/gcc.yaml", &gcc),
            ("toolchains/gdb.yaml", &gdb),
        ]);
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn declared_addon_without_recipe_is_a_load_error() {
        let gcc = toolchain("gcc", "13", "os-a", "addons:\n  gdb: [\"14\", \"15\"]\n");
        let gdb = toolchain("gdb", "14", "os-a", "");
        let errs = load_errors(&[
            ("os/a.yaml", OS),
            ("toolchains/gcc.yaml", &gcc),
            ("toolchains/gdb.yaml", &gdb),
        ]);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("gdb:15"), "{errs:?}");
    }

    #[test]
    fn declared_addon_on_other_os_only_is_a_load_error() {
        let gcc = toolchain("gcc", "13", "os-a", "addons:\n  gdb: [\"14\"]\n");
        let gdb = toolchain("gdb", "14", "os-b", "");
        let errs = load_errors(&[
            ("os/a.yaml", OS),
            ("toolchains/gcc.yaml", &gcc),
            ("toolchains/gdb.yaml", &gdb),
        ]);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("supports none"), "{errs:?}");
    }

    #[test]
    fn self_addon_is_a_load_error() {
        let gcc = toolchain("gcc", "13", "os-a", "addons:\n  gcc: [\"13\"]\n");
        let errs = load_errors(&[("os/a.yaml", OS), ("toolchains/gcc.yaml", &gcc)]);
        assert!(errs.iter().any(|e| e.contains("itself")), "{errs:?}");
    }

    fn hook(yaml: &str) -> ArgHook<PkgArg> {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn arg_hook_with_declared_tokens_passes() {
        let h = hook(
            "args: [packages]\nsteps:\n  - run: ['apt-get install -y {packages} ${HOME} {{x}}']\n",
        );
        assert!(check_arg_hook("pkg.install", &h).is_empty());
    }

    #[test]
    fn arg_hook_undeclared_token_is_reported() {
        let h = hook("args: [packages]\nsteps:\n  - run: ['apt-get install -y {pacakges}']\n");
        let problems = check_arg_hook("pkg.install", &h);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("{pacakges}"), "{problems:?}");
    }

    #[test]
    fn arg_hook_token_without_any_declared_args_is_reported() {
        let h = hook("steps:\n  - run: ['apt-get install -y {packages}']\n");
        assert_eq!(check_arg_hook("pkg.install", &h).len(), 1);
    }

    #[test]
    fn arg_hook_install_step_is_reported() {
        let h = hook("args: [packages]\nsteps:\n  - packages: [gcc]\n");
        let problems = check_arg_hook("pkg.install", &h);
        assert!(problems[0].contains("may not install"), "{problems:?}");
    }
}
