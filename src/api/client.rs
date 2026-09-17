//! 用量查询 HTTP 客户端

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
    AccountSpec, Balance, ERR_BODY_CHARS, ErrorInfo, FetchError, Platform, TOKEN_LEGS, TokenStats,
    UsageSnapshot, parse_response, parse_token_total,
};

/// 响应体读取硬上限
pub(crate) const MAX_BODY_BYTES: u64 = 1024 * 1024;
/// 错误响应体读取上限
const ERR_BODY_BYTES: u64 = 64 * 1024;
/// 官方提示的字符上限
const ERR_MSG_CHARS: usize = 100;

/// 长短超时双 Agent 缓存
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
    /// Agent 缓存的廉价克隆
    /// 内部是 Arc 句柄，克隆只复制引用。
    fn cloned(&self) -> AgentCache {
        AgentCache {
            proxy: self.proxy.clone(),
            short: self.short.clone(),
            long: self.long.clone(),
        }
    }
}

/// 取全局 Agent 缓存
/// 缓存缺失时按无代理构建。
fn agents() -> AgentCache {
    if let Some(c) = agents_read().as_ref() {
        return c.cloned();
    }
    let mut guard = agents_write();
    // 双检：等写锁期间别的线程可能已构建，直接复用避免重复建 Agent。
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

/// 校验代理地址形态
/// 显式 http:// 或 socks5:// 前缀、host 非空、端口 1–65535。
/// ureq 会默认补前缀且不校验端口，须自行解析。
pub fn proxy_valid(s: &str) -> Result<(), String> {
    let lower = s.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("socks5://"))
        .ok_or("must start with http:// or socks5://")?;
    let hostport = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest);
    let (host, port) = match hostport.rfind(']') {
        Some(i) => (&hostport[..=i], hostport[i + 1..].strip_prefix(':')),
        None => hostport
            .rsplit_once(':')
            .map_or((hostport, None), |(h, p)| (h, Some(p))),
    };
    if host.is_empty() {
        return Err("empty host".into());
    }
    if let Some(p) = port {
        let n: u32 = p.parse().map_err(|_| "invalid port")?;
        if !(1..=65535).contains(&n) {
            return Err("port out of range".into());
        }
    }
    Ok(())
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

/// Agent 统一配置
/// 全局超时与 https_only。状态码不转错误，集中在 `http_get_text` 分类。
fn build_agent(timeout_secs: u64, proxy: Option<&ureq::Proxy>) -> ureq::Agent {
    let mut builder = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .https_only(true)
        .http_status_as_error(false)
        // 空闲连接默认 15s 回收，短于轮询间隔致每轮重建连接；放宽让池子跨轮询存活。
        .max_idle_age(Duration::from_secs(86400));
    builder = match proxy {
        Some(p) => builder.proxy(Some(p.clone())),
        None => builder.proxy(None),
    };
    builder.build().into()
}

/// 需要 Bearer 前缀的 key 集合
/// 401 换形态重试成功后记忆，下轮首轮即用对的凭证形态。
static NEEDS_BEARER: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// key → 稳定 hash
fn key_hash(key: &str) -> u64 {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

/// 查询 key 是否已记忆为需 Bearer 形态
fn needs_bearer(key: &str) -> bool {
    NEEDS_BEARER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&key_hash(key))
}

/// 记忆或遗忘 key 的 Bearer 形态
/// 锁中毒时数据仍完好，取内部数据继续用。
fn set_needs_bearer(key: &str, on: bool) {
    let mut set = NEEDS_BEARER.lock().unwrap_or_else(|e| e.into_inner());
    if on {
        set.insert(key_hash(key));
    } else {
        set.remove(&key_hash(key));
    }
}

/// join 作用域线程
/// 线程 panic 原样上抛，等价于无线程时的直接传播。
fn join<T: Send>(handle: Option<std::thread::ScopedJoinHandle<'_, T>>) -> Option<T> {
    handle.map(|h| match h.join() {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    })
}

/// 余额端点日志状态变化才记：同账号同状态不重复记，恢复清底再故障
/// 重记；键含 key 指纹，账号间互不遮蔽
fn log_balance_state(key: &str, line: Option<String>) {
    static LAST: LazyLock<Mutex<(u64, Option<String>)>> = LazyLock::new(|| Mutex::new((0, None)));
    let id = key_hash(key);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.0 == id && last.1 == line {
        return;
    }
    if let Some(line) = line.as_deref() {
        crate::platform::log(line);
    }
    *last = (id, line);
}

/// 查询一次用量快照
pub fn fetch_usage(spec: &AccountSpec) -> Result<UsageSnapshot, FetchError> {
    let team = spec.team_scope();
    // quota 是主数据，失败语义独立。Auth 换凭证形态重试，其余错误直接上抛。
    let plain = spec.api_key.as_str();
    let bearer = format!("Bearer {plain}");
    let remembered = needs_bearer(plain);
    let (first, second) = if remembered {
        (bearer.as_str(), plain)
    } else {
        (plain, bearer.as_str())
    };
    let TokenWindows { starts, end } = token_params(chrono::Local::now());
    // 五路并行：主数据不被慢附加端点拖住，附加路失败只缺块不拖垮主用量。
    // 换形态那轮附加端点可能 401 丢块，下轮记忆形态后恢复。
    let (quota, legs, balance) = std::thread::scope(|scope| {
        let quota = scope.spawn(|| match fetch_quota(spec.platform, first, team) {
            Ok(snap) => Ok(snap),
            // 换形态重试只认 401/403 状态：信封鉴权失败发生在 HTTP 200，
            // 凭证形态已被接受，重试无意义。
            Err(FetchError::Unauthenticated(info)) if is_auth_status(info.status) => {
                let snap = fetch_quota(spec.platform, second, team)?;
                // 换形态重试成功：记忆本轮生效的形态，下轮首轮直用。
                set_needs_bearer(plain, !remembered);
                Ok(snap)
            }
            Err(e) => Err(e),
        });
        let end = end.as_str();
        // 档机制单源：增删档位只改 TOKEN_LEGS。
        let legs: [_; TOKEN_LEGS.len()] = starts.map(|s| {
            s.map(|s| scope.spawn(move || fetch_token_window(spec.platform, first, team, &s, end)))
        });
        // 余额端点仅国内版有。
        let balance = (spec.platform == Platform::Cn).then(|| scope.spawn(|| fetch_balance(first)));
        let quota = quota
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p));
        (quota, legs.map(join), join(balance))
    });
    let mut snap = quota?;
    // 逐档独立：单档失败不拖垮其余档，三档全缺才整体隐藏。
    // 失败档聚合一行日志，持久故障不刷屏。
    let failed: Vec<String> = TOKEN_LEGS
        .iter()
        .zip(&legs)
        .filter_map(|((_, _, tag), leg)| match leg {
            Some(Err(e)) => Some(format!("{tag}: {e}")),
            _ => None,
        })
        .collect();
    if !failed.is_empty() {
        crate::platform::log(&format!(
            "[Quotify] Token 档拉取失败: {}",
            failed.join("; ")
        ));
    }
    // 子端点 429 不单独退避：主端点健康即按常规节奏，下一轮自然重试。
    // 任一档在场即块的活跃时刻。
    let stats = TokenStats {
        legs: legs.map(|l| l.and_then(Result::ok)),
        as_of: chrono::Utc::now(),
    };
    if stats.legs.iter().any(Option::is_some) {
        snap.token_stats = Some(stats);
    }
    if let Some(balance) = balance {
        match balance {
            Ok(Some(b)) => {
                snap.balance = Some(b);
                log_balance_state(plain, None);
            }
            // 信封失败与形态不符同按无数据处理：日志留痕，旧值由回填沿用。
            Ok(None) => log_balance_state(plain, Some("[Quotify] 余额响应无有效数据".into())),
            Err(e) => log_balance_state(plain, Some(format!("[Quotify] 余额拉取失败: {e}"))),
        }
    }
    Ok(snap)
}

/// Token 统计的三区间时间窗
/// 三起点按 TOKEN_LEGS 档序、终点共用。单窗起点本地化失败仅该窗缺席，
/// 不牵连其余窗。
struct TokenWindows {
    starts: [Option<String>; TOKEN_LEGS.len()],
    end: String,
}

/// Token 统计区间参数
/// 今日、6 天前、29 天前 0 点三起点加当天末终点，周/月窗含今天
/// 共 7 / 30 个自然日，月非自然月。起点须在日期侧减天数再本地化，
/// DateTime 直接减 Duration 是 UTC 瞬时运算，夏令时交界会漂移一小时。
fn token_params(now: chrono::DateTime<chrono::Local>) -> TokenWindows {
    // 夏令时回拨致 0 点歧义时取早侧：宁早一小时，不缺整窗。
    let day_start = |offset: u64, tag: &str| {
        let t = (now.date_naive() - chrono::Days::new(offset))
            .and_hms_opt(0, 0, 0)
            .and_then(|t| chrono::TimeZone::from_local_datetime(&chrono::Local, &t).earliest());
        if t.is_none() {
            crate::platform::log(&format!("[Quotify] Token {tag} 窗口起点本地化失败，跳过"));
        }
        t
    };
    // 夏令时回拨致当天末歧义时取晚侧：宁晚一小时，不短整窗。
    let end = now
        .date_naive()
        .and_hms_opt(23, 59, 59)
        .and_then(|t| chrono::TimeZone::from_local_datetime(&chrono::Local, &t).latest())
        .unwrap_or(now);
    TokenWindows {
        starts: TOKEN_LEGS.map(|(_, back, tag)| day_start(back, tag).map(|t| stamp(&t))),
        end: stamp(&end),
    }
}

/// 拉取单个区间的 token 总消耗
/// 合计优先取服务端 totalUsage。
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

/// 时间戳转 `YYYY-MM-DD HH:mm:ss`
/// 空格百分号编码后可直接拼 URL。
fn stamp(dt: &chrono::DateTime<chrono::Local>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S")
        .to_string()
        .replace(' ', "%20")
}

/// 账户余额查询[仅国内版]
fn fetch_balance(api_key: &str) -> Result<Option<Balance>, FetchError> {
    let url = "https://www.bigmodel.cn/api/biz/account/query-customer-account-report";
    let body = http_get(url, api_key)?;
    Ok(parse_balance(&body))
}

/// 余额报告响应解析
/// 信封校验与配额侧 envelope_check 同源，标记失败即丢弃，由落库
/// 回填沿用旧值。availableBalance 优先、balance 回退，非有限数与
/// 两值皆缺按无数据处理。
fn parse_balance(v: &Value) -> Option<Balance> {
    if super::envelope_check(v).is_err() {
        return None;
    }
    let data = v.get("data").filter(|d| d.is_object())?;
    let num = |k: &str| {
        data.get(k)
            .and_then(Value::as_f64)
            .filter(|x| x.is_finite())
    };
    let available = num("availableBalance").or_else(|| num("balance"))?;
    Some(Balance {
        available,
        as_of: Utc::now(),
        recharged: num("rechargeAmount"),
        granted: num("giveAmount"),
        spent: num("totalSpendAmount"),
    })
}

/// GET 并解析 JSON
fn http_get(url: &str, api_key: &str) -> Result<Value, FetchError> {
    let body = http_get_text(&agent_short(), url, api_key, &[])?;
    serde_json::from_str(&body)
        .map_err(|e| FetchError::Failure(ErrorInfo::local(format!("parse failed: {e}"))))
}

/// 发送 GET 返回响应体文本
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
        // 429 单独归类，Retry-After 头须在 into_body 消费前读取。
        let retry_after_secs = (status == 429)
            .then(|| {
                resp.headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_retry_after)
            })
            .flatten();
        let body = read_body_prefix(resp.into_body(), ERR_BODY_BYTES);
        let (code, message) = error_body_fields(&body);
        return Err(classify_status(status, code, message, retry_after_secs));
    }
    read_body_capped(resp.into_body(), MAX_BODY_BYTES)
        .map_err(|e| FetchError::Network(e.to_string()))
}

/// 非 2xx 响应体的字段提取
/// 官方形态为 `{"error":{"code","message"}}`，code 解析同引
/// code_value 单源，message 截到字符上限。形态不符回退截断原文。
fn error_body_fields(body: &str) -> (Option<i64>, String) {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            // 非对象 error 形态与缺失同走原文回退。
            let e = v.get("error")?.as_object()?;
            let code = e.get("code").and_then(crate::api::code_value);
            // message 缺失为空串走只有码降级。
            let message: String = e
                .get("message")
                .and_then(Value::as_str)
                .map(|m| m.chars().take(ERR_MSG_CHARS).collect())
                .unwrap_or_default();
            // 码与提示全空的空壳对象与形态不符同走原文回退。
            (code.is_some() || !message.is_empty()).then_some((code, message))
        })
        .unwrap_or_else(|| (None, body.chars().take(ERR_BODY_CHARS).collect()))
}

/// 429 退避等待的封顶秒数
pub(crate) const RATE_LIMIT_BACKOFF_MAX_SECS: u64 = 300;

/// Retry-After 头解析
/// 支持整数秒与 HTTP-date 两种格式，负偏移归零并钳到退避上限。
fn parse_retry_after(v: &str) -> Option<u64> {
    if let Ok(secs) = v.trim().parse::<i64>() {
        return Some(secs.clamp(0, RATE_LIMIT_BACKOFF_MAX_SECS as i64) as u64);
    }
    DateTime::parse_from_rfc2822(v.trim()).ok().map(|t| {
        (t.timestamp() - Utc::now().timestamp()).clamp(0, RATE_LIMIT_BACKOFF_MAX_SECS as i64) as u64
    })
}

/// 鉴权状态判定：401/403。
/// 换形态重试守卫与 classify_status 的变体选择同引此谓词；
/// 两侧码集一旦分叉，重试对新生效的状态就静默失效。
fn is_auth_status(status: u16) -> bool {
    matches!(status, 401 | 403)
}

/// 非 2xx 状态分类：429 全部归类的单点。
/// 401/403 归 Unauthenticated；429 先看身体码——账户与订阅状态类
/// 不是速率限制，退避等不来恢复，1309 与信封路径同判空态，其余
/// 账户态归 Failure，无码或其余码按 Throttled 携带 Retry-After 退避；
/// 其余状态归 Failure，大类由 category 按状态类判定。鉴权判定只按
/// 状态：非 401/403 的故障即使回显鉴权码也不换形态重试。
fn classify_status(
    status: u16,
    code: Option<i64>,
    message: String,
    retry_after_secs: Option<u64>,
) -> FetchError {
    if is_auth_status(status) {
        return FetchError::Unauthenticated(ErrorInfo {
            status,
            code,
            message,
        });
    }
    if status == 429 {
        if code == Some(1309) {
            return FetchError::Empty;
        }
        if !code.is_some_and(crate::api::is_account_state) {
            return FetchError::Throttled {
                info: ErrorInfo {
                    status,
                    code,
                    message,
                },
                retry_after_secs,
            };
        }
    }
    FetchError::Failure(ErrorInfo {
        status,
        code,
        message,
    })
}

/// 按上限读 body
/// 无效 UTF-8 以 `?` 替换，对齐 ureq 默认行为。
fn read_body_capped(body: ureq::Body, limit: u64) -> Result<String, ureq::Error> {
    body.into_with_config()
        .limit(limit)
        .lossy_utf8(true)
        .read_to_string()
}

/// 按上限读错误体前缀
/// 达到上限保留已读部分继续走提取，不因超限整体丢弃。
fn read_body_prefix(body: ureq::Body, limit: u64) -> String {
    use std::io::Read;
    let mut buf = Vec::new();
    let _ = body.into_reader().take(limit).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// 拉取主用量 quota/limit
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
    use super::{parse_balance, proxy_valid, token_params};
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
        // 信封 success/code 显式失败视为无数据，落库回填沿用旧值
        assert!(parse_balance(&json!({ "success": false, "data": { "balance": 7.0 } })).is_none());
        assert!(
            parse_balance(&json!({ "code": 500, "success": true, "data": { "balance": 7.0 } }))
                .is_none()
        );
        assert!(parse_balance(&json!({ "success": true, "data": { "balance": 7.0 } })).is_some());
        // 真实响应形态钉位：null 次级字段、科学计数法与
        // 字符串字段并存，充值+赠送-花费=可用。
        let b = parse_balance(&json!({
            "code": 200,
            "msg": "操作成功",
            "data": {
                "balance": 137.66,
                "rechargeAmount": 100.0,
                "giveAmount": 50.0,
                "totalSpendAmount": 12.34,
                "todaySpendAmount": null,
                "availableBalance": 137.66,
                "frozenBalance": 0e-9,
                "creditBalance": null,
                "availableCreditBalance": null,
                "creditStatus": "NOT_OPEN",
                "modelSpendAmountList": null,
                "isKA": false
            },
            "success": true
        }))
        .unwrap();
        assert_eq!(b.available, 137.66);
        assert_eq!(b.recharged, Some(100.0));
        assert_eq!(b.granted, Some(50.0));
        assert_eq!(b.spent, Some(12.34));
        // 非有限数按缺处理
        let b =
            parse_balance(&json!({ "data": { "balance": 3.0, "rechargeAmount": null } })).unwrap();
        assert_eq!(b.recharged, None);
    }

    #[test]
    fn proxy_valid_shapes() {
        // 显式前缀 + 合法结构才放行
        assert!(proxy_valid("http://127.0.0.1:7890").is_ok());
        assert!(proxy_valid("socks5://proxy.example.com:1080").is_ok());
        assert!(proxy_valid("HTTP://host:80").is_ok());
        assert!(proxy_valid("  http://host:80  ").is_ok());
        assert!(proxy_valid("http://user:pass@host:8080").is_ok());
        assert!(proxy_valid("socks5://[::1]:1080").is_ok());
        assert!(proxy_valid("http://localhost").is_ok());
        // ureq 会默认补 scheme 的裸主机名，全部拦下
        assert!(proxy_valid("127").is_err());
        assert!(proxy_valid("127.0.0.1:7890").is_err());
        assert!(proxy_valid("localhost").is_err());
        // 前缀对但结构坏，自行解析拦下
        assert!(proxy_valid("http://").is_err());
        assert!(proxy_valid("http://host:notaport").is_err());
        assert!(proxy_valid("http://host:0").is_err());
        assert!(proxy_valid("http://host:99999").is_err());
        assert!(proxy_valid("ftp://host:21").is_err());
        // 裸 IPv6 无方括号在结构校验滑过，由 ureq 的 Uri 解析兜底拒绝
        assert!(proxy_valid("http://::1").is_ok());
        assert!(ureq::Proxy::new("http://::1").is_err());
    }

    #[test]
    fn token_window_params_shape() {
        use chrono::TimeZone;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 8, 29, 20, 15, 30)
            .unwrap();
        let w = token_params(now);
        // 四值均已百分号编码；档序 = TOKEN_LEGS 今日/周/月，终点取当天
        // 末，周/月起点含今天共 7 / 30 个自然日
        assert_eq!(w.starts[0].as_deref(), Some("2026-08-29%2000:00:00"));
        assert_eq!(w.starts[1].as_deref(), Some("2026-08-23%2000:00:00"));
        assert_eq!(w.starts[2].as_deref(), Some("2026-07-31%2000:00:00"));
        assert_eq!(w.end, "2026-08-29%2023:59:59");
    }

    #[test]
    fn status_classification() {
        use super::classify_status;
        assert!(matches!(
            classify_status(401, Some(1000), "denied".into(), None),
            FetchError::Unauthenticated(_)
        ));
        assert!(matches!(
            classify_status(403, None, String::new(), None),
            FetchError::Unauthenticated(_)
        ));
        // 429 单点分流：1309 与信封同判空态，账户态不走退避，其余
        // 含无码按 Throttled 携带 Retry-After
        assert!(matches!(
            classify_status(429, Some(1309), "expired".into(), None),
            FetchError::Empty
        ));
        assert!(matches!(
            classify_status(429, Some(1314), "企业套餐已失效".into(), Some(45)),
            FetchError::Failure(_)
        ));
        assert!(matches!(
            classify_status(429, Some(1302), "限速".into(), Some(45)),
            FetchError::Throttled {
                retry_after_secs: Some(45),
                ..
            }
        ));
        assert!(matches!(
            classify_status(429, None, String::new(), None),
            FetchError::Throttled { .. }
        ));
        // 其余非 2xx 归 Failure，大类由 category 按状态类判
        for status in [400, 404, 500, 503] {
            assert!(matches!(
                classify_status(status, None, String::new(), None),
                FetchError::Failure(_)
            ));
        }
    }

    #[test]
    fn error_body_fields_shapes() {
        use super::error_body_fields;
        // 官方错误形态：码与提示成对提取，字符串形态统一收数字
        assert_eq!(
            error_body_fields(
                r#"{"error":{"code":"1113","message":"您的账户已欠费，请充值后重试"}}"#
            ),
            (Some(1113), "您的账户已欠费，请充值后重试".to_string())
        );
        // code 数字形态也兼容
        assert_eq!(
            error_body_fields(r#"{"error":{"code":1309,"message":"expired"}}"#),
            (Some(1309), "expired".to_string())
        );
        // 非 JSON / 形态不符：回退截断原文
        assert_eq!(
            error_body_fields("gateway html"),
            (None, "gateway html".into())
        );
        assert_eq!(error_body_fields(""), (None, String::new()));
        // error 无 message 字段：只有码降级，不整体回退
        assert_eq!(
            error_body_fields(r#"{"error":{"code":"1000"}}"#),
            (Some(1000), String::new())
        );
        // 非对象 error：原文回退，响应内容不丢
        assert_eq!(
            error_body_fields(r#"{"error":"bad gateway"}"#),
            (None, r#"{"error":"bad gateway"}"#.to_string())
        );
        assert_eq!(
            error_body_fields(r#"{"error":null}"#),
            (None, r#"{"error":null}"#.to_string())
        );
        // 超长提示截到字符上限
        let long = "x".repeat(300);
        let body = format!(r#"{{"error":{{"code":"1113","message":"{long}"}}}}"#);
        let (_, msg) = error_body_fields(&body);
        assert_eq!(msg.chars().count(), super::ERR_MSG_CHARS);
    }

    #[test]
    fn retry_after_shapes() {
        use super::parse_retry_after;
        // 整数秒与带空白
        assert_eq!(parse_retry_after("30"), Some(30));
        assert_eq!(parse_retry_after(" 120 \r\n"), Some(120));
        // 负偏移归零：立即即可重试
        assert_eq!(parse_retry_after("-5"), Some(0));
        // 病态大值钳到退避上限
        assert_eq!(parse_retry_after("999999999"), Some(300));
        assert_eq!(parse_retry_after("17600000000000"), Some(300));
        // 超出 i64 的垃圾值解析失败归 None，走无头退避
        assert_eq!(parse_retry_after("99999999999999999999"), None);
        // 不可解析
        assert_eq!(parse_retry_after("soon"), None);
        assert_eq!(parse_retry_after(""), None);
        // HTTP-date（RFC 1123）绝对时刻：远过去归零、远未来按差值钳上限。
        // 星期几须与日期相符，chrono 的 RFC 2822 解析校验一致性
        assert_eq!(parse_retry_after("Mon, 01 Jan 2001 00:00:00 GMT"), Some(0));
        assert_eq!(
            parse_retry_after("Sat, 15 Sep 2096 12:00:00 GMT"),
            Some(300)
        );
    }
}
