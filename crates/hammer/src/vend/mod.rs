use std::path::{Path, PathBuf};
use std::process::Command;

use miette::{Result, miette};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use pedagog_core::recipe::os::OsDef;
use pedagog_core::recipe::platform::PlatformRecipe;
use pedagog_core::recipe::primitives::{Ingredient, IngredientSource};
use pedagog_core::recipe::toolchain::ToolchainRecipe;

use crate::cli::VendArgs;

mod github;

struct RecipeTarget {
    /// Relative path used in output, e.g. "platform/workspace"
    label: String,
    ingredients: Vec<Ingredient>,
    /// Absolute path to the recipe's source directory
    recipe_dir: PathBuf,
}

pub fn run_vend(args: VendArgs, dirs: Vec<PathBuf>) -> Result<()> {
    let targets = collect_targets(&args, &dirs)?;
    if args.prune {
        for removed in prune(&dirs, &targets)? {
            println!("Pruned {}", removed.display());
        }
    }

    if targets.is_empty() {
        println!("Nothing to vend.");
        return Ok(());
    }

    let mut any_error = false;
    for target in targets {
        if target.ingredients.is_empty() {
            println!("Vending {} ... nothing to vend", target.label);
            continue;
        }
        for ingredient in &target.ingredients {
            let out_dir = target.recipe_dir.join("ingredients").join(&target.label);
            std::fs::create_dir_all(&out_dir)
                .map_err(|e| miette!("failed to create {}: {}", out_dir.display(), e))?;
            let out_path = out_dir.join(&ingredient.output);
            print!("Vending {} ... {} ", target.label, ingredient.output);
            if is_cached(ingredient, &out_path) {
                println!("✓ (cached)");
                continue;
            }
            let fetched = fetch(ingredient, &out_path).and_then(|()| match &ingredient.sha256 {
                Some(expected) => verify_sha256(&out_path, expected),
                None => Ok(()),
            });
            match fetched {
                Ok(()) => println!("✓"),
                Err(e) => {
                    println!("✗");
                    eprintln!("  error: {e}");
                    any_error = true;
                }
            }
        }
    }

    if any_error {
        Err(miette!("one or more ingredients failed to vend"))
    } else {
        Ok(())
    }
}

/// Delete files under each recipe directory's `ingredients/` that no target
/// declares (keeping `.keep`), then the directories left empty. Returns what
/// was removed, relative to its recipe directory.
fn prune(dirs: &[PathBuf], targets: &[RecipeTarget]) -> Result<Vec<PathBuf>> {
    let declared: std::collections::HashSet<PathBuf> = targets
        .iter()
        .flat_map(|t| {
            t.ingredients.iter().map(|i| {
                t.recipe_dir
                    .join("ingredients")
                    .join(&t.label)
                    .join(&i.output)
            })
        })
        .collect();
    let mut removed = Vec::new();
    for dir in dirs {
        let root = dir.join("ingredients");
        if !root.is_dir() {
            continue;
        }
        // Deepest first, so a directory is visited after its contents.
        for entry in WalkDir::new(&root).min_depth(1).contents_first(true) {
            let entry = entry.map_err(|e| miette!("cannot read {}: {e}", root.display()))?;
            let path = entry.path();
            let relative = path.strip_prefix(dir).unwrap_or(path).to_owned();
            if entry.file_type().is_dir() {
                if std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_none()) {
                    std::fs::remove_dir(path)
                        .map_err(|e| miette!("cannot remove {}: {e}", path.display()))?;
                }
            } else if entry.file_name() != ".keep" && !declared.contains(path) {
                std::fs::remove_file(path)
                    .map_err(|e| miette!("cannot remove {}: {e}", path.display()))?;
                removed.push(relative);
            }
        }
    }
    Ok(removed)
}

fn fetch(ingredient: &Ingredient, out_path: &Path) -> Result<(), String> {
    match &ingredient.source {
        IngredientSource::Github(gh) => github::download(gh, out_path)?,
        IngredientSource::Url(url) => {
            let status = Command::new("curl")
                .args(["-fsSL", url.as_str(), "-o", &out_path.to_string_lossy()])
                .status()
                .map_err(|e| format!("failed to run curl: {e}"))?;
            if !status.success() {
                return Err(format!("curl failed for {url}"));
            }
        }
    }
    Ok(())
}

/// A previously vended file is reused only when the recipe pins its checksum
/// and the file still matches (otherwise it's deleted and fetched again).
fn is_cached(ingredient: &Ingredient, path: &Path) -> bool {
    match &ingredient.sha256 {
        Some(expected) if path.exists() => verify_sha256(path, expected).is_ok(),
        _ => false,
    }
}

/// Refuses (and deletes) a download whose SHA-256 isn't the recipe's.
fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "checksum mismatch: expected sha256 {expected}, got {actual}"
        ));
    }
    Ok(())
}

fn collect_targets(args: &VendArgs, dirs: &[PathBuf]) -> Result<Vec<RecipeTarget>> {
    let mut targets = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for dir in dirs {
        collect_dir_targets(dir, args, &mut targets, &mut warnings)?;
    }

    for w in &warnings {
        eprintln!("warning: {w}");
    }

    Ok(targets)
}

fn collect_dir_targets(
    dir: &Path,
    args: &VendArgs,
    targets: &mut Vec<RecipeTarget>,
    warnings: &mut Vec<String>,
) -> Result<()> {
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
            let path = entry.path();
            let src = std::fs::read_to_string(path)
                .map_err(|e| miette!("cannot read {}: {}", path.display(), e))?;

            match subdir {
                "os" => {
                    let def = match serde_yaml::from_str::<OsDef>(&src) {
                        Ok(d) => d,
                        Err(e) => {
                            warnings.push(format!("{}: {e}", path.display()));
                            continue;
                        }
                    };
                    if !matches_os_filter(args, def.id.as_str()) {
                        continue;
                    }
                    targets.push(RecipeTarget {
                        label: format!("os/{}", def.id),
                        ingredients: def.ingredients,
                        recipe_dir: dir.to_owned(),
                    });
                }
                "platforms" => {
                    let recipe = match serde_yaml::from_str::<PlatformRecipe>(&src) {
                        Ok(r) => r,
                        Err(e) => {
                            warnings.push(format!("{}: {e}", path.display()));
                            continue;
                        }
                    };
                    if !matches_platform_filter(args, &recipe.platform.to_string()) {
                        continue;
                    }
                    targets.push(RecipeTarget {
                        label: format!("platform/{}", recipe.platform),
                        ingredients: recipe.ingredients,
                        recipe_dir: dir.to_owned(),
                    });
                }
                "toolchains" => {
                    let tc = match serde_yaml::from_str::<ToolchainRecipe>(&src) {
                        Ok(t) => t,
                        Err(e) => {
                            warnings.push(format!("{}: {e}", path.display()));
                            continue;
                        }
                    };
                    let label = format!("toolchain/{}/{}", tc.id, tc.version);
                    if !matches_toolchain_filter(args, &label) {
                        continue;
                    }
                    targets.push(RecipeTarget {
                        label,
                        ingredients: tc.ingredients,
                        recipe_dir: dir.to_owned(),
                    });
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn matches_os_filter(args: &VendArgs, id: &str) -> bool {
    if no_filter(args) {
        return true;
    }
    args.os.as_deref() == Some(id)
}

fn matches_platform_filter(args: &VendArgs, kind: &str) -> bool {
    if no_filter(args) {
        return true;
    }
    args.platform.as_deref() == Some(kind)
}

fn matches_toolchain_filter(args: &VendArgs, label: &str) -> bool {
    if no_filter(args) {
        return true;
    }
    // label is "toolchain/<id>/<version>"; --toolchain may be "gcc" or "gcc:12"
    if let Some(filter) = &args.toolchain {
        let rest = label.strip_prefix("toolchain/").unwrap_or(label);
        let normalized = rest.replace('/', ":");
        return normalized == *filter || rest.starts_with(&format!("{}/", filter));
    }
    false
}

fn no_filter(args: &VendArgs) -> bool {
    args.assignment.is_none()
        && args.platform.is_none()
        && args.os.is_none()
        && args.toolchain.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_file(name: &str, content: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hammer-vend-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn prune_removes_only_what_no_recipe_declares() {
        let dir = std::env::temp_dir().join(format!("hammer-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ing = dir.join("ingredients");
        for (path, content) in [
            ("platform/workspace/code-server.deb", "new"),
            ("platform/interactive/code-server.deb", "stale"),
            ("toolchain/python/3.12/notes.txt", "stale"),
            (".keep", ""),
        ] {
            std::fs::create_dir_all(ing.join(path).parent().unwrap()).unwrap();
            std::fs::write(ing.join(path), content).unwrap();
        }
        let ingredient: Ingredient =
            serde_yaml::from_str("output: code-server.deb\nurl: https://example.com/cs.deb\n")
                .unwrap();
        let targets = [RecipeTarget {
            label: "platform/workspace".into(),
            ingredients: vec![ingredient],
            recipe_dir: dir.clone(),
        }];
        let mut removed = prune(std::slice::from_ref(&dir), &targets).unwrap();
        removed.sort();
        assert_eq!(
            removed,
            [
                PathBuf::from("ingredients/platform/interactive/code-server.deb"),
                PathBuf::from("ingredients/toolchain/python/3.12/notes.txt"),
            ]
        );
        assert!(ing.join("platform/workspace/code-server.deb").exists());
        assert!(ing.join(".keep").exists());
        assert!(
            !ing.join("platform/interactive").exists(),
            "empty dirs go too"
        );
        assert!(!ing.join("toolchain").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // sha256("hello")
    const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn matching_checksum_keeps_the_file() {
        let path = scratch_file("ok", b"hello");
        verify_sha256(&path, HELLO).unwrap();
        assert!(path.exists());
    }

    fn ingredient(sha256: Option<&str>) -> Ingredient {
        let yaml = match sha256 {
            Some(s) => format!("output: f\nurl: 'https://example.com/f'\nsha256: {s}\n"),
            None => "output: f\nurl: 'https://example.com/f'\n".to_owned(),
        };
        serde_yaml::from_str(&yaml).unwrap()
    }

    #[test]
    fn only_pinned_matching_files_count_as_cached() {
        let path = scratch_file("cache", b"hello");
        assert!(is_cached(&ingredient(Some(HELLO)), &path));
        assert!(
            !is_cached(&ingredient(None), &path),
            "unpinned: always refetch"
        );
        let other = "0".repeat(64);
        assert!(
            !is_cached(&ingredient(Some(&other)), &path),
            "stale: refetch"
        );
        assert!(!path.exists(), "a stale file is removed");
        assert!(
            !is_cached(&ingredient(Some(HELLO)), &path),
            "missing: fetch"
        );
    }

    #[test]
    fn mismatched_checksum_deletes_the_file() {
        let path = scratch_file("bad", b"tampered");
        let err = verify_sha256(&path, HELLO).unwrap_err();
        assert!(
            err.contains("checksum mismatch") && err.contains(HELLO),
            "{err}"
        );
        assert!(!path.exists());
    }
}
