//! Repository news

use crate::api::client::{MAX_BODY_BYTES, agent_long};
use crate::service::update::REPO;

/// Upper bound on the news item count
pub const NEWS_MAX: usize = 3;

/// One news item
#[derive(Debug, Clone, PartialEq)]
pub struct NewsItem {
    pub date: String,
    pub title: String,
    pub lines: Vec<String>,
}

/// Unread decision: no read marker yet, or the item's date is newer than
/// the marker. The NEW badge's slot reservation (cache build), the
/// node/badge drawing, and the read-marker advance on click all read this
/// one predicate — diverging copies would light a badge with no reserved
/// slot or clip a title that has one. Lexical comparison assumes both
/// sides carry the canonical zero-padded form: item dates are validated
/// at parse, the marker is normalized at config load.
pub fn is_unread(item_date: &str, read_marker: Option<&str>) -> bool {
    read_marker.is_none_or(|r| item_date > r)
}

/// Canonical YYYY-MM-DD of a date-ish string: accepts non-zero-padded
/// shapes (a hand-edited read marker), so lexical comparison against
/// item dates stays meaningful; None when not a date at all.
pub fn normalize_date(s: &str) -> Option<String> {
    let mut it = s.split('-');
    let (y, m, d) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() || y.len() != 4 || y.bytes().any(|b| !b.is_ascii_digit()) {
        return None;
    }
    let m: u8 = m.parse().ok()?;
    let d: u8 = d.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(format!("{y}-{m:02}-{d:02}"))
}

/// Fetch and parse the latest news
pub fn fetch_latest() -> Result<Vec<NewsItem>, String> {
    let url = format!("https://raw.githubusercontent.com/{REPO}/main/docs/WHAT_IS_NEW.md");
    let resp = agent_long()
        .get(&url)
        // GitHub rejects requests without a User-Agent; the value's owner
        // is update.rs.
        .header("User-Agent", crate::service::update::github_user_agent())
        .call()
        .map_err(|e| format!("network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body = crate::api::client::read_body_capped(resp.into_body(), MAX_BODY_BYTES)
        .map_err(|e| format!("read failed: {e}"))?;
    Ok(parse_news(&body))
}

/// Parse `## date · title` sections; input with no valid section yields an
/// empty list; only the newest NEWS_MAX items stay resident
pub fn parse_news(text: &str) -> Vec<NewsItem> {
    let mut out: Vec<NewsItem> = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("## ") else {
            // Non-header lines go into the current section's body; blank
            // lines and Markdown rules are dropped.
            if let Some(cur) = out.last_mut() {
                let t = line.trim();
                if !t.is_empty() && t != "---" {
                    cur.lines.push(t.to_string());
                }
            }
            continue;
        };
        // A section header looks like `2026-08-27 · title`; a section whose
        // date is invalid is dropped whole. normalize_date is the single
        // date validator: canonical (zero-padded) input round-trips to
        // itself, anything else — invalid or non-padded — drops.
        let Some((date, title)) = rest.split_once(" · ") else {
            continue;
        };
        let trimmed = date.trim();
        let title = title.trim().to_string();
        let Some(date) = normalize_date(trimmed) else {
            continue;
        };
        if title.is_empty() || date != trimmed {
            continue;
        }
        out.push(NewsItem {
            date: date.to_string(),
            title,
            lines: Vec::new(),
        });
    }
    // Parsing consumes the full input; residency keeps only the newest
    // NEWS_MAX items.
    out.truncate(NEWS_MAX);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sections_newest_first() {
        let md = "# What's New\n\n## 2026-08-27 · 新模型上线\nGLM-5 可用。\n消耗降为原来的一半。\n\n## 2026-08-26 · v1.0.2 发布\n修复输入法兼容。\n";
        let items = parse_news(md);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].date, "2026-08-27");
        assert_eq!(items[0].title, "新模型上线");
        assert_eq!(items[0].lines, vec!["GLM-5 可用。", "消耗降为原来的一半。"]);
        assert_eq!(items[1].lines, vec!["修复输入法兼容。"]);
    }

    #[test]
    fn malformed_sections_dropped() {
        let md = "## 无日期 · 标题\n正文\n## 2026-13-01 · 坏月份\n正文\n## 2026-08-01无分隔\n";
        assert!(parse_news(md).is_empty());
    }

    #[test]
    fn empty_and_plain_text() {
        assert!(parse_news("").is_empty());
        assert!(parse_news("没有任何节头的普通文本").is_empty());
    }

    #[test]
    fn blank_and_rule_lines_dropped() {
        let md = "## 2026-08-27 · 标题\n\n正文一\n\n---\n\n正文二\n";
        let items = parse_news(md);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].lines, vec!["正文一", "正文二"]);
    }

    /// Parsing consumes the full input, residency truncates: beyond NEWS_MAX
    /// only the document's front-most (newest) sections are kept
    #[test]
    fn parse_news_truncates_to_news_max() {
        let mut md = String::new();
        for i in (1..=5).rev() {
            md.push_str(&format!("## 2026-08-0{i} · 标题{i}\n\n正文{i}\n\n"));
        }
        let items = parse_news(&md);
        assert_eq!(items.len(), NEWS_MAX);
        assert_eq!(
            items.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(),
            vec!["标题5", "标题4", "标题3"]
        );
    }

    /// Read-marker normalization: zero-padding is canonical (lexical
    /// comparison with item dates), non-zero-padded forms fold onto it,
    /// and non-dates carry no marker semantics
    #[test]
    fn normalize_date_folds_padding() {
        assert_eq!(normalize_date("2026-08-27").as_deref(), Some("2026-08-27"));
        assert_eq!(normalize_date("2026-8-7").as_deref(), Some("2026-08-07"));
        assert_eq!(normalize_date("26-08-27"), None, "年份非四位");
        assert_eq!(normalize_date("2026-13-01"), None, "月份越界");
        assert_eq!(normalize_date("2026-08-32"), None, "日期越界");
        assert_eq!(normalize_date("2026/08/27"), None, "非日期");
        assert_eq!(normalize_date("2026-08-27-extra"), None, "多余段");
    }
}
