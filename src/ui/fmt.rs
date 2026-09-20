//! Display formatting

use chrono::{DateTime, Local, Utc};

use crate::ui::i18n::Lang;

/// Placeholder for missing values
pub const MISSING: &str = "—";

/// Compact abbreviation for large numbers.
///
/// Values ≥1000 shrink to k/M/B with one decimal place; crossing a tier boundary promotes
/// the suffix.
pub fn compact_number(v: f64) -> String {
    let (mut num, unit) = compact_number_split(v);
    if !unit.is_empty() {
        num.push_str(unit);
    }
    num
}

/// Split compact abbreviation for large numbers.
///
/// Returns the number and the unit separately so card layouts can pair a big number with
/// a small unit.
pub fn compact_number_split(v: f64) -> (String, &'static str) {
    if !v.is_finite() {
        return ("0".to_string(), "");
    }
    let abs = v.abs();
    if abs < 1000.0 {
        return (format!("{}", v as i64), "");
    }
    let (mut div, mut suffix) = if abs >= 1e9 {
        (1e9, "B")
    } else if abs >= 1e6 {
        (1e6, "M")
    } else {
        (1e3, "k")
    };
    let mut s = format!("{:.1}", v / div);
    // Carry across tiers: decided by absolute value, so negatives promote too —
    // never produces "1000k"/"1000M".
    if suffix != "B" && s.parse::<f64>().is_ok_and(|n| n.abs() >= 1000.0) {
        div *= 1000.0;
        suffix = if suffix == "k" { "M" } else { "B" };
        s = format!("{:.1}", v / div);
    }
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    (s, suffix)
}

/// Percentage display
pub fn percent(p: f64) -> String {
    let clamped = p.clamp(0.0, 999.0);
    format!("{}%", clamped.round() as i64)
}

/// Reset countdown
pub fn countdown(until: DateTime<Utc>, lang: Lang) -> String {
    countdown_from(until, Utc::now(), lang)
}

/// Countdown core.
///
/// The time source is injected for decoupling, so tests can pin it.
fn countdown_from(until: DateTime<Utc>, now: DateTime<Utc>, lang: Lang) -> String {
    let s = i18n_units(lang);
    let u = |unit: &str| -> String {
        match lang {
            Lang::Zh => format!(" {unit}"),
            Lang::En => unit.to_string(),
        }
    };
    let secs = (until - now).num_seconds().max(0);
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let sec = secs % 60;
    if days > 0 {
        if hours > 0 {
            format!("{days}{} {hours}{}", u(s.day), u(s.hour))
        } else {
            format!("{days}{}", u(s.day))
        }
    } else if hours > 0 {
        format!("{hours}{} {mins}{}", u(s.hour), u(s.minute))
    } else if mins > 0 {
        format!("{mins}{}", u(s.minute))
    } else {
        format!("{sec}{}", u(s.second))
    }
}

/// Elapsed-minute bucket of a millisecond delta: the single source shared
/// by the footer text's minute tier and the footer cache key — ago() buckets
/// whole elapsed seconds / 60, and the key must floor the identical way or
/// a cached footer keeps the previous tier's copy up to a minute late.
pub(crate) fn elapsed_minutes(delta_ms: i64) -> i64 {
    delta_ms.div_euclid(60_000)
}

/// Elapsed-minute bucket of two wall-clock millisecond stamps with the
/// zero clamp built in: the single "a wall-clock step back must not
/// bucket negative" policy — the footer cache key and the copy tiers
/// (updated_text) both derive here, so the clamp cannot exist in one
/// and be missing from the other.
pub(crate) fn elapsed_minutes_since(now_ms: i64, at_ms: i64) -> i64 {
    elapsed_minutes((now_ms - at_ms).max(0))
}

/// Footer "data updated" text.
///
/// "Just updated" is exactly elapsed-minutes bucket 0 — the same boundary
/// function as the footer cache key, so the copy tier and the key switch
/// land on the same tick and a cached footer never keeps the previous
/// tier's copy; the swap animation's old_text and the static footer share
/// this function, so the two implementations cannot drift and misplace the
/// animation start.
pub fn updated_text(s: &crate::ui::i18n::Strings, lang: Lang, at: DateTime<Local>) -> String {
    // The zero clamp lives in elapsed_minutes_since: a wall-clock step
    // back (NTP correction) leaves the snapshot stamped in the future —
    // a negative bucket would read as -1 minutes and fall through to
    // "1m ago"; zero keeps "just updated" until wall time passes the
    // stamp, matching ago()'s own clamp.
    let now_ms = Local::now().timestamp_millis();
    if elapsed_minutes_since(now_ms, at.timestamp_millis()) == 0 {
        s.updated_just_now.to_string()
    } else {
        s.updated_ago.replace("{t}", &ago(at, lang))
    }
}

/// Short elapsed-time phrasing for the updated-time footnote
pub fn ago(at: DateTime<Local>, lang: Lang) -> String {
    let secs = (Local::now() - at).num_seconds().clamp(0, i64::MAX);
    match lang {
        Lang::Zh => {
            if secs < 3600 {
                format!("{} 分钟", (secs / 60).max(1))
            } else if secs < 86400 {
                format!("{} 小时", secs / 3600)
            } else {
                format!("{} 天", secs / 86400)
            }
        }
        Lang::En => {
            if secs < 3600 {
                format!("{}m", (secs / 60).max(1))
            } else if secs < 86400 {
                format!("{}h", secs / 3600)
            } else {
                format!("{}d", secs / 86400)
            }
        }
    }
}

/// Countdown unit strings
struct Units {
    day: &'static str,
    hour: &'static str,
    minute: &'static str,
    second: &'static str,
}

/// Gets the countdown unit strings for the language
fn i18n_units(lang: Lang) -> Units {
    let s = lang.strings();
    Units {
        day: s.unit_day,
        hour: s.unit_hour,
        minute: s.unit_minute,
        second: s.unit_second,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The minute bucket must agree with ago()'s text tiering (whole
    /// elapsed seconds / 60): the footer cache key derives from this, and a
    /// disagreement caches a stale tier's copy.
    #[test]
    fn elapsed_minutes_matches_text_bucket() {
        for &delta in &[
            0i64, 59_999, 60_000, 60_001, 119_999, 120_000, 3_599_999, 3_600_000,
        ] {
            let secs = delta / 1000; // num_seconds truncation for non-negative deltas
            assert_eq!(elapsed_minutes(delta), secs / 60, "delta {delta}");
        }
    }

    /// The zero clamp lives in elapsed_minutes_since: a stepped-back
    /// wall clock (now before the stamp) never buckets negative — the
    /// footer cache key and the copy tiers share this one clamp.
    #[test]
    fn elapsed_minutes_since_clamps_negative() {
        assert_eq!(elapsed_minutes_since(1_000, 5_000), 0);
        assert_eq!(elapsed_minutes_since(5_000, 1_000), 0);
        assert_eq!(elapsed_minutes_since(5_000, 61_000), 0);
        assert_eq!(elapsed_minutes_since(61_000, 1_000), 1);
    }

    #[test]
    fn compact() {
        assert_eq!(compact_number(87.0), "87");
        assert_eq!(compact_number(4233.0), "4.2k");
        assert_eq!(compact_number(10000.0), "10k");
        assert_eq!(compact_number(1_200_000.0), "1.2M");
        assert_eq!(compact_number(3_400_000_000.0), "3.4B");
        assert_eq!(compact_number(1200.0), "1.2k");
        assert_eq!(compact_number(999.0), "999");
        // Carry across tiers: the suffix promotes with it, never producing "1000k"/"1000M"
        assert_eq!(compact_number(999_960.0), "1M");
        assert_eq!(compact_number(999_950_000.0), "1B");
        // Negatives promote by absolute value as well
        assert_eq!(compact_number(-4233.0), "-4.2k");
        assert_eq!(compact_number(-999_960.0), "-1M");
        assert_eq!(compact_number(-999_950_000.0), "-1B");
        // Non-finite values fall back to zero: a saturated cast would produce a 19-digit
        // integer and blow up the card
        assert_eq!(compact_number(f64::INFINITY), "0");
        assert_eq!(compact_number(f64::NAN), "0");
    }

    #[test]
    fn percent_fmt() {
        assert_eq!(percent(87.4), "87%");
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(-3.0), "0%");
        assert_eq!(percent(1000.0), "999%");
    }

    #[test]
    fn countdown_granularity() {
        let zh = Lang::Zh;
        let now = Utc::now();
        let t = now + chrono::Duration::seconds(90);
        assert_eq!(countdown_from(t, now, zh), "1 分钟");
        let t = now + chrono::Duration::seconds(30);
        assert!(countdown_from(t, now, zh).ends_with("秒"));
        let t = now + chrono::Duration::hours(2) + chrono::Duration::minutes(13);
        assert_eq!(countdown_from(t, now, zh), "2 小时 13 分钟");
        let t = now + chrono::Duration::days(3);
        assert_eq!(countdown_from(t, now, zh), "3 天");
        // Already expired → 0 seconds, never negative
        assert_eq!(
            countdown_from(now - chrono::Duration::minutes(5), now, zh),
            "0 秒"
        );
    }

    #[test]
    fn countdown_en_stays_compact() {
        let now = Utc::now();
        let t = now + chrono::Duration::hours(2) + chrono::Duration::minutes(13);
        assert_eq!(countdown_from(t, now, Lang::En), "2h 13m");
        let t = now + chrono::Duration::days(3) + chrono::Duration::hours(4);
        assert_eq!(countdown_from(t, now, Lang::En), "3d 4h");
    }
}
