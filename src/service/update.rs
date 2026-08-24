//! 检查更新：查询 GitHub Releases 最新版本，只提示不自动安装。

use std::time::Duration;

use serde::Deserialize;

pub const REPO: &str = "TengMMVP/quotify";

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
}

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag: String,
    /// Release 页面链接（「前往下载」跳转用）
    #[allow(dead_code)]
    pub url: String,
}

/// 查询最新 Release。阻塞调用，UI 侧应在后台线程触发。
pub fn check_latest() -> Result<ReleaseInfo, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let resp = agent
        .get(&url)
        // GitHub API 强制要求 User-Agent
        .header("User-Agent", format!("quotify/{}", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| format!("网络错误: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let rel: GithubRelease = resp
        .into_body()
        .read_json()
        .map_err(|e| format!("解析失败: {e}"))?;
    if rel.draft || rel.prerelease {
        return Err("无正式版本".into());
    }
    Ok(ReleaseInfo { tag: rel.tag_name, url: rel.html_url })
}

/// 比较当前版本与远端 tag（均按 "v0.1.0" / "0.1.0" 宽松解析）。
pub fn is_newer(remote_tag: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split('.')
            .map(|p| p.trim().parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (r, c) = (parse(remote_tag), parse(current));
    for i in 0..r.len().max(c.len()) {
        let rv = r.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if rv != cv {
            return rv > cv;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "v0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
    }
}
