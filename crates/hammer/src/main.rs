mod cli;
mod submit;
mod upload;
mod vend;

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Parser;
use miette::{GraphicalReportHandler, MietteDiagnostic, Report, Result, Severity, miette};

use cli::{Cli, Command, Format, PlanArgs, SubmitArgs, UploadArgs, VendArgs};
use pedagog_core::recipe::assignment::{AssignmentYaml, FileSources};
use pedagog_core::recipe::primitives::Id;
use pedagog_core::recipe::render::containerfile::Containerfile;
use pedagog_core::recipe::render::describe::Describe;
use pedagog_core::recipe::render::{AssignmentFiles, FromSource, Render, RenderOptions};
use pedagog_core::recipe::resolve::{ResolveError, resolve, resolve_base};
use pedagog_core::recipe::store::RecipeStore;

fn main() -> Result<()> {
    // Use try_parse so clap errors go through miette's renderer instead of
    // clap's own formatter.
    // Let clap handle all parse errors and help/version output — it formats them well on its own.
    let cli = Cli::try_parse().unwrap_or_else(|e| e.exit());

    match cli.command {
        Command::Plan(args) => run_plan(args),
        Command::Vend(args) => run_vend(args),
        Command::Upload(args) => run_upload(args),
        Command::Submit(args) => run_submit(args),
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
                let name = e
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| e.path.display().to_string());
                let mut lines = e.message.lines();
                let first = lines.next().unwrap_or("");
                let rest: String = lines.map(|l| format!("\n         {l}")).collect();
                format!("  · {name}: {first}{rest}")
            })
            .collect();
        miette!(
            "failed to load {} recipe file(s):\n{}",
            messages.len(),
            messages.join("\n")
        )
    })?;

    let spec = match (&args.assignment, &args.os) {
        (Some(path), None) => {
            let path = expand_tilde(path.clone());
            let src = std::fs::read_to_string(&path)
                .map_err(|e| miette!("cannot read {}: {}", path.display(), e))?;
            let assignment: AssignmentYaml =
                serde_yaml::from_str(&src).map_err(|e| miette!("{}", e))?;
            let files = detect_files(&path, &assignment.files)?;
            if let Some(problem) = files.problems(&assignment).into_iter().next() {
                return Err(miette!("{problem}"));
            }
            resolve(&assignment, &store)
                .map_err(resolve_report)?
                .with_files(files)
        }
        (None, Some(os_str)) => {
            let os_id = Id::try_from(os_str.clone()).map_err(|e| miette!("{}", e))?;
            resolve_base(&os_id, &store).map_err(resolve_report)?
        }
        _ => unreachable!("clap ArgGroup ensures exactly one of -a / -o"),
    };

    // --show-base emits a self-contained Containerfile from the upstream image;
    // otherwise we FROM the pre-built base image.
    let from = if args.show_base {
        FromSource::Standalone
    } else {
        FromSource::PrebuiltBase
    };
    let opts = RenderOptions {
        registry: args.registry.clone(),
        from,
    };

    let text = match args.format {
        Format::Containerfile => Containerfile::render(&spec, &opts).to_string(),
        Format::Describe => Describe::render(&spec, &opts).to_string(),
    };

    // describe output is ANSI-styled: anstream strips it when stdout isn't a
    // terminal (or NO_COLOR is set); files never get it.
    match &args.output {
        Some(path) => {
            let path = expand_tilde(path.clone());
            let mut f = std::fs::File::create(&path)
                .map_err(|e| miette!("cannot create {}: {}", path.display(), e))?;
            write!(f, "{}", anstream::adapter::strip_str(&text))
                .map_err(|e| miette!("write error: {}", e))?;
        }
        None => anstream::stdout()
            .write_all(text.as_bytes())
            .map_err(|e| miette!("write error: {}", e))?,
    }
    Ok(())
}

/// Archive directories next to `assignment.yml` (the build context) become the
/// ASSIGNMENT FILES phase. A missing directory under its default name just isn't
/// baked; a missing directory the assignment configured is an error (likely a typo).
fn detect_files(assignment_path: &Path, sources: &FileSources) -> Result<AssignmentFiles> {
    let dir = assignment_path.parent().unwrap_or(Path::new("."));
    AssignmentFiles::detect(sources, |src| dir.join(src).is_dir())
        .map_err(|e| miette!("{e} ({})", assignment_path.display()))
}

fn resolve_report(err: ResolveError) -> Report {
    let mut diag = MietteDiagnostic::new(err.to_string());
    if let Some(help) = err.help() {
        diag = diag.with_help(help);
    }
    Report::from(diag)
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
    if let Some(rest) = s.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(format!("{home}/{rest}"));
    }
    path
}

fn run_vend(args: VendArgs) -> Result<()> {
    let dirs = recipe_dirs(&args.recipes, args.quiet)?;
    vend::run_vend(args, dirs)
}

fn run_submit(args: SubmitArgs) -> Result<()> {
    let paths: Vec<PathBuf> = args.paths.into_iter().map(expand_tilde).collect();
    let files = submit::collect(&paths).map_err(|e| miette!("{e}"))?;
    let url = jobs_url(args.jobs_url);
    let submitted =
        submit::send(&url, &args.assignment, &args.student, &files).map_err(|e| miette!("{e}"))?;
    for line in submit::describe(&submitted) {
        println!("{line}");
    }
    Ok(())
}

fn jobs_url(flag: Option<String>) -> String {
    flag.or_else(|| std::env::var(upload::JOBS_URL_ENV).ok())
        .unwrap_or_else(|| upload::DEFAULT_JOBS_URL.to_owned())
}

fn run_upload(args: UploadArgs) -> Result<()> {
    let dir = expand_tilde(args.dir.clone());
    let assignment_path = dir.join("assignment.yml");
    let src = std::fs::read_to_string(&assignment_path)
        .map_err(|e| miette!("cannot read {}: {}", assignment_path.display(), e))?;
    let assignment: AssignmentYaml = serde_yaml::from_str(&src).map_err(|e| miette!("{}", e))?;
    let files = detect_files(&assignment_path, &assignment.files)?;
    if let Some(problem) = files.problems(&assignment).into_iter().next() {
        return Err(miette!("{problem}"));
    }
    let configured = std::env::var_os("HAMMER_RECIPES").is_some() || !args.recipes.is_empty();
    if configured {
        let dirs = recipe_dirs(&args.recipes, args.quiet)?;
        let store = RecipeStore::load(&dirs)
            .map_err(|errs| miette!("failed to load {} recipe file(s)", errs.len()))?;
        resolve(&assignment, &store).map_err(resolve_report)?;
    } else {
        emit_warning(
            "skipping the local resolve check (no recipes configured)",
            "Set HAMMER_RECIPES or pass --recipes <dir> to check before uploading; jobs checks either way.",
            args.quiet,
        );
    }

    let url = jobs_url(args.jobs_url);
    let archive = upload::pack(&dir).map_err(|e| miette!("{e}"))?;
    let uploaded = upload::send(&url, archive, args.force).map_err(|e| miette!("{e}"))?;
    println!(
        "uploaded {} (tree {}, {} bytes) as upload {}",
        uploaded.assignment_id,
        &uploaded.tree_hash[..12.min(uploaded.tree_hash.len())],
        uploaded.size_bytes,
        uploaded.upload_id
    );
    match uploaded.build.status.as_str() {
        "already_built" => println!(
            "already built and validated as build {}; nothing to do (--force rebuilds)",
            uploaded.build.build_id
        ),
        _ => println!("build {} dispatched", uploaded.build.build_id),
    }
    for warning in &uploaded.warnings {
        emit_warning(warning, "", args.quiet);
    }
    if !args.wait {
        return Ok(());
    }
    let status = upload::wait(
        &url,
        &uploaded.assignment_id,
        uploaded.build.build_id,
        |s| {
            let stage = match (s.build_status.as_str(), s.validation_status.as_str()) {
                ("running", _) => "building",
                ("succeeded", "pending" | "running") => "validating",
                _ => return,
            };
            println!("{stage}...");
        },
    )
    .map_err(|e| miette!("{e}"))?;
    let (ok, line) = upload::outcome(&status);
    if ok {
        println!("{line}");
        println!("image: {}", status.image_ref);
        return Ok(());
    }
    let mut details: Vec<String> = status.validation_problems.clone();
    if let Some(error) = &status.error {
        details.push(error.clone());
    }
    Err(miette!("{line}\n{}", details.join("\n")))
}

fn emit_warning(message: &str, help: &str, quiet: bool) {
    if quiet {
        return;
    }
    let mut diag = MietteDiagnostic::new(message).with_severity(Severity::Warning);
    if !help.is_empty() {
        diag = diag.with_help(help.to_string());
    }
    let report = Report::from(diag);
    let handler = GraphicalReportHandler::new();
    let mut buf = String::new();
    let _ = handler.render_report(&mut buf, report.as_ref());
    // Trailing newline separates consecutive warnings or a warning from an error.
    eprintln!("{buf}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str, dirs: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hammer-{name}-{}", std::process::id()));
        for d in dirs {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        dir
    }

    fn sources(yaml: &str) -> FileSources {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn detects_default_dirs_next_to_the_assignment() {
        let dir = scratch("defaults", &["tests", "starter"]);
        let files = detect_files(&dir.join("assignment.yml"), &FileSources::default()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            files,
            AssignmentFiles {
                starter: Some("starter".into()),
                tests: Some("tests".into()),
                solution: None,
            }
        );
    }

    #[test]
    fn uses_configured_source_dirs() {
        let dir = scratch("configured", &["src", "grading/hidden"]);
        let files = detect_files(
            &dir.join("assignment.yml"),
            &sources("starter: src\ntests: grading/hidden\n"),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(files.starter.as_deref(), Some("src"));
        assert_eq!(files.tests.as_deref(), Some("grading/hidden"));
        assert_eq!(files.solution, None);
    }

    #[test]
    fn missing_configured_dir_is_an_error() {
        let dir = scratch("typo", &["tests"]);
        let err = detect_files(&dir.join("assignment.yml"), &sources("tests: gradng\n"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.unwrap_err().to_string().contains("gradng/ not found"));
    }
}
