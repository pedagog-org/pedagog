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
