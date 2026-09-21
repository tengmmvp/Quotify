//! Peak-hours determination

use chrono::{DateTime, Datelike, FixedOffset, Timelike};

/// Minute range within a day, [start, end); the default is the official 14:00–18:00
pub type PeakRange = (u32, u32);

/// Sole source of the default; app/config.rs's default value and template
/// both render from this constant
pub(crate) const DEFAULT_PEAK: PeakRange = (14 * 60, 18 * 60);

fn beijing() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("固定偏移恒合法")
}

/// Peak hours: the given half-open range on Monday through Friday.
/// start > end means the range crosses midnight, e.g. 22:00–09:00
pub fn is_peak(now: DateTime<FixedOffset>, range: PeakRange) -> bool {
    let weekday = now.weekday().number_from_monday();
    let minute = now.hour() * 60 + now.minute();
    if !(1..=5).contains(&weekday) {
        return false;
    }
    let (s, e) = range;
    if s < e {
        s <= minute && minute < e
    } else {
        minute >= s || minute < e
    }
}

/// Whether the current Beijing time is in peak hours
pub fn is_peak_now(range: PeakRange) -> bool {
    is_peak(chrono::Local::now().with_timezone(&beijing()), range)
}

/// Parses HH:MM into minutes within the day
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Parses the peak-range config; invalid formats or equal endpoints fall back to the
/// official default,
/// and start > end is kept as-is as a midnight-crossing range.
pub fn peak_range(start: &str, end: &str) -> PeakRange {
    match (parse_hhmm(start), parse_hhmm(end)) {
        (Some(s), Some(e)) if s != e => (s, e),
        _ => DEFAULT_PEAK,
    }
}

/// Renders minutes-of-day as HH:MM — the single HH:MM formatter, shared
/// by fmt_range and config's default/template rendering
pub(crate) fn hhmm(min: u32) -> String {
    format!("{:02}:{:02}", min / 60, min % 60)
}

/// Formats the range as HH:MM–HH:MM
pub fn fmt_range(range: PeakRange) -> String {
    format!("{}–{}", hhmm(range.0), hhmm(range.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32, min: u32, weekday_offset: i64) -> DateTime<FixedOffset> {
        // 2026-08-24 is a Monday; offset 0..6 covers a full week
        beijing()
            .with_ymd_and_hms(2026, 8, (24 + weekday_offset) as u32, hour, min, 0)
            .unwrap()
    }

    #[test]
    fn peak_boundaries() {
        assert!(is_peak(at(14, 0, 0), DEFAULT_PEAK));
        assert!(is_peak(at(17, 59, 0), DEFAULT_PEAK));
        assert!(!is_peak(at(18, 0, 0), DEFAULT_PEAK));
        assert!(!is_peak(at(13, 59, 0), DEFAULT_PEAK));
    }

    #[test]
    fn weekends_never_peak() {
        assert!(is_peak(at(15, 0, 4), DEFAULT_PEAK));
        assert!(!is_peak(at(15, 0, 5), DEFAULT_PEAK));
        assert!(!is_peak(at(15, 0, 6), DEFAULT_PEAK));
    }

    #[test]
    fn custom_range() {
        let r = (9 * 60 + 30, 12 * 60);
        assert!(is_peak(at(9, 30, 0), r));
        assert!(is_peak(at(11, 59, 0), r));
        assert!(!is_peak(at(12, 0, 0), r));
        assert!(!is_peak(at(9, 29, 0), r));
    }

    #[test]
    fn overnight_range() {
        // 14:00–09:00 next day
        let r = (14 * 60, 9 * 60);
        assert!(is_peak(at(14, 0, 0), r));
        assert!(is_peak(at(23, 59, 0), r));
        assert!(is_peak(at(0, 0, 1), r));
        assert!(is_peak(at(8, 59, 1), r));
        assert!(!is_peak(at(9, 0, 1), r));
        assert!(!is_peak(at(13, 59, 0), r));
    }

    #[test]
    fn hhmm_roundtrip() {
        assert_eq!(parse_hhmm("14:00"), Some(14 * 60));
        assert_eq!(parse_hhmm(" 9:5 "), Some(9 * 60 + 5));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert_eq!(parse_hhmm("abc"), None);
        assert_eq!(hhmm(9 * 60 + 5), "09:05");
        assert_eq!(fmt_range(DEFAULT_PEAK), "14:00–18:00");
    }

    #[test]
    fn peak_range_config_fallbacks() {
        // Equal endpoints fall back to the official default
        assert_eq!(peak_range("09:00", "09:00"), DEFAULT_PEAK);
        // Invalid formats fall back as well
        assert_eq!(peak_range("9am", "18:00"), DEFAULT_PEAK);
        assert_eq!(peak_range("09:00", "6pm"), DEFAULT_PEAK);
        // A valid window
        assert_eq!(peak_range("09:00", "18:00"), (9 * 60, 18 * 60));
        // start > end means midnight-crossing; kept as-is, not swapped
        assert_eq!(peak_range("22:00", "06:00"), (22 * 60, 6 * 60));
    }
}
