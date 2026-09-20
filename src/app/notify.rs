//! Decision logic for usage-event notifications

use chrono::{DateTime, Utc};

use crate::api::QuotaBucket;

/// Title shared by all notifications
pub(crate) const NOTIFY_TITLE: &str = "Quotify";

/// A changed reset time means a new window; armed here means "the new
/// window starts from low usage", so it is re-armed regardless of the
/// notification toggle. A missing resets_at is no information rather
/// than a reset: the baseline is kept so the next real window still
/// notifies. Produces the message text without sending; routing is
/// composed by the caller.
pub(crate) fn check_reset(
    bucket: Option<&QuotaBucket>,
    last: &mut Option<DateTime<Utc>>,
    armed: &mut bool,
    enabled: bool,
    msg: &'static str,
) -> Option<String> {
    if let Some(b) = bucket
        && let Some(resets_at) = b.resets_at
    {
        if last.is_some_and(|old| old != resets_at) {
            *armed = true;
            *last = Some(resets_at);
            return enabled.then(|| msg.to_string());
        }
        *last = Some(resets_at);
    }
    None
}

/// Crossing the threshold alerts once (armed set to false); falling back
/// below the threshold re-arms.
pub(crate) fn check_threshold(
    bucket: Option<&QuotaBucket>,
    armed: &mut bool,
    th: f64,
    label: &'static str,
    title: &'static str,
) -> Option<String> {
    if let Some(b) = bucket {
        if b.used_percent >= th && *armed {
            *armed = false;
            // The percentage format shares the same single source as the
            // tooltip/metric rows, with identical out-of-range clamping.
            return Some(format!(
                "{label} {title} {}",
                crate::ui::fmt::percent(b.used_percent)
            ));
        } else if b.used_percent < th {
            *armed = true;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn bucket(used: f64, resets_at: Option<DateTime<Utc>>) -> QuotaBucket {
        QuotaBucket {
            used_percent: used,
            resets_at,
            total: None,
            current: None,
        }
    }

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 26, hour, 0, 0).unwrap()
    }

    #[test]
    fn reset_change_notifies_once_and_rearms() {
        let mut last: Option<DateTime<Utc>> = None;
        let mut armed = false;

        // The first snapshot only establishes the baseline; it does not trigger.
        assert_eq!(
            check_reset(
                Some(&bucket(50.0, Some(at(9)))),
                &mut last,
                &mut armed,
                true,
                "5h 已重置"
            ),
            None
        );
        assert_eq!(last, Some(at(9)));

        // Reset time changed → notify once and re-arm.
        assert_eq!(
            check_reset(
                Some(&bucket(20.0, Some(at(14)))),
                &mut last,
                &mut armed,
                true,
                "5h 已重置"
            ),
            Some("5h 已重置".to_string())
        );
        assert!(armed);
    }

    #[test]
    fn reset_unchanged_stays_silent() {
        let mut last: Option<DateTime<Utc>> = None;
        let mut armed = false;

        check_reset(
            Some(&bucket(50.0, Some(at(9)))),
            &mut last,
            &mut armed,
            true,
            "5h 已重置",
        );
        assert_eq!(
            check_reset(
                Some(&bucket(60.0, Some(at(9)))),
                &mut last,
                &mut armed,
                true,
                "5h 已重置"
            ),
            None
        );
        assert!(!armed);
    }

    #[test]
    fn reset_disabled_skips_notify_but_rearms() {
        let mut last: Option<DateTime<Utc>> = None;
        let mut armed = false;

        check_reset(
            Some(&bucket(50.0, Some(at(9)))),
            &mut last,
            &mut armed,
            true,
            "5h 已重置",
        );
        // Toggle off: no notification is produced, but armed must still be
        // re-armed — otherwise threshold alerts that fired while disabled
        // stay permanently silent.
        assert_eq!(
            check_reset(
                Some(&bucket(20.0, Some(at(14)))),
                &mut last,
                &mut armed,
                false,
                "5h 已重置"
            ),
            None
        );
        assert!(armed);
    }

    #[test]
    fn threshold_cross_notifies_once_until_rearm() {
        let mut armed = true;

        assert_eq!(
            check_threshold(
                Some(&bucket(85.4, None)),
                &mut armed,
                80.0,
                "5h",
                "已超 80%"
            ),
            Some("5h 已超 80% 85%".to_string())
        );
        assert!(!armed);

        // Still above the line and not re-armed: does not fire again.
        assert_eq!(
            check_threshold(
                Some(&bucket(90.0, None)),
                &mut armed,
                80.0,
                "5h",
                "已超 80%"
            ),
            None
        );
        assert!(!armed);
    }

    #[test]
    fn none_bucket_missing_reset_and_exact_boundary() {
        let mut last: Option<DateTime<Utc>> = None;
        let mut armed = true;

        // None bucket: both functions stay silent and leave state untouched.
        assert_eq!(
            check_reset(None, &mut last, &mut armed, true, "5h 已重置"),
            None
        );
        assert_eq!(last, None);
        assert!(armed);
        assert_eq!(
            check_threshold(None, &mut armed, 80.0, "5h", "已超 80%"),
            None
        );
        assert!(armed);

        // resets_at missing: no information, not a reset — the baseline is
        // kept untouched (still the initial None here).
        assert_eq!(
            check_reset(
                Some(&bucket(50.0, None)),
                &mut last,
                &mut armed,
                true,
                "5h 已重置"
            ),
            None
        );
        assert_eq!(last, None);

        // Exactly reaching the threshold fires: the check is >=.
        assert_eq!(
            check_threshold(
                Some(&bucket(80.0, None)),
                &mut armed,
                80.0,
                "5h",
                "已超 80%"
            ),
            Some("5h 已超 80% 80%".to_string())
        );
        assert!(!armed);
    }

    /// A missing resets_at is no information, not a reset: the Some(T1)
    /// baseline survives the missing round, so the following real new
    /// window (Some(T2)) still notifies.
    #[test]
    fn missing_reset_round_keeps_baseline_for_next_window() {
        let mut last: Option<DateTime<Utc>> = None;
        let mut armed = false;

        check_reset(
            Some(&bucket(50.0, Some(at(9)))),
            &mut last,
            &mut armed,
            true,
            "5h 已重置",
        );
        assert_eq!(last, Some(at(9)));

        // Missing round: silent, baseline kept.
        assert_eq!(
            check_reset(
                Some(&bucket(50.0, None)),
                &mut last,
                &mut armed,
                true,
                "5h 已重置"
            ),
            None
        );
        assert_eq!(last, Some(at(9)));

        // The next real new window is still a change against the kept
        // baseline: it must notify.
        assert_eq!(
            check_reset(
                Some(&bucket(20.0, Some(at(14)))),
                &mut last,
                &mut armed,
                true,
                "5h 已重置"
            ),
            Some("5h 已重置".to_string())
        );
        assert_eq!(last, Some(at(14)));
        assert!(armed);
    }

    #[test]
    fn threshold_rearm_after_falling_below() {
        let mut armed = true;

        assert_eq!(
            check_threshold(
                Some(&bucket(85.0, None)),
                &mut armed,
                80.0,
                "周",
                "已超 80%"
            ),
            Some("周 已超 80% 85%".to_string())
        );
        assert!(!armed);

        // Falls back below the line: silently re-arms.
        assert_eq!(
            check_threshold(
                Some(&bucket(60.0, None)),
                &mut armed,
                80.0,
                "周",
                "已超 80%"
            ),
            None
        );
        assert!(armed);

        // Crosses the line again: fires once more.
        assert_eq!(
            check_threshold(
                Some(&bucket(82.0, None)),
                &mut armed,
                80.0,
                "周",
                "已超 80%"
            ),
            Some("周 已超 80% 82%".to_string())
        );
        assert!(!armed);
    }
}
