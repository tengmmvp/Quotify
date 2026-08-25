//! GLM Coding Plan 用量数据模型与响应解析

use chrono::{DateTime, Local, Utc};
use serde_json::Value;

pub mod client;

/// 套餐代际
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanVersion {
    V1,
    V2,
    V3,
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

/// 套餐等级
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanTier {
    Lite,
    Pro,
    Max,
    Unknown,
}

impl PlanTier {
    /// 解析套餐标签
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

/// 单个限额桶
#[derive(Debug, Clone)]
pub struct QuotaBucket {
    /// 已用百分比 0–100
    pub used_percent: f64,
    /// 重置时刻
    pub resets_at: Option<DateTime<Utc>>,
    /// 总量
    pub total: Option<f64>,
    /// 当前用量
    pub current: Option<f64>,
}

/// MCP 工具用量按模型明细
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct McpDetail {
    pub model_code: String,
    pub usage: f64,
}

/// MCP 工具用量，月度
#[derive(Debug, Clone)]
pub struct McpUsage {
    pub used_percent: f64,
    /// 当前用量
    pub current_value: f64,
    /// 总量
    pub total: f64,
    /// 月度重置时刻
    pub resets_at: Option<DateTime<Utc>>,
    /// 按模型明细
    #[allow(dead_code)]
    pub details: Vec<McpDetail>,
}

/// 一次成功查询得到的用量快照
#[derive(Debug, Clone)]
pub struct UsageSnapshot {
    pub plan_version: PlanVersion,
    pub tier: PlanTier,
    pub plan_label: Option<String>,
    pub five_hour: Option<QuotaBucket>,
    pub weekly: Option<QuotaBucket>,
    pub mcp: Option<McpUsage>,
    pub balance: Option<Balance>,
    pub queried_at: DateTime<Local>,
}

/// 账户余额，仅国内版
#[derive(Debug, Clone, Default)]
pub struct Balance {
    pub available: f64,
    #[allow(dead_code)]
    pub recharged: Option<f64>,
    #[allow(dead_code)]
    pub granted: Option<f64>,
    #[allow(dead_code)]
    pub spent: Option<f64>,
}

/// 查询失败分类
#[derive(Debug, Clone)]
pub enum FetchError {
    /// 凭据失效（HTTP 401/403 或信封 code 401/403），触发 Bearer 重试与设置页修复提示
    Auth,
    /// 空 limits：团队版缺组织/项目选择头，或 key 无 Coding Plan 权限
    EmptyLimits,
    /// 其余业务错误，detail 为 HTTP 状态 + 响应片段 / 信封 msg
    Api(String),
    /// 网络层失败（超时、断连），保留旧数据并允许重试
    Network(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Auth => write!(f, "unauthorized (401/403)"),
            FetchError::EmptyLimits => write!(f, "empty limits"),
            FetchError::Api(d) | FetchError::Network(d) => write!(f, "{d}"),
        }
    }
}

/// `unit` 单位代码 → 分钟乘数：1/4=天、3=小时、5=月、6=周
fn window_minutes(item: &Value) -> Option<i64> {
    const MULTIPLIERS: &[(i64, i64)] =
        &[(1, 1440), (3, 60), (4, 1440), (5, 30 * 24 * 60), (6, 10080)];
    let unit = item.get("unit").and_then(Value::as_i64)?;
    let number = item
        .get("number")
        .and_then(Value::as_i64)
        .filter(|&n| n > 0)?;
    MULTIPLIERS
        .iter()
        .find(|(u, _)| *u == unit)
        .and_then(|(_, m)| number.checked_mul(*m))
}

fn limits_of(data: &Value) -> Option<&Vec<Value>> {
    data.get("limits")
        .and_then(Value::as_array)
        .or_else(|| data.as_array())
}

/// 条目类型标识
fn entry_kind(item: &Value) -> &str {
    item.get("type")
        .or_else(|| item.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// 额度桶判定
fn is_quota_entry(item: &Value) -> bool {
    let t = entry_kind(item);
    t.eq_ignore_ascii_case("TOKENS_LIMIT") || t.eq_ignore_ascii_case("CREDIT_LIMIT")
}

fn is_credit_entry(item: &Value) -> bool {
    entry_kind(item).eq_ignore_ascii_case("CREDIT_LIMIT")
}

/// MCP 通道判定：TIME_LIMIT 为主，MCP_LIMIT 为别名
fn is_mcp_entry(item: &Value) -> bool {
    let t = entry_kind(item);
    t.eq_ignore_ascii_case("TIME_LIMIT") || t.eq_ignore_ascii_case("MCP_LIMIT")
}

fn parse_reset_time(item: &Value) -> Option<DateTime<Utc>> {
    item.get("nextResetTime")
        .and_then(Value::as_i64)
        .filter(|&ms| ms > 0)
        .and_then(DateTime::from_timestamp_millis)
}

/// 解析限额桶
fn parse_bucket(item: &Value) -> QuotaBucket {
    let total = item
        .get("usage")
        .and_then(Value::as_f64)
        .filter(|v| *v > 0.0);
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
            item.get("percentage")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .clamp(0.0, 100.0)
        }
    } else {
        item.get("percentage")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .clamp(0.0, 100.0)
    };

    QuotaBucket {
        used_percent,
        resets_at: parse_reset_time(item),
        total,
        current,
    }
}

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

/// 统一用量快照解析
pub fn parse_usage(data: &Value) -> UsageSnapshot {
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
    let mut timed: Vec<(QuotaBucket, i64)> = Vec::new();
    let mut unclassified: Vec<QuotaBucket> = Vec::new();

    if let Some(limits) = limits_of(data) {
        for item in limits {
            if is_quota_entry(item) {
                quota_count += 1;
                has_credit |= is_credit_entry(item);
                let bucket = parse_bucket(item);
                match window_minutes(item) {
                    Some(minutes) => timed.push((bucket, minutes)),
                    None => unclassified.push(bucket),
                }
            } else if is_mcp_entry(item) {
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

    if !timed.is_empty() {
        timed.sort_by_key(|&(_, minutes)| minutes);
        if timed.len() == 1 {
            let (bucket, minutes) = timed.remove(0);
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

/// 信封内业务失败分类：401/403/1000/1001 与鉴权关键词归 `Auth`，
/// 1309 与 "coding plan" 无套餐归 `EmptyLimits`，其余归 `Api`
fn inband_error(code: Option<i64>, msg: &str) -> FetchError {
    let m = msg.to_ascii_lowercase();
    if code == Some(401)
        || code == Some(403)
        || code == Some(1000)
        || code == Some(1001)
        || m.contains("unauthorized")
        || m.contains("token")
        || m.contains("api key")
        || m.contains("apikey")
    {
        FetchError::Auth
    } else if code == Some(1309) || m.contains("coding plan") {
        FetchError::EmptyLimits
    } else {
        FetchError::Api(msg.to_string())
    }
}

/// 解析完整响应体
pub fn parse_response(body: &str) -> Result<UsageSnapshot, FetchError> {
    let v: Value =
        serde_json::from_str(body).map_err(|e| FetchError::Api(format!("parse failed: {e}")))?;

    let data = if v.is_array() {
        &v
    } else {
        if v.get("success").and_then(Value::as_bool) == Some(false) {
            let msg = v
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            let code = v.get("code").and_then(Value::as_i64);
            return Err(inband_error(code, msg));
        }
        if let Some(code) = v.get("code").and_then(Value::as_i64)
            && code != 200
        {
            let msg = v.get("msg").and_then(Value::as_str).unwrap_or("");
            return Err(inband_error(Some(code), &format!("code {code}: {msg}")));
        }
        v.get("data")
            .ok_or_else(|| FetchError::Api("missing data field".into()))?
    };

    if limits_of(data).is_none_or(|limits| limits.is_empty()) {
        return Err(FetchError::EmptyLimits);
    }
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

    #[test]
    fn in_band_auth_failures_map_to_auth() {
        for body in [
            r#"{ "code": 401, "msg": "Unauthorized", "data": null, "success": false }"#,
            r#"{ "success": false, "msg": "token invalid" }"#,
            r#"{ "success": true, "code": 403, "msg": "forbidden" }"#,
            // 业务码族 1000/1001 也是鉴权失败
            r#"{ "success": false, "code": 1001, "msg": "auth required" }"#,
            r#"{ "success": false, "code": 1000, "msg": "" }"#,
        ] {
            let err = parse_response(body).unwrap_err();
            assert!(matches!(err, FetchError::Auth), "{body} → {err:?}");
        }
    }

    /// 1309 = 套餐过期；msg 含 "coding plan" = key 有效但无编码套餐
    /// （该形态信封 code 为 500，不能按码判）
    #[test]
    fn in_band_plan_state_maps_to_empty_limits() {
        for body in [
            r#"{ "success": false, "code": 1309, "msg": "plan expired" }"#,
            r#"{ "success": false, "code": 500, "msg": "No coding plan found for this key" }"#,
        ] {
            let err = parse_response(body).unwrap_err();
            assert!(matches!(err, FetchError::EmptyLimits), "{body} → {err:?}");
        }
    }

    /// MCP 通道接受 MCP_LIMIT 别名；类型字段可由 name 承载
    #[test]
    fn mcp_alias_and_name_field() {
        let body = r#"{
            "success": true,
            "data": {
                "limits": [
                    { "name": "MCP_LIMIT", "usage": 400, "currentValue": 40, "remaining": 360,
                      "usageDetails": [ { "modelCode": "search-prime", "usage": 40 } ] },
                    { "name": "TOKENS_LIMIT", "unit": 3, "number": 5, "percentage": 8.0 }
                ]
            }
        }"#;
        let snap = parse_response(body).unwrap();
        let mcp = snap.mcp.unwrap();
        assert_eq!(mcp.total, 400.0);
        assert_eq!(mcp.current_value, 40.0);
        assert!((snap.five_hour.unwrap().used_percent - 8.0).abs() < 1e-9);
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
    fn window_minutes_overflow_falls_back() {
        let body = r#"{
            "success": true,
            "data": {
                "limits": [
                    { "type": "TOKENS_LIMIT", "unit": 6, "number": 9223372036854775807, "percentage": 7.0 }
                ]
            }
        }"#;
        let snap = parse_response(body).unwrap();
        assert!(snap.weekly.is_none());
        assert!((snap.five_hour.unwrap().used_percent - 7.0).abs() < 1e-9);
    }

    #[test]
    fn unit4_day_window_supported() {
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
        let body = r#"[
            { "type": "CREDIT_LIMIT", "unit": 3, "number": 5, "usage": 28000, "currentValue": 2585, "remaining": 25414, "percentage": 9, "nextResetTime": 1786592963348 },
            { "type": "CREDIT_LIMIT", "unit": 6, "number": 1, "usage": 140000, "currentValue": 58386, "remaining": 81613, "percentage": 41, "nextResetTime": 1786692650981 }
        ]"#;
        let snap = parse_response(body).unwrap();
        assert_eq!(snap.plan_version, PlanVersion::V3);
        let fh = snap.five_hour.unwrap();
        assert!((fh.used_percent - 2586.0 / 28000.0 * 100.0).abs() < 1e-9);
        let wk = snap.weekly.unwrap();
        assert!((wk.used_percent - 58387.0 / 140000.0 * 100.0).abs() < 1e-9);
    }

    #[test]
    fn empty_limits_returns_actionable_error() {
        for body in [
            r#"{ "success": true, "code": 200, "msg": "操作成功", "data": { "limits": [] } }"#,
            r#"{ "success": true, "data": {} }"#,
        ] {
            let err = parse_response(body).unwrap_err();
            assert!(matches!(err, FetchError::EmptyLimits), "{body} → {err:?}");
        }
    }

    /// 真实响应形态：单 5h 桶 + MCP 明细，level 小写 "max"
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
