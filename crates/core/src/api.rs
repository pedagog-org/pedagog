//! The api's request and response bodies, shared with the web UI so both
//! sides agree on them at compile time. Endpoints move here one by one; the
//! JSON they produce doesn't change.

use serde::{Deserialize, Serialize};

/// A person, as listings show them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserView {
    pub email: String,
    pub name: String,
    /// Their global role: `admin`, `staff` or `student`.
    pub role: String,
}

/// One of the caller's courses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyCourse {
    pub id: String,
    pub name: String,
    /// `instructor`, `grader` or `student`.
    pub role: String,
}

/// `GET /api/me`: who's logged in, and their courses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    pub user: UserView,
    pub courses: Vec<MyCourse>,
}

/// `POST /api/client-errors`: something unexpected in the web UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientError {
    /// The page's path (no query string).
    pub path: String,
    pub message: String,
    /// The UI's build, e.g. `pedagog-web@0.0.0`.
    pub version: String,
}

/// When an assignment takes submissions, as a value to badge (the sentence
/// beside it says why).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    /// No dates anywhere: it isn't open.
    NoDates,
    /// It opens later.
    Upcoming,
    Open,
    /// Past due, inside the late window.
    OpenLate,
    /// Past the last deadline.
    Closed,
}

/// The dates in effect (RFC 3339; `open` may also be `always`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatesView {
    pub open: Option<String>,
    pub due: Option<String>,
    pub late_due: Option<String>,
}

/// A grading run's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Queued,
    Grading,
    Graded,
    Error,
}

/// The score over the tests a student may see now.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shown {
    pub score: f64,
    pub total: f64,
}

/// A student's own standing on an assignment: the submission that counts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mine {
    pub submission: i64,
    pub status: RunState,
    /// When graded.
    pub shown: Option<Shown>,
    /// "late by …".
    pub late: Option<String>,
    /// How many they've sent.
    pub submissions: usize,
}

/// An assignment as a student sees it in lists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StudentAssignment {
    pub slug: String,
    pub name: String,
    pub dates: DatesView,
    pub window: Window,
    /// The window in words: "open", "open, late", "it opens at …".
    pub status: String,
    /// Whether a submission now would be taken (open, with a usable build).
    pub accepting: bool,
    /// `None` until they submit.
    pub mine: Option<Mine>,
}

/// What a submission must be, for checking files before sending them. The
/// server applies the full rules and has the last word.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitRules {
    /// Paths that must be present, after unpacking and stripping.
    pub required: Vec<String>,
    /// The most a submission may total, in bytes.
    pub max_bytes: Option<u64>,
    /// A single .zip / .tar.gz is unpacked into the submission.
    pub unpack_archives: bool,
    /// A folder wrapping every file is dropped.
    pub strip_wrapper: bool,
}

/// `GET /api/courses/{c}/assignments/{a}` for a student.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StudentAssignmentPage {
    #[serde(flatten)]
    pub assignment: StudentAssignment,
    /// `None` without a usable build.
    pub rules: Option<SubmitRules>,
}

/// One of your courses on Home, with what needs you.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HomeCourse {
    pub id: String,
    pub name: String,
    pub role: String,
    /// For students: open or due soon, soonest first.
    pub items: Vec<StudentAssignment>,
    /// For staff: what needs their attention.
    #[serde(default)]
    pub staff_items: Vec<StaffItem>,
}

/// Something in a course that needs its staff, on Home.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StaffItem {
    /// The latest build failed and nothing newer is on the way (instructors).
    BuildFailed {
        slug: String,
        name: String,
        build: i64,
        /// It built, but the reference solution disagreed with the tests.
        validation: bool,
    },
    /// Students whose submission that counts couldn't be graded.
    NotGraded {
        slug: String,
        name: String,
        count: i64,
    },
    /// Due within a week.
    DueSoon {
        slug: String,
        name: String,
        due: String,
        submitted: i64,
        students: i64,
    },
    /// A build on its way.
    Building {
        slug: String,
        name: String,
        build: i64,
        validating: bool,
    },
    Regrading {
        slug: String,
        name: String,
        done: i64,
        total: i64,
    },
}

/// `GET /api/me/courses`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MyCourses {
    pub courses: Vec<HomeCourse>,
}

/// What intake did with a submission (stored, and returned to the submitter).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// The files submitted, after normalization.
    pub files: Vec<String>,
    /// Files the rules left out.
    pub ignored: Vec<String>,
    /// OS clutter dropped (`__MACOSX/`, `.DS_Store`, ...).
    pub junk: usize,
    /// The archive that was unpacked, if any.
    pub unpacked: Option<String>,
    /// The wrapping folder that was dropped, if any.
    pub stripped: Option<String>,
    pub size_bytes: u64,
}

/// Jobs' reply to a submission: stored and queued.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submitted {
    pub submission_id: i64,
    pub run_id: i64,
    pub build_id: i64,
    pub tree_hash: String,
    /// 1-based position in the grading queue.
    pub queue_position: Option<i64>,
    #[serde(flatten)]
    pub receipt: Receipt,
}

/// `POST …/submissions`: jobs' reply, and how it counts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitReply {
    #[serde(flatten)]
    pub submitted: Submitted,
    /// The owner's email.
    #[serde(rename = "for")]
    pub for_: String,
    /// Who sent it, when someone else did.
    pub submitted_by: Option<String>,
    pub staff: bool,
    pub counted_at: String,
    /// "late by …".
    pub late: Option<String>,
    pub due: Option<String>,
}

/// One of a student's submissions to an assignment, latest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmissionRow {
    pub id: i64,
    pub counted_at: String,
    pub late: Option<String>,
    pub submitted_by: Option<String>,
    pub staff: bool,
    /// Its latest run's.
    pub status: Option<RunState>,
    pub counts: bool,
    pub activated_by: Option<String>,
    /// What the student sees of its score now, once graded.
    pub shown: Option<Shown>,
    pub files: usize,
    pub size_bytes: u64,
    /// Staff only, from here on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
}

/// `GET …/my-results`: your submission (the one that counts, or one asked
/// for), and its results as you may see them now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MyResults {
    pub submission: i64,
    pub counts: bool,
    /// Staff made it the one that counts.
    pub activated: bool,
    pub counted_at: String,
    pub submitted_by: Option<String>,
    pub late: Option<String>,
    pub status: RunState,
    /// While queued.
    pub queue_position: Option<i64>,
    /// Why it couldn't be graded, and what to do.
    pub why: Option<String>,
    pub next: Option<String>,
    /// What stopped it, sanitized, when the helper says (e.g. a syntax error's
    /// file, line and message).
    #[serde(default)]
    pub detail: Option<String>,
    /// When graded.
    pub results: Option<crate::view::StudentView>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn me_round_trips() {
        let me = Me {
            user: UserView {
                email: "a@ufl.edu".into(),
                name: "Ana".into(),
                role: "student".into(),
            },
            courses: vec![MyCourse {
                id: "cop3502-f26".into(),
                name: "Programming 1".into(),
                role: "student".into(),
            }],
        };
        let json = serde_json::to_string(&me).unwrap();
        assert!(json.contains(r#""courses":[{"id":"cop3502-f26""#), "{json}");
        assert_eq!(serde_json::from_str::<Me>(&json).unwrap(), me);
    }
}

/// `POST …/builds/{id}/current`: what making an earlier build current did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MadeCurrent {
    /// It's current now; `cancelled` is the build that was in flight.
    Current { cancelled: Option<i64> },
    /// Its image was cleaned up, so it's building again as `build`, which
    /// becomes current once it validates.
    Rebuilding { build: i64, cancelled: Option<i64> },
}

/// A build attempt as staff lists show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildSummary {
    pub id: i64,
    /// `running`, `succeeded`, `failed` or `superseded`.
    pub build_status: String,
    /// `pending`, `running`, `passed`, `failed`, `skipped` or `invalid`.
    pub validation_status: String,
    pub finished: bool,
    pub current: bool,
    /// When it started (RFC 3339).
    pub started_at: String,
    /// Who started it (an upload, or making an old build current).
    pub uploaded_by: Option<String>,
    /// The build whose upload it built again.
    pub rebuild_of: Option<i64>,
    /// Cleanup deleted its image: making it current rebuilds it first.
    pub image_deleted: bool,
    /// It built and validated, and isn't current.
    pub usable: bool,
}

/// Where an assignment's students stand.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentCounts {
    /// Active students in the course.
    pub students: i64,
    /// Of them, those with a submission.
    pub submitted: i64,
    /// Of them, those whose submission that counts couldn't be graded.
    pub not_graded: i64,
    /// The latest regrade, while it runs.
    pub regrading: Option<Progress>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub done: i64,
    pub total: i64,
}

/// An assignment in the staff list (`GET /api/courses/{c}/assignments`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaffAssignment {
    pub slug: String,
    pub name: String,
    pub dates: DatesView,
    pub window: Window,
    pub status: String,
    pub current_build: Option<i64>,
    pub latest_build: Option<BuildSummary>,
    pub counts: AssignmentCounts,
}

/// `GET /api/courses/{c}/assignments/{a}` for staff: the overview.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaffAssignmentPage {
    pub course: String,
    #[serde(flatten)]
    pub assignment: StaffAssignment,
    /// When the current build became current (its validation's end).
    pub current_since: Option<String>,
    pub released_at: Option<String>,
    /// The current build's test helper doesn't sanitize errors.
    pub students_see_raw_errors: bool,
    /// The latest builds, newest first (`…/builds` pages through the rest).
    pub builds: Vec<BuildSummary>,
}

/// A test of a validation run: what the reference solution did, against
/// what the test declares.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReferenceTest {
    #[serde(flatten)]
    pub test: crate::view::ShownTest,
    pub expected: crate::results::Expect,
    pub as_expected: bool,
}

/// `GET /api/courses/{c}/assignments/{a}/builds/{id}` (staff).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildPage {
    #[serde(flatten)]
    pub build: BuildSummary,
    pub course: String,
    pub assignment: String,
    pub image_ref: String,
    /// A failure in a line.
    pub error: Option<String>,
    pub validation_problems: Vec<String>,
    pub validation_warnings: Vec<String>,
    pub build_ms: Option<i64>,
    pub validation_ms: Option<i64>,
    /// Each phase's log length, while its log is kept (`…/log?phase=`).
    pub build_log_lines: Option<i32>,
    pub validation_log_lines: Option<i32>,
    /// The reference solution's results, every part shown.
    pub tests: Vec<ReferenceTest>,
    /// Over every test.
    pub score: Option<f64>,
    pub total: Option<f64>,
}

/// `GET …/builds`: a page of builds, newest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildsPage {
    pub builds: Vec<BuildSummary>,
    /// Pass as `before` for the next page.
    pub next: Option<i64>,
}

/// One of a validation's date differences: the course's (in use) against the file's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateDiff {
    pub field: String,
    pub course: Option<String>,
    pub file: Option<String>,
}

/// `POST /api/courses/{c}/assignments`: what the upload started.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Uploaded {
    pub course: String,
    pub assignment: String,
    pub upload_id: i64,
    pub tree_hash: String,
    pub size_bytes: i64,
    pub warnings: Vec<String>,
    /// `{"status": "dispatched", "build_id": n}`.
    pub build: serde_json::Value,
    pub policy: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub date_diff: Vec<DateDiff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dates_note: Option<String>,
}
