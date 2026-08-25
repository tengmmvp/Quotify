//! GLM Coding Plan 用量数据模型与响应解析。
//!
//! 数据源为智谱 monitor 系列接口（社区逆向 + 官方插件 `glm-plan-usage`
//! 交叉验证），国内版（open.bigmodel.cn）与国际版（api.z.ai）路径与
//! 响应结构完全一致，仅在 base 域名上区分。
//!
//! 关键结构（`GET {base}/api/monitor/usage/quota/limit` 响应 `data`）：
//! - `level`：套餐等级（"lite" / "pro" / "max"）
//! - `limits[]`：
//!   - `TOKENS_LIMIT` / `CREDIT_LIMIT`：额度桶。窗口由 `unit`（时间单位
//!     代码）× `number`（数量）的时长决定：最短桶 = 5 小时滚动窗
//!     （`unit:3 number:5`）、最长桶 = 周窗（`unit:6 number:1`），30 天
//!     滚动窗（`unit:1 number:30`）实测也存在；`percentage` 为已用百分比，
//!     `nextResetTime` 为毫秒时间戳（5h 桶在 0% 等状态可能缺失）
//!   - `TIME_LIMIT`：MCP 工具用量（月度）。`percentage` 已用百分比、
//!     `currentValue` 当前值、`usage` 总量、`usageDetails` 明细
//! - 老套餐（V1，2026-02-12 前订阅）只回一条额度桶，无周窗

use chrono::{DateTime, Local, Utc};
use serde_json::Value;

pub mod client;

/// 套餐代际。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanVersion {
    /// 旧套餐：仅 5 小时限额
    V1,
    /// 旧套餐：周限额 + 5 小时限额（非积分制）
    V2,
    /// 现行套餐：周限额 + 5 小时限额（积分制）
    V3,
    /// 无法识别（响应里没有任何额度桶）
    Unknown,
}

impl PlanVersion {
    pub fn label(self) -> &'static str {
        match self {
            PlanVersion::V1 => "V1",
            PlanVersion::V2 => "V2",
            PlanVersion::V3 => "V3",
            PlanVersion::Unknown => "",
        }
    }
}

/// 套餐等级（Lite / Pro / Max）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanTier {
    Lite,
    Pro,
    Max,
    Unknown,
}

impl PlanTier {
    /// 解析套餐标签。接口字段名与取值在不同代际/平台间不统一
    /// （CodexBar 实测：`planName` / `plan` / `plan_type` / `packageName` / `level`，
    /// 值可能是 "max" 也可能是完整套餐名），这里在整串里识别档位关键词。
    fn from_label(label: &str) -> PlanTier {
        let s = label.trim().to_ascii_lowercase();
        if s.contains("lite") {
            PlanTier::Lite
        } else if s.contains("max") {
            PlanTier::Max
        } else if s.contains("pro") {
            PlanTier::Pro
        } else {
            PlanTier::Unknown
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PlanTier::Lite => "Lite",
            PlanTier::Pro => "Pro",
            PlanTier::Max => "Max",
            PlanTier::Unknown => "",
        }
    }
}

/// 单个限额桶（5 小时窗 / 周窗通用）。
#[derive(Debug, Clone)]
pub struct QuotaBucket {
    /// 已用百分比（0–100；接口的 `percentage` 仅在缺少绝对值时使用，
    /// 有 `usage`+`remaining`/`currentValue` 时重算并钳制）
    pub used_percent: f64,
    /// 重置时刻（缺失表示当前无活跃窗口，如 0% 时的 5h 桶）
    pub resets_at: Option<DateTime<Utc>>,
    /// 总量（绝对值，接口可能不带）
    pub total: Option<f64>,
    /// 当前用量（绝对值，接口可能不带）
    pub current: Option<f64>,
}

/// MCP 工具用量明细条目（`usageDetails[]`，按模型）。
/// 字段已随数据层集成，面板明细展示随后续渲染迭代消费。
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct McpDetail {
    pub model_code: String,
    pub usage: f64,
}

/// MCP 工具用量（月度，`TIME_LIMIT` 条目）。
#[derive(Debug, Clone)]
pub struct McpUsage {
    pub used_percent: f64,
    /// 当前用量（绝对值）
    pub current_value: f64,
    /// 总量（绝对值）
    pub total: f64,
    /// 月度重置时刻（真实响应携带 `nextResetTime`）
    pub resets_at: Option<DateTime<Utc>>,
    /// 按模型明细（已集成，渲染待接）
    #[allow(dead_code)]
    pub details: Vec<McpDetail>,
}

/// 一次成功查询得到的用量快照（统一模型，跨平台跨套餐代际）。
#[derive(Debug, Clone)]
pub struct UsageSnapshot {
    pub plan_version: PlanVersion,
    pub tier: PlanTier,
    /// 原始套餐名字符串（`planName`/`plan`/`plan_type`/`packageName`/`level`
    /// 首个非空值）；档位识别不出时用于展示
    pub plan_label: Option<String>,
    pub five_hour: Option<QuotaBucket>,
    pub weekly: Option<QuotaBucket>,
    pub mcp: Option<McpUsage>,
    /// 账户余额（仅国内版，可选）
    pub balance: Option<Balance>,
    /// 本地时刻：何时查询成功
    pub queried_at: DateTime<Local>,
}

/// 账户余额（国内版 `query-customer-account-report` 端点）。
#[derive(Debug, Clone, Default)]
pub struct Balance {
    pub available: f64,
    /// 明细字段已随数据层集成，面板余额详情随后续渲染迭代消费
    #[allow(dead_code)]
    pub recharged: Option<f64>,
    #[allow(dead_code)]
    pub granted: Option<f64>,
    #[allow(dead_code)]
    pub spent: Option<f64>,
}

/// 查询失败分类，决定面板/图标的呈现与是否保留旧数据。
#[derive(Debug, Clone)]
pub enum FetchError {
    /// 凭据失效（HTTP 401/403）：提示用户检查 API key
    Auth(String),
    /// 业务错误（success:false / 非 2xx / JSON 解析失败）
    Api(String),
    /// 网络瞬时失败（超时、断连）：保留旧数据并允许重试
    Network(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Auth(m) => write!(f, "{m}"),
            FetchError::Api(m) => write!(f, "{m}"),
            FetchError::Network(m) => write!(f, "{m}"),
        }
    }
}

/// `unit`（时间单位代码）→ 分钟乘数：1=天、3=小时、4=天、5=分钟、6=周。
/// 乘数表来自 CodexBar（`{1:1440, 3:60, 5:1, 6:10080}`），unit:4 由
/// glm-usage-monitor 实测枚举补充。窗口时长 = `number × unit` 分钟，
/// 如 `3×60=300`（5 小时）、`1×10080`（周）、`30×1440`（30 天滚动窗）。
fn window_minutes(item: &Value) -> Option<i64> {
    const MULTIPLIERS: &[(i64, i64)] = &[(1, 1440), (3, 60), (4, 1440), (5, 1), (6, 10080)];
    let unit = item.get("unit").and_then(Value::as_i64)?;
    let number = item.get("number").and_then(Value::as_i64).filter(|&n| n > 0)?;
    MULTIPLIERS.iter().find(|(u, _)| *u == unit).map(|(_, m)| number * m)
}

/// 取 `limits` 数组。兼容三种位置：`data.limits`、`data` 本身即数组、
/// 响应顶层即数组（V3 实测形态，无 `{data:{limits}}` 信封）。
fn limits_of(data: &Value) -> Option<&Vec<Value>> {
    data.get("limits")
        .and_then(Value::as_array)
        .or_else(|| data.as_array())
}

/// 单个 `limits[]` 条目是否为额度桶（积分制 V3 回 `CREDIT_LIMIT`，
/// V1/V2 回 `TOKENS_LIMIT`，大小写不敏感兼容）。
fn is_quota_entry(item: &Value) -> bool {
    let t = item.get("type").and_then(Value::as_str).unwrap_or("");
    t.eq_ignore_ascii_case("TOKENS_LIMIT") || t.eq_ignore_ascii_case("CREDIT_LIMIT")
}

fn is_credit_entry(item: &Value) -> bool {
    item.get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| t.eq_ignore_ascii_case("CREDIT_LIMIT"))
}

fn parse_reset_time(item: &Value) -> Option<DateTime<Utc>> {
    item.get("nextResetTime")
        .and_then(Value::as_i64)
        .filter(|&ms| ms > 0)
        .and_then(DateTime::from_timestamp_millis)
}

/// 从条目解析限额桶。百分比优先用绝对值重算（接口 `percentage`
/// 在部分代际语义不稳，CodexBar 同样处理），缺失时退回 `percentage`。
fn parse_bucket(item: &Value) -> QuotaBucket {
    let total = item.get("usage").and_then(Value::as_f64).filter(|v| *v > 0.0);
    let current = item.get("currentValue").and_then(Value::as_f64);
    let remaining = item.get("remaining").and_then(Value::as_f64);

    let used_percent = if let Some(total) = total {
        let used = match (remaining, current) {
            (Some(rem), Some(cur)) => (total - rem).max(cur),
            (Some(rem), None) => total - rem,
            (None, Some(cur)) => cur,
            (None, None) => f64::NAN,
        };
        if used.is_finite() {
            (used.clamp(0.0, total) / total * 100.0).clamp(0.0, 100.0)
        } else {
            item.get("percentage").and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 100.0)
        }
    } else {
        item.get("percentage").and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 100.0)
    };

    QuotaBucket {
        used_percent,
        resets_at: parse_reset_time(item),
        total,
        current,
    }
}

/// 解析 MCP `usageDetails[]`（按模型明细，`{modelCode, usage}`）。
fn parse_mcp_details(item: &Value) -> Vec<McpDetail> {
    item.get("usageDetails")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|d| {
                    Some(McpDetail {
                        model_code: d.get("modelCode")?.as_str()?.to_string(),
                        usage: d.get("usage").and_then(Value::as_f64).unwrap_or(0.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 解析 `data` 为统一用量快照。不涉及任何 IO，便于单测。
pub fn parse_usage(data: &Value) -> UsageSnapshot {
    // 套餐标签多字段 fallback（不同代际/平台字段名不统一，CodexBar 实测）
    let plan_label = ["planName", "plan", "plan_type", "packageName", "level"]
        .iter()
        .find_map(|k| {
            data.get(*k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        });
    let tier = plan_label
        .as_deref()
        .map(PlanTier::from_label)
        .unwrap_or(PlanTier::Unknown);

    let mut five_hour: Option<QuotaBucket> = None;
    let mut weekly: Option<QuotaBucket> = None;
    let mut mcp: Option<McpUsage> = None;
    let mut has_credit = false;
    let mut quota_count = 0usize;
    // 能算出窗口时长的桶 / 不能的兜底桶，分开处理
    let mut timed: Vec<(QuotaBucket, i64)> = Vec::new();
    let mut unclassified: Vec<QuotaBucket> = Vec::new();

    if let Some(limits) = limits_of(data) {
        for item in limits {
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
            if is_quota_entry(item) {
                quota_count += 1;
                has_credit |= is_credit_entry(item);
                let bucket = parse_bucket(item);
                match window_minutes(item) {
                    Some(minutes) => timed.push((bucket, minutes)),
                    None => unclassified.push(bucket),
                }
            } else if kind.eq_ignore_ascii_case("TIME_LIMIT") {
                // MCP 工具用量（月度）
                let bucket = parse_bucket(item);
                mcp = Some(McpUsage {
                    used_percent: bucket.used_percent,
                    current_value: bucket.current.unwrap_or(0.0),
                    total: bucket.total.unwrap_or(0.0),
                    resets_at: bucket.resets_at,
                    details: parse_mcp_details(item),
                });
            }
        }
    }

    // 时长分类（CodexBar 共识）：额度桶按窗口时长归类，最短 = 5 小时
    // 滚动窗、最长 = 周窗，中间桶丢弃；30 天滚动窗由此正确落入最长桶，
    // 而按 unit 数值身份（3/6）严格匹配会把 30 天窗漏掉。
    if !timed.is_empty() {
        timed.sort_by_key(|&(_, minutes)| minutes);
        if timed.len() == 1 {
            let (bucket, minutes) = timed.remove(0);
            // 单桶：5 小时窗归主槽，其余（如仅周窗的账号）归周槽，避免错标
            if minutes == 300 {
                five_hour = Some(bucket);
            } else {
                weekly = Some(bucket);
            }
        } else {
            weekly = Some(timed.pop().map(|(b, _)| b).unwrap());
            five_hour = Some(timed.remove(0).0);
        }
    }

    // 兜底：无 reset 的优先归 5h（0% 状态的 5h 桶没有 nextResetTime），其余升序回填
    unclassified.sort_by_key(|b| (b.resets_at.is_some(), b.resets_at.map(|t| t.timestamp())));
    for bucket in unclassified {
        if five_hour.is_none() {
            five_hour = Some(bucket);
        } else if weekly.is_none() {
            weekly = Some(bucket);
        }
    }

    let plan_version = if has_credit {
        PlanVersion::V3
    } else if quota_count >= 2 {
        PlanVersion::V2
    } else if quota_count == 1 {
        PlanVersion::V1
    } else {
        PlanVersion::Unknown
    };

    UsageSnapshot {
        plan_version,
        tier,
        plan_label,
        five_hour,
        weekly,
        mcp,
        balance: None,
        queried_at: Local::now(),
    }
}

/// 信封内业务失败 → 错误分类。鉴权失败常以 HTTP 200 + `code:401` /
/// `success:false` 出现（ai-usagebar 抓包证实），需归为 `Auth` 才能触发
/// 设置页的修复提示；其余归 `Api`。
fn inband_error(code: Option<i64>, msg: &str) -> FetchError {
    let m = msg.to_ascii_lowercase();
    if code == Some(401)
        || code == Some(403)
        || m.contains("unauthorized")
        || m.contains("token")
        || m.contains("api key")
        || m.contains("apikey")
    {
        FetchError::Auth(msg.to_string())
    } else {
        FetchError::Api(format!("接口错误: {msg}"))
    }
}

/// 解析完整响应体（`{success, msg?, data}` 信封）。
/// V3 实测形态下顶层可能直接是 `limits` 数组而无信封，一并兼容。
pub fn parse_response(body: &str) -> Result<UsageSnapshot, FetchError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| FetchError::Api(format!("响应解析失败: {e}")))?;

    let data = if v.is_array() {
        &v
    } else {
        if v.get("success").and_then(Value::as_bool) == Some(false) {
            let msg = v.get("msg").and_then(Value::as_str).unwrap_or("未知错误");
            let code = v.get("code").and_then(Value::as_i64);
            return Err(inband_error(code, msg));
        }
        // 信封里的 `code` 与 `success` 并存（CodexBar 实测），非 200 视为业务错误
        if let Some(code) = v.get("code").and_then(Value::as_i64) {
            if code != 200 {
                let msg = v.get("msg").and_then(Value::as_str).unwrap_or("");
                return Err(inband_error(Some(code), &format!("code {code}: {msg}")));
            }
        }
        v.get("data")
            .ok_or_else(|| FetchError::Api("响应缺少 data 字段".into()))?
    };

    // 空额度检测：团队版缺组织/项目选择头时接口仍返回 success，但 limits
    // 为空（cc-switch #4222/#6402 根因）；个人版 key 无 Coding Plan 权限时
    // 同形态。给出可行动的错误而不是静默空面板。
    if limits_of(&data).is_none_or(|limits| limits.is_empty()) {
        return Err(FetchError::Api(
            "未返回额度数据：请确认 API key 属于编码套餐（团队版需填写组织/项目 ID）".into(),
        ));
    }
    Ok(parse_usage(&data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v3_two_credit_buckets_with_mcp() {
        let body = r#"{
            "success": true,
            "code": 200,
            "data": {
                "level": "Max",
                "limits": [
                    { "type": "CREDIT_LIMIT", "unit": 3, "number": 5, "percentage": 26.5, "nextResetTime": 1778806800000 },
                    { "type": "CREDIT_LIMIT", "unit": 6, "number": 7, "percentage": 5.0, "nextResetTime": 1779062400000 },
                    { "type": "TIME_LIMIT", "percentage": 12.0, "currentValue": 120, "usage": 1000,
                      "usageDetails": [ { "modelCode": "mcp-server", "usage": 80 } ] }
                ]
            }
        }"#;
        let snap = parse_response(body).unwrap();
        assert_eq!(snap.plan_version, PlanVersion::V3);
        assert_eq!(snap.tier, PlanTier::Max);
        let fh = snap.five_hour.unwrap();
        assert!((fh.used_percent - 26.5).abs() < 1e-9);
        assert!(fh.resets_at.is_some());
        assert!((snap.weekly.unwrap().used_percent - 5.0).abs() < 1e-9);
        let mcp = snap.mcp.unwrap();
        assert!((mcp.used_percent - 12.0).abs() < 1e-9);
        assert!((mcp.total - 1000.0).abs() < 1e-9);
        assert_eq!(mcp.details.len(), 1);
        assert_eq!(mcp.details[0].model_code, "mcp-server");
    }

    #[test]
    fn absolute_values_recompute_percentage() {
        // 带绝对值时重算百分比并钳制，不信任 percentage 字段
        let body = r#"{
            "success": true,
            "data": {
                "limits": [
                    { "type": "CREDIT_LIMIT", "unit": 3, "percentage": 99.0,
                      "usage": 500, "remaining": 100, "nextResetTime": 1778806800000 }
                ]
            }
        }"#;
        let snap = parse_response(body).unwrap();
        let fh = snap.five_hour.unwrap();
        assert!((fh.used_percent - 80.0).abs() < 1e-9);
        assert_eq!(fh.total, Some(500.0));
        assert_eq!(fh.current, None);
    }

    #[test]
    fn envelope_code_mismatch_is_error() {
        let body = r#"{ "success": true, "code": 500, "msg": "boom" }"#;
        assert!(matches!(parse_response(body), Err(FetchError::Api(_))));
    }

    #[test]
    fn v2_two_tokens_buckets() {
        let body = r#"{
            "success": true,
            "data": {
                "level": "pro",
                "limits": [
                    { "type": "TOKENS_LIMIT", "unit": 6, "number": 1, "percentage": 30.0, "nextResetTime": 2000000000000 },
                    { "type": "TOKENS_LIMIT", "unit": 3, "number": 5, "percentage": 10.0, "nextResetTime": 1000000000000 }
                ]
            }
        }"#;
        let snap = parse_response(body).unwrap();
        assert_eq!(snap.plan_version, PlanVersion::V2);
        assert_eq!(snap.tier, PlanTier::Pro);
        // unit 优先于数组顺序与重置时间
        assert!((snap.five_hour.unwrap().used_percent - 10.0).abs() < 1e-9);
        assert!((snap.weekly.unwrap().used_percent - 30.0).abs() < 1e-9);
    }

    #[test]
    fn v1_single_bucket_falls_back_to_five_hour() {
        let body = r#"{
            "success": true,
            "data": {
                "level": "Lite",
                "limits": [
                    { "type": "TOKENS_LIMIT", "percentage": 2.0, "nextResetTime": 1774967594803 },
                    { "type": "TIME_LIMIT", "percentage": 0.0 }
                ]
            }
        }"#;
        let snap = parse_response(body).unwrap();
        assert_eq!(snap.plan_version, PlanVersion::V1);
        assert_eq!(snap.tier, PlanTier::Lite);
        assert!(snap.weekly.is_none());
        assert!((snap.five_hour.unwrap().used_percent - 2.0).abs() < 1e-9);
    }

    #[test]
    fn missing_unit_falls_back_by_reset_order() {
        // 周期末尾：周桶比 5h 桶先重置，unit 缺失时不能按时间把两桶标反
        let body = r#"{
            "success": true,
            "data": {
                "limits": [
                    { "type": "TOKENS_LIMIT", "percentage": 25.0, "nextResetTime": 2000000000000 },
                    { "type": "TOKENS_LIMIT", "percentage": 0.0 }
                ]
            }
        }"#;
        let snap = parse_response(body).unwrap();
        // 无 reset 的优先归 5h（0% 桶），带 reset 的归周
        assert!(snap.five_hour.unwrap().resets_at.is_none());
        assert!(snap.weekly.unwrap().resets_at.is_some());
    }

    #[test]
    fn business_error_maps_to_api() {
        let body = r#"{ "success": false, "msg": "boom" }"#;
        let err = parse_response(body).unwrap_err();
        assert!(matches!(err, FetchError::Api(_)));
        assert!(err.to_string().contains("boom"));
    }

    /// 无效 key 常见形态：HTTP 200 + success:false + code 401 / token 字样
    /// → 必须归 Auth（设置页修复提示只认 Auth）
    #[test]
    fn in_band_auth_failures_map_to_auth() {
        for body in [
            r#"{ "code": 401, "msg": "Unauthorized", "data": null, "success": false }"#,
            r#"{ "success": false, "msg": "token invalid" }"#,
            r#"{ "success": true, "code": 403, "msg": "forbidden" }"#,
        ] {
            let err = parse_response(body).unwrap_err();
            assert!(matches!(err, FetchError::Auth(_)), "{body} → {err:?}");
        }
    }

    #[test]
    fn case_insensitive_type_and_invalid_percentage() {
        let body = r#"{
            "success": true,
            "data": {
                "limits": [
                    { "type": "tokens_limit", "unit": 3, "percentage": "bad" },
                    { "type": "TOKENS_LIMIT", "unit": 6, "percentage": null }
                ]
            }
        }"#;
        let snap = parse_response(body).unwrap();
        assert_eq!(snap.plan_version, PlanVersion::V2);
        assert_eq!(snap.five_hour.unwrap().used_percent, 0.0);
        assert_eq!(snap.weekly.unwrap().used_percent, 0.0);
    }

    #[test]
    fn thirty_day_rolling_window_classifies_by_duration() {
        // 30 天滚动窗（unit:1 number:30）实测存在，按时长归最长桶（周槽），
        // 按 unit 数值身份（3/6）匹配会把它整个漏掉
        let body = r#"{
            "success": true,
            "data": {
                "limits": [
                    { "type": "TOKENS_LIMIT", "unit": 1, "number": 30, "percentage": 50.0, "nextResetTime": 1787112000000 },
                    { "type": "TOKENS_LIMIT", "unit": 3, "number": 5, "percentage": 10.0, "nextResetTime": 1778806800000 }
                ]
            }
        }"#;
        let snap = parse_response(body).unwrap();
        assert!((snap.five_hour.unwrap().used_percent - 10.0).abs() < 1e-9);
        assert!((snap.weekly.unwrap().used_percent - 50.0).abs() < 1e-9);
    }

    #[test]
    fn unit4_day_window_supported() {
        // unit:4=天（glm-usage-monitor 实测枚举），7 天 = 10080 分钟与周窗同级
        let body = r#"{
            "success": true,
            "data": {
                "limits": [
                    { "type": "TOKENS_LIMIT", "unit": 4, "number": 7, "percentage": 40.0 },
                    { "type": "TOKENS_LIMIT", "unit": 3, "number": 5, "percentage": 5.0 }
                ]
            }
        }"#;
        let snap = parse_response(body).unwrap();
        assert!((snap.five_hour.unwrap().used_percent - 5.0).abs() < 1e-9);
        assert!((snap.weekly.unwrap().used_percent - 40.0).abs() < 1e-9);
    }

    #[test]
    fn single_weekly_only_bucket_goes_to_weekly_slot() {
        // 仅周窗的账号（Codex 场景回退）：单桶非 5h 时长归周槽，不错标成 5 小时窗
        let body = r#"{
            "success": true,
            "data": {
                "limits": [
                    { "type": "TOKENS_LIMIT", "unit": 6, "number": 1, "percentage": 33.0, "nextResetTime": 2000000000000 }
                ]
            }
        }"#;
        let snap = parse_response(body).unwrap();
        assert!(snap.five_hour.is_none());
        assert!((snap.weekly.unwrap().used_percent - 33.0).abs() < 1e-9);
    }

    #[test]
    fn top_level_array_response_parses() {
        // V3 实测形态：顶层直接是额度数组，无 {data:{limits}} 信封
        // （样本来自社区 dump，绝对值重算百分比）
        let body = r#"[
            { "type": "CREDIT_LIMIT", "unit": 3, "number": 5, "usage": 28000, "currentValue": 2585, "remaining": 25414, "percentage": 9, "nextResetTime": 1786592963348 },
            { "type": "CREDIT_LIMIT", "unit": 6, "number": 1, "usage": 140000, "currentValue": 58386, "remaining": 81613, "percentage": 41, "nextResetTime": 1786692650981 }
        ]"#;
        let snap = parse_response(body).unwrap();
        assert_eq!(snap.plan_version, PlanVersion::V3);
        // used = max(usage - remaining, currentValue)
        let fh = snap.five_hour.unwrap();
        assert!((fh.used_percent - 2586.0 / 28000.0 * 100.0).abs() < 1e-9);
        let wk = snap.weekly.unwrap();
        assert!((wk.used_percent - 58387.0 / 140000.0 * 100.0).abs() < 1e-9);
    }

    #[test]
    fn empty_limits_returns_actionable_error() {
        // 团队版缺组织/项目选择头 / key 无 Coding Plan 权限：
        // success + 空 limits，不能静默成空面板
        for body in [
            r#"{ "success": true, "code": 200, "msg": "操作成功", "data": { "limits": [] } }"#,
            r#"{ "success": true, "data": {} }"#,
        ] {
            let err = parse_response(body).unwrap_err();
            assert!(matches!(err, FetchError::Api(_)), "{body} → {err:?}");
            assert!(err.to_string().contains("编码套餐"), "{body} → {err}");
        }
    }

    /// 2026-08-25 控制台实测样本（bigmodel.cn）：单 5h 桶 + MCP 明细，
    /// level 小写 "max"，MCP 三明细之和恰等于 currentValue
    #[test]
    fn real_console_sample_single_bucket_with_mcp_details() {
        let body = r#"{
            "code": 200,
            "msg": "操作成功",
            "data": {
                "limits": [
                    {
                        "type": "TIME_LIMIT", "unit": 5, "number": 1,
                        "usage": 4000, "currentValue": 85, "remaining": 3915,
                        "percentage": 2, "nextResetTime": 1789707570998,
                        "usageDetails": [
                            { "modelCode": "search-prime", "usage": 56 },
                            { "modelCode": "web-reader", "usage": 29 },
                            { "modelCode": "zread", "usage": 0 }
                        ]
                    },
                    {
                        "type": "TOKENS_LIMIT", "unit": 3, "number": 5,
                        "percentage": 11, "nextResetTime": 1787633490996
                    }
                ],
                "level": "max"
            },
            "success": true
        }"#;
        let snap = parse_response(body).unwrap();
        assert_eq!(snap.plan_version, PlanVersion::V1);
        assert_eq!(snap.tier, PlanTier::Max);
        assert!(snap.weekly.is_none());
        let fh = snap.five_hour.unwrap();
        assert!((fh.used_percent - 11.0).abs() < 1e-9);
        assert!(fh.resets_at.is_some());
        let mcp = snap.mcp.unwrap();
        assert_eq!(mcp.total, 4000.0);
        assert_eq!(mcp.current_value, 85.0);
        assert_eq!(mcp.details.len(), 3);
        assert_eq!(mcp.details[0].model_code, "search-prime");
        assert!(mcp.resets_at.is_some());
    }
}
