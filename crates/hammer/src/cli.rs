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
    /// Check, pack and upload an assignment directory (through the api, to jobs).
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
    /// An assignment's dates in this course: the file's, with the course's edits.
    #[command(subcommand)]
    Dates(DatesCmd),
    /// What students see of each test: show the rules, or override them.
    #[command(subcommand)]
    Visibility(VisibilityCmd),
    /// Results: release them to students (or take a release back).
    #[command(subcommand)]
    Results(ResultsCmd),
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
    long_about = "Check, pack and upload an assignment directory, through the api, to jobs.

The directory (the one containing assignment.yml) is checked locally first:
assignment.yml parses, its files are present, and it resolves against your
recipes when HAMMER_RECIPES or --recipes is set. jobs then repeats the checks
against its own recipes, which are authoritative.

What gets uploaded: every regular file under the directory, except VCS metadata
(.git, .hg, .svn). Symlinks and special files are refused.

jobs refuses an archive over 50 MiB compressed, 200 MiB unpacked, or 10 000
files (its configured defaults), and identifies uploads by their files' paths,
executable bits and contents: uploading identical files again is recognized,
however they were packed. Every upload builds a new image, identical files
included (its base image or pinned ingredients may have moved); a newer upload
supersedes a build still in progress."
)]
pub struct UploadArgs {
    /// Assignment directory (contains assignment.yml).
    pub dir: PathBuf,

    /// The course [default: the nearest course.yml at or above DIR, else `hammer use`].
    #[arg(short = 'c', long)]
    pub course: Option<String>,

    /// Once this upload's build validates and becomes current, regrade every
    /// student's latest submission against it (earlier results are kept).
    #[arg(long)]
    pub regrade: bool,

    /// Adopt the file's dates once this build is current, dropping the
    /// course's date edits (by default the course's edits stay).
    #[arg(long)]
    pub take_dates: bool,

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
--at now, at their last submission's time, or at a given time.")]
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
}

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct SubmissionsArgs {
    #[command(subcommand)]
    pub action: Option<SubmissionsCmd>,
    /// The assignment's id.
    pub assignment: Option<String>,
    /// A student's, by email (course staff); yours by default.
    #[arg(long = "for", value_name = "EMAIL")]
    pub for_: Option<String>,
    #[arg(short = 'c', long)]
    pub course: Option<String>,
}

#[derive(clap::Subcommand)]
pub enum SubmissionsCmd {
    /// Make an earlier submission the one that counts (course staff), until
    /// the student submits something newer; ids come from `hammer submissions
    /// ASSIGNMENT --for EMAIL`.
    Activate {
        /// The submission's id.
        id: i64,
        /// Go back to the latest submission counting.
        #[arg(long)]
        clear: bool,
        #[arg(short = 'c', long)]
        course: Option<String>,
    },
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

#[derive(clap::Subcommand)]
pub enum DatesCmd {
    /// The dates in effect and where each comes from (anyone in the course).
    Show {
        assignment: String,
        #[arg(short = 'c', long)]
        course: Option<String>,
    },
    /// Edit the course's dates (RFC 3339 with an offset, e.g.
    /// 2026-09-08T23:59:00-04:00); other fields keep their values.
    Set {
        assignment: String,
        #[arg(short = 'c', long)]
        course: Option<String>,
        /// When submissions open, or `always`.
        #[arg(long, value_name = "TIME")]
        open: Option<String>,
        /// After this, submissions are late.
        #[arg(long, value_name = "TIME")]
        due: Option<String>,
        /// After this, submissions are refused.
        #[arg(long, value_name = "TIME")]
        late_due: Option<String>,
    },
    /// Drop the course's edits (all, or the fields given): the file's apply again.
    Clear {
        assignment: String,
        #[arg(short = 'c', long)]
        course: Option<String>,
        #[arg(long)]
        open: bool,
        #[arg(long)]
        due: bool,
        #[arg(long)]
        late_due: bool,
    },
}

#[derive(clap::Subcommand)]
pub enum VisibilityCmd {
    /// Each test's resolved visibility, where it comes from, and whether
    /// students see it now (staff).
    Show {
        assignment: String,
        #[arg(short = 'c', long)]
        course: Option<String>,
        /// Only this test (its id, node or name).
        #[arg(long)]
        test: Option<String>,
        /// Every layer's setting, not just the one that decided.
        #[arg(short = 'v', long)]
        verbose: bool,
    },
    /// Override when students see each part of every test, or one: visible,
    /// hidden, after_end (after the deadline, or release) or after_publish
    /// (on release).
    Set {
        assignment: String,
        #[arg(short = 'c', long)]
        course: Option<String>,
        /// One test, by id (else its node); every test if absent.
        #[arg(long)]
        test: Option<String>,
        /// Points, and pass/fail.
        #[arg(long, value_name = "VISIBILITY")]
        score: Option<String>,
        /// The checks (each item's steps).
        #[arg(long, value_name = "VISIBILITY")]
        feedback: Option<String>,
        /// What the test printed.
        #[arg(long, value_name = "VISIBILITY")]
        stdout: Option<String>,
        /// What went wrong (failures, exceptions, the values compared).
        #[arg(long, value_name = "VISIBILITY")]
        error: Option<String>,
    },
    /// Drop overrides for every test (the all-tests entry), one test, or all.
    Clear {
        assignment: String,
        #[arg(short = 'c', long)]
        course: Option<String>,
        #[arg(long, conflicts_with = "all")]
        test: Option<String>,
        /// Every override, all tests and each test.
        #[arg(long)]
        all: bool,
    },
}

#[derive(clap::Subcommand)]
pub enum ResultsCmd {
    /// One row per student who submitted (staff): what counts, lateness, and
    /// the score at each stage of visibility; * marks what the student sees now.
    List {
        assignment: String,
        #[arg(short = 'c', long)]
        course: Option<String>,
        /// Part of a name or email.
        #[arg(long)]
        search: Option<String>,
        /// Include dropped students.
        #[arg(long)]
        dropped: bool,
        /// Include instructors' and graders' own submissions.
        #[arg(long)]
        staff: bool,
    },
    /// Your results (as you may see them now), or a student's in full (staff).
    Show {
        assignment: String,
        /// Part of a student's email or name (staff); yours if absent.
        student: Option<String>,
        #[arg(short = 'c', long)]
        course: Option<String>,
        /// Another submission than the one that counts.
        #[arg(long)]
        submission: Option<i64>,
    },
    /// Release results: what's shown "after_publish" (and "after_end") shows now.
    Release {
        assignment: String,
        #[arg(short = 'c', long)]
        course: Option<String>,
        /// Take the release back (students may already have seen them).
        #[arg(long)]
        undo: bool,
    },
    /// Email students what they see now (instructors): everyone who has a
    /// submission that counts, or one student.
    Notify {
        assignment: String,
        #[arg(short = 'c', long)]
        course: Option<String>,
        /// One student's email.
        #[arg(long = "for")]
        student: Option<String>,
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
