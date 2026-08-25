//! 用量查询 HTTP 客户端

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

use super::{Balance, FetchError, UsageSnapshot, parse_response};

/// body 读取硬上限
pub(crate) const MAX_BODY_BYTES: u64 = 1024 * 1024;
/// 非 2xx 错误消息携带的 body 前缀长度（字符数）上限
const ERR_BODY_CHARS: usize = 160;

/// 5s 短超时 Agent
static AGENT_SHORT: OnceLock<ureq::Agent> = OnceLock::new();
/// 15s 长超时 Agent
static AGENT_LONG: OnceLock<ureq::Agent> = OnceLock::new();

fn agent_short() -> &'static ureq::Agent {
    AGENT_SHORT.get_or_init(|| build_agent(5))
}

pub(crate) fn agent_long() -> &'static ureq::Agent {
    AGENT_LONG.get_or_init(|| build_agent(15))
}

/// 统一配置：全局超时 + https_only；状态码不转错误，集中在 `http_get_text` 分类
fn build_agent(timeout_secs: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .https_only(true)
        .http_status_as_error(false)
        .build()
        .into()
}

/// API 平台
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// 国内版 open.bigmodel.cn
    Cn,
    /// 国际版 api.z.ai
    Intl,
}

impl Platform {
    pub fn base_url(self) -> &'static str {
        match self {
            Platform::Cn => "https://open.bigmodel.cn",
            Platform::Intl => "https://api.z.ai",
        }
    }
}

/// 一次查询的账号参数。团队版仅国内站：API Key + `?type=2` + 两个
/// 选择头缺一不可；缺 selector 回 success + 空 limits
#[derive(Debug, Clone)]
pub struct AccountSpec {
    pub platform: Platform,
    pub api_key: String,
    /// 团队版：组织 ID（`Bigmodel-Organization`）
    pub org_id: String,
    /// 团队版：项目 ID（`Bigmodel-Project`）
    pub project_id: String,
}

impl AccountSpec {
    /// 两个选择头齐全才按团队查询，不全时按个人版
    fn team_scope(&self) -> Option<(&str, &str)> {
        let org = self.org_id.trim();
        let project = self.project_id.trim();
        (!org.is_empty() && !project.is_empty()).then_some((org, project))
    }
}

/// 查询一次用量快照
pub fn fetch_usage(spec: &AccountSpec) -> Result<UsageSnapshot, FetchError> {
    let team = spec.team_scope();
    let mut snap = match fetch_quota(spec.platform, &spec.api_key, team) {
        Err(FetchError::Auth) => {
            fetch_quota(spec.platform, &format!("Bearer {}", spec.api_key), team)?
        }
        other => other?,
    };
    if spec.platform == Platform::Cn {
        snap.balance = fetch_balance(&spec.api_key).ok();
    }
    Ok(snap)
}

/// 账户余额，仅国内版
fn fetch_balance(api_key: &str) -> Result<Balance, FetchError> {
    let url = "https://www.bigmodel.cn/api/biz/account/query-customer-account-report";
    let body = http_get(url, api_key)?;
    let data = body
        .get("data")
        .ok_or_else(|| FetchError::Api("balance response missing data".into()))?;
    let num = |k: &str| {
        data.get(k)
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
    };
    // availableBalance 优先，回退 balance
    let available = num("availableBalance").or_else(|| num("balance")).unwrap_or(0.0);
    Ok(Balance {
        available,
        recharged: num("rechargeAmount"),
        granted: num("giveAmount"),
        spent: num("totalSpendAmount"),
    })
}

fn http_get(url: &str, api_key: &str) -> Result<Value, FetchError> {
    let body = http_get_text(agent_short(), url, api_key, &[])?;
    serde_json::from_str(&body).map_err(|e| FetchError::Api(format!("parse failed: {e}")))
}

/// 共用请求底层
fn http_get_text(
    agent: &ureq::Agent,
    url: &str,
    auth: &str,
    extra_headers: &[(&str, &str)],
) -> Result<String, FetchError> {
    let mut req = agent
        .get(url)
        .header("Authorization", auth)
        .header("Accept-Language", "en-US,en")
        .header("Content-Type", "application/json");
    for &(name, value) in extra_headers {
        req = req.header(name, value);
    }
    let resp = req.call().map_err(|e| FetchError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if !(200..=299).contains(&status) {
        // 错误路径 body 只进消息前缀，读取失败按空串处理
        let body = read_body_capped(resp.into_body()).unwrap_or_default();
        let prefix: String = body.chars().take(ERR_BODY_CHARS).collect();
        return Err(classify_status(status, &prefix).expect("非 2xx 必有分类"));
    }
    read_body_capped(resp.into_body()).map_err(|e| FetchError::Network(e.to_string()))
}

/// 非 2xx 状态分类：401/403 → `Auth`，429/5xx → `Network`，其余 → `Api`
fn classify_status(status: u16, detail: &str) -> Option<FetchError> {
    if status == 401 || status == 403 {
        return Some(FetchError::Auth);
    }
    if (200..=299).contains(&status) {
        return None;
    }
    let text = if detail.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {detail}")
    };
    Some(if status == 429 || status >= 500 {
        FetchError::Network(text)
    } else {
        FetchError::Api(text)
    })
}

/// 按上限读 body；无效 UTF-8 以 `?` 替换，对齐 ureq 默认行为
fn read_body_capped(body: ureq::Body) -> Result<String, ureq::Error> {
    body.into_with_config()
        .limit(MAX_BODY_BYTES)
        .lossy_utf8(true)
        .read_to_string()
}

fn fetch_quota(
    platform: Platform,
    auth: &str,
    team: Option<(&str, &str)>,
) -> Result<UsageSnapshot, FetchError> {
    let mut url = format!("{}/api/monitor/usage/quota/limit", platform.base_url());
    if team.is_some() {
        url.push_str("?type=2");
    }
    let mut extra = Vec::new();
    if let Some((org, project)) = team {
        extra.push(("Bigmodel-Organization", org));
        extra.push(("Bigmodel-Project", project));
    }
    let body_text = http_get_text(agent_long(), &url, auth, &extra)?;
    parse_response(&body_text)
}

#[cfg(test)]
mod tests {
    use super::classify_status;
    use crate::api::FetchError;

    #[test]
    fn status_classification() {
        assert!(classify_status(200, "").is_none());
        assert!(classify_status(204, "").is_none());
        assert!(matches!(classify_status(401, "denied"), Some(FetchError::Auth)));
        assert!(matches!(classify_status(403, ""), Some(FetchError::Auth)));
        assert!(matches!(classify_status(429, "Too Many Requests"), Some(FetchError::Network(_))));
        assert!(matches!(classify_status(500, ""), Some(FetchError::Network(_))));
        assert!(matches!(classify_status(503, "busy"), Some(FetchError::Network(_))));
        // 其余非 2xx 为确定性业务错误
        assert!(matches!(classify_status(404, "nope"), Some(FetchError::Api(_))));
        assert!(matches!(classify_status(400, ""), Some(FetchError::Api(_))));
        // detail 拼接格式
        assert_eq!(
            classify_status(429, "slow down").unwrap().to_string(),
            "HTTP 429: slow down"
        );
    }
}
