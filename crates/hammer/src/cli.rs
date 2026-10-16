use std::path::PathBuf;

use clap::{ArgGroup, Parser, ValueEnum};

#[derive(Parser)]
#[command(
    name = "hammer",
    about = "Compile Pedagog recipes into container build plans"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Produce a Containerfile for an assignment or an OS base image.
    Plan(PlanArgs),
    /// Download and cache recipe assets into each recipe directory's ingredients/ folder.
    Vend(VendArgs),
    /// Check, pack and upload an assignment directory to the jobs service.
    Upload(UploadArgs),
    Submit(SubmitArgs),
}

#[derive(clap::Args)]
#[command(
    group(ArgGroup::new("target").required(true).args(["assignment", "os"])),
    override_usage = "hammer plan [OPTIONS] (-a <FILE> | --os <OS_ID>)",
    long_about = "Produce a build plan from one of two targets:

  -a <FILE>       Full assignment image plan (all build phases + runtime)
  --os <OS_ID>    Reusable OS base image plan

By default an assignment plan builds FROM the pre-built base image. Add
-b / --show-base to emit a self-contained Containerfile from the upstream image.",
)]
pub struct PlanArgs {
    /// Assignment YAML file (mutually exclusive with --os).
    #[arg(short = 'a', long, value_name = "FILE", conflicts_with = "os")]
    pub assignment: Option<PathBuf>,

    /// OS id — base image plan.
    #[arg(long, value_name = "OS_ID")]
    pub os: Option<String>,

    /// Additional recipe directory; repeatable, searched after HAMMER_RECIPES.
    #[arg(short = 'r', long, value_name = "DIR", action = clap::ArgAction::Append)]
    pub recipes: Vec<PathBuf>,

    /// Output format: containerfile, or describe for a human-readable tree.
    #[arg(short = 'f', long, value_enum, default_value_t = Format::Containerfile)]
    pub format: Format,

    /// Write output to a file instead of stdout.
    #[arg(short = 'O', long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Registry holding pedagog-built base images (e.g. localhost). Prefixes the
    /// pre-built base image only; upstream images are never prefixed.
    #[arg(long, value_name = "REGISTRY")]
    pub registry: Option<String>,

    /// Emit a self-contained Containerfile from the upstream image (include OS
    /// layer) instead of building FROM the pre-built base image.
    #[arg(short = 'b', long)]
    pub show_base: bool,

    /// Suppress non-fatal warnings (e.g. HAMMER_RECIPES not set).
    #[arg(short = 'q', long)]
    pub quiet: bool,
}

#[derive(clap::Args)]
#[command(
    long_about = "Download and cache assets declared in recipe `ingredients:` blocks.

Assets are written to <recipe-dir>/ingredients/<type>/<id>/<filename>.
Without filter flags, all ingredients across all recipe directories are vendored.
With filter flags, only matching recipes are processed.

  --os <ID>               Vendor ingredients for a specific OS recipe
  --platform <ID>         Vendor ingredients for a specific platform recipe
  --toolchain <ID[:VER]>  Vendor ingredients for a specific toolchain recipe"
)]
pub struct VendArgs {
    /// Filter: vend only assets for a specific OS recipe.
    #[arg(long, value_name = "OS_ID")]
    pub os: Option<String>,

    /// Filter: vend only assets for a specific platform recipe.
    #[arg(long, value_name = "PLATFORM_ID")]
    pub platform: Option<String>,

    /// Filter: vend only assets for a specific toolchain recipe (id or id:version).
    #[arg(long, value_name = "ID[:VERSION]")]
    pub toolchain: Option<String>,

    /// Assignment YAML file to scope vendoring to (planned — acts as a filter trigger).
    #[arg(long, value_name = "FILE", hide = true)]
    pub assignment: Option<PathBuf>,

    /// Additional recipe directory; repeatable, searched after HAMMER_RECIPES.
    #[arg(long, value_name = "DIR", action = clap::ArgAction::Append)]
    pub recipes: Vec<PathBuf>,

    /// Afterwards, delete vended files no recipe declares any more (e.g.
    /// after a recipe was renamed). Not with a filter.
    #[arg(long, conflicts_with_all = ["os", "platform", "toolchain", "assignment"])]
    pub prune: bool,

    /// Suppress non-fatal warnings.
    #[arg(short = 'q', long)]
    pub quiet: bool,
}

#[derive(clap::Args)]
#[command(
    long_about = "Check, pack and upload an assignment directory to the jobs service.

The directory (the one containing assignment.yml) is checked locally first:
assignment.yml parses, its files are present, and it resolves against your
recipes when HAMMER_RECIPES or --recipes is set. jobs then repeats the checks
against its own recipes, which are authoritative.

What gets uploaded: every regular file under the directory, except VCS metadata
(.git, .hg, .svn). Symlinks and special files are refused.

jobs refuses an archive over 50 MiB compressed, 200 MiB unpacked, or 10 000
files (its configured defaults), and identifies uploads by their files' paths,
executable bits and contents: uploading identical files again is recognized,
however they were packed. An upload whose image (its rendered Containerfile and
files) was already built and validated isn't rebuilt unless --force is given; a
newer upload supersedes a build still in progress.

jobs isn't exposed outside the cluster; `just kind-upload <dir>` port-forwards
to it and runs this command."
)]
pub struct UploadArgs {
    /// Assignment directory (contains assignment.yml).
    pub dir: PathBuf,

    /// jobs service URL [env: PEDAGOG_JOBS_URL] [default: http://localhost:18081].
    #[arg(long, value_name = "URL")]
    pub jobs_url: Option<String>,

    /// Rebuild even if this exact image was already built and validated.
    #[arg(long)]
    pub force: bool,

    /// Once this upload's build validates and becomes current, regrade every
    /// student's latest submission against it (earlier results are kept).
    #[arg(long)]
    pub regrade: bool,

    /// With --regrade: when students are told about their new results.
    #[arg(long, value_enum, default_value_t = Notify::Never, requires = "regrade")]
    pub notify: Notify,

    /// Wait for the build and validation, print the outcome, and fail if either fails.
    #[arg(short = 'w', long)]
    pub wait: bool,

    /// Additional recipe directory for the local check; repeatable, searched after HAMMER_RECIPES.
    #[arg(short = 'r', long, value_name = "DIR", action = clap::ArgAction::Append)]
    pub recipes: Vec<PathBuf>,

    /// Suppress non-fatal warnings.
    #[arg(short = 'q', long)]
    pub quiet: bool,
}

#[derive(clap::Args)]
#[command(
    long_about = "Submit files to an upload assignment as a student, for grading.

Give files, directories, or one archive. A directory sends every regular file
under it, by its path relative to the directory (VCS metadata skipped); a file
is sent by its name. A single .zip, .tar.gz or .tgz is unpacked by jobs unless
the assignment says `archives: keep`.

jobs applies the assignment's submission rules (include/exclude, required
files, size), drops OS clutter such as .DS_Store and __MACOSX, and drops a
folder that wraps everything (`strip: auto`), then prints what it kept and
ignored. The submission is graded against the assignment's current build;
it's refused if there's none yet.

The student id is an opaque string until accounts exist. `just kind-submit`
port-forwards to jobs on the kind cluster and runs this command."
)]
pub struct SubmitArgs {
    /// Files, directories, or one archive to submit.
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,

    /// Assignment id (from its assignment.yml).
    #[arg(short = 'a', long, value_name = "ID")]
    pub assignment: String,

    /// Student id.
    #[arg(short = 's', long, value_name = "ID")]
    pub student: String,

    /// Wait for grading, print the score and the tests that didn't pass, and
    /// fail if the run couldn't be graded.
    #[arg(short = 'w', long)]
    pub wait: bool,

    /// jobs service URL [env: PEDAGOG_JOBS_URL] [default: http://localhost:18081].
    #[arg(long, value_name = "URL")]
    pub jobs_url: Option<String>,
}

/// When a regrade tells students about their new results.
#[derive(ValueEnum, Clone, Copy, PartialEq, Debug)]
pub enum Notify {
    /// Never (the default: the instructor may be testing).
    Never,
    /// Only if their results changed.
    Changed,
    /// Always.
    Always,
}

impl Notify {
    pub fn as_str(self) -> &'static str {
        match self {
            Notify::Never => "never",
            Notify::Changed => "changed",
            Notify::Always => "always",
        }
    }
}

#[derive(ValueEnum, Clone, PartialEq)]
pub enum Format {
    /// Containerfile (Dockerfile) syntax (default).
    Containerfile,
    /// Human-readable tree of phases, layers, steps, and runtime.
    Describe,
}
