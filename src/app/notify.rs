//! 用量事件通知的判定逻辑

use chrono::{DateTime, Utc};

use crate::api::QuotaBucket;

/// 所有通知共用标题
pub(crate) const NOTIFY_TITLE: &str = "Quotify";

/// 重置时刻变化即新窗口；armed 在此的语义是「新窗口从低用量起步」，无论通知开关都重新武装
pub(crate) fn check_reset(
    bucket: Option<&QuotaBucket>,
    last: &mut Option<DateTime<Utc>>,
    armed: &mut bool,
    enabled: bool,
    msg: &'static str,
    notify: &dyn Fn(&str),
) {
    if let Some(b) = bucket {
        if last.is_some_and(|old| old != b.resets_at.unwrap_or(old)) {
            if enabled {
                notify(msg);
            }
            *armed = true;
        }
        *last = b.resets_at;
    }
}

/// 越线提醒一次（armed 置 false），回落线下重新武装
pub(crate) fn check_threshold(
    bucket: Option<&QuotaBucket>,
    armed: &mut bool,
    th: f64,
    label: &'static str,
    title: &'static str,
    notify: &dyn Fn(&str),
) {
    if let Some(b) = bucket {
        if b.used_percent >= th && *armed {
            *armed = false;
            let body = format!("{label} {title} {}%", b.used_percent.round() as i64);
            notify(&body);
        } else if b.used_percent < th {
            *armed = true;
        }
    }
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

    /// 通知收集器：RefCell 让闭包只共享捕获以满足 `&dyn Fn`；
    /// 阈值文案是 format 出的临时 String，泄漏副本延寿以便 &str 断言
    struct Recorder {
        sent: std::cell::RefCell<Vec<&'static str>>,
    }

    impl Recorder {
        fn new() -> Self {
            Self {
                sent: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn notify(&self, s: &str) {
            self.sent
                .borrow_mut()
                .push(Box::leak(s.to_owned().into_boxed_str()));
        }
    }

    #[test]
    fn reset_change_notifies_once_and_rearms() {
        let rec = Recorder::new();
        let notify = |s: &str| rec.notify(s);
        let mut last: Option<DateTime<Utc>> = None;
        let mut armed = false;

        // 首个快照只建立基线，不触发
        check_reset(
            Some(&bucket(50.0, Some(at(9)))),
            &mut last,
            &mut armed,
            true,
            "5h 已重置",
            &notify,
        );
        assert!(rec.sent.borrow().is_empty());
        assert_eq!(last, Some(at(9)));

        // 重置时刻变化 → 通知一次并重新武装
        check_reset(
            Some(&bucket(20.0, Some(at(14)))),
            &mut last,
            &mut armed,
            true,
            "5h 已重置",
            &notify,
        );
        assert_eq!(*rec.sent.borrow(), ["5h 已重置"]);
        assert!(armed);
    }

    #[test]
    fn reset_unchanged_stays_silent() {
        let rec = Recorder::new();
        let notify = |s: &str| rec.notify(s);
        let mut last: Option<DateTime<Utc>> = None;
        let mut armed = false;

        check_reset(
            Some(&bucket(50.0, Some(at(9)))),
            &mut last,
            &mut armed,
            true,
            "5h 已重置",
            &notify,
        );
        check_reset(
            Some(&bucket(60.0, Some(at(9)))),
            &mut last,
            &mut armed,
            true,
            "5h 已重置",
            &notify,
        );
        assert!(rec.sent.borrow().is_empty());
        assert!(!armed);
    }

    #[test]
    fn reset_disabled_skips_notify_but_rearms() {
        let rec = Recorder::new();
        let notify = |s: &str| rec.notify(s);
        let mut last: Option<DateTime<Utc>> = None;
        let mut armed = false;

        check_reset(
            Some(&bucket(50.0, Some(at(9)))),
            &mut last,
            &mut armed,
            true,
            "5h 已重置",
            &notify,
        );
        // 开关关闭：不弹通知，armed 仍要重新武装，否则关闭期间的越线提醒永久哑火
        check_reset(
            Some(&bucket(20.0, Some(at(14)))),
            &mut last,
            &mut armed,
            false,
            "5h 已重置",
            &notify,
        );
        assert!(rec.sent.borrow().is_empty());
        assert!(armed);
    }

    #[test]
    fn threshold_cross_notifies_once_until_rearm() {
        let rec = Recorder::new();
        let notify = |s: &str| rec.notify(s);
        let mut armed = true;

        check_threshold(
            Some(&bucket(85.4, None)),
            &mut armed,
            80.0,
            "5h",
            "已超 80%",
            &notify,
        );
        assert_eq!(*rec.sent.borrow(), ["5h 已超 80% 85%"]);
        assert!(!armed);

        // 仍在线上且未重新武装：不再触发
        check_threshold(
            Some(&bucket(90.0, None)),
            &mut armed,
            80.0,
            "5h",
            "已超 80%",
            &notify,
        );
        assert_eq!(rec.sent.borrow().len(), 1);
        assert!(!armed);
    }

    #[test]
    fn none_bucket_missing_reset_and_exact_boundary() {
        let rec = Recorder::new();
        let notify = |s: &str| rec.notify(s);
        let mut last: Option<DateTime<Utc>> = None;
        let mut armed = true;

        // None 桶：两函数都静默，状态不动
        check_reset(None, &mut last, &mut armed, true, "5h 已重置", &notify);
        assert_eq!(last, None);
        assert!(armed);
        check_threshold(None, &mut armed, 80.0, "5h", "已超 80%", &notify);
        assert!(armed);
        assert!(rec.sent.borrow().is_empty());

        // resets_at 缺失：不视为重置，基线回落 None 待下个快照重建
        check_reset(
            Some(&bucket(50.0, None)),
            &mut last,
            &mut armed,
            true,
            "5h 已重置",
            &notify,
        );
        assert_eq!(last, None);
        assert!(rec.sent.borrow().is_empty());

        // 恰达阈值即触发：判定为 >=
        check_threshold(
            Some(&bucket(80.0, None)),
            &mut armed,
            80.0,
            "5h",
            "已超 80%",
            &notify,
        );
        assert_eq!(rec.sent.borrow().len(), 1);
        assert!(!armed);
    }

    #[test]
    fn threshold_rearm_after_falling_below() {
        let rec = Recorder::new();
        let notify = |s: &str| rec.notify(s);
        let mut armed = true;

        check_threshold(
            Some(&bucket(85.0, None)),
            &mut armed,
            80.0,
            "周",
            "已超 80%",
            &notify,
        );
        assert_eq!(rec.sent.borrow().len(), 1);
        assert!(!armed);

        // 回落到线下：静默重新武装
        check_threshold(
            Some(&bucket(60.0, None)),
            &mut armed,
            80.0,
            "周",
            "已超 80%",
            &notify,
        );
        assert_eq!(rec.sent.borrow().len(), 1);
        assert!(armed);

        // 再次越线：再触发一次
        check_threshold(
            Some(&bucket(82.0, None)),
            &mut armed,
            80.0,
            "周",
            "已超 80%",
            &notify,
        );
        assert_eq!(*rec.sent.borrow(), ["周 已超 80% 85%", "周 已超 80% 82%"]);
        assert!(!armed);
    }
}
