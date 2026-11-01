//! The `assignment:` block: when submissions are accepted (`dates`).
//! Everything is optional; the file's values are defaults that a course can
//! edit field by field. What students see of their results is the tests'
//! own (`@visibility`); `results:` here is retired.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::results::Visibility;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentPolicy {
    #[serde(default)]
    pub dates: Dates,
    /// Retired (visibility lives in the tests): read, and ignored, in builds
    /// stored before; refused in uploads (see [`AssignmentPolicy::problems`]).
    #[serde(default, skip_serializing)]
    pub results: Option<Retired>,
}

impl AssignmentPolicy {
    /// What an upload's `assignment:` block may no longer say.
    pub fn problems(&self) -> Vec<String> {
        if self.results.is_some() {
            vec![
                "assignment.results: visibility lives in the tests now (@visibility on each \
                 test); remove assignment.results and upload again"
                    .to_owned(),
            ]
        } else {
            Vec::new()
        }
    }
}

/// A retired block, whatever it held.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Retired;

impl<'de> Deserialize<'de> for Retired {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(d)?;
        Ok(Retired)
    }
}

/// When submissions are accepted. Each field is optional; with none set
/// anywhere, the assignment is closed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, try_from = "RawDates")]
pub struct Dates {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<Open>,
    /// After this, submissions are late.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<Instant>,
    /// After this, submissions are refused (default: `due`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub late_due: Option<Instant>,
}

/// A course's edits to an assignment's dates, field by field. Unlike
/// [`Dates`], they needn't make sense alone (a `late_due` over the file's
/// `due`); the merged dates are checked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DateEdits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<Open>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<Instant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late_due: Option<Instant>,
}

/// When submissions open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub enum Open {
    /// From now on (an open-ended assignment, unless it has a due date).
    Always,
    At(Instant),
}

/// A moment written in RFC 3339 with its offset (`2026-09-08T23:59:00-04:00`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct Instant(pub OffsetDateTime);

impl TryFrom<String> for Instant {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        OffsetDateTime::parse(&s, &Rfc3339).map(Instant).map_err(|_| {
            format!(
                "{s:?} isn't a date with an offset (RFC 3339), e.g. \"2026-09-08T23:59:00-04:00\""
            )
        })
    }
}

impl From<Instant> for String {
    fn from(i: Instant) -> Self {
        i.0.format(&Rfc3339).unwrap_or_default()
    }
}

impl TryFrom<String> for Open {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        if s == "always" {
            Ok(Open::Always)
        } else {
            Instant::try_from(s).map(Open::At)
        }
    }
}

impl From<Open> for String {
    fn from(o: Open) -> Self {
        match o {
            Open::Always => "always".to_owned(),
            Open::At(i) => i.into(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDates {
    open: Option<Open>,
    due: Option<Instant>,
    late_due: Option<Instant>,
}

impl TryFrom<RawDates> for Dates {
    type Error = String;

    fn try_from(raw: RawDates) -> Result<Self, Self::Error> {
        Dates {
            open: raw.open,
            due: raw.due,
            late_due: raw.late_due,
        }
        .checked()
    }
}

/// Whether a submission is accepted now, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acceptance {
    OnTime,
    Late,
    /// Refused, with the reason to show.
    Closed(String),
}

impl Dates {
    /// The dates make sense: a late window needs a due date, and the order
    /// is open ≤ due ≤ late_due.
    pub fn checked(self) -> Result<Self, String> {
        if self.late_due.is_some() && self.due.is_none() {
            return Err("late_due needs a due date".to_owned());
        }
        if let (Some(Open::At(open)), Some(due)) = (self.open, self.due)
            && open > due
        {
            return Err("open is after due".to_owned());
        }
        if let (Some(due), Some(late)) = (self.due, self.late_due)
            && due > late
        {
            return Err("due is after late_due".to_owned());
        }
        Ok(self)
    }

    /// The course's edits over the file's defaults, field by field (unchecked:
    /// call [`checked`](Self::checked) before saving edits).
    pub fn merged(file: &Dates, course: &DateEdits) -> Dates {
        Dates {
            open: course.open.or(file.open),
            due: course.due.or(file.due),
            late_due: course.late_due.or(file.late_due),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_none() && self.due.is_none() && self.late_due.is_none()
    }

    /// The last moment submissions are accepted (`late_due`, else `due`), if any.
    pub fn end(&self) -> Option<OffsetDateTime> {
        self.late_due.or(self.due).map(|i| i.0)
    }

    /// Whether a submission at `now` is accepted.
    pub fn acceptance(&self, now: OffsetDateTime) -> Acceptance {
        if self.is_empty() {
            return Acceptance::Closed("it isn't open yet: no dates set".to_owned());
        }
        match self.open {
            Some(Open::At(open)) if now < open.0 => {
                return Acceptance::Closed(format!("it opens at {}", String::from(open)));
            }
            // A due date alone opens it right away.
            None | Some(_) => {}
        }
        if let Some(end) = self.end()
            && now > end
        {
            return Acceptance::Closed("it's closed: the deadline has passed".to_owned());
        }
        match self.due {
            Some(due) if now > due.0 => Acceptance::Late,
            _ => Acceptance::OnTime,
        }
    }

    /// [`acceptance`](Self::acceptance) as a value to badge.
    pub fn window(&self, now: OffsetDateTime) -> crate::api::Window {
        use crate::api::Window;
        if self.is_empty() {
            return Window::NoDates;
        }
        match (self.open, self.acceptance(now)) {
            (_, Acceptance::OnTime) => Window::Open,
            (_, Acceptance::Late) => Window::OpenLate,
            (Some(Open::At(open)), Acceptance::Closed(_)) if now < open.0 => Window::Upcoming,
            (_, Acceptance::Closed(_)) => Window::Closed,
        }
    }

    /// Whether `after_end` results are visible at `now`.
    pub fn ended(&self, now: OffsetDateTime) -> bool {
        self.end().is_some_and(|end| now > end)
    }

    /// How far past the due date a submission counted at `at` is (`None` if
    /// on time, or there's no due date).
    pub fn late_by(&self, at: OffsetDateTime) -> Option<time::Duration> {
        let due = self.due?.0;
        (at > due).then(|| at - due)
    }
}

/// A duration in its two largest units, e.g. "2 hours 14 minutes", "4 days".
pub fn spoken(d: time::Duration) -> String {
    let secs = d.whole_seconds().max(0);
    let units = [
        (secs / 86_400, "day"),
        (secs / 3_600 % 24, "hour"),
        (secs / 60 % 60, "minute"),
        (secs % 60, "second"),
    ];
    let first = units.iter().position(|(n, _)| *n > 0).unwrap_or(3);
    units[first..]
        .iter()
        .take(2)
        .enumerate()
        .filter(|(i, (n, _))| *i == 0 || *n > 0)
        .map(|(_, (n, unit))| format!("{n} {unit}{}", if *n == 1 { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether students see something now, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Shown {
    pub shown: bool,
    pub why: String,
}

impl Visibility {
    /// [`shown`](Self::shown) with the reason, for instructors inspecting
    /// what students see. `end` is the last accepted time, if there is one.
    pub fn shown_now(
        self,
        end: Option<OffsetDateTime>,
        now: OffsetDateTime,
        released: bool,
    ) -> Shown {
        let ended = end.is_some_and(|e| now > e);
        let end_text = end.map(|e| String::from(Instant(e)));
        let why = match self {
            Visibility::Visible => "always shown".to_owned(),
            Visibility::Hidden => "never shown to students".to_owned(),
            Visibility::AfterEnd if released => "shown: results are released".to_owned(),
            Visibility::AfterEnd => match (ended, end_text) {
                (true, Some(e)) => format!("shown: the assignment ended at {e}"),
                (false, Some(e)) => format!("not yet: until the end ({e}) or release"),
                (_, None) => "not yet: no end date, so only on release".to_owned(),
            },
            Visibility::AfterPublish if released => "shown: results are released".to_owned(),
            Visibility::AfterPublish => "not yet: until results are released".to_owned(),
        };
        Shown {
            shown: self.shown(ended, released),
            why,
        }
    }
}

impl Visibility {
    /// Whether a student sees this now: `after_end` once the assignment has
    /// ended (or results are released), `after_publish` only once released.
    pub fn shown(self, ended: bool, released: bool) -> bool {
        match self {
            Visibility::Visible => true,
            Visibility::Hidden => false,
            Visibility::AfterEnd => ended || released,
            Visibility::AfterPublish => released,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(yaml: &str) -> Result<AssignmentPolicy, String> {
        serde_yaml::from_str(yaml).map_err(|e| e.to_string())
    }

    fn at(s: &str) -> OffsetDateTime {
        OffsetDateTime::parse(s, &Rfc3339).unwrap()
    }

    const DATES: &str = "dates:\n  open: \"2026-09-01T14:00:00-04:00\"\n  due: \"2026-09-08T23:59:00-04:00\"\n  late_due: \"2026-09-10T23:59:00-04:00\"\n";

    #[test]
    fn dates_parse_with_offsets_only() {
        let p = policy(DATES).unwrap();
        assert_eq!(p.dates.due.unwrap().0, at("2026-09-09T03:59:00Z"));
        let err = policy("dates:\n  due: \"2026-09-08T23:59:00\"\n").unwrap_err();
        assert!(err.contains("offset"), "{err}");
        assert_eq!(
            policy("dates:\n  open: always\n").unwrap().dates.open,
            Some(Open::Always)
        );
    }

    #[test]
    fn dates_must_be_in_order() {
        let err =
            policy("dates:\n  open: \"2026-09-09T00:00:00Z\"\n  due: \"2026-09-08T00:00:00Z\"\n")
                .unwrap_err();
        assert!(err.contains("open is after due"), "{err}");
        let err = policy("dates:\n  late_due: \"2026-09-08T00:00:00Z\"\n").unwrap_err();
        assert!(err.contains("needs a due date"), "{err}");
    }

    #[test]
    fn lateness_is_measured_from_the_due_date() {
        let d = policy(DATES).unwrap().dates;
        let due = d.due.unwrap().0;
        assert_eq!(d.late_by(due), None, "on the dot is on time");
        assert_eq!(
            d.late_by(due + time::Duration::minutes(134)),
            Some(time::Duration::minutes(134))
        );
        assert_eq!(Dates::default().late_by(due), None, "no due date");
    }

    #[test]
    fn durations_are_said_in_two_units() {
        let s = |secs: i64| spoken(time::Duration::seconds(secs));
        assert_eq!(s(134 * 60), "2 hours 14 minutes");
        assert_eq!(s(4 * 86_400), "4 days");
        assert_eq!(s(86_400 + 3 * 3600 + 59), "1 day 3 hours");
        assert_eq!(s(65), "1 minute 5 seconds");
        assert_eq!(s(1), "1 second");
        assert_eq!(s(0), "0 seconds");
        assert_eq!(
            s(86_400 + 59),
            "1 day",
            "the second unit, when zero, is left out"
        );
    }

    #[test]
    fn acceptance_follows_the_window() {
        let d = policy(DATES).unwrap().dates;
        assert!(
            matches!(d.acceptance(at("2026-08-31T12:00:00Z")), Acceptance::Closed(r) if r.contains("opens at"))
        );
        assert_eq!(d.acceptance(at("2026-09-05T12:00:00Z")), Acceptance::OnTime);
        assert_eq!(d.acceptance(at("2026-09-09T12:00:00Z")), Acceptance::Late);
        assert!(
            matches!(d.acceptance(at("2026-09-12T12:00:00Z")), Acceptance::Closed(r) if r.contains("deadline"))
        );
        assert!(!d.ended(at("2026-09-09T12:00:00Z")) && d.ended(at("2026-09-12T12:00:00Z")));
    }

    #[test]
    fn the_window_names_each_stage() {
        use crate::api::Window;
        let d = policy(DATES).unwrap().dates;
        assert_eq!(d.window(at("2026-08-31T12:00:00Z")), Window::Upcoming);
        assert_eq!(d.window(at("2026-09-05T12:00:00Z")), Window::Open);
        assert_eq!(d.window(at("2026-09-09T12:00:00Z")), Window::OpenLate);
        assert_eq!(d.window(at("2026-09-12T12:00:00Z")), Window::Closed);
        assert_eq!(
            Dates::default().window(at("2026-09-05T12:00:00Z")),
            Window::NoDates
        );
    }

    #[test]
    fn undated_is_closed_and_always_is_open_ended() {
        let now = at("2026-09-05T12:00:00Z");
        assert!(
            matches!(Dates::default().acceptance(now), Acceptance::Closed(r) if r.contains("no dates"))
        );
        let always = policy("dates:\n  open: always\n").unwrap().dates;
        assert_eq!(always.acceptance(now), Acceptance::OnTime);
        assert!(!always.ended(now), "no end");
    }

    #[test]
    fn course_edits_override_field_by_field() {
        let file = policy(DATES).unwrap().dates;
        let course = DateEdits {
            due: Some(Instant(at("2026-09-15T23:59:00-04:00"))),
            ..Default::default()
        };
        let merged = Dates::merged(&file, &course);
        assert_eq!(merged.open, file.open);
        assert_eq!(merged.due, course.due);
        assert_eq!(merged.late_due, file.late_due);

        // An edit that needs the file's due date reads back fine on its own.
        let late_only: DateEdits =
            serde_json::from_str(r#"{"late_due": "2026-09-20T23:59:00-04:00"}"#).unwrap();
        assert!(Dates::merged(&file, &late_only).checked().is_ok());
    }

    #[test]
    fn dates_round_trip() {
        let p = policy(DATES).unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<AssignmentPolicy>(&json).unwrap(), p);
    }

    #[test]
    fn a_results_block_is_read_but_refused() {
        let old = policy("results:\n  visibility: { error: hidden }\n").unwrap();
        assert_eq!(
            old.results,
            Some(Retired),
            "builds stored before still read"
        );
        assert!(old.problems()[0].contains("visibility lives in the tests"));
        assert!(
            policy("dates: { open: always }\n")
                .unwrap()
                .problems()
                .is_empty()
        );
        let stored = serde_json::to_string(&old).unwrap();
        assert!(!stored.contains("results"), "{stored}");
    }

    #[test]
    fn shown_now_says_why() {
        let end = Some(at("2026-09-10T23:59:00-04:00"));
        let before = at("2026-09-05T12:00:00Z");
        let after = at("2026-09-12T12:00:00Z");
        let s = Visibility::AfterEnd.shown_now(end, before, false);
        assert!(
            !s.shown && s.why.contains("until the end (2026-09-10T23:59:00-04:00)"),
            "{}",
            s.why
        );
        assert!(
            Visibility::AfterEnd
                .shown_now(end, after, false)
                .why
                .contains("ended at")
        );
        assert!(
            Visibility::AfterEnd
                .shown_now(None, after, false)
                .why
                .contains("only on release")
        );
        let s = Visibility::AfterPublish.shown_now(end, after, false);
        assert!(!s.shown && s.why.contains("until results are released"));
        assert!(Visibility::AfterPublish.shown_now(end, before, true).shown);
    }

    #[test]
    fn shown_depends_on_end_and_release() {
        assert!(Visibility::Visible.shown(false, false));
        assert!(!Visibility::Hidden.shown(true, true));
        assert!(!Visibility::AfterEnd.shown(false, false));
        assert!(Visibility::AfterEnd.shown(true, false) && Visibility::AfterEnd.shown(false, true));
        assert!(
            !Visibility::AfterPublish.shown(true, false)
                && Visibility::AfterPublish.shown(false, true)
        );
    }
}
