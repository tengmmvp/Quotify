//! 用量查询 HTTP 客户端

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use chrono::Timelike;
use serde_json::Value;

use super::{
    AccountSpec, Balance, ERR_BODY_CHARS, FetchError, Platform, TokenStats, UsageSnapshot,
    parse_response, parse_token_total,
};

/// body 读取硬上限
pub(crate) const MAX_BODY_BYTES: u64 = 1024 * 1024;

/// 双超时 Agent 缓存
struct AgentCache {
    proxy: Option<String>,
    short: ureq::Agent,
    long: ureq::Agent,
}

static AGENTS: RwLock<Option<AgentCache>> = RwLock::new(None);

fn agents_read() -> RwLockReadGuard<'static, Option<AgentCache>> {
    AGENTS.read().unwrap_or_else(|e| e.into_inner())
}

fn agents_write() -> RwLockWriteGuard<'static, Option<AgentCache>> {
    AGENTS.write().unwrap_or_else(|e| e.into_inner())
}

impl AgentCache {
    /// Agent 内部是 Arc 句柄，clone 只复制引用
    fn cloned(&self) -> AgentCache {
        AgentCache {
            proxy: self.proxy.clone(),
            short: self.short.clone(),
            long: self.long.clone(),
        }
    }
}

/// 缓存缺失时按无代理构建；读锁判空到取写锁之间可能被并发回填，须双检
fn agents() -> AgentCache {
    if let Some(c) = agents_read().as_ref() {
        return c.cloned();
    }
    let mut guard = agents_write();
    // 双检：等写锁期间别的线程可能已构建，直接复用避免重复建 Agent
    if let Some(c) = guard.as_ref() {
        return c.cloned();
    }
    let cache = AgentCache {
        proxy: None,
        short: build_agent(5, None),
        long: build_agent(15, None),
    };
    *guard = Some(cache.cloned());
    cache
}

fn agent_short() -> ureq::Agent {
    agents().short
}

pub(crate) fn agent_long() -> ureq::Agent {
    agents().long
}

/// 设置代理并重建两个 Agent
pub fn set_proxy(proxy: Option<String>) -> Result<(), String> {
    let proxy = proxy
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if agents_read().as_ref().is_some_and(|c| c.proxy == proxy) {
        return Ok(());
    }
    let parsed = match proxy.as_deref() {
        Some(p) => match ureq::Proxy::new(p) {
            Ok(px) => Some(px),
            Err(e) => return Err(e.to_string()),
        },
        None => None,
    };
    let short = build_agent(5, parsed.as_ref());
    let long = build_agent(15, parsed.as_ref());
    *agents_write() = Some(AgentCache { proxy, short, long });
    Ok(())
}

/// 统一配置：全局超时 + https_only；状态码不转错误，集中在 `http_get_text` 分类
fn build_agent(timeout_secs: u64, proxy: Option<&ureq::Proxy>) -> ureq::Agent {
    let mut builder = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .https_only(true)
        .http_status_as_error(false)
        // 空闲连接默认 15s 回收，短于轮询间隔致每轮重建连接；放宽让池子跨轮询存活
        .max_idle_age(Duration::from_secs(86400));
    builder = match proxy {
        Some(p) => builder.proxy(Some(p.clone())),
        None => builder.proxy(None),
    };
    builder.build().into()
}

/// 需要 Bearer 前缀的 key 集合[进程内记忆，存 key 的稳定 hash]：
/// 401 换形态重试成功后更新，下轮首轮即用对的凭证形态，免去每轮一次
/// 注定 401 的白费请求；切账号后 hash 不同自然隔离，无需清理
static NEEDS_BEARER: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// key → 稳定 hash。仅作进程内集合去重，无安全要求。
fn key_hash(key: &str) -> u64 {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

fn needs_bearer(key: &str) -> bool {
    NEEDS_BEARER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&key_hash(key))
}

/// 记忆/遗忘 key 的 Bearer 形态；锁中毒沿 AGENTS 惯例取内部数据继续用
fn set_needs_bearer(key: &str, on: bool) {
    let mut set = NEEDS_BEARER.lock().unwrap_or_else(|e| e.into_inner());
    if on {
        set.insert(key_hash(key));
    } else {
        set.remove(&key_hash(key));
    }
}

/// join 作用域线程；线程 panic 原样上抛，等价于无线程时的直接传播
fn join<T: Send>(handle: Option<std::thread::ScopedJoinHandle<'_, T>>) -> Option<T> {
    handle.map(|h| match h.join() {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    })
}

/// 查询一次用量快照
pub fn fetch_usage(spec: &AccountSpec) -> Result<UsageSnapshot, FetchError> {
    let team = spec.team_scope();
    // quota 是主数据，失败语义独立：Auth 换凭证形态重试，其余错误直接上抛
    let plain = spec.api_key.as_str();
    let bearer = format!("Bearer {plain}");
    let remembered = needs_bearer(plain);
    let (first, second) = if remembered {
        (bearer.as_str(), plain)
    } else {
        (plain, bearer.as_str())
    };
    let params = token_params(chrono::Local::now());
    // 四路全并行：主数据不被慢附加端点拖住。附加三路用记忆形态首发：
    // 切换轮可能 401 丢块（下轮即对）、主路失败轮照发白费（可忽略）、
    // 端点失败只缺块不拖垮主用量
    let (quota, today, week, balance) = std::thread::scope(|scope| {
        let quota = scope.spawn(|| match fetch_quota(spec.platform, first, team) {
            Ok(snap) => Ok(snap),
            Err(FetchError::Auth) => {
                let snap = fetch_quota(spec.platform, second, team)?;
                // 换形态重试成功：记忆本轮生效的形态，下轮首轮直用
                set_needs_bearer(plain, !remembered);
                Ok(snap)
            }
            Err(e) => Err(e),
        });
        // 时间窗换算失败时两路区间请求都不发
        let (today, week) = match params.as_ref() {
            Some((today, week, end)) => (
                Some(scope.spawn(|| fetch_token_window(spec.platform, first, team, today, end))),
                Some(scope.spawn(|| fetch_token_window(spec.platform, first, team, week, end))),
            ),
            None => (None, None),
        };
        // 余额端点仅国内版有
        let balance = (spec.platform == Platform::Cn).then(|| scope.spawn(|| fetch_balance(first)));
        let quota = quota
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p));
        (quota, join(today), join(week), join(balance))
    });
    let mut snap = quota?;
    // 两区间要么都有要么都没有
    snap.token_stats = match (today, week) {
        (Some(Ok(today)), Some(Ok(week))) => Some(TokenStats { today, week }),
        (Some(Err(e)), _) | (_, Some(Err(e))) => {
            crate::platform::log(&format!("[Quotify] Token 统计拉取失败: {e}"));
            None
        }
        _ => None,
    };
    if let Some(balance) = balance {
        match balance {
            Ok(b) => snap.balance = b,
            Err(e) => crate::platform::log(&format!("[Quotify] 余额拉取失败: {e}")),
        }
    }
    Ok(snap)
}

/// Token 统计区间参数：今日与 7 天前 0 点两起点 + 当前小时末终点，
/// 空格已编码可直拼 URL；起点换算失败如 DST 交界返 None，两路请求
/// 都不发，终点失败则退化为当前刻。
fn token_params(now: chrono::DateTime<chrono::Local>) -> Option<(String, String, String)> {
    let end = now
        .with_minute(59)
        .and_then(|t| t.with_second(59))
        .unwrap_or(now);
    let today_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|t| chrono::TimeZone::from_local_datetime(&chrono::Local, &t).single());
    let week_start = today_start.map(|t| t - chrono::Duration::days(7));
    let (Some(today_start), Some(week_start)) = (today_start, week_start) else {
        return None;
    };
    Some((stamp(&today_start), stamp(&week_start), stamp(&end)))
}

/// 拉取单个区间的 token 总消耗，合计优先取服务端 totalUsage
fn fetch_token_window(
    platform: Platform,
    api_key: &str,
    team: Option<(&str, &str)>,
    start: &str,
    end: &str,
) -> Result<f64, FetchError> {
    let mut query = format!("?startTime={start}&endTime={end}");
    if team.is_some() {
        query.push_str("&type=3");
    }
    let url = format!(
        "{}/api/monitor/usage/model-usage{query}",
        platform.base_url()
    );
    let mut extra = Vec::new();
    if let Some((org, project)) = team {
        extra.push(("Bigmodel-Organization", org));
        extra.push(("Bigmodel-Project", project));
    }
    let body = http_get_text(&agent_short(), &url, api_key, &extra)?;
    parse_token_total(&body)
}

/// 时间戳 → `YYYY-MM-DD HH:mm:ss`，空格百分号编码后可直接拼 URL
fn stamp(dt: &chrono::DateTime<chrono::Local>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S")
        .to_string()
        .replace(' ', "%20")
}

/// 账户余额[仅国内版]
fn fetch_balance(api_key: &str) -> Result<Option<Balance>, FetchError> {
    let url = "https://www.bigmodel.cn/api/biz/account/query-customer-account-report";
    let body = http_get(url, api_key)?;
    Ok(parse_balance(&body))
}

/// 余额报告响应解析：availableBalance 优先、balance 回退，两者皆缺
/// 说明形态未知宁缺毋假；数值字段非有限数按缺处理。
fn parse_balance(v: &Value) -> Option<Balance> {
    let data = v.get("data").filter(|d| d.is_object())?;
    let num = |k: &str| {
        data.get(k)
            .and_then(Value::as_f64)
            .filter(|x| x.is_finite())
    };
    let available = num("availableBalance").or_else(|| num("balance"))?;
    Some(Balance {
        available,
        recharged: num("rechargeAmount"),
        granted: num("giveAmount"),
        spent: num("totalSpendAmount"),
    })
}

fn http_get(url: &str, api_key: &str) -> Result<Value, FetchError> {
    let body = http_get_text(&agent_short(), url, api_key, &[])?;
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
        // 分类函数只在 2xx 返回 None，此处必非 2xx；兜底防分类逻辑与分支漂移后 panic
        return Err(classify_status(status, &prefix)
            .unwrap_or_else(|| FetchError::Api(format!("HTTP {status}"))));
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
    let body_text = http_get_text(&agent_long(), &url, auth, &extra)?;
    parse_response(&body_text)
}

#[cfg(test)]
mod tests {
    use super::{classify_status, parse_balance, token_params};
    use crate::api::FetchError;
    use serde_json::json;

    #[test]
    fn balance_report_shapes() {
        // 字段齐：availableBalance 优先，三个次级字段随行
        let b = parse_balance(&json!({
            "data": {
                "availableBalance": 12.5,
                "balance": 99.0,
                "rechargeAmount": 20.0,
                "giveAmount": 5.0,
                "totalSpendAmount": 12.5,
            }
        }))
        .unwrap();
        assert_eq!(b.available, 12.5);
        assert_eq!(b.recharged, Some(20.0));
        assert_eq!(b.granted, Some(5.0));
        assert_eq!(b.spent, Some(12.5));
        // 无 availableBalance 时回退 balance
        let b = parse_balance(&json!({ "data": { "balance": 7.0 } })).unwrap();
        assert_eq!(b.available, 7.0);
        assert_eq!(b.recharged, None);
        // 两者皆缺：形态未知宁缺毋假
        assert!(parse_balance(&json!({ "data": {} })).is_none());
        // data 非对象 / 缺失同 None
        assert!(parse_balance(&json!({ "data": 3 })).is_none());
        assert!(parse_balance(&json!({})).is_none());
        // 非有限数按缺处理
        let b =
            parse_balance(&json!({ "data": { "balance": 3.0, "rechargeAmount": null } })).unwrap();
        assert_eq!(b.recharged, None);
    }

    #[test]
    fn token_window_params_shape() {
        use chrono::TimeZone;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 8, 29, 20, 15, 30)
            .unwrap();
        let (today, week, end) = token_params(now).unwrap();
        // 三值均已百分号编码；终点取当前小时末
        assert_eq!(today, "2026-08-29%2000:00:00");
        assert_eq!(week, "2026-08-22%2000:00:00");
        assert_eq!(end, "2026-08-29%2020:59:59");
    }

    #[test]
    fn status_classification() {
        assert!(classify_status(200, "").is_none());
        assert!(classify_status(204, "").is_none());
        assert!(matches!(
            classify_status(401, "denied"),
            Some(FetchError::Auth)
        ));
        assert!(matches!(classify_status(403, ""), Some(FetchError::Auth)));
        assert!(matches!(
            classify_status(429, "Too Many Requests"),
            Some(FetchError::Network(_))
        ));
        assert!(matches!(
            classify_status(500, ""),
            Some(FetchError::Network(_))
        ));
        assert!(matches!(
            classify_status(503, "busy"),
            Some(FetchError::Network(_))
        ));
        // 其余非 2xx 为确定性业务错误
        assert!(matches!(
            classify_status(404, "nope"),
            Some(FetchError::Api(_))
        ));
        assert!(matches!(classify_status(400, ""), Some(FetchError::Api(_))));
        // detail 拼接格式
        assert_eq!(
            classify_status(429, "slow down").unwrap().to_string(),
            "HTTP 429: slow down"
        );
    }
}
