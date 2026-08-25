//! 展示格式化：紧凑数字、百分比、重置倒计时。

use chrono::{DateTime, Local, Utc};

use crate::ui::i18n::Lang;

/// 大数字紧凑缩写：4233 → "4.2k"，1_200_000 → "1.2M"，3.4e9 → "3.4G"。
/// 不足 1000 时按原样显示。
pub fn compact_number(v: f64) -> String {
    let abs = v.abs();
    if !v.is_finite() || abs < 1000.0 {
        return format!("{}", v as i64);
    }
    let (scaled, suffix) = if abs >= 1e9 {
        (v / 1e9, "G")
    } else if abs >= 1e6 {
        (v / 1e6, "M")
    } else {
        (v / 1e3, "k")
    };
    let s = format!("{scaled:.1}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s}{suffix}")
}

/// 百分比显示：四舍五入取整，越界值钳制到 0–999。
pub fn percent(p: f64) -> String {
    let clamped = p.clamp(0.0, 999.0);
    format!("{}%", clamped.round() as i64)
}

/// 重置倒计时：按剩余时长选择粒度（≥1 天 → "3 天 4 小时"；
/// ≥1 小时 → "2 小时 13 分"；≥1 分 → "8 分"；否则 "45 秒"）。
/// 单位词由语言表提供（中文全称 / 英文缩写）。
pub fn countdown(until: DateTime<Utc>, lang: Lang) -> String {
    countdown_from(until, Utc::now(), lang)
}

/// 可注入当前时刻的倒计时实现（便于确定性单测）。
/// 中文遵循「盘古之白」：数字与汉字之间一律留一个空格（与静态标签
/// 「5 小时窗口」等一致）；英文保持紧凑缩写（"2h 13m"）。
fn countdown_from(until: DateTime<Utc>, now: DateTime<Utc>, lang: Lang) -> String {
    let s = i18n_units(lang);
    // 中文单位前垫空格，英文单位直接拼接
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

/// 「数据截至」时间戳（本地时区，短格式 HH:mm）。
pub fn as_of_time(at: DateTime<Local>) -> String {
    at.format("%H:%M").to_string()
}

/// 距今时长的短表述（更新时间脚注用）：分钟 / 小时 / 天粒度。
/// 「刚刚」由调用方判 <60s 后走 `updated_just_now`，不进这里。
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

struct Units {
    day: &'static str,
    hour: &'static str,
    minute: &'static str,
    second: &'static str,
}

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

    #[test]
    fn compact() {
        assert_eq!(compact_number(87.0), "87");
        assert_eq!(compact_number(4233.0), "4.2k");
        assert_eq!(compact_number(10000.0), "10k");
        assert_eq!(compact_number(1_200_000.0), "1.2M");
        assert_eq!(compact_number(3_400_000_000.0), "3.4G");
        assert_eq!(compact_number(1200.0), "1.2k");
        assert_eq!(compact_number(999.0), "999");
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
        assert_eq!(countdown_from(t, now, zh), "1 分");
        let t = now + chrono::Duration::seconds(30);
        assert!(countdown_from(t, now, zh).ends_with("秒"));
        let t = now + chrono::Duration::hours(2) + chrono::Duration::minutes(13);
        assert_eq!(countdown_from(t, now, zh), "2 小时 13 分");
        let t = now + chrono::Duration::days(3);
        assert_eq!(countdown_from(t, now, zh), "3 天");
        // 已过期 → 0 秒（不出现负数）
        assert_eq!(countdown_from(now - chrono::Duration::minutes(5), now, zh), "0 秒");
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
