//! 用量查询 HTTP 客户端。国内 / 国际版共用同一实现，仅 base 域名不同。

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

use super::{Balance, FetchError, UsageSnapshot, parse_response};

/// 响应体读取硬上限：用量 / 余额 / Release 均为 KB 级 JSON，1 MiB 绰绰
/// 有余；封顶防御异常超大响应（错误页 / 恶意服务端）耗尽内存。
/// `service::update` 的 read_json 同样套用此上限。
pub(crate) const MAX_BODY_BYTES: u64 = 1024 * 1024;
/// 非 2xx 错误消息携带的 body 前缀长度（字符数）上限，防超长错误页刷屏。
const ERR_BODY_CHARS: usize = 160;

/// 5s 短超时 Agent（余额等辅助请求）。静态复用连接池：TLS 会话跨轮询
/// 保持，避免每轮询 2-3 次完整握手；`https_only` 固化仅 HTTPS 约束。
static AGENT_SHORT: OnceLock<ureq::Agent> = OnceLock::new();
/// 15s 长超时 Agent（主用量请求；`service::update` 检查更新复用）。
static AGENT_LONG: OnceLock<ureq::Agent> = OnceLock::new();

fn agent_short() -> &'static ureq::Agent {
    AGENT_SHORT.get_or_init(|| build_agent(5))
}

/// 长超时共享 Agent（检查更新复用）。
pub(crate) fn agent_long() -> &'static ureq::Agent {
    AGENT_LONG.get_or_init(|| build_agent(15))
}

/// 统一 Agent 配置：全局超时 + 仅 HTTPS；状态码不转错误，统一在
/// `http_get_text` 里集中分类。
fn build_agent(timeout_secs: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .https_only(true)
        .http_status_as_error(false)
        .build()
        .into()
}

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

/// 一次查询的账号参数。团队版仅国内站：quota/limit 追加 `?type=2` 并带
/// `Bigmodel-Organization` / `Bigmodel-Project` 两个选择头（cc-switch #4222
/// 实测：API Key + type=2 + 两头，三者缺一不可；缺 selector 时接口返回
/// success + 空 limits）。
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
    /// 团队查询是否生效（标记了团队且两个选择头齐全；不全时按个人版查询）。
    fn team_scope(&self) -> Option<(&str, &str)> {
        let org = self.org_id.trim();
        let project = self.project_id.trim();
        (!org.is_empty() && !project.is_empty()).then_some((org, project))
    }
}

/// 查询一次用量快照。
///
/// 主数据来自 `quota/limit`；国内版尽力附带账户余额——扩展数据失败
/// 不影响主数据返回。
///
/// 认证注意：智谱 monitor 系列接口的 `Authorization` 头直接放 API key，
/// **不加 Bearer 前缀**（官方插件、cc-switch、opencode-glm-quota 三方实测一致；
/// CodexBar 则带 Bearer 也能通过——服务端两种格式都收）。这里无 Bearer 优先，
/// 收到 401/403 时再带 Bearer 重试一次，双格式兜底。
pub fn fetch_usage(spec: &AccountSpec) -> Result<UsageSnapshot, FetchError> {
    let team = spec.team_scope();
    let mut snap = match fetch_quota(spec.platform, &spec.api_key, team) {
        Err(FetchError::Auth(_)) => fetch_quota(spec.platform, &format!("Bearer {}", spec.api_key), team)?,
        other => other?,
    };
    if spec.platform == Platform::Cn {
        snap.balance = fetch_balance(&spec.api_key).ok();
    }
    Ok(snap)
}

/// 账户余额（仅国内版；`www.bigmodel.cn` 控制台端点）。
fn fetch_balance(api_key: &str) -> Result<Balance, FetchError> {
    let url = "https://www.bigmodel.cn/api/biz/account/query-customer-account-report";
    let body = http_get(url, api_key)?;
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

/// GET + JSON 解析（余额等辅助请求的薄封装，走 5s 短超时 Agent）。
fn http_get(url: &str, api_key: &str) -> Result<Value, FetchError> {
    let body = http_get_text(agent_short(), url, api_key, &[])?;
    serde_json::from_str(&body).map_err(|e| FetchError::Api(format!("响应解析失败: {e}")))
}

/// 共用请求底层：GET → 状态码分类 → 返回 body 文本。
/// 401/403 归 `Auth`（触发 Bearer 重试与设置页修复提示），其余非 2xx
/// 归 `Api`（错误消息携带截断到 `ERR_BODY_CHARS` 的 body 前缀）。
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
    let resp = req.call().map_err(|e| FetchError::Network(format!("网络错误: {e}")))?;
    let status = resp.status().as_u16();
    if status == 401 || status == 403 {
        return Err(FetchError::Auth("API key 无效或已失效".into()));
    }
    if !(200..=299).contains(&status) {
        // 错误路径 body 只进消息前缀，读取失败按空串处理
        let body = read_body_capped(resp.into_body()).unwrap_or_default();
        let prefix: String = body.chars().take(ERR_BODY_CHARS).collect();
        return Err(FetchError::Api(format!("HTTP {status}: {prefix}")));
    }
    read_body_capped(resp.into_body()).map_err(|e| FetchError::Network(format!("读取响应失败: {e}")))
}

/// 按全局上限读取 body 文本；无效 UTF-8 以 `?` 替换（与 ureq 默认
/// `read_to_string` 行为一致）。
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
    // 主请求 15s 长超时 Agent；信封/limits 解析在 parse_response
    let body_text = http_get_text(agent_long(), &url, auth, &extra)?;
    parse_response(&body_text)
}
