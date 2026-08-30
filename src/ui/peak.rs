//! 峰谷时段判定

use chrono::{DateTime, Datelike, FixedOffset, Timelike};

/// 一天内的分钟区间 [start, end)，默认官方口径 14:00–18:00
pub type PeakRange = (u32, u32);

pub const DEFAULT_PEAK: PeakRange = (14 * 60, 18 * 60);

fn beijing() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("固定偏移恒合法")
}

/// 高峰时段：周一至周五的给定区间（半开）。
/// start > end 表示跨午夜，如 22:00–09:00
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

/// 当前北京时间是否处于高峰
pub fn is_peak_now(range: PeakRange) -> bool {
    is_peak(chrono::Local::now().with_timezone(&beijing()), range)
}

/// 解析 HH:MM 为当日分钟数
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// 解析高峰区间配置；格式无效或两端相等回退官方默认，
/// start > end 视为跨午夜原样保留。
pub fn peak_range(start: &str, end: &str) -> PeakRange {
    match (parse_hhmm(start), parse_hhmm(end)) {
        (Some(s), Some(e)) if s != e => (s, e),
        _ => DEFAULT_PEAK,
    }
}

/// 区间显示为 HH:MM–HH:MM
pub fn fmt_range(range: PeakRange) -> String {
    let hhmm = |t: u32| format!("{:02}:{:02}", t / 60, t % 60);
    format!("{}–{}", hhmm(range.0), hhmm(range.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32, min: u32, weekday_offset: i64) -> DateTime<FixedOffset> {
        // 2026-08-24 为周一，offset 0..6 覆盖一周
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
        // 14:00–次日 09:00
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
        assert_eq!(fmt_range(DEFAULT_PEAK), "14:00–18:00");
    }

    #[test]
    fn peak_range_config_fallbacks() {
        // 两端相等回退官方默认
        assert_eq!(peak_range("09:00", "09:00"), DEFAULT_PEAK);
        // 格式非法同样回退
        assert_eq!(peak_range("9am", "18:00"), DEFAULT_PEAK);
        assert_eq!(peak_range("09:00", "6pm"), DEFAULT_PEAK);
        // 合法窗口
        assert_eq!(peak_range("09:00", "18:00"), (9 * 60, 18 * 60));
        // start > end 表示跨午夜，原样保留不交换
        assert_eq!(peak_range("22:00", "06:00"), (22 * 60, 6 * 60));
    }
}
