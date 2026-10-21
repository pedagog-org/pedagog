//! Times as people read them: in a time zone (the course's, once courses have
//! one; until then the viewer's), with the zone named, and relative to now
//! ("in 3 days", "2 hours ago"). Formatting uses the browser's `Intl`, so no
//! time zone data ships in the app.

use js_sys::{Array, Date, Intl, Object, Reflect};
use wasm_bindgen::JsValue;

/// Milliseconds since the epoch of an RFC 3339 time, if it parses.
pub fn millis(rfc3339: &str) -> Option<f64> {
    let ms = Date::parse(rfc3339);
    (!ms.is_nan()).then_some(ms)
}

pub fn now_millis() -> f64 {
    Date::now()
}

fn formatted(ms: f64, options: &[(&str, &str)]) -> Option<String> {
    let o = Object::new();
    for (k, v) in options {
        let _ = Reflect::set(&o, &JsValue::from_str(k), &JsValue::from_str(v));
    }
    Intl::DateTimeFormat::new(&Array::of1(&JsValue::from_str("en-US")), &o)
        .format()
        .call1(&JsValue::NULL, &Date::new(&JsValue::from_f64(ms)))
        .ok()
        .and_then(|s| s.as_string())
}

/// A moment as the pages show it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Moment {
    pub ms: f64,
    /// `Fri, Oct 23`
    pub day: String,
    /// `Fri, Oct 23 · 11:59 PM`
    pub text: String,
    /// `Eastern Daylight Time`, for a hover title.
    pub zone: String,
}

/// An RFC 3339 time in the viewer's zone, if it parses.
pub fn moment(rfc3339: &str) -> Option<Moment> {
    let ms = millis(rfc3339)?;
    let day = formatted(
        ms,
        &[("weekday", "short"), ("month", "short"), ("day", "numeric")],
    )?;
    let clock = formatted(ms, &[("hour", "numeric"), ("minute", "2-digit")])?;
    let zone = formatted(ms, &[("timeZoneName", "long")])
        .and_then(|s| s.split_once(", ").map(|(_, z)| z.to_owned()))
        .unwrap_or_default();
    Some(Moment {
        ms,
        text: format!("{day} · {clock}"),
        day,
        zone,
    })
}

/// The viewer's zone, short (`EDT`) and long (`Eastern Daylight Time`), now.
pub fn zone_now() -> (String, String) {
    let ms = now_millis();
    let name = |style| {
        formatted(ms, &[("timeZoneName", style)])
            .and_then(|s| s.split_once(", ").map(|(_, z)| z.to_owned()))
            .unwrap_or_default()
    };
    (name("short"), name("long"))
}

/// A clock time: `3:05 PM`.
pub fn clock(rfc3339: &str) -> String {
    millis(rfc3339)
        .and_then(|ms| formatted(ms, &[("hour", "numeric"), ("minute", "2-digit")]))
        .unwrap_or_default()
}

/// How far `then` is from `now` (both in milliseconds), as people say it:
/// "in 3 days", "2 hours ago", "in a minute", "just now".
pub fn relative(then: f64, now: f64) -> String {
    let seconds = (then - now) / 1000.0;
    let ahead = seconds >= 0.0;
    let s = seconds.abs();
    if s < 45.0 {
        return "just now".into();
    }
    let (n, unit) = if s < 90.0 {
        (1.0, "minute")
    } else if s < 3600.0 {
        ((s / 60.0).round(), "minute")
    } else if s < 86_400.0 {
        ((s / 3600.0).round(), "hour")
    } else if s < 86_400.0 * 30.0 {
        ((s / 86_400.0).round(), "day")
    } else if s < 86_400.0 * 365.0 {
        ((s / (86_400.0 * 30.0)).round(), "month")
    } else {
        ((s / (86_400.0 * 365.0)).round(), "year")
    };
    let phrase = if n == 1.0 {
        format!("{} {unit}", if unit == "hour" { "an" } else { "a" })
    } else {
        format!("{n} {unit}s")
    };
    if ahead {
        format!("in {phrase}")
    } else {
        format!("{phrase} ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: f64 = 60_000.0;

    #[test]
    fn relative_times_read_naturally() {
        assert_eq!(relative(10_000.0, 0.0), "just now");
        assert_eq!(relative(MIN, 0.0), "in a minute");
        assert_eq!(relative(0.0, 25.0 * MIN), "25 minutes ago");
        assert_eq!(relative(60.0 * MIN, 0.0), "in an hour");
        assert_eq!(relative(3.0 * 24.0 * 60.0 * MIN, 0.0), "in 3 days");
        assert_eq!(
            relative(0.0, 2.0 * 30.0 * 24.0 * 60.0 * MIN),
            "2 months ago"
        );
    }
}
