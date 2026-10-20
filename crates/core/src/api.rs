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
