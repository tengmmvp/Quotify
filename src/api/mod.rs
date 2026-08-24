//! GLM Coding Plan 用量数据模型与响应解析。
//!
//! 数据源为智谱 monitor 系列接口（社区逆向 + 官方插件 `glm-plan-usage`
//! 交叉验证），国内版（open.bigmodel.cn）与国际版（api.z.ai）路径与
//! 响应结构完全一致，仅在 base 域名上区分。
//!
//! 关键结构（`GET {base}/api/monitor/usage/quota/limit` 响应 `data`）：
//! - `level`：套餐等级（"lite" / "pro" / "max"）
//! - `limits[]`：
//!   - `TOKENS_LIMIT` / `CREDIT_LIMIT`：额度桶。`unit:3` = 5 小时滚动窗，
//!     `unit:6` = 周窗；`percentage` 为已用百分比，`nextResetTime` 为
//!     毫秒时间戳（5h 桶在 0% 等状态可能缺失）
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
    /// 近 24h 模型用量（独立端点，可选，失败不阻塞主数据）
    pub model_usage: Option<ModelUsage>,
    /// 账户余额（仅国内版，可选）
    pub balance: Option<Balance>,
    /// 本地时刻：何时查询成功
    pub queried_at: DateTime<Local>,
}

/// 模型用量明细（`model-usage` 端点，时间窗内按模型/时段的 token 消耗）。
#[derive(Debug, Clone, Default)]
pub struct ModelUsage {
    /// (时间标签, 该时段全部模型 token 合计)
    pub series: Vec<(String, i64)>,
    /// (模型名, 窗口内 token 合计)，按量降序
    pub by_model: Vec<(String, i64)>,
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

/// 额度桶分类（智谱 `unit` 字段的显式窗口分类，参考 cc-switch 实测）。
enum WindowKind {
    FiveHour,
    Weekly,
}

fn classify_window(item: &Value) -> Option<WindowKind> {
    match item.get("unit").and_then(Value::as_i64) {
        Some(3) => Some(WindowKind::FiveHour),
        Some(6) => Some(WindowKind::Weekly),
        _ => None,
    }
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
    // unit 缺失/不认识时的兜底桶，按重置时间升序回填空缺槽位
    let mut unclassified: Vec<QuotaBucket> = Vec::new();

    if let Some(limits) = data.get("limits").and_then(Value::as_array) {
        for item in limits {
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
            if is_quota_entry(item) {
                quota_count += 1;
                has_credit |= is_credit_entry(item);
                let bucket = parse_bucket(item);
                match classify_window(item) {
                    Some(WindowKind::FiveHour) if five_hour.is_none() => five_hour = Some(bucket),
                    Some(WindowKind::Weekly) if weekly.is_none() => weekly = Some(bucket),
                    _ => unclassified.push(bucket),
                }
            } else if kind.eq_ignore_ascii_case("TIME_LIMIT") {
                // MCP 工具用量（月度）
                let bucket = parse_bucket(item);
                mcp = Some(McpUsage {
                    used_percent: bucket.used_percent,
                    current_value: bucket.current.unwrap_or(0.0),
                    total: bucket.total.unwrap_or(0.0),
                    details: parse_mcp_details(item),
                });
            }
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
        model_usage: None,
        balance: None,
        queried_at: Local::now(),
    }
}

/// 解析完整响应体（`{success, msg?, data}` 信封）。
pub fn parse_response(body: &str) -> Result<UsageSnapshot, FetchError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| FetchError::Api(format!("响应解析失败: {e}")))?;

    if v.get("success").and_then(Value::as_bool) == Some(false) {
        let msg = v.get("msg").and_then(Value::as_str).unwrap_or("未知错误");
        return Err(FetchError::Api(format!("接口错误: {msg}")));
    }
    // 信封里的 `code` 与 `success` 并存（CodexBar 实测），非 200 视为业务错误
    if let Some(code) = v.get("code").and_then(Value::as_i64) {
        if code != 200 {
            let msg = v.get("msg").and_then(Value::as_str).unwrap_or("");
            return Err(FetchError::Api(format!("接口错误 code {code}: {msg}")));
        }
    }
    let data = v
        .get("data")
        .ok_or_else(|| FetchError::Api("响应缺少 data 字段".into()))?;
    Ok(parse_usage(data))
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
        let body = r#"{ "success": false, "msg": "token invalid" }"#;
        let err = parse_response(body).unwrap_err();
        assert!(matches!(err, FetchError::Api(_)));
        assert!(err.to_string().contains("token invalid"));
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
}
