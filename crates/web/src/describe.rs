//! An assignment in words, as the student pages show it: its window as a
//! badge and a "when" line, and the student's own state as a badge. Times
//! come in already formatted ([`Moments`]), so this is plain logic.

use pedagog_core::api::{Mine, RunState, Shown, StudentAssignment, Window};

use crate::time::{Moment, relative};

/// A badge: its class (`b-pass`, …), glyph and words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Badge {
    pub class: &'static str,
    pub glyph: &'static str,
    pub text: String,
}

fn badge(class: &'static str, glyph: &'static str, text: impl Into<String>) -> Badge {
    Badge {
        class,
        glyph,
        text: text.into(),
    }
}

/// The assignment's dates, formatted (`open` is `None` for "always").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Moments {
    pub open: Option<Moment>,
    pub due: Option<Moment>,
    pub late_due: Option<Moment>,
}

/// "9/10" without trailing zeros: `9.5/10`.
pub fn points(n: f64) -> String {
    let rounded = (n * 100.0).round() / 100.0;
    if rounded.fract() == 0.0 {
        format!("{rounded:.0}")
    } else {
        format!("{rounded}")
    }
}

pub fn score(s: Shown) -> String {
    format!("{}/{}", points(s.score), points(s.total))
}

/// The window, short: `Open`, `Open, late`, `Opens Mon, Oct 26`, `Closed`.
pub fn window_badge(window: Window, m: &Moments) -> Badge {
    match window {
        Window::Open => badge("b-pass", "●", "Open"),
        Window::OpenLate => badge("b-warn", "!", "Open, late"),
        Window::Upcoming => badge(
            "b-info",
            "○",
            m.open
                .as_ref()
                .map_or("Opens later".to_owned(), |o| format!("Opens {}", o.day)),
        ),
        Window::NoDates => badge("", "–", "No dates set"),
        Window::Closed => badge("", "■", "Closed"),
    }
}

/// When it's due, opens or closed, as one line.
pub fn when_line(window: Window, m: &Moments, now: f64) -> String {
    let rel = |x: &Moment| relative(x.ms, now);
    match window {
        Window::Open => match &m.due {
            Some(due) => {
                let mut line = format!("Due {} · {}", due.text, rel(due));
                if let Some(late) = &m.late_due {
                    line.push_str(&format!(" · late until {}", late.day));
                }
                line
            }
            None => "Open, no deadline".to_owned(),
        },
        Window::OpenLate => {
            let due = m.due.as_ref().map(|d| d.day.clone()).unwrap_or_default();
            match &m.late_due {
                Some(late) => format!("Was due {due} · late until {}", late.text),
                None => format!("Was due {due}"),
            }
        }
        Window::Upcoming => match &m.open {
            Some(o) => format!("Opens {} · {}", o.text, rel(o)),
            None => "Opens later".to_owned(),
        },
        Window::Closed => match m.late_due.as_ref().or(m.due.as_ref()) {
            Some(end) => format!("Closed: the deadline passed {}", end.day),
            None => "Closed".to_owned(),
        },
        Window::NoDates => "Not open yet: no dates set".to_owned(),
    }
}

/// The student's own state on it.
pub fn mine_badge(a: &StudentAssignment) -> Badge {
    match &a.mine {
        None if matches!(a.window, Window::Upcoming | Window::NoDates) => {
            badge("", "–", "Not open yet")
        }
        None => badge("", "–", "Not submitted"),
        Some(m) => run_badge(m),
    }
}

fn run_badge(m: &Mine) -> Badge {
    match (m.status, m.shown) {
        (RunState::Queued | RunState::Grading, _) => badge("b-info", "●", "Grading"),
        (RunState::Error, _) => badge("b-fail", "✕", "Couldn’t be graded"),
        (RunState::Graded, Some(s)) => {
            badge("b-pass", "✓", format!("{} on the tests shown", score(s)))
        }
        (RunState::Graded, None) => badge("b-pass", "✓", "Graded"),
    }
}

/// A run's state, for history rows (`9/10 shown`).
pub fn history_badge(status: Option<RunState>, shown: Option<Shown>) -> Badge {
    match (status, shown) {
        (None | Some(RunState::Queued), _) => badge("b-info", "●", "Queued"),
        (Some(RunState::Grading), _) => badge("b-info", "●", "Grading"),
        (Some(RunState::Error), _) => badge("b-fail", "✕", "Couldn’t be graded"),
        (Some(RunState::Graded), Some(s)) => badge("b-pass", "✓", format!("{} shown", score(s))),
        (Some(RunState::Graded), None) => badge("b-pass", "✓", "Graded"),
    }
}

/// `8.1 KB`, `1.2 MB`.
pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if b < 1024.0 {
        format!("{bytes} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use pedagog_core::api::DatesView;

    use super::*;

    const DAY: f64 = 86_400_000.0;

    fn at(ms: f64, day: &str, clock: &str) -> Option<Moment> {
        Some(Moment {
            ms,
            day: day.into(),
            text: format!("{day} · {clock}"),
            zone: String::new(),
        })
    }

    fn assignment(window: Window, mine: Option<Mine>) -> StudentAssignment {
        StudentAssignment {
            slug: "a".into(),
            name: "A".into(),
            dates: DatesView::default(),
            window,
            status: String::new(),
            accepting: true,
            mine,
        }
    }

    #[test]
    fn the_window_reads_as_the_design_writes_it() {
        let m = Moments {
            open: at(-DAY, "Mon, Oct 12", "9:00 AM"),
            due: at(3.0 * DAY, "Fri, Oct 23", "11:59 PM"),
            late_due: at(5.0 * DAY, "Sun, Oct 25", "11:59 PM"),
        };
        assert_eq!(
            when_line(Window::Open, &m, 0.0),
            "Due Fri, Oct 23 · 11:59 PM · in 3 days · late until Sun, Oct 25"
        );
        assert_eq!(
            when_line(Window::OpenLate, &m, 4.0 * DAY),
            "Was due Fri, Oct 23 · late until Sun, Oct 25 · 11:59 PM"
        );
        assert_eq!(
            when_line(Window::Closed, &m, 9.0 * DAY),
            "Closed: the deadline passed Sun, Oct 25"
        );
        let upcoming = Moments {
            open: at(6.0 * DAY, "Mon, Oct 26", "9:00 AM"),
            ..Default::default()
        };
        assert_eq!(
            when_line(Window::Upcoming, &upcoming, 0.0),
            "Opens Mon, Oct 26 · 9:00 AM · in 6 days"
        );
        assert_eq!(
            window_badge(Window::Upcoming, &upcoming).text,
            "Opens Mon, Oct 26"
        );
        assert_eq!(
            when_line(Window::Open, &Moments::default(), 0.0),
            "Open, no deadline"
        );
        assert_eq!(window_badge(Window::OpenLate, &m).class, "b-warn");
    }

    #[test]
    fn your_state_says_what_happened_to_the_one_that_counts() {
        let mine = |status, shown| {
            Some(Mine {
                submission: 1,
                status,
                shown,
                late: None,
                submissions: 1,
            })
        };
        assert_eq!(
            mine_badge(&assignment(Window::Open, None)).text,
            "Not submitted"
        );
        assert_eq!(
            mine_badge(&assignment(Window::Upcoming, None)).text,
            "Not open yet"
        );
        let graded = assignment(
            Window::Open,
            mine(
                RunState::Graded,
                Some(Shown {
                    score: 9.0,
                    total: 10.0,
                }),
            ),
        );
        assert_eq!(mine_badge(&graded).text, "9/10 on the tests shown");
        assert_eq!(
            mine_badge(&assignment(Window::Open, mine(RunState::Queued, None))).text,
            "Grading"
        );
        assert_eq!(
            mine_badge(&assignment(Window::Open, mine(RunState::Error, None))).class,
            "b-fail"
        );
        assert_eq!(
            history_badge(
                Some(RunState::Graded),
                Some(Shown {
                    score: 7.5,
                    total: 10.0
                })
            )
            .text,
            "7.5/10 shown"
        );
    }

    #[test]
    fn sizes_and_points_read_plainly() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(8_300), "8.1 KB");
        assert_eq!(size(10 << 20), "10.0 MB");
        assert_eq!(points(9.0), "9");
        assert_eq!(points(2.333), "2.33");
    }
}
