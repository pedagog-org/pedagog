use std::path::{Path, PathBuf};
use std::process::Command;

use miette::{Result, miette};
use walkdir::WalkDir;

use pedagog_core::recipe::os::OsDef;
use pedagog_core::recipe::platform::PlatformRecipe;
use pedagog_core::recipe::primitives::{Ingredient, IngredientSource};
use pedagog_core::recipe::toolchain::ToolchainRecipe;

use crate::cli::VendArgs;

struct RecipeTarget {
    /// Relative path used in output, e.g. "platform/interactive"
    label: String,
    ingredients: Vec<Ingredient>,
    /// Absolute path to the recipe's source directory
    recipe_dir: PathBuf,
}

pub fn run_vend(args: VendArgs, dirs: Vec<PathBuf>) -> Result<()> {
    let targets = collect_targets(&args, &dirs)?;

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
            let out_dir = target.recipe_dir
                .join("ingredients")
                .join(&target.label);
            std::fs::create_dir_all(&out_dir)
                .map_err(|e| miette!("failed to create {}: {}", out_dir.display(), e))?;
            let out_path = out_dir.join(&ingredient.output);
            print!("Vending {} ... {} ", target.label, ingredient.output);
            match fetch(ingredient, &out_path) {
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

fn fetch(ingredient: &Ingredient, out_path: &Path) -> Result<(), String> {
    match &ingredient.source {
        IngredientSource::Github(gh) => {
            let status = Command::new("gh")
                .args([
                    "release", "download", &gh.tag,
                    "--repo", &gh.repo,
                    "--pattern", &gh.asset,
                    "--output", &out_path.to_string_lossy(),
                    "--clobber",
                ])
                .status()
                .map_err(|e| format!("failed to run gh: {e}"))?;
            if !status.success() {
                return Err(format!(
                    "gh release download failed for {}/{} tag {}",
                    gh.repo, gh.asset, gh.tag
                ));
            }
        }
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
                    if !matches_os_filter(args, def.id.as_str()) { continue; }
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
                    if !matches_platform_filter(args, &recipe.platform.to_string()) { continue; }
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
                    if !matches_toolchain_filter(args, &label) { continue; }
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
    if no_filter(args) { return true; }
    args.os.as_deref() == Some(id)
}

fn matches_platform_filter(args: &VendArgs, kind: &str) -> bool {
    if no_filter(args) { return true; }
    args.platform.as_deref() == Some(kind)
}

fn matches_toolchain_filter(args: &VendArgs, label: &str) -> bool {
    if no_filter(args) { return true; }
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
