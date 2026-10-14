// ResolveError is large, but it's built at most once per invocation on the
// error path; boxing would only add noise.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::num::NonZeroU16;

use crate::recipe::assignment::{AssignmentYaml, Testing, ToolchainPick, ToolchainSelection};
use crate::recipe::os::PkgArg;
use crate::recipe::platform::PlatformSpec;
use crate::recipe::primitives::{
    ArgHook, Command, OsId, ParamVal, PkgName, Step, ToolchainId, ToolchainRef,
};
use crate::recipe::render::{
    AssignmentFiles, BuildPlan, ImageSpec, Layer, LayerSource, ResolvedStep, Runtime,
    ToolchainOrigin,
};
use crate::recipe::store::RecipeStore;
use crate::recipe::toolchain::ToolchainRecipe;

mod error;
pub(crate) mod params;
pub use error::{ConflictEntry, ResolveError};
use params::{interpolate, interpolate_args, resolve_params};

const STUDENT_USER: &str = "student";

type Params = HashMap<String, ParamVal>;

/// Resolve a reusable OS base image (no assignment context).
pub fn resolve_base(os_id: &OsId, store: &RecipeStore) -> Result<ImageSpec, ResolveError> {
    let os = os_artifact(os_id, store)?;
    Ok(ImageSpec::Base {
        upstream: os.upstream.to_owned(),
        image: os.image.to_owned(),
        os: os.init,
    })
}

/// Resolve a full assignment image: every build phase plus runtime.
pub fn resolve(
    assignment: &AssignmentYaml,
    store: &RecipeStore,
) -> Result<ImageSpec, ResolveError> {
    let env = &assignment.environment;
    let os = os_artifact(&env.os, store)?;
    let platform = platform_artifact(&env.platform, &os, store)?;
    let toolchain = toolchain_layers(&env.toolchains, assignment.testing.as_ref(), &os, store)?;
    let assignment_layer = Layer {
        source: LayerSource::Assignment(assignment.id.clone()),
        steps: resolve_steps(&env.steps, os.pkg_install, &Params::new())?,
    };

    Ok(ImageSpec::Full {
        upstream: os.upstream.to_owned(),
        base_image: os.image.to_owned(),
        plan: BuildPlan {
            os: os.init,
            platform: platform.layer,
            toolchain,
            assignment: assignment_layer,
            os_cleanup: os.cleanup,
        },
        runtime: Runtime {
            user: STUDENT_USER.to_owned(),
            entrypoint: platform.entrypoint,
        },
        ports: platform.ports,
        network: env.network.clone(),
        testing: assignment.testing.clone(),
        files: AssignmentFiles::default(),
    })
}

// ---- Per-recipe artifacts ----------------------------------------------------

struct OsArtifact<'a> {
    id: &'a OsId,
    upstream: &'a str,
    image: &'a str,
    pkg_install: &'a ArgHook<PkgArg>,
    init: Layer,
    cleanup: Layer,
}

fn os_artifact<'a>(os_id: &OsId, store: &'a RecipeStore) -> Result<OsArtifact<'a>, ResolveError> {
    let os = store
        .os(os_id)
        .ok_or_else(|| ResolveError::OsNotFound(os_id.clone()))?;
    let pkg_install = &os.hooks.pkg.install;
    let empty = Params::new();
    Ok(OsArtifact {
        id: &os.id,
        upstream: &os.upstream,
        image: &os.image,
        pkg_install,
        init: Layer {
            source: LayerSource::Os(os.id.clone()),
            steps: resolve_steps(&os.hooks.build.init, pkg_install, &empty)?,
        },
        cleanup: Layer {
            source: LayerSource::OsCleanup(os.id.clone()),
            steps: resolve_steps(&os.hooks.build.cleanup, pkg_install, &empty)?,
        },
    })
}

struct PlatformArtifact {
    layer: Layer,
    entrypoint: Vec<String>,
    ports: Vec<u16>,
}

fn platform_artifact(
    spec: &PlatformSpec,
    os: &OsArtifact,
    store: &RecipeStore,
) -> Result<PlatformArtifact, ResolveError> {
    let platform =
        store
            .platform(spec.kind.clone(), os.id)
            .ok_or_else(|| ResolveError::PlatformNotFound {
                kind: spec.kind.clone(),
                os: os.id.clone(),
            })?;
    let params = resolve_params(&platform.hooks.build.params, &spec.params)
        .map_err(ResolveError::InvalidParams)?;
    let entrypoint = interpolate_args(&platform.hooks.entrypoint, &params)
        .map_err(|message| interp_error("platform entrypoint", message))?;
    let ports = interpolate_args(&platform.hooks.ports, &params)
        .map_err(|message| interp_error("platform ports", message))?
        .iter()
        .map(|p| parse_port(p))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PlatformArtifact {
        layer: Layer {
            source: LayerSource::Platform(spec.kind.clone()),
            steps: resolve_steps(&platform.hooks.build.steps, os.pkg_install, &params)?,
        },
        entrypoint,
        ports,
    })
}

fn toolchain_layers(
    selections: &[ToolchainSelection],
    testing: Option<&Testing>,
    os: &OsArtifact,
    store: &RecipeStore,
) -> Result<Vec<Layer>, ResolveError> {
    let picked = pick_toolchains(selections, testing, os.id, store)?;
    check_versions(&picked)?;
    picked
        .into_iter()
        .map(|p| {
            Ok(Layer {
                steps: resolve_steps(&p.recipe.steps, os.pkg_install, &p.recipe.params)?,
                source: LayerSource::Toolchain {
                    tc: p.tc,
                    origins: p.origins,
                },
            })
        })
        .collect()
}

struct Picked<'a> {
    tc: ToolchainRef,
    recipe: &'a ToolchainRecipe,
    origins: Vec<ToolchainOrigin>,
    /// True only if every assignment entry for this toolchain is marked `!`.
    allow_coexist: bool,
}

/// Expands selections into install order (each toolchain followed by its
/// addons), merging exact id+version repeats into the first occurrence.
fn pick_toolchains<'a>(
    selections: &[ToolchainSelection],
    testing: Option<&Testing>,
    os: &OsId,
    store: &'a RecipeStore,
) -> Result<Vec<Picked<'a>>, ResolveError> {
    let helper = testing
        .map(|t| t.helper().map(|tc| (t, tc)))
        .transpose()
        .map_err(|message| interp_error("testing.framework", message))?;
    let mut helper_placed = false;
    let mut picked: Vec<Picked> = Vec::new();
    for sel in selections {
        let parent_ref = sel.parent.tc.clone();
        let parent = find_toolchain(&parent_ref, ToolchainOrigin::TopLevel, os, store)?;
        add_picked(&mut picked, &sel.parent, parent, ToolchainOrigin::TopLevel);
        for pick in &sel.addons {
            let addon = &pick.tc;
            let declared = parent.addons.get(&addon.id).cloned().unwrap_or_default();
            if !declared.contains(&addon.version) {
                return Err(ResolveError::AddonNotDeclared {
                    parent: parent_ref,
                    addon: addon.clone(),
                    declared,
                });
            }
            let origin = ToolchainOrigin::AddonOf(parent_ref.clone());
            let recipe = find_toolchain(addon, origin.clone(), os, store)?;
            add_picked(&mut picked, pick, recipe, origin);
        }
        // The framework's helper rides along with the first toolchain that
        // declares it compatible, right after that toolchain's own addons.
        if let Some((_, helper)) = &helper
            && !helper_placed
            && offers(parent, helper)
        {
            let origin = ToolchainOrigin::AddonOf(parent_ref.clone());
            let recipe = find_toolchain(helper, origin.clone(), os, store)?;
            let pick = ToolchainPick {
                tc: helper.clone(),
                allow_coexist: false,
            };
            add_picked(&mut picked, &pick, recipe, origin);
            helper_placed = true;
        }
    }
    if let Some((testing, helper)) = helper
        && !helper_placed
    {
        let offered_by = picked
            .iter()
            .filter(|p| p.origins.contains(&ToolchainOrigin::TopLevel))
            .filter_map(|p| {
                let versions = p.recipe.addons.get(&helper.id)?;
                Some((p.tc.clone(), versions.clone()))
            })
            .collect();
        return Err(ResolveError::FrameworkUnavailable {
            framework: testing.framework.clone(),
            helper,
            offered_by,
        });
    }
    Ok(picked)
}

fn offers(recipe: &ToolchainRecipe, addon: &ToolchainRef) -> bool {
    recipe
        .addons
        .get(&addon.id)
        .is_some_and(|versions| versions.contains(&addon.version))
}

fn find_toolchain<'a>(
    tc: &ToolchainRef,
    origin: ToolchainOrigin,
    os: &OsId,
    store: &'a RecipeStore,
) -> Result<&'a ToolchainRecipe, ResolveError> {
    store
        .toolchain(&tc.id, &tc.version, os)
        .ok_or_else(|| ResolveError::ToolchainNotFound {
            tc: tc.clone(),
            os: os.clone(),
            origin,
        })
}

fn add_picked<'a>(
    picked: &mut Vec<Picked<'a>>,
    pick: &ToolchainPick,
    recipe: &'a ToolchainRecipe,
    origin: ToolchainOrigin,
) {
    match picked.iter_mut().find(|p| p.tc == pick.tc) {
        Some(p) => {
            p.allow_coexist &= pick.allow_coexist;
            if !p.origins.contains(&origin) {
                p.origins.push(origin);
            }
        }
        None => picked.push(Picked {
            tc: pick.tc.clone(),
            recipe,
            origins: vec![origin],
            allow_coexist: pick.allow_coexist,
        }),
    }
}

/// One toolchain id at several versions is allowed only when each of those
/// versions either sets `coexist` in its recipe or is marked `!` on every
/// assignment entry for it.
fn check_versions(picked: &[Picked]) -> Result<(), ResolveError> {
    let mut checked: Vec<&ToolchainId> = Vec::new();
    for p in picked {
        if checked.contains(&&p.tc.id) {
            continue;
        }
        checked.push(&p.tc.id);
        let group: Vec<&Picked> = picked.iter().filter(|q| q.tc.id == p.tc.id).collect();
        if group.len() < 2 || group.iter().all(|q| q.recipe.coexist || q.allow_coexist) {
            continue;
        }
        let entries = group
            .iter()
            .map(|q| ConflictEntry {
                version: q.tc.version.clone(),
                origins: q.origins.clone(),
                allowed_by_parent: q
                    .origins
                    .iter()
                    .filter_map(|origin| match origin {
                        ToolchainOrigin::AddonOf(parent) => {
                            let recipe = picked.iter().find(|r| &r.tc == parent)?.recipe;
                            let allowed = recipe.addons.get(&p.tc.id).cloned().unwrap_or_default();
                            Some((parent.clone(), allowed))
                        }
                        ToolchainOrigin::TopLevel => None,
                    })
                    .collect(),
                coexist: q.recipe.coexist,
                allow_coexist: q.allow_coexist,
            })
            .collect();
        return Err(ResolveError::VersionConflict {
            id: p.tc.id.clone(),
            entries,
        });
    }
    Ok(())
}

// ---- Steps -------------------------------------------------------------------

fn resolve_steps(
    steps: &[Step],
    pkg_install: &ArgHook<PkgArg>,
    params: &Params,
) -> Result<Vec<ResolvedStep>, ResolveError> {
    steps
        .iter()
        .map(|s| resolve_step(s, pkg_install, params))
        .collect()
}

fn resolve_step(
    step: &Step,
    pkg_install: &ArgHook<PkgArg>,
    params: &Params,
) -> Result<ResolvedStep, ResolveError> {
    match step {
        Step::Run { name, run } => {
            let commands = run
                .iter()
                .map(|s| {
                    interpolate(s, params)
                        .map(Command)
                        .map_err(|message| interp_error(&format!("step {name:?}"), message))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResolvedStep {
                name: name.clone(),
                commands,
            })
        }
        Step::Install { name, packages } => {
            let mut args = Params::new();
            args.insert("packages".to_owned(), pkg_names(packages));
            let commands = expand_arg_hook(pkg_install, &args)
                .map_err(|message| interp_error("pkg.install hook", message))?;
            Ok(ResolvedStep {
                name: name.clone(),
                commands,
            })
        }
    }
}

/// Expand an OS arg-hook's steps by interpolating each command with `args`
/// (keyed by the hook's declared arg names, e.g. `packages`).
fn expand_arg_hook<A: Ord>(hook: &ArgHook<A>, args: &Params) -> Result<Vec<Command>, String> {
    let mut out = Vec::new();
    for step in &hook.steps {
        match step {
            Step::Run { run, .. } => {
                for cmd in run {
                    out.push(Command(interpolate(cmd, args)?));
                }
            }
            Step::Install { .. } => {
                return Err("arg hook steps may not contain 'install' entries".to_owned());
            }
        }
    }
    Ok(out)
}

fn interp_error(context: &str, message: String) -> ResolveError {
    ResolveError::Interpolation {
        context: context.to_owned(),
        message,
    }
}

fn parse_port(raw: &str) -> Result<u16, ResolveError> {
    raw.parse::<NonZeroU16>()
        .map(NonZeroU16::get)
        .map_err(|_| ResolveError::InvalidPort {
            raw: raw.to_owned(),
        })
}

fn pkg_names(packages: &[PkgName]) -> ParamVal {
    ParamVal::List(
        packages
            .iter()
            .map(|p| ParamVal::Str(p.as_str().to_owned()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::recipe::network::NetworkSpec;

    fn arg_hook(yaml: &str) -> ArgHook<PkgArg> {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn expand_arg_hook_substitutes_packages() {
        let hook =
            arg_hook("args: [packages]\nsteps:\n  - run: ['apt-get install -y {packages}']\n");
        let mut args = HashMap::new();
        args.insert(
            "packages".to_owned(),
            ParamVal::List(vec![
                ParamVal::Str("gcc".to_owned()),
                ParamVal::Str("make".to_owned()),
            ]),
        );
        let cmds = expand_arg_hook(&hook, &args).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].0, "apt-get install -y gcc make");
    }

    #[test]
    fn expand_arg_hook_rejects_install_steps() {
        let hook = arg_hook("args: [packages]\nsteps:\n  - packages: [gcc]\n");
        let args = HashMap::new();
        assert!(expand_arg_hook(&hook, &args).is_err());
    }

    #[test]
    fn resolve_step_install_expands_via_pkg_hook() {
        let pkg = arg_hook("args: [packages]\nsteps:\n  - run: ['install {packages}']\n");
        let step: Step = serde_yaml::from_str("packages: [gcc-13, g++]\n").unwrap();
        let resolved = resolve_step(&step, &pkg, &HashMap::new()).unwrap();
        assert_eq!(resolved.commands[0].0, "install gcc-13 g++");
    }

    #[test]
    fn parse_port_accepts_valid_range() {
        assert_eq!(parse_port("8080").unwrap(), 8080);
        assert_eq!(parse_port("65535").unwrap(), 65535);
    }

    #[test]
    fn parse_port_rejects_zero_out_of_range_and_text() {
        for bad in ["0", "65536", "-1", "http", ""] {
            assert!(parse_port(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    // ---- Toolchain/addon resolution (fixture stores) ------------------------

    const FIX_OS: &str = "id: os-a\nupstream: base:1\nimage: pedagog/a:1\nhooks:\n  build:\n    init: []\n  pkg:\n    install:\n      args: [packages]\n      steps: []\n";
    const FIX_OS_B: &str = "id: os-b\nupstream: base:2\nimage: pedagog/b:1\nhooks:\n  build:\n    init: []\n  pkg:\n    install:\n      args: [packages]\n      steps: []\n";
    const FIX_PLATFORM: &str = "platform: interactive\nos: [os-a, os-b]\nhooks:\n  build:\n    steps: []\n  entrypoint: [run]\n";

    fn fix_tc(id: &str, version: &str, os: &str, extra: &str) -> (String, String) {
        (
            format!("toolchains/{id}-{version}.yaml"),
            format!(
                "id: {id}\nversion: \"{version}\"\nos: {os}\nsteps:\n  - run: ['install {id} {version}']\n{extra}"
            ),
        )
    }

    /// gcc:13 offers gdb 14/15 + valgrind 3; clang:17 offers gdb 13/14; tcc:1
    /// offers gdb 13. gdb:14 exists only on os-b-capable recipes as noted.
    fn fix_store(gdb_coexist: bool) -> RecipeStore {
        let coexist = if gdb_coexist { "coexist: true\n" } else { "" };
        let tcs = [
            fix_tc(
                "gcc",
                "13",
                "[os-a, os-b]",
                "addons:\n  gdb: [\"14\", \"15\"]\n  valgrind: [\"3\"]\n",
            ),
            fix_tc("clang", "17", "os-a", "addons:\n  gdb: [\"13\", \"14\"]\n"),
            fix_tc("tcc", "1", "os-a", "addons:\n  gdb: [\"13\"]\n"),
            fix_tc("gdb", "13", "os-a", coexist),
            fix_tc("gdb", "14", "[os-a, os-b]", coexist),
            fix_tc("gdb", "15", "os-a", coexist),
            fix_tc("valgrind", "3", "os-a", ""),
        ];
        let mut files: Vec<(&str, &str)> = vec![
            ("os/a.yaml", FIX_OS),
            ("os/b.yaml", FIX_OS_B),
            ("platforms/interactive.yaml", FIX_PLATFORM),
        ];
        files.extend(tcs.iter().map(|(p, s)| (p.as_str(), s.as_str())));
        crate::recipe::store::fixture_store(&files).unwrap()
    }

    fn resolve_toolchains(
        store: &RecipeStore,
        os: &str,
        toolchains: &str,
    ) -> Result<Vec<Layer>, ResolveError> {
        let yaml = format!(
            "id: t\nname: T\nenvironment:\n  os: {os}\n  platform:\n    kind: interactive\n  toolchains: {toolchains}\n"
        );
        let a: AssignmentYaml = serde_yaml::from_str(&yaml).unwrap();
        match resolve(&a, store)? {
            ImageSpec::Full { plan, .. } => Ok(plan.toolchain),
            ImageSpec::Base { .. } => panic!("expected Full"),
        }
    }

    fn layer_refs(layers: &[Layer]) -> Vec<String> {
        layers
            .iter()
            .map(|l| match &l.source {
                LayerSource::Toolchain { tc, .. } => tc.to_string(),
                _ => panic!("expected toolchain layer"),
            })
            .collect()
    }

    fn origins(layer: &Layer) -> Vec<String> {
        match &layer.source {
            LayerSource::Toolchain { origins, .. } => {
                origins.iter().map(ToString::to_string).collect()
            }
            _ => panic!("expected toolchain layer"),
        }
    }

    #[test]
    fn addons_are_layered_right_after_their_parent() {
        let store = fix_store(false);
        let layers =
            resolve_toolchains(&store, "os-a", "[gcc:13+valgrind:3+gdb:14, clang:17]").unwrap();
        assert_eq!(
            layer_refs(&layers),
            ["gcc:13", "valgrind:3", "gdb:14", "clang:17"]
        );
        assert_eq!(origins(&layers[2]), ["addon of gcc:13"]);
        assert_eq!(layers[2].steps[0].commands[0].0, "install gdb 14");
    }

    #[test]
    fn undeclared_addon_version_is_rejected() {
        let store = fix_store(false);
        let err = resolve_toolchains(&store, "os-a", "[gcc:13+gdb:13]").unwrap_err();
        assert!(
            matches!(err, ResolveError::AddonNotDeclared { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("14, 15"), "{err}");
    }

    #[test]
    fn addon_not_offered_by_parent_is_rejected() {
        let store = fix_store(false);
        let err = resolve_toolchains(&store, "os-a", "[clang:17+valgrind:3]").unwrap_err();
        assert!(
            err.to_string().contains("offers no valgrind addon"),
            "{err}"
        );
    }

    #[test]
    fn addon_missing_for_assignment_os_names_its_parent() {
        let store = fix_store(false);
        let err = resolve_toolchains(&store, "os-b", "[gcc:13+gdb:15]").unwrap_err();
        assert_eq!(
            err.to_string(),
            r#"toolchain gdb:15 (addon of gcc:13) not found for os "os-b""#
        );
    }

    #[test]
    fn exact_duplicates_collapse_into_first_occurrence() {
        let store = fix_store(false);
        let layers =
            resolve_toolchains(&store, "os-a", "[gcc:13+gdb:14, clang:17+gdb:14, gdb:14]").unwrap();
        assert_eq!(layer_refs(&layers), ["gcc:13", "gdb:14", "clang:17"]);
        assert_eq!(
            origins(&layers[1]),
            ["addon of gcc:13", "addon of clang:17", "listed directly"]
        );
    }

    #[test]
    fn differing_versions_without_coexist_conflict() {
        let store = fix_store(false);
        let err =
            resolve_toolchains(&store, "os-a", "[gcc:13+gdb:15, clang:17+gdb:13]").unwrap_err();
        let text = err.to_string();
        assert!(
            matches!(err, ResolveError::VersionConflict { .. }),
            "{text}"
        );
        assert!(
            text.contains(
                "gdb:15  addon of gcc:13 (allows 14, 15)  coexist: recipe=no assignment=no"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "gdb:13  addon of clang:17 (allows 13, 14)  coexist: recipe=no assignment=no"
            ),
            "{text}"
        );
        assert!(
            err.help().unwrap().starts_with("use gdb:14 everywhere"),
            "{err:?}"
        );
    }

    #[test]
    fn conflict_with_no_common_version_says_so() {
        let store = fix_store(false);
        let err = resolve_toolchains(&store, "os-a", "[gcc:13+gdb:15, tcc:1+gdb:13]").unwrap_err();
        assert!(
            err.to_string()
                .contains("versions allowed by every parent: none"),
            "{err}"
        );
    }

    #[test]
    fn marking_every_conflicting_entry_allows_both() {
        let store = fix_store(false);
        let layers =
            resolve_toolchains(&store, "os-a", "[gcc:13+gdb:15!, clang:17+gdb:13!]").unwrap();
        assert_eq!(
            layer_refs(&layers),
            ["gcc:13", "gdb:15", "clang:17", "gdb:13"]
        );
    }

    #[test]
    fn marking_only_some_entries_still_conflicts() {
        let store = fix_store(false);
        let err =
            resolve_toolchains(&store, "os-a", "[gcc:13+gdb:15!, clang:17+gdb:13]").unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains(
                "gdb:15  addon of gcc:13 (allows 14, 15)  coexist: recipe=no assignment=yes"
            ),
            "{text}"
        );
        assert!(
            err.help()
                .unwrap()
                .ends_with("marking every such entry: gdb:13!"),
            "{err:?}"
        );
    }

    #[test]
    fn deduped_entry_needs_mark_on_every_occurrence() {
        let store = fix_store(false);
        let err = resolve_toolchains(
            &store,
            "os-a",
            "[gcc:13+gdb:14!, clang:17+gdb:14, tcc:1+gdb:13!]",
        )
        .unwrap_err();
        assert!(err.to_string().contains("gdb:14  addon of gcc:13 (allows 14, 15); addon of clang:17 (allows 13, 14)  coexist: recipe=no assignment=no"), "{err}");
    }

    #[test]
    fn differing_versions_all_coexisting_install_both() {
        let store = fix_store(true);
        let layers =
            resolve_toolchains(&store, "os-a", "[gcc:13+gdb:15, clang:17+gdb:13]").unwrap();
        assert_eq!(
            layer_refs(&layers),
            ["gcc:13", "gdb:15", "clang:17", "gdb:13"]
        );
    }

    // ---- Testing framework helper placement --------------------------------

    /// python:3.12 offers pedagog-pytest 1 and numpy 1; python:3.11 offers
    /// pedagog-pytest 2. Both pythons coexist.
    fn framework_store() -> RecipeStore {
        let tcs = [
            fix_tc(
                "python",
                "3.12",
                "os-a",
                "coexist: true\naddons:\n  pedagog-pytest: [\"1\"]\n  numpy: [\"1\"]\n",
            ),
            fix_tc(
                "python",
                "3.11",
                "os-a",
                "coexist: true\naddons:\n  pedagog-pytest: [\"2\"]\n",
            ),
            fix_tc("numpy", "1", "os-a", ""),
            fix_tc("pedagog-pytest", "1", "os-a", ""),
            fix_tc("pedagog-pytest", "2", "os-a", ""),
        ];
        let mut files: Vec<(&str, &str)> = vec![
            ("os/a.yaml", FIX_OS),
            ("platforms/interactive.yaml", FIX_PLATFORM),
        ];
        files.extend(tcs.iter().map(|(p, s)| (p.as_str(), s.as_str())));
        crate::recipe::store::fixture_store(&files).unwrap()
    }

    fn resolve_with_testing(toolchains: &str, framework: &str) -> Result<Vec<Layer>, ResolveError> {
        let yaml = format!(
            "id: t\nname: T\nenvironment:\n  os: os-a\n  platform:\n    kind: interactive\n  toolchains: {toolchains}\ntesting:\n  framework: {framework}\n"
        );
        let a: AssignmentYaml = serde_yaml::from_str(&yaml).unwrap();
        match resolve(&a, &framework_store())? {
            ImageSpec::Full { plan, .. } => Ok(plan.toolchain),
            ImageSpec::Base { .. } => panic!("expected Full"),
        }
    }

    #[test]
    fn framework_helper_follows_its_toolchain_and_explicit_addons() {
        let layers = resolve_with_testing("[python:3.12+numpy:1]", "pytest:1").unwrap();
        assert_eq!(
            layer_refs(&layers),
            ["python:3.12", "numpy:1", "pedagog-pytest:1"]
        );
        assert_eq!(origins(&layers[2]), ["addon of python:3.12"]);
    }

    #[test]
    fn framework_helper_goes_with_the_first_toolchain_offering_its_version() {
        let layers = resolve_with_testing("[python:3.11, python:3.12]", "pytest:1").unwrap();
        assert_eq!(
            layer_refs(&layers),
            ["python:3.11", "python:3.12", "pedagog-pytest:1"]
        );
    }

    #[test]
    fn explicitly_listed_helper_is_not_duplicated() {
        let layers = resolve_with_testing("[python:3.12+pedagog-pytest:1]", "pytest:1").unwrap();
        assert_eq!(layer_refs(&layers), ["python:3.12", "pedagog-pytest:1"]);
    }

    #[test]
    fn unavailable_framework_names_what_is_offered() {
        let err = resolve_with_testing("[python:3.12]", "pytest:2").unwrap_err();
        assert!(
            matches!(err, ResolveError::FrameworkUnavailable { .. }),
            "{err}"
        );
        assert_eq!(
            err.to_string(),
            "testing framework pytest:2 needs pedagog-pytest:2, which none of the selected toolchains declares compatible\n  python:3.12 offers pedagog-pytest 1"
        );
    }

    #[test]
    fn framework_without_any_matching_toolchain_errors() {
        let err = resolve_with_testing("[numpy:1]", "pytest:1").unwrap_err();
        assert!(
            matches!(err, ResolveError::FrameworkUnavailable { ref offered_by, .. } if offered_by.is_empty()),
            "{err}"
        );
    }

    // ---- Store-backed tests (real recipes submodule) ------------------------

    const SKIP_CORPUS_ENV: &str = "PEDAGOG_SKIP_RECIPE_CORPUS";

    /// The real recipes corpus. Missing submodule fails the test unless
    /// `PEDAGOG_SKIP_RECIPE_CORPUS` is set, so corpus coverage can't silently vanish.
    fn recipes_dir() -> Option<PathBuf> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../recipes");
        if dir.join("os").exists() {
            return Some(dir);
        }
        if std::env::var_os(SKIP_CORPUS_ENV).is_some() {
            return None;
        }
        panic!(
            "recipes/ submodule is not checked out: run `git submodule update --init recipes`, \
             or set {SKIP_CORPUS_ENV}=1 to skip corpus tests"
        );
    }

    fn load_store() -> Option<RecipeStore> {
        let dir = recipes_dir()?;
        Some(
            RecipeStore::load(&[dir])
                .unwrap_or_else(|errs| panic!("recipes failed to load: {errs:#?}")),
        )
    }

    fn assignment(network_block: &str) -> AssignmentYaml {
        let yaml = format!(
            "id: test-assignment\nname: Test\nenvironment:\n  os: ubuntu-22\n  platform:\n    kind: interactive\n{network_block}"
        );
        serde_yaml::from_str(&yaml).unwrap()
    }

    #[test]
    fn resolve_carries_network_policy_through() {
        let Some(store) = load_store() else { return };
        let spec = resolve(
            &assignment("  network:\n    mode: deny\n    allow:\n      - cidr: 10.0.0.0/8\n"),
            &store,
        )
        .unwrap();
        match spec {
            ImageSpec::Full { network, .. } => {
                assert!(matches!(network, NetworkSpec::Deny { allow } if allow.len() == 1));
            }
            ImageSpec::Base { .. } => panic!("expected Full"),
        }
    }

    #[test]
    fn every_os_resolves_to_base_and_every_example_to_full() {
        let Some(dir) = recipes_dir() else { return };
        let Some(store) = load_store() else { return };

        for entry in std::fs::read_dir(dir.join("os")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "yaml") {
                let os: crate::recipe::os::OsDef =
                    serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                resolve_base(&os.id, &store)
                    .unwrap_or_else(|e| panic!("os {:?} failed to resolve: {e}", os.id.as_str()));
            }
        }

        let examples = dir.join("examples");
        if examples.exists() {
            for entry in std::fs::read_dir(examples).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_some_and(|e| e == "yaml") {
                    let a: AssignmentYaml =
                        serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                    resolve(&a, &store)
                        .unwrap_or_else(|e| panic!("{} failed to resolve: {e}", path.display()));
                }
            }
        }
    }
}
