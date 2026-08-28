//! 仓库动态

use crate::api::client::{MAX_BODY_BYTES, agent_long};
use crate::service::update::REPO;

/// 动态条数上限
pub const NEWS_MAX: usize = 3;

/// 单条动态
#[derive(Debug, Clone, PartialEq)]
pub struct NewsItem {
    pub date: String,
    pub title: String,
    pub lines: Vec<String>,
}

/// 拉取并解析最新动态
pub fn fetch_latest() -> Result<Vec<NewsItem>, String> {
    let url = format!("https://raw.githubusercontent.com/{REPO}/main/docs/WHAT_IS_NEW.md");
    let resp = agent_long()
        .get(&url)
        // GitHub 拒绝无 User-Agent 的请求，同 update.rs
        .header(
            "User-Agent",
            format!("quotify/{}", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|e| format!("network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body = resp
        .into_body()
        .into_with_config()
        .limit(MAX_BODY_BYTES)
        .lossy_utf8(true)
        .read_to_string()
        .map_err(|e| format!("read failed: {e}"))?;
    Ok(parse_news(&body))
}

/// 解析 `## 日期 · 标题` 分节；无有效节的输入返回空表；仅保留最新
/// NEWS_MAX 条常驻
pub fn parse_news(text: &str) -> Vec<NewsItem> {
    let mut out: Vec<NewsItem> = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("## ") else {
            // 非节头行归入当前节正文；空行与 Markdown 分隔线丢弃
            if let Some(cur) = out.last_mut() {
                let t = line.trim();
                if !t.is_empty() && t != "---" {
                    cur.lines.push(t.to_string());
                }
            }
            continue;
        };
        // 节头形如 `2026-08-27 · 标题`；日期不合法的节整节丢弃
        let Some((date, title)) = rest.split_once(" · ") else {
            continue;
        };
        let (date, title) = (date.trim(), title.trim().to_string());
        if !valid_date(date) || title.is_empty() {
            continue;
        }
        out.push(NewsItem {
            date: date.to_string(),
            title,
            lines: Vec::new(),
        });
    }
    // 解析吃全量，常驻只留最新 NEWS_MAX 条
    out.truncate(NEWS_MAX);
    out
}

/// YYYY-MM-DD 形状且数值在域内即可，不做历法校验
fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    digits(0..4)
        && digits(5..7)
        && digits(8..10)
        && s[5..7].parse::<u8>().is_ok_and(|m| (1..=12).contains(&m))
        && s[8..10].parse::<u8>().is_ok_and(|d| (1..=31).contains(&d))
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

    /// 解析吃全量、常驻截断：超出 NEWS_MAX 的节只留文档最前（最新）的
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
}
