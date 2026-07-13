mod cli;
mod loader;
mod params;
mod render;
mod resolve;
mod vend;

use std::io::Write;
use std::path::PathBuf;

use clap::Parser;
use miette::{
    GraphicalReportHandler, MietteDiagnostic, Report, Result, Severity, miette,
};

use cli::{Cli, Command, Format, PlanArgs, VendArgs};
use loader::RecipeStore;
use pedagog_core::recipe::platform::PlatformKind;
use pedagog_core::recipe::primitives::Id;
use render::Renderer;

fn main() -> Result<()> {
    // Use try_parse so clap errors go through miette's renderer instead of
    // clap's own formatter.
    // Let clap handle all parse errors and help/version output — it formats them well on its own.
    let cli = Cli::try_parse().unwrap_or_else(|e| e.exit());

    match cli.command {
        Command::Plan(args) => run_plan(args),
        Command::Vend(args) => run_vend(args),
    }
}

fn run_plan(args: PlanArgs) -> Result<()> {
    let dirs = recipe_dirs(&args.recipes, args.quiet)?;
    let store = RecipeStore::load(&dirs).map_err(|errs| {
        let messages: Vec<String> = errs
            .into_iter()
            .map(|e| {
                // Use only the filename to keep lines short; the full path is rarely needed
                // for a parse error since the message identifies the problem.
                let name = e.path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| e.path.display().to_string());
                let mut lines = e.message.lines();
                let first = lines.next().unwrap_or("");
                let rest: String = lines
                    .map(|l| format!("\n         {l}"))
                    .collect();
                format!("  · {name}: {first}{rest}")
            })
            .collect();
        miette!("failed to load {} recipe file(s):\n{}", messages.len(), messages.join("\n"))
    })?;

    let renderer = make_renderer(&args.format, args.quiet);
    let text = match (&args.assignment, &args.platform, &args.os) {
        (Some(path), None, None) => {
            let path = &expand_tilde(path.clone());
            let src = std::fs::read_to_string(path)
                .map_err(|e| miette!("cannot read {}: {}", path.display(), e))?;
            let assignment: pedagog_core::recipe::assignment::AssignmentYaml =
                serde_yaml::from_str(&src).map_err(|e| miette!("{}", e))?;
            let plan =
                resolve::resolve_build(&assignment, &store).map_err(|e| miette!("{}", e))?;

            if args.show_base {
                let base = resolve::resolve_base(&assignment.environment.os, &store)
                    .map_err(|e| miette!("{}", e))?;
                renderer.render_build_with_base(&base, &plan)
            } else {
                renderer.render_build(&plan)
            }
        }
        (None, Some(platform_str), Some(os_str)) => {
            let os_id = Id::try_from(os_str.clone()).map_err(|e| miette!("{}", e))?;
            let kind = PlatformKind::try_from(platform_str.clone()).map_err(|e| miette!("{}", e))?;
            let plan = resolve::resolve_platform(&kind, &os_id, &store)
                .map_err(|e| miette!("{}", e))?;

            if args.show_base {
                let base = resolve::resolve_base(&os_id, &store).map_err(|e| miette!("{}", e))?;
                renderer.render_build_with_base(&base, &plan)
            } else {
                renderer.render_build(&plan)
            }
        }
        (None, None, Some(os_str)) => {
            let os_id = Id::try_from(os_str.clone()).map_err(|e| miette!("{}", e))?;
            let plan = resolve::resolve_base(&os_id, &store).map_err(|e| miette!("{}", e))?;
            renderer.render_base(&plan)
        }
        _ => unreachable!("clap ArgGroup ensures a valid target combination"),
    };

    match &args.output {
        Some(path) => {
            let path = expand_tilde(path.clone());
            let mut f = std::fs::File::create(&path)
                .map_err(|e| miette!("cannot create {}: {}", path.display(), e))?;
            f.write_all(text.as_bytes())
                .map_err(|e| miette!("write error: {}", e))?;
        }
        None => print!("{text}"),
    }
    Ok(())
}

fn make_renderer(format: &Format, _quiet: bool) -> Box<dyn Renderer> {
    match format {
        Format::Describe => Box::new(render::describe::Describe),
        Format::Containerfile => Box::new(render::containerfile::Containerfile),
    }
}

fn recipe_dirs(extra: &[PathBuf], quiet: bool) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    match std::env::var("HAMMER_RECIPES") {
        Ok(val) => dirs.push(expand_tilde(PathBuf::from(val))),
        Err(_) => emit_warning(
            "HAMMER_RECIPES environment variable is not set",
            "Set it to the directory containing your os/, platforms/, and toolchains/ recipes. \
             Without it, no default recipe directory is used — pass --recipes <dir> instead.",
            quiet,
        ),
    }

    for path in extra {
        dirs.push(expand_tilde(path.clone()));
    }

    for dir in &dirs {
        if !dir.exists() {
            emit_warning(
                &format!("recipe directory does not exist: {}", dir.display()),
                "Check the path and make sure it contains os/, platforms/, and toolchains/ subdirectories.",
                quiet,
            );
        }
    }

    if dirs.is_empty() {
        return Err(miette!(
            "no recipe directories configured\n\
             Set the HAMMER_RECIPES environment variable or pass --recipes <dir>"
        ));
    }

    Ok(dirs)
}

fn expand_tilde(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(format!("{}/{}", home, &s[2..]));
        }
    }
    path
}

fn run_vend(args: VendArgs) -> Result<()> {
    let dirs = recipe_dirs(&args.recipes, args.quiet)?;
    vend::run_vend(args, dirs)
}

fn emit_warning(message: &str, help: &str, quiet: bool) {
    if quiet {
        return;
    }
    let diag = MietteDiagnostic::new(message)
        .with_severity(Severity::Warning)
        .with_help(help.to_string());
    let report = Report::from(diag);
    let handler = GraphicalReportHandler::new();
    let mut buf = String::new();
    let _ = handler.render_report(&mut buf, report.as_ref());
    // Trailing newline separates consecutive warnings or a warning from an error.
    eprint!("{buf}\n");
}
