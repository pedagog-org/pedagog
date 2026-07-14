use std::path::PathBuf;

use clap::{ArgGroup, Parser, ValueEnum};

#[derive(Parser)]
#[command(name = "hammer", about = "Compile Pedagog recipes into container build plans")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Produce a build plan for an assignment, a platform, or an OS base image.
    Plan(PlanArgs),
    /// Download and cache recipe assets into each recipe directory's ingredients/ folder.
    Vend(VendArgs),
}

#[derive(clap::Args)]
#[command(
    group(ArgGroup::new("target").required(true).args(["assignment", "os"])),
    override_usage = "hammer plan [OPTIONS] (-a <FILE> | -o <OS_ID> | -p <PLATFORM_ID> -o <OS_ID>)",
    long_about = "Produce a build plan from one of three targets:

  -a <FILE>                    Full build plan from an assignment YAML (all layers)
  -p <PLATFORM_ID> -o <OS_ID>  Platform layer plan with default params
  -o <OS_ID>                   OS base image plan

Add -b / --show-base to prepend OS init layers to an assignment or platform plan.
Add -f containerfile to emit a Containerfile instead of the default tree view.",
)]
pub struct PlanArgs {
    /// Assignment YAML file (mutually exclusive with -o / -p).
    #[arg(short = 'a', long, value_name = "FILE", conflicts_with_all = ["os", "platform"])]
    pub assignment: Option<PathBuf>,

    /// OS id — base image plan, or pairs with -p for a platform plan.
    #[arg(short = 'o', long, value_name = "OS_ID")]
    pub os: Option<String>,

    /// Platform id — platform layer plan (requires -o).
    #[arg(short = 'p', long, value_name = "PLATFORM_ID", requires = "os", conflicts_with = "assignment")]
    pub platform: Option<String>,

    /// Additional recipe directory; repeatable, searched after HAMMER_RECIPES.
    #[arg(short = 'r', long, value_name = "DIR", action = clap::ArgAction::Append)]
    pub recipes: Vec<PathBuf>,

    /// Output format: describe (tree view) or containerfile.
    #[arg(short = 'f', long, value_enum, default_value_t = Format::Describe)]
    pub format: Format,

    /// Write output to a file instead of stdout.
    #[arg(short = 'O', long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Registry prefix for base image names in Containerfile output (e.g. localhost).
    #[arg(long, value_name = "REGISTRY")]
    pub registry: Option<String>,

    /// Prepend OS base image layers to an assignment or platform plan.
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
  --toolchain <ID[:VER]>  Vendor ingredients for a specific toolchain recipe",
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

    /// Suppress non-fatal warnings.
    #[arg(short = 'q', long)]
    pub quiet: bool,
}

#[derive(ValueEnum, Clone, PartialEq)]
pub enum Format {
    /// Tree view showing layers, steps, and commands (default).
    Describe,
    /// Containerfile (Dockerfile) syntax.
    Containerfile,
}
