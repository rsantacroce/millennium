//! The world clock: a 40-minute day, computed from wall-clock time.
//!
//! Every server and client works out the same hour from their own clock, so nothing needs to
//! be synchronised (clocks a few seconds apart don't matter at this scale).

/// Real seconds per game day.
pub const DAY_SECS: f64 = 40.0 * 60.0;

/// The hour of the day (0.0..24.0) at this Unix time in milliseconds.
pub fn hour(now_ms: i64) -> f32 {
    let secs = now_ms as f64 / 1000.0 + shift_hours() * DAY_SECS / 24.0;
    (secs.rem_euclid(DAY_SECS) / DAY_SECS * 24.0) as f32
}

/// For testing: `MILLENNIUM_CLOCK_SHIFT_HOURS=12` moves the game clock 12 hours on (set the
/// same value for the server and its clients).
fn shift_hours() -> f64 {
    static SHIFT: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *SHIFT.get_or_init(|| std::env::var("MILLENNIUM_CLOCK_SHIFT_HOURS").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0))
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// How bright it is: 1 at midday, 0 at night, with an hour of dawn (6-7) and dusk (19-20).
pub fn daylight(hour: f32) -> f32 {
    let ramp = |from: f32| ((hour - from).clamp(0.0, 1.0) * std::f32::consts::PI / 2.0).sin();
    match hour {
        h if !(6.0..20.0).contains(&h) => 0.0,
        h if h < 7.0 => ramp(6.0),
        h if h < 19.0 => 1.0,
        _ => 1.0 - ramp(19.0),
    }
}

pub fn is_night(hour: f32) -> bool {
    daylight(hour) < 0.25
}

/// Is `hour` within `from..to`? Wraps past midnight (e.g. 20..4).
pub fn within(hours: (f32, f32), hour: f32) -> bool {
    let (from, to) = hours;
    if from <= to { (from..to).contains(&hour) } else { hour >= from || hour < to }
}

/// "9:45 PM"
pub fn label(hour: f32) -> String {
    let minutes = (hour * 60.0) as u32 % (24 * 60);
    let (h, m) = (minutes / 60, minutes % 60);
    let (h12, ampm) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    format!("{h12}:{m:02} {ampm}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_wraps() {
        assert_eq!(hour(0), 0.0);
        let half = (DAY_SECS * 500.0) as i64;
        assert!((hour(half) - 12.0).abs() < 1e-3);
        assert!(hour(i64::MAX / 2) < 24.0);
    }

    #[test]
    fn daylight_curve() {
        assert_eq!(daylight(3.0), 0.0);
        assert_eq!(daylight(12.0), 1.0);
        assert!(daylight(6.5) > 0.0 && daylight(6.5) < 1.0);
        assert!(daylight(19.5) > 0.0 && daylight(19.5) < 1.0);
        assert!(is_night(23.0) && !is_night(12.0));
    }

    #[test]
    fn opening_hours() {
        assert!(within((20.0, 4.0), 23.0));
        assert!(within((20.0, 4.0), 2.0));
        assert!(!within((20.0, 4.0), 12.0));
        assert!(within((9.0, 17.0), 12.0));
        assert!(!within((9.0, 17.0), 17.0));
    }

    #[test]
    fn labels() {
        assert_eq!(label(0.0), "12:00 AM");
        assert_eq!(label(13.5), "1:30 PM");
        assert_eq!(label(21.75), "9:45 PM");
    }
}
