use std::collections::HashMap;

use crate::recipe::assignment::{AssignmentYaml, NetworkSpec};
use crate::recipe::os::PkgArg;
use crate::recipe::primitives::{ArgHook, Command, OsId, ParamVal, PkgName, Step};
use crate::recipe::render::{BuildPlan, ImageSpec, Layer, LayerSource, ResolvedStep, Runtime};
use crate::recipe::store::RecipeStore;

mod params;
use params::{interpolate, resolve_params};

const STUDENT_USER: &str = "student";

/// Resolve a reusable OS base image (no assignment context).
pub fn resolve_base(os_id: &OsId, store: &RecipeStore) -> Result<ImageSpec, String> {
    let os = store
        .os(os_id)
        .ok_or_else(|| format!("os {:?} not found", os_id.as_str()))?;

    let empty = HashMap::new();
    let os_layer = Layer {
        source: LayerSource::Os(os_id.clone()),
        steps: resolve_steps(&os.hooks.build.init, &os.hooks.pkg.install, &empty)?,
    };

    Ok(ImageSpec::Base {
        upstream: os.upstream.clone(),
        image: os.image.clone(),
        os: os_layer,
    })
}

/// Resolve a full assignment image: every build phase plus runtime.
pub fn resolve(assignment: &AssignmentYaml, store: &RecipeStore) -> Result<ImageSpec, String> {
    let env = &assignment.environment;
    let os_id = &env.os;
    let os = store
        .os(os_id)
        .ok_or_else(|| format!("os {:?} not found", os_id.as_str()))?;
    let pkg_install = &os.hooks.pkg.install;
    let empty = HashMap::new();

    let os_layer = Layer {
        source: LayerSource::Os(os_id.clone()),
        steps: resolve_steps(&os.hooks.build.init, pkg_install, &empty)?,
    };

    let platform_kind = env.platform.kind.clone();
    let platform = store.platform(platform_kind.clone(), os_id).ok_or_else(|| {
        format!(
            "no {:?} platform recipe found for os {:?}",
            platform_kind.as_str(),
            os_id.as_str()
        )
    })?;
    let params = resolve_params(&platform.hooks.build.params, &env.platform.params)?;
    let platform_layer = Layer {
        source: LayerSource::Platform(platform_kind.clone()),
        steps: resolve_steps(&platform.hooks.build.steps, pkg_install, &params)?,
    };

    let mut toolchain_layers = Vec::new();
    for tc_ref in &env.toolchains {
        let tc = store
            .toolchain(&tc_ref.id, &tc_ref.version, os_id)
            .ok_or_else(|| {
                format!(
                    "toolchain {:?} version {:?} not found for os {:?}",
                    tc_ref.id.as_str(),
                    tc_ref.version.as_str(),
                    os_id.as_str()
                )
            })?;
        toolchain_layers.push(Layer {
            source: LayerSource::Toolchain(tc_ref.clone()),
            steps: resolve_steps(&tc.steps, pkg_install, &tc.params)?,
        });
    }

    let assignment_layer = Layer {
        source: LayerSource::Assignment(assignment.id.clone()),
        steps: resolve_steps(&env.steps, pkg_install, &empty)?,
    };

    let os_configure = match &env.network {
        NetworkSpec::Allow => None,
        NetworkSpec::Deny { allow } => {
            let mut args = HashMap::new();
            args.insert("cidrs".to_owned(), string_list(allow));
            let commands = expand_arg_hook(&os.hooks.network.transcribe, &args)?;
            Some(Layer {
                source: LayerSource::OsConfigure(os_id.clone()),
                steps: vec![ResolvedStep {
                    name: Some("restrict network egress".to_owned()),
                    commands,
                }],
            })
        }
    };

    let os_cleanup = Layer {
        source: LayerSource::OsCleanup(os_id.clone()),
        steps: resolve_steps(&os.hooks.build.cleanup, pkg_install, &empty)?,
    };

    let entrypoint = interpolate(&platform.hooks.entrypoint, &params)
        .map_err(|e| format!("entrypoint interpolation failed: {e}"))?;
    let pre_root = match &env.network {
        NetworkSpec::Allow => Vec::new(),
        NetworkSpec::Deny { .. } => expand_arg_hook(&os.hooks.network.enable, &empty)?,
    };
    let runtime = Runtime {
        user: STUDENT_USER.to_owned(),
        entrypoint: Command(entrypoint),
        pre_root,
    };

    Ok(ImageSpec::Full {
        upstream: os.upstream.clone(),
        base_image: os.image.clone(),
        plan: BuildPlan {
            os: os_layer,
            platform: platform_layer,
            toolchain: toolchain_layers,
            assignment: assignment_layer,
            os_configure,
            os_cleanup,
        },
        runtime,
        ports: platform.hooks.ports.clone(),
    })
}

fn resolve_steps(
    steps: &[Step],
    pkg_install: &ArgHook<PkgArg>,
    params: &HashMap<String, ParamVal>,
) -> Result<Vec<ResolvedStep>, String> {
    steps
        .iter()
        .map(|s| resolve_step(s, pkg_install, params))
        .collect()
}

fn resolve_step(
    step: &Step,
    pkg_install: &ArgHook<PkgArg>,
    params: &HashMap<String, ParamVal>,
) -> Result<ResolvedStep, String> {
    match step {
        Step::Run { name, run } => {
            let commands = run
                .iter()
                .map(|s| {
                    interpolate(s, params)
                        .map(Command)
                        .map_err(|e| format!("interpolation error in step {name:?}: {e}"))
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(ResolvedStep {
                name: name.clone(),
                commands,
            })
        }
        Step::Install { name, packages } => {
            let mut args = HashMap::new();
            args.insert("packages".to_owned(), pkg_names(packages));
            let commands = expand_arg_hook(pkg_install, &args)?;
            Ok(ResolvedStep {
                name: name.clone(),
                commands,
            })
        }
    }
}

/// Expand an OS arg-hook's steps by interpolating each command with `args`
/// (keyed by the hook's declared arg names, e.g. `packages`, `cidrs`).
fn expand_arg_hook<A: Ord>(
    hook: &ArgHook<A>,
    args: &HashMap<String, ParamVal>,
) -> Result<Vec<Command>, String> {
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

fn pkg_names(packages: &[PkgName]) -> ParamVal {
    ParamVal::List(
        packages
            .iter()
            .map(|p| ParamVal::Str(p.as_str().to_owned()))
            .collect(),
    )
}

fn string_list(items: &[String]) -> ParamVal {
    ParamVal::List(items.iter().map(|s| ParamVal::Str(s.clone())).collect())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn arg_hook(yaml: &str) -> ArgHook<PkgArg> {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn expand_arg_hook_substitutes_packages() {
        let hook = arg_hook("args: [packages]\nsteps:\n  - run: ['apt-get install -y {packages}']\n");
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

    // ---- Store-backed tests (real recipes submodule) ------------------------

    fn recipes_dir() -> Option<PathBuf> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../recipes");
        dir.join("os").exists().then_some(dir)
    }

    fn load_store() -> Option<RecipeStore> {
        let dir = recipes_dir()?;
        RecipeStore::load(&[dir]).ok()
    }

    fn assignment(network_block: &str) -> AssignmentYaml {
        let yaml = format!(
            "id: test-assignment\nname: Test\nenvironment:\n  os: ubuntu-22\n  platform:\n    kind: interactive\n{network_block}"
        );
        serde_yaml::from_str(&yaml).unwrap()
    }

    #[test]
    fn resolve_allow_has_no_os_configure_and_empty_pre_root() {
        let Some(store) = load_store() else { return };
        let spec = resolve(&assignment(""), &store).unwrap();
        match spec {
            ImageSpec::Full {
                plan, runtime, ..
            } => {
                assert!(plan.os_configure.is_none());
                assert!(runtime.pre_root.is_empty());
            }
            ImageSpec::Base { .. } => panic!("expected Full"),
        }
    }

    #[test]
    fn resolve_deny_builds_os_configure_and_pre_root() {
        let Some(store) = load_store() else { return };
        let spec = resolve(
            &assignment("  network:\n    mode: deny\n    allow: [\"10.0.0.0/8\"]\n"),
            &store,
        )
        .unwrap();
        match spec {
            ImageSpec::Full { plan, runtime, .. } => {
                let configure = plan.os_configure.expect("os_configure present under deny");
                let joined: String = configure.steps[0]
                    .commands
                    .iter()
                    .map(|c| c.0.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(joined.contains("10.0.0.0/8"));
                assert!(!runtime.pre_root.is_empty());
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
