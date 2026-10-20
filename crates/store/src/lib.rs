//! Persistent build-state models, and (behind features) Postgres access (`db`)
//! and S3-compatible object storage (`objects`).
//!
//! The model types are always available and dependency-light so a WASM consumer
//! can depend on `store` for them; the features gate the sqlx and object_store layers.

use strum_macros::{Display, EnumString};

#[cfg(feature = "db")]
pub mod db;
#[cfg(feature = "objects")]
pub mod objects;

/// State of a single build attempt (`build_runs.status`).
///
/// Serialized to a lowercase `TEXT` column via strum. strum only guards the Rust
/// write path, so the migration adds a `CHECK` constraint as the DB-side backstop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum BuildStatus {
    Running,
    Succeeded,
    Failed,
}

/// A `build_runs` row, as needed by crash/restart reconciliation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildRun {
    pub id: i64,
    pub os_id: String,
    pub hash: String,
    pub image_ref: String,
}

/// Build stage of an assignment build attempt (`assignment_builds.build_status`).
/// `Superseded` means a newer upload cancelled the attempt, at whatever stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum AssignmentBuildStatus {
    Running,
    Succeeded,
    Failed,
    Superseded,
}

/// Validation stage of an assignment build attempt (`assignment_builds.validation_status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum ValidationStatus {
    /// Not started (or never will be, if the build failed).
    Pending,
    /// Waiting in the admission queue.
    Queued,
    Running,
    Passed,
    /// The results are consistent, but the reference solution disagrees with the tests.
    Failed,
    /// No reference solution to validate against.
    Skipped,
    /// No results, or results that fail the format checks (untrusted).
    Invalid,
}

impl ValidationStatus {
    /// Whether a build that reached this status can become the assignment's current build.
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Passed | Self::Skipped)
    }
}

/// An `assignments` row: an assignment of one course, keyed by `id`; `slug` is
/// the id in its `assignment.yml`, unique within the course.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub id: i64,
    pub course_id: String,
    pub slug: String,
    pub name: String,
    /// The build sessions and grading use.
    pub current_build_id: Option<i64>,
}

/// An `assignment_uploads` row: one distinct file tree of an assignment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssignmentUpload {
    pub id: i64,
    pub assignment_id: i64,
    pub tree_hash: String,
    pub object_key: String,
    pub size_bytes: i64,
}

/// An `assignment_builds` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssignmentBuild {
    pub id: i64,
    pub assignment_id: i64,
    /// The assignment's course and slug (for names in the cluster).
    pub course_id: String,
    pub slug: String,
    pub upload_id: i64,
    pub image_hash: String,
    pub image_ref: String,
    pub build_status: AssignmentBuildStatus,
    pub validation_status: ValidationStatus,
    /// The parsed results as JSON text (kept as text so this crate needs no JSON library).
    pub results: Option<String>,
    pub validation_problems: Vec<String>,
    pub error: Option<String>,
    /// Whether the attempt (build and validation) is over.
    pub finished: bool,
    /// What its test runs need; `None` for builds from before they were saved.
    pub settings: Option<BuildSettings>,
    /// The file's `assignment:` policy as JSON text; `None` for builds from
    /// before it was saved (the default policy).
    pub policy: Option<String>,
}

/// A build's run settings as stored; `jobs` owns what the JSON means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildSettings {
    pub platform: PlatformKind,
    pub isolation: Isolation,
    /// Egress as JSON text.
    pub egress: String,
    /// The parsed `submission:` rules as JSON text.
    pub submission: String,
}

/// Where a submission came from; each platform kind takes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum SubmissionSource {
    Upload,
    Workspace,
}

/// A `submissions` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub id: i64,
    pub assignment_id: i64,
    pub student_id: String,
    /// The build current when the submission was accepted.
    pub build_id: i64,
    pub source: SubmissionSource,
    pub tree_hash: String,
    pub object_key: String,
    pub size_bytes: i64,
    /// What intake did, as JSON text.
    pub receipt: String,
}

/// State of a grading run (`grading_runs.status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum RunStatus {
    /// Waiting in the admission queue.
    Queued,
    Grading,
    /// Results are in (failing tests included).
    Graded,
    /// No usable results: missing, invalid, a crash, or a timeout.
    Error,
}

/// Why a grading run exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum RunReason {
    Submission,
    Regrade,
}

/// A person, identified by email.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub name: String,
    pub role: GlobalRole,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role == GlobalRole::Admin
    }

    /// Admins and staff create courses.
    pub fn creates_courses(&self) -> bool {
        matches!(self.role, GlobalRole::Admin | GlobalRole::Staff)
    }
}

/// What a user is on the platform (`users.role`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum GlobalRole {
    /// Runs the platform: every course, every user.
    Admin,
    /// Creates courses.
    Staff,
    Student,
}

/// A course offering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Course {
    pub id: String,
    pub name: String,
}

/// What a member is in a course (`memberships.role`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum CourseRole {
    /// Everything in the course.
    Instructor,
    /// Sees the roster and every result; changes nothing.
    Grader,
    Student,
}

/// How someone joined a course (`memberships.source`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum JoinSource {
    /// Named when the course was created.
    Created,
    Manual,
    Import,
}

/// A `memberships` row with its course and user. Times are RFC 3339 (UTC).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Membership {
    pub course: Course,
    pub user: User,
    pub role: CourseRole,
    pub source: JoinSource,
    /// The email of whoever added them, if known.
    pub added_by: Option<String>,
    pub joined_at: String,
    pub dropped_at: Option<String>,
}

/// Where a session was opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum SessionKind {
    Web,
    Cli,
}

pub use pedagog_core::changes::Notify;
pub use pedagog_core::recipe::assignment::Isolation;
pub use pedagog_core::recipe::platform::PlatformKind;

/// A `grading_runs` row.
#[derive(Clone, Debug, PartialEq)]
pub struct GradingRun {
    pub id: i64,
    pub submission_id: i64,
    pub build_id: i64,
    pub reason: RunReason,
    pub regrade_batch_id: Option<i64>,
    pub status: RunStatus,
    pub isolation: Option<Isolation>,
    pub job_name: Option<String>,
    pub score: Option<f64>,
    pub total: Option<f64>,
    /// The parsed results as JSON text.
    pub results: Option<String>,
    pub problems: Vec<String>,
    pub error: Option<String>,
    pub log_tail: Option<String>,
    /// For regrades: the difference from the previous graded run, as JSON text.
    pub changes: Option<String>,
    pub notify: bool,
}

/// A test run waiting for (or just given) admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueuedRun {
    /// Validation of this assignment build.
    Validation(i64),
    /// This grading run.
    Grading(i64),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn assignment_statuses_roundtrip_lowercase() {
        for s in [
            AssignmentBuildStatus::Running,
            AssignmentBuildStatus::Succeeded,
            AssignmentBuildStatus::Failed,
            AssignmentBuildStatus::Superseded,
        ] {
            assert_eq!(AssignmentBuildStatus::from_str(&s.to_string()).unwrap(), s);
        }
        assert_eq!(AssignmentBuildStatus::Superseded.to_string(), "superseded");
        for s in [
            ValidationStatus::Pending,
            ValidationStatus::Queued,
            ValidationStatus::Running,
            ValidationStatus::Passed,
            ValidationStatus::Failed,
            ValidationStatus::Skipped,
            ValidationStatus::Invalid,
        ] {
            assert_eq!(ValidationStatus::from_str(&s.to_string()).unwrap(), s);
        }
    }

    #[test]
    fn only_passed_and_skipped_validations_are_usable() {
        use ValidationStatus::*;
        let usable: Vec<_> = [Pending, Queued, Running, Passed, Failed, Skipped, Invalid]
            .into_iter()
            .filter(|s| s.is_usable())
            .collect();
        assert_eq!(usable, [Passed, Skipped]);
    }

    #[test]
    fn grading_enums_roundtrip_lowercase() {
        for s in [
            RunStatus::Queued,
            RunStatus::Grading,
            RunStatus::Graded,
            RunStatus::Error,
        ] {
            assert_eq!(RunStatus::from_str(&s.to_string()).unwrap(), s);
        }
        assert_eq!(RunReason::Regrade.to_string(), "regrade");
        assert_eq!(SubmissionSource::Workspace.to_string(), "workspace");
    }

    #[test]
    fn build_status_serializes_lowercase() {
        assert_eq!(BuildStatus::Running.to_string(), "running");
        assert_eq!(BuildStatus::Succeeded.to_string(), "succeeded");
        assert_eq!(BuildStatus::Failed.to_string(), "failed");
    }

    #[test]
    fn build_status_roundtrips() {
        for s in [
            BuildStatus::Running,
            BuildStatus::Succeeded,
            BuildStatus::Failed,
        ] {
            assert_eq!(BuildStatus::from_str(&s.to_string()).unwrap(), s);
        }
    }

    #[test]
    fn build_status_rejects_unknown() {
        assert!(BuildStatus::from_str("bogus").is_err());
    }
}
