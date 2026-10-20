//! Which course a command means: `-c`, else the nearest `course.yml` at or
//! above the directory it works in, else the one saved with `hammer use`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const COURSE_FILE: &str = "course.yml";

/// A course repo's `course.yml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CourseFile {
    /// The course id, e.g. `cop3502-f26` (templated per semester).
    pub course: String,
    /// The api the course lives on, if the repo says.
    pub api: Option<String>,
}

/// Where the course came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Flag,
    File(PathBuf),
    Saved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    pub id: String,
    pub source: Source,
    /// The api named by the course file, if that's where the course came from.
    pub api: Option<String>,
}

impl Chosen {
    /// `(from ../course.yml)` and the like, for messages.
    pub fn origin(&self) -> String {
        match &self.source {
            Source::Flag => String::new(),
            Source::File(p) => format!(" (from {})", p.display()),
            Source::Saved => " (from `hammer use`)".to_owned(),
        }
    }
}

/// The nearest `course.yml` at or above `start`.
pub fn find_file(start: &Path) -> Result<Option<(PathBuf, CourseFile)>, String> {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    for dir in start.ancestors() {
        let path = dir.join(COURSE_FILE);
        if path.is_file() {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let file: CourseFile =
                serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            return Ok(Some((path, file)));
        }
    }
    Ok(None)
}

pub fn choose(flag: Option<String>, start: &Path, saved: Option<String>) -> Result<Chosen, String> {
    if let Some(id) = flag {
        return Ok(Chosen {
            id,
            source: Source::Flag,
            api: None,
        });
    }
    if let Some((path, file)) = find_file(start)? {
        return Ok(Chosen {
            id: file.course,
            source: Source::File(path),
            api: file.api,
        });
    }
    match saved {
        Some(id) => Ok(Chosen {
            id,
            source: Source::Saved,
            api: None,
        }),
        None => Err(
            "which course? pass -c <course>, add a course.yml to the course repo, or run `hammer use <course>`"
                .to_owned(),
        ),
    }
}

/// A course file that names another api than the session's is a mistake to
/// stop at, not guess around.
pub fn check_api(chosen: &Chosen, session_api: &str) -> Result<(), String> {
    match &chosen.api {
        Some(api) if api.trim_end_matches('/') != session_api.trim_end_matches('/') => {
            Err(format!(
                "{}{} is on {api}, but you're logged in to {session_api}: run `hammer login --api {api}`",
                chosen.id,
                chosen.origin()
            ))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str, course_yml: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("hammer-course-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("linked-list/tests")).unwrap();
        std::fs::write(root.join(COURSE_FILE), course_yml).unwrap();
        root
    }

    #[test]
    fn the_flag_then_the_nearest_file_then_the_saved_course() {
        let root = repo(
            "order",
            "course: cop3502-f26\napi: https://pedagog.example\n",
        );
        let deep = root.join("linked-list/tests");
        let flag = choose(Some("cop3502-s27".into()), &deep, Some("saved".into())).unwrap();
        assert_eq!(
            (flag.id.as_str(), &flag.source),
            ("cop3502-s27", &Source::Flag)
        );

        let file = choose(None, &deep, Some("saved".into())).unwrap();
        assert_eq!(file.id, "cop3502-f26");
        assert_eq!(file.api.as_deref(), Some("https://pedagog.example"));
        assert!(file.origin().contains("course.yml"), "{}", file.origin());

        let elsewhere =
            std::env::temp_dir().join(format!("hammer-course-none-{}", std::process::id()));
        std::fs::create_dir_all(&elsewhere).unwrap();
        // Only if no course.yml sits above the temp dir.
        if find_file(&elsewhere).unwrap().is_none() {
            assert_eq!(
                choose(None, &elsewhere, Some("saved".into()))
                    .unwrap()
                    .source,
                Source::Saved
            );
            assert!(
                choose(None, &elsewhere, None)
                    .unwrap_err()
                    .contains("hammer use")
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_bad_course_file_is_an_error_and_apis_must_match() {
        let root = repo("bad", "course: cop3502-f26\nsemester: fall\n");
        assert!(choose(None, &root, None).unwrap_err().contains("semester"));
        let _ = std::fs::remove_dir_all(&root);

        let root = repo("api", "course: cop3502-f26\napi: https://a.example/\n");
        let chosen = choose(None, &root, None).unwrap();
        assert!(check_api(&chosen, "https://a.example").is_ok());
        assert!(
            check_api(&chosen, "https://b.example")
                .unwrap_err()
                .contains("hammer login --api")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
