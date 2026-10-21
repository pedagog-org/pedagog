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
