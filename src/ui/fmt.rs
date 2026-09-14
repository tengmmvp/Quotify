//! 展示格式化

use chrono::{DateTime, Local, Utc};

use crate::ui::i18n::Lang;

/// 缺失值占位符
pub const MISSING: &str = "—";

/// 大数字紧凑缩写
///
/// ≥1000 缩为 k/M/B 保留一位小数，跨档进位升后缀。
pub fn compact_number(v: f64) -> String {
    let (mut num, unit) = compact_number_split(v);
    if !unit.is_empty() {
        num.push_str(unit);
    }
    num
}

/// 分离式大数字紧凑缩写
///
/// 数字与单位分离返回，卡片排版用大数字配小单位。
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
    // 进位跨档：按绝对值判定，负值同升档，不出 "1000k"/"1000M"
    if suffix != "B" && s.parse::<f64>().is_ok_and(|n| n.abs() >= 1000.0) {
        div *= 1000.0;
        suffix = if suffix == "k" { "M" } else { "B" };
        s = format!("{:.1}", v / div);
    }
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    (s, suffix)
}

/// 百分比显示
pub fn percent(p: f64) -> String {
    let clamped = p.clamp(0.0, 999.0);
    format!("{}%", clamped.round() as i64)
}

/// 重置倒计时
pub fn countdown(until: DateTime<Utc>, lang: Lang) -> String {
    countdown_from(until, Utc::now(), lang)
}

/// 倒计时主体
///
/// 时间源注入解耦，供测试钉位。
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

/// 「数据截至」时间戳
pub fn as_of_time(at: DateTime<Local>) -> String {
    at.format("%H:%M").to_string()
}

/// 页脚「数据更新」文案
///
/// 60 秒内「刚刚更新」，否则相对时长；换装动画的 old_text 与静态
/// 页脚共用本函数，防两份实现漂移致动画起点错。
pub fn updated_text(s: &crate::ui::i18n::Strings, lang: Lang, at: DateTime<Local>) -> String {
    if (Local::now() - at).num_seconds() < 60 {
        s.updated_just_now.to_string()
    } else {
        s.updated_ago.replace("{t}", &ago(at, lang))
    }
}

/// 更新时间脚注用的距今时长短表述
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

/// 倒计时单位文案
struct Units {
    day: &'static str,
    hour: &'static str,
    minute: &'static str,
    second: &'static str,
}

/// 按语言取倒计时单位文案
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
        assert_eq!(compact_number(3_400_000_000.0), "3.4B");
        assert_eq!(compact_number(1200.0), "1.2k");
        assert_eq!(compact_number(999.0), "999");
        // 进位跨档：后缀随之升档，不出 "1000k"/"1000M"
        assert_eq!(compact_number(999_960.0), "1M");
        assert_eq!(compact_number(999_950_000.0), "1B");
        // 负值按绝对值同升档
        assert_eq!(compact_number(-4233.0), "-4.2k");
        assert_eq!(compact_number(-999_960.0), "-1M");
        assert_eq!(compact_number(-999_950_000.0), "-1B");
        // 非有限值归零：饱和转换会产出 19 位整数撑爆卡片
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
        // 已过期 → 0 秒，不出现负数
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
