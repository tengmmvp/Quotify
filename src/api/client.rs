//! 用量查询 HTTP 客户端。国内 / 国际版共用同一实现，仅 base 域名不同。

use std::time::Duration;

use serde_json::Value;

use super::{Balance, FetchError, ModelUsage, UsageSnapshot, parse_response};

/// API 平台。
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

    /// 配置/UI 用的短标签。
    pub fn platform_tag(self) -> &'static str {
        match self {
            Platform::Cn => "cn",
            Platform::Intl => "intl",
        }
    }
}

/// 查询一次用量快照。
///
/// 主数据来自 `quota/limit`；成功后尽力附带 `model-usage`（近 24h 模型消耗）
/// 与账户余额（仅国内版）——扩展数据失败不影响主数据返回。
///
/// 认证注意：智谱 monitor 系列接口的 `Authorization` 头直接放 API key，
/// **不加 Bearer 前缀**（官方插件、cc-switch、opencode-glm-quota 三方实测一致；
/// CodexBar 则带 Bearer 也能通过——服务端两种格式都收）。这里无 Bearer 优先，
/// 收到 401/403 时再带 Bearer 重试一次，双格式兜底。
pub fn fetch_usage(platform: Platform, api_key: &str) -> Result<UsageSnapshot, FetchError> {
    let mut snap = match fetch_quota(platform, &format!("{api_key}")) {
        Err(FetchError::Auth(_)) => fetch_quota(platform, &format!("Bearer {api_key}"))?,
        other => other?,
    };
    snap.model_usage = fetch_model_usage(platform, api_key).ok().flatten();
    if platform == Platform::Cn {
        snap.balance = fetch_balance(api_key).ok();
    }
    Ok(snap)
}

/// 近 24h 模型用量（时间窗：昨天此刻 → 今天此刻，官方插件同款窗口）。
fn fetch_model_usage(
    platform: Platform,
    api_key: &str,
) -> Result<Option<ModelUsage>, FetchError> {
    use chrono::Local;
    let fmt = |t: chrono::DateTime<Local>| t.format("%Y-%m-%d %H:%M:%S").to_string();
    let end = Local::now();
    let start = end - chrono::Duration::hours(24);
    let url = format!(
        "{}/api/monitor/usage/model-usage?startTime={}&endTime={}",
        platform.base_url(),
        urlencode(&fmt(start)),
        urlencode(&fmt(end)),
    );
    let body = http_get(&url, api_key, 5)?;

    let data = body
        .get("data")
        .ok_or_else(|| FetchError::Api("model-usage 缺少 data".into()))?;
    let labels = data.get("x_time").and_then(Value::as_array).cloned().unwrap_or_default();
    let models = data.get("modelDataList").and_then(Value::as_array).cloned().unwrap_or_default();

    // 各时段合计
    let mut series = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        let total: i64 = models
            .iter()
            .filter_map(|m| m.get("tokensUsage").and_then(Value::as_array))
            .filter_map(|arr| arr.get(i))
            .filter_map(Value::as_i64)
            .filter(|v| *v > 0)
            .sum();
        if total > 0 {
            series.push((label.as_str().unwrap_or("").to_string(), total));
        }
    }
    // 各模型合计，降序
    let mut by_model: Vec<(String, i64)> = models
        .iter()
        .filter_map(|m| {
            let name = m.get("modelName").and_then(Value::as_str)?.to_string();
            let total: i64 = m
                .get("tokensUsage")
                .and_then(Value::as_array)?
                .iter()
                .filter_map(Value::as_i64)
                .filter(|v| *v > 0)
                .sum();
            Some((name, total))
        })
        .filter(|(_, v)| *v > 0)
        .collect();
    by_model.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    if series.is_empty() && by_model.is_empty() {
        return Ok(None);
    }
    Ok(Some(ModelUsage { series, by_model }))
}

/// 账户余额（仅国内版；`www.bigmodel.cn` 控制台端点）。
fn fetch_balance(api_key: &str) -> Result<Balance, FetchError> {
    let url = "https://www.bigmodel.cn/api/biz/account/query-customer-account-report";
    let body = http_get(url, api_key, 5)?;
    let data = body
        .get("data")
        .ok_or_else(|| FetchError::Api("余额响应缺少 data".into()))?;
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

/// GET + JSON 解析（共用小工具）。
fn http_get(url: &str, api_key: &str, timeout_secs: u64) -> Result<Value, FetchError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .http_status_as_error(false)
        .build()
        .into();
    let resp = agent
        .get(url)
        .header("Authorization", api_key)
        .header("Accept-Language", "en-US,en")
        .header("Content-Type", "application/json")
        .call()
        .map_err(|e| FetchError::Network(format!("网络错误: {e}")))?;
    let status = resp.status().as_u16();
    if status == 401 || status == 403 {
        return Err(FetchError::Auth("API key 无效或已失效".into()));
    }
    if !(200..=299).contains(&status) {
        return Err(FetchError::Api(format!("HTTP {status}")));
    }
    resp.into_body()
        .read_json()
        .map_err(|e| FetchError::Api(format!("响应解析失败: {e}")))
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn fetch_quota(platform: Platform, auth: &str) -> Result<UsageSnapshot, FetchError> {
    let url = format!("{}/api/monitor/usage/quota/limit", platform.base_url());
    // 主请求 15s 超时；信封/limits 解析在 parse_response
    let body_text = {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .build()
            .into();
        let resp = agent
            .get(&url)
            .header("Authorization", auth)
            .header("Accept-Language", "en-US,en")
            .header("Content-Type", "application/json")
            .call()
            .map_err(|e| FetchError::Network(format!("网络错误: {e}")))?;
        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return Err(FetchError::Auth("API key 无效或已失效".into()));
        }
        if !(200..=299).contains(&status) {
            let body = resp.into_body().read_to_string().unwrap_or_default();
            return Err(FetchError::Api(format!("HTTP {status}: {body}")));
        }
        resp.into_body()
            .read_to_string()
            .map_err(|e| FetchError::Network(format!("读取响应失败: {e}")))?
    };
    parse_response(&body_text)
}
