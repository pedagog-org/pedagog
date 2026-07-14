pub mod plan;

use std::collections::HashMap;

use pedagog_core::recipe::assignment::AssignmentYaml;
use pedagog_core::recipe::os::PkgArg;
use pedagog_core::recipe::platform::PlatformKind;
use pedagog_core::recipe::primitives::{ArgHook, Id, OsId, ParamVal, PkgName, Step};

use crate::loader::RecipeStore;
use crate::params::{interpolate, resolve_params};
use plan::{BasePlan, BuildPlan, Command, Layer, LayerSource, ResolvedStep};

pub fn resolve_base(os_id: &OsId, store: &RecipeStore) -> Result<BasePlan, String> {
    let os = store
        .os(os_id)
        .ok_or_else(|| format!("os {:?} not found", os_id.as_str()))?;

    let empty_params = HashMap::new();
    let layers = vec![Layer {
        source: LayerSource::Os(os_id.clone()),
        steps: resolve_steps(&os.hooks.build.init, &os.hooks.pkg.install, &empty_params)?,
    }];

    Ok(BasePlan {
        os_id: os_id.clone(),
        upstream: os.upstream.clone(),
        image: os.image.clone(),
        layers,
    })
}

pub fn resolve_platform(
    platform_kind: &PlatformKind,
    os_id: &OsId,
    store: &RecipeStore,
) -> Result<BuildPlan, String> {
    let os = store
        .os(os_id)
        .ok_or_else(|| format!("os {:?} not found", os_id.as_str()))?;
    let platform = store
        .platform(platform_kind.clone(), os_id)
        .ok_or_else(|| {
            format!(
                "no \"{platform_kind}\" platform recipe found for os {:?}",
                os_id.as_str()
            )
        })?;

    let empty_params = HashMap::new();
    let params = resolve_params(&platform.hooks.build.params, &empty_params)?;
    let entrypoint = interpolate(&platform.hooks.entrypoint, &params)
        .map_err(|e| format!("entrypoint interpolation failed: {e}"))?;

    let id = Id::try_from(platform_kind.as_str().to_owned())
        .map_err(|e| format!("internal: {e}"))?;

    let mut layers = Vec::new();

    layers.push(Layer {
        source: LayerSource::Platform(platform_kind.clone()),
        steps: resolve_steps(&platform.hooks.build.steps, &os.hooks.pkg.install, &params)?,
    });

    if !os.hooks.build.cleanup.is_empty() {
        layers.push(Layer {
            source: LayerSource::BuildCleanup,
            steps: resolve_steps(&os.hooks.build.cleanup, &os.hooks.pkg.install, &HashMap::new())?,
        });
    }

    Ok(BuildPlan {
        name: format!("{platform_kind} platform"),
        id,
        base_image: os.image.clone(),
        layers,
        entrypoint,
    })
}

pub fn resolve_build(assignment: &AssignmentYaml, store: &RecipeStore) -> Result<BuildPlan, String> {
    let os_id = &assignment.environment.os;
    let os = store
        .os(os_id)
        .ok_or_else(|| format!("os {:?} not found", os_id.as_str()))?;

    let platform_kind = assignment.environment.platform.kind.clone();
    let platform = store
        .platform(platform_kind.clone(), os_id)
        .ok_or_else(|| {
            format!(
                "no \"{platform_kind}\" platform recipe found for os {:?}",
                os_id.as_str()
            )
        })?;

    let params = resolve_params(&platform.hooks.build.params, &assignment.environment.platform.params)?;

    let entrypoint = interpolate(&platform.hooks.entrypoint, &params)
        .map_err(|e| format!("entrypoint interpolation failed: {e}"))?;

    let mut layers = Vec::new();

    layers.push(Layer {
        source: LayerSource::Platform(platform_kind),
        steps: resolve_steps(&platform.hooks.build.steps, &os.hooks.pkg.install, &params)?,
    });

    for tc_ref in &assignment.environment.toolchains {
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

        layers.push(Layer {
            source: LayerSource::Toolchain(tc_ref.clone()),
            steps: resolve_steps(&tc.steps, &os.hooks.pkg.install, &tc.params)?,
        });
    }

    if !os.hooks.build.cleanup.is_empty() {
        layers.push(Layer {
            source: LayerSource::BuildCleanup,
            steps: resolve_steps(&os.hooks.build.cleanup, &os.hooks.pkg.install, &HashMap::new())?,
        });
    }

    Ok(BuildPlan {
        id: assignment.id.clone(),
        name: assignment.name.clone(),
        base_image: os.image.clone(),
        layers,
        entrypoint,
    })
}

fn resolve_steps(
    steps: &[Step],
    pkg_install: &ArgHook<PkgArg>,
    params: &HashMap<String, ParamVal>,
) -> Result<Vec<ResolvedStep>, String> {
    steps.iter().map(|step| resolve_step(step, pkg_install, params)).collect()
}

fn resolve_step(
    step: &Step,
    pkg_install: &ArgHook<PkgArg>,
    params: &HashMap<String, ParamVal>,
) -> Result<ResolvedStep, String> {
    let (name, commands) = match step {
        Step::Install { name, packages } => {
            (name.clone(), expand_pkg_install(packages, pkg_install))
        }
        Step::Run { name, run } => {
            (name.clone(), run.iter().map(|s| Command(s.clone())).collect())
        }
    };

    let commands = commands
        .into_iter()
        .map(|cmd| {
            interpolate(&cmd.0, params)
                .map(Command)
                .map_err(|e| format!("interpolation error in step {:?}: {e}", name))
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(ResolvedStep { name, commands })
}

fn expand_pkg_install(packages: &[PkgName], hook: &ArgHook<PkgArg>) -> Vec<Command> {
    let pkg_str = packages
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    hook.steps
        .iter()
        .flat_map(|step| match step {
            Step::Run { run, .. } => run
                .iter()
                .map(|cmd| Command(cmd.replace("{packages}", &pkg_str)))
                .collect::<Vec<_>>(),
            Step::Install { packages: pkgs, .. } => pkgs
                .iter()
                .map(|p| Command(format!("apt-get install -y {}", p.as_str())))
                .collect(),
        })
        .collect()
}

