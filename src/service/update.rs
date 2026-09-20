//! Update check

use serde::Deserialize;

use crate::api::client::{MAX_BODY_BYTES, agent_long};

pub const REPO: &str = "tengmmvp/Quotify";

/// Upper bound (in chars) on the body prefix carried in HTTP error messages
/// Truncation char cap for GitHub error response bodies
/// A different value from the api-side envelope error cap; the distinct
/// name prevents mistaken reuse
const GITHUB_ERR_CHARS: usize = 120;

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    /// The officially encoded release page URL: a hand-assembled template
    /// would lose the tag's percent-encoding
    html_url: String,
}

/// Latest stable release
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag: String,
    pub url: String,
}

/// GitHub requires a User-Agent on every request: one owner for the
/// value, shared by the update check and the news fetch.
pub(crate) fn github_user_agent() -> String {
    format!("quotify/{}", env!("CARGO_PKG_VERSION"))
}

/// Query the latest release
pub fn check_latest() -> Result<ReleaseInfo, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = agent_long()
        .get(&url)
        .header("User-Agent", github_user_agent())
        .call()
        .map_err(|e| format!("network error: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        if status.as_u16() == 404 {
            // The repo is questionable or the release is not yet published;
            // give a readable message instead of a bare status code.
            return Err("release not found".into());
        }
        // Failure details (such as a 403 rate-limit Retry-After hint) live
        // in the body; carry a prefix into the message. Reading shares the
        // client's prefix reader with the error-tier cap — the success
        // cap would allocate up to 1 MiB only for chars().take(120) to
        // discard all but a slice.
        let body = crate::api::client::read_body_prefix(
            resp.into_body(),
            crate::api::client::ERR_BODY_BYTES,
        );
        let prefix: String = body.chars().take(GITHUB_ERR_CHARS).collect();
        return Err(if prefix.is_empty() {
            format!("HTTP {status}")
        } else {
            format!("HTTP {status}: {prefix}")
        });
    }
    let rel: GithubRelease = crate::api::client::read_json_capped(resp.into_body(), MAX_BODY_BYTES)
        .map_err(|e| format!("parse failed: {e}"))?;
    if rel.draft || rel.prerelease {
        return Err("no stable release".into());
    }
    // The download entry uses the official html_url (correctly encoded when
    // the tag contains #/non-ASCII); validate the host and path shape
    // rather than pinning the owner — after a repo rename/transfer GitHub
    // normalizes html_url to the new address, and pinning would leave
    // existing installs forever unable to reach new versions (the
    // self-update chicken-and-egg), while arbitrary redirect targets are
    // still blocked by the shape check.
    let u = rel.html_url.as_str();
    let ok =
        u.starts_with("https://github.com/") && u.contains("/releases/tag/") && !u.contains("..");
    if !ok {
        return Err(format!("unexpected release url: {u}"));
    }
    Ok(ReleaseInfo {
        tag: rel.tag_name,
        url: rel.html_url,
    })
}

/// Compare the current version against the remote tag; prerelease tags
/// never prompt automatically
pub fn is_newer(remote_tag: &str, current: &str) -> bool {
    if remote_tag.contains('-') {
        return false;
    }
    let parse = |s: &str| -> Vec<u64> {
        s.to_ascii_lowercase()
            .trim_start_matches('v')
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
        assert!(is_newer("V2.0.0", "1.9"));
        assert!(is_newer("2.0", "1.9.9"));
    }

    #[test]
    fn prerelease_tag_never_prompts() {
        assert!(!is_newer("v1.2.3-beta", "1.1.0"));
        assert!(!is_newer("v2.0.0-rc.1", "1.9"));
        assert!(!is_newer("1.2.3", "1.2.3"));
    }
}
