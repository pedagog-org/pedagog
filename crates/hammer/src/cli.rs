use std::path::PathBuf;

use clap::builder::TypedValueParser;
use clap::{ArgGroup, Parser, ValueEnum};
use pedagog_core::changes::Notify;

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
    /// Log in to the api with a code sent to your email.
    Login(LoginArgs),
    /// End your api session.
    Logout,
    /// Show who you're logged in as, and your courses.
    Whoami,
    /// Find users and set their global roles (admins only).
    #[command(subcommand)]
    Users(UsersCmd),
    /// Create, list and inspect courses.
    #[command(subcommand)]
    Courses(CoursesCmd),
    /// A course's instructors, graders and students.
    #[command(subcommand)]
    Members(MembersCmd),
    /// A course's assignments and their builds.
    #[command(subcommand)]
    Assignments(AssignmentsCmd),
    /// Set the course commands use when there's no -c and no course.yml.
    Use(UseArgs),
    /// Your submissions to an assignment (or a student's, for staff).
    Submissions(SubmissionsArgs),
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

    /// The course [default: the nearest course.yml at or above DIR, else `hammer use`].
    #[arg(short = 'c', long)]
    pub course: Option<String>,

    /// Upload straight to jobs at this URL, without logging in (operators).
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
    #[arg(
        long,
        default_value = "never",
        requires = "regrade",
        value_parser = clap::builder::PossibleValuesParser::new(["never", "changed", "always"])
            .try_map(|s| s.parse::<Notify>()),
    )]
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
#[command(long_about = "Submit files to an upload assignment, for grading.

Give files, directories, or one archive. A directory sends every regular file
under it, by its path relative to the directory (VCS metadata skipped); a file
is sent by its name. A single .zip, .tar.gz or .tgz is unpacked by jobs unless
the assignment says `archives: keep`.

jobs applies the assignment's submission rules (include/exclude, required
files, size), drops OS clutter such as .DS_Store and __MACOSX, and drops a
folder that wraps everything (`strip: auto`), then prints what it kept and
ignored. The submission is graded against the assignment's current build;
it's refused if there's none yet.

Students submit while the assignment is open (late after its due date, until
its late_due). Course staff submit their own work whenever, and with --for
submit a student's on their behalf (e.g. to fix a broken submission), counted
--at now, at their last submission's time, or at a given time.

Operators can send straight to jobs with --jobs-url and --student (`just
kind-submit` port-forwards to jobs on kind).")]
pub struct SubmitArgs {
    /// The assignment's id (from its assignment.yml).
    pub assignment: String,

    /// Files, directories, or one archive to submit.
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,

    /// The course [default: the nearest course.yml, else `hammer use`].
    #[arg(short = 'c', long)]
    pub course: Option<String>,

    /// Submit for a student, by email (course staff); accepted whatever the dates.
    #[arg(long = "for", value_name = "EMAIL")]
    pub for_: Option<String>,

    /// With --for: the time it counts at: now (default), last (their latest
    /// submission's), or an RFC 3339 time.
    #[arg(long, requires = "for_", value_name = "WHEN")]
    pub at: Option<String>,

    /// Wait for grading and print the outcome; fail if the run couldn't be graded.
    #[arg(short = 'w', long)]
    pub wait: bool,

    /// Submit straight to jobs at this URL, without logging in (operators).
    #[arg(long, value_name = "URL", requires = "student")]
    pub jobs_url: Option<String>,

    /// With --jobs-url: the student id to record.
    #[arg(
        short = 's',
        long,
        value_name = "ID",
        requires = "jobs_url",
        conflicts_with = "for_"
    )]
    pub student: Option<String>,
}

#[derive(clap::Args)]
pub struct SubmissionsArgs {
    /// The assignment's id.
    pub assignment: String,
    /// A student's, by email (course staff); yours by default.
    #[arg(long = "for", value_name = "EMAIL")]
    pub for_: Option<String>,
    #[arg(short = 'c', long)]
    pub course: Option<String>,
}

#[derive(clap::Args)]
#[command(
    long_about = "Log in to the api: it emails you a short code, which you type in here.

The session lasts 30 days and is saved in ~/.config/pedagog/session.json
(or $XDG_CONFIG_HOME/pedagog), readable only by you. `hammer logout` ends it."
)]
pub struct LoginArgs {
    /// Your email (as registered by your course or an admin).
    #[arg(long)]
    pub email: String,

    /// api URL [env: PEDAGOG_API_URL] [default: the saved session's].
    #[arg(long, value_name = "URL")]
    pub api: Option<String>,
}

#[derive(clap::Subcommand)]
pub enum UsersCmd {
    /// List users, optionally by global role or matching part of an email or name.
    List {
        /// admin, staff or student.
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        search: Option<String>,
        #[arg(long, default_value_t = 1)]
        page: i64,
    },
    /// A user's role, logins, sessions and courses; QUERY is part of an email or name.
    Show { query: String },
    /// Set a user's global role (creating them if needed): admin, staff (creates
    /// courses) or student.
    Role { email: String, role: String },
    /// End every session a user has.
    Signout { email: String },
}

#[derive(clap::Subcommand)]
pub enum CoursesCmd {
    /// Courses you're in; --all lists every course (admins).
    List {
        #[arg(long)]
        all: bool,
        /// With --all: part of a course id or name.
        #[arg(long, requires = "all")]
        search: Option<String>,
        #[arg(long, default_value_t = 1, requires = "all")]
        page: i64,
    },
    /// A course's instructors and graders, and how many students it has.
    Show {
        #[arg(short = 'c', long)]
        course: Option<String>,
    },
    /// Change a course's id (slug) and/or display name (instructors and admins).
    Rename {
        #[arg(short = 'c', long)]
        course: Option<String>,
        /// The new id, e.g. cop3502-fall26.
        #[arg(long)]
        id: Option<String>,
        /// The new display name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Create a course (staff and admins). You teach it unless you name its
    /// instructors.
    Create {
        /// The course id: lowercase letters, digits and dashes, e.g. cop3502-f26.
        id: String,
        /// The display name, e.g. "Programming Fundamentals 1 (Fall 2026)".
        name: String,
        /// An instructor's email; repeatable.
        #[arg(long = "instructor", value_name = "EMAIL")]
        instructors: Vec<String>,
    },
}

#[derive(clap::Subcommand)]
pub enum MembersCmd {
    /// List a course's members.
    List {
        #[arg(short = 'c', long)]
        course: Option<String>,
        /// instructor, grader or student.
        #[arg(long)]
        role: Option<String>,
        /// Include dropped members.
        #[arg(long)]
        dropped: bool,
    },
    /// One member; QUERY is part of an email or name.
    Show {
        query: String,
        #[arg(short = 'c', long)]
        course: Option<String>,
    },
    /// Add people (or restore them, or change their role).
    Add {
        #[arg(required = true)]
        emails: Vec<String>,
        #[arg(short = 'c', long)]
        course: Option<String>,
        /// instructor, grader or student.
        #[arg(long, default_value = "student")]
        role: String,
        /// Their name (with one email).
        #[arg(long)]
        name: Option<String>,
    },
    /// Drop people: they lose access and keep their history.
    Drop {
        #[arg(required = true)]
        emails: Vec<String>,
        #[arg(short = 'c', long)]
        course: Option<String>,
    },
    /// Enroll students from a CSV: `email,name` (with a header), or Canvas's
    /// gradebook export. Adds students; staff are left alone.
    Import {
        file: PathBuf,
        #[arg(short = 'c', long)]
        course: Option<String>,
        /// Also drop students missing from the file (asks first).
        #[arg(long)]
        sync: bool,
        /// With --sync: don't ask.
        #[arg(long, requires = "sync")]
        yes: bool,
    },
}

#[derive(clap::Subcommand)]
pub enum AssignmentsCmd {
    /// The course's assignments (staff also see each one's latest build).
    List {
        #[arg(short = 'c', long)]
        course: Option<String>,
    },
    /// An assignment's recent builds (staff).
    Show {
        /// The assignment's id (from its assignment.yml).
        assignment: String,
        #[arg(short = 'c', long)]
        course: Option<String>,
    },
}

#[derive(clap::Args)]
pub struct UseArgs {
    /// The course; without one, show the current default.
    pub course: Option<String>,
    /// Forget the default.
    #[arg(long, conflicts_with = "course")]
    pub clear: bool,
}

#[derive(ValueEnum, Clone, PartialEq)]
pub enum Format {
    /// Containerfile (Dockerfile) syntax (default).
    Containerfile,
    /// Human-readable tree of phases, layers, steps, and runtime.
    Describe,
}
