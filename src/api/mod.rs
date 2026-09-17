//! GLM Coding Plan 用量数据模型与响应解析

use chrono::{DateTime, Local, Utc};
use serde_json::Value;

pub mod client;

/// 错误详情携带的响应文本字符上限
pub(crate) const ERR_BODY_CHARS: usize = 160;

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

/// 一次查询的账号参数
/// 团队版仅国内站，API Key、type=2 与两个选择头缺一不可；缺选择头时
/// 服务端回 success 加空 limits。
#[derive(Debug, Clone)]
pub struct AccountSpec {
    pub platform: Platform,
    /// 团队版：组织 ID（`Bigmodel-Organization`）
    pub org_id: String,
    /// 团队版：项目 ID（`Bigmodel-Project`）
    pub project_id: String,
    pub api_key: String,
}

impl AccountSpec {
    /// 两个选择头齐全才按团队查询，不全时按个人版。
    fn team_scope(&self) -> Option<(&str, &str)> {
        let org = self.org_id.trim();
        let project = self.project_id.trim();
        (!org.is_empty() && !project.is_empty()).then_some((org, project))
    }
}

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
    pub token_stats: Option<TokenStats>,
    pub balance: Option<Balance>,
    pub queried_at: DateTime<Local>,
}

impl UsageSnapshot {
    /// MCP 构成区陈列判定
    /// 明细非空即陈列，高度侧与渲染侧同引。
    pub fn has_mcp_details(&self) -> bool {
        self.mcp.as_ref().is_some_and(|m| !m.details.is_empty())
    }

    /// 在场桶按绘制序枚举：行集构成、倒计时行与渲染推进的唯一来源，
    /// 高度侧的行数特征同引此序
    pub(crate) fn present_buckets(&self) -> impl Iterator<Item = PresentBucket<'_>> {
        self.five_hour
            .as_ref()
            .map(PresentBucket::FiveHour)
            .into_iter()
            .chain(self.weekly.as_ref().map(PresentBucket::Weekly))
            .chain(self.mcp.as_ref().map(PresentBucket::Mcp))
    }

    /// 等级显示文案
    /// tier 标签优先，缺则回退套餐标签。
    pub fn tier_label(&self) -> &str {
        let t = self.tier.label();
        if t.is_empty() {
            self.plan_label.as_deref().unwrap_or("")
        } else {
            t
        }
    }
}

/// 在场指标桶的绘制序变体：五小时 → 周 → MCP
pub(crate) enum PresentBucket<'a> {
    FiveHour(&'a QuotaBucket),
    Weekly(&'a QuotaBucket),
    Mcp(&'a McpUsage),
}

impl PresentBucket<'_> {
    /// 重置时刻：倒计时行取材
    pub(crate) fn resets_at(&self) -> Option<DateTime<Utc>> {
        match self {
            PresentBucket::FiveHour(b) | PresentBucket::Weekly(b) => b.resets_at,
            PresentBucket::Mcp(m) => m.resets_at,
        }
    }

    /// 已用百分比：tooltip 摘要与指标行取材
    pub(crate) fn used_percent(&self) -> f64 {
        match self {
            PresentBucket::FiveHour(b) | PresentBucket::Weekly(b) => b.used_percent,
            PresentBucket::Mcp(m) => m.used_percent,
        }
    }
}

/// 附加块回填
/// 余额与 Token 档本轮拉取失败时沿用上一份快照的值，绝不为空；
/// 从未拉到过的块保持空。余额超过一天仍无新值即不再回填——余额端点
/// 持久故障时面板主数据照常刷新，冻结的假数字不随行，宁可消失可察觉。
pub(crate) fn merge_sidecar(new: &mut UsageSnapshot, prev: Option<&UsageSnapshot>) {
    let Some(prev) = prev else { return };
    // 年龄非负且一天内才回填：负年龄（时钟回拨）按恒新鲜处理会把冻结
    // 窗口拉长一个回拨量。
    let fresh = |as_of: DateTime<Utc>| {
        let age = Utc::now() - as_of;
        age >= chrono::Duration::zero() && age <= chrono::Duration::hours(24)
    };
    if new.balance.is_none() && prev.balance.as_ref().is_some_and(|b| fresh(b.as_of)) {
        new.balance = prev.balance.clone();
    }
    // 当日校验只约束今日档（legs[0]）：跨零点后旧值属于昨日窗口，回填
    // 会让今日档挂着昨日总量；周/月是滚动窗，旧值照常代表。整块缺时
    // 按块的活跃时刻计龄，部分缺档说明端点活着，回填不限龄。
    let same_day = prev.queried_at.date_naive() == new.queried_at.date_naive();
    match (&mut new.token_stats, prev.token_stats) {
        (Some(n), Some(p)) => {
            for i in 0..n.legs.len() {
                if n.legs[i].is_none() && (same_day || i > 0) {
                    n.legs[i] = p.legs[i];
                }
            }
        }
        (None, Some(p)) if fresh(p.as_of) => {
            let mut legs = p.legs;
            if !same_day {
                legs[0] = None;
            }
            new.token_stats = Some(TokenStats {
                legs,
                as_of: p.as_of,
            });
        }
        _ => {}
    }
}

/// 主视图高度特征
/// 指标行数、构成区、Token 统计、余额四特征的唯一事实源，
/// sync_main_height 的高度入参由此派生，区块陈列判定须与本处同源，
/// 漏同步的后果是内容被裁或底部留白。
pub(crate) fn main_features(snap: Option<&UsageSnapshot>) -> (usize, bool, bool, bool) {
    match snap {
        None => (0, false, false, false),
        Some(s) => (
            s.present_buckets().count(),
            s.has_mcp_details(),
            s.token_stats.is_some(),
            s.balance.is_some(),
        ),
    }
}

/// Token 统计档位
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenLeg {
    Today,
    Week,
    Month,
}

/// Token 统计三档
/// 档位、起点回退天数与日志标签，档序即卡片序。
pub(crate) const TOKEN_LEGS: [(TokenLeg, u64, &str); 3] = [
    (TokenLeg::Today, 0, "today"),
    (TokenLeg::Week, 6, "week"),
    (TokenLeg::Month, 29, "month"),
];

/// Token 消耗合计
/// 今日本地 0 点起与近 7 / 30 个自然日，legs 档序与 TOKEN_LEGS 一致。
/// 逐档独立，单档拉取失败为 None，不拖垮其余档。as_of 为块的活跃
/// 时刻：任一档拉到即刷新，整块缺时回填按它计龄。
#[derive(Debug, Clone, Copy)]
pub struct TokenStats {
    pub legs: [Option<f64>; TOKEN_LEGS.len()],
    pub as_of: DateTime<Utc>,
}

/// 账户余额，仅国内版
#[derive(Debug, Clone, Default)]
pub struct Balance {
    pub available: f64,
    /// 数值的拉取时刻：回填链上保持本体时刻，作陈旧判定
    pub as_of: DateTime<Utc>,
    #[allow(dead_code)]
    pub recharged: Option<f64>,
    #[allow(dead_code)]
    pub granted: Option<f64>,
    #[allow(dead_code)]
    pub spent: Option<f64>,
}

/// 官方错误信息三元组
/// code/message 缺失为 None/空串，显示侧按降级链处理。
#[derive(Debug, Clone, Hash)]
pub struct ErrorInfo {
    /// HTTP 状态码；HTTP 200 信封失败路径填 200
    pub status: u16,
    /// 官方业务码：官方样本字符串与数字形态并存，解析侧统一收 i64，
    /// 码族表与展示同用一份
    pub code: Option<i64>,
    /// 官方提示；响应形态不符时回退截断的原文，可为空
    pub message: String,
}

impl ErrorInfo {
    /// 本地程序错误
    /// 无 HTTP 状态与业务码，归类走服务端档兜底。
    pub(crate) fn local(msg: impl Into<String>) -> Self {
        ErrorInfo {
            status: 0,
            code: None,
            message: msg.into(),
        }
    }

    /// 码与提示的组合形态
    /// 组合形态判定单源，Display 与降级链取材同引。
    pub(crate) fn shape(&self) -> FieldShape<'_> {
        match (self.code, self.message.is_empty()) {
            (Some(c), false) => FieldShape::CodeAndMsg {
                code: c,
                msg: &self.message,
            },
            (Some(c), true) => FieldShape::CodeOnly { code: c },
            (None, false) => FieldShape::MsgOnly { msg: &self.message },
            (None, true) => FieldShape::Empty,
        }
    }

    /// 降级链取材
    /// 码与提示齐全 → 只有码 → 提示原文 → 错误状态回显 → 兜底。
    /// 错误卡正文与页脚尾段同引此判定，展示侧只管拼格式。
    pub(crate) fn detail(&self) -> ErrorDetail<'_> {
        match self.shape() {
            FieldShape::CodeAndMsg { code, msg } => ErrorDetail::CodeAndMsg { code, msg },
            FieldShape::CodeOnly { code } => ErrorDetail::CodeOnly { code },
            FieldShape::MsgOnly { msg } => ErrorDetail::Msg(msg.into()),
            // 码与提示皆缺：错误状态按状态回显，其余落到调用方兜底
            FieldShape::Empty if (400..600).contains(&self.status) => {
                ErrorDetail::Msg(format!("HTTP {}", self.status).into())
            }
            FieldShape::Empty => ErrorDetail::Fallback,
        }
    }
}

/// 降级链的四种组合
/// 码与提示齐全 → 只有码 → 只有提示 → 全无
pub(crate) enum FieldShape<'a> {
    CodeAndMsg { code: i64, msg: &'a str },
    CodeOnly { code: i64 },
    MsgOnly { msg: &'a str },
    Empty,
}

/// 错误展示的取材档
/// 提示档含状态回显；文本混编借入与拥有。
pub(crate) enum ErrorDetail<'a> {
    CodeAndMsg { code: i64, msg: &'a str },
    CodeOnly { code: i64 },
    Msg(std::borrow::Cow<'a, str>),
    Fallback,
}

/// 查询失败分类
/// Hash 为内容指纹，页脚摘要缓存键据此派生，无 Display 中间串分配。
#[derive(Debug, Clone, Hash)]
pub enum FetchError {
    /// 凭据失效
    /// HTTP 401/403 或信封鉴权族；换形态重试只限状态来源。
    Unauthenticated(ErrorInfo),
    /// 未失败但无数据
    /// 2xx 且信封通过但空 limits，或信封 1309 与套餐缺失文案，
    /// 或 HTTP 429 身体码 1309（与信封路径同判）。
    Empty,
    /// 响应失败
    /// 非 2xx 错误状态或信封业务失败；大类按状态或码族分归两档。
    Failure(ErrorInfo),
    /// 本地网络层失败
    /// 超时、断连，保留旧数据并允许重试。
    Network(String),
    /// HTTP 429
    /// retry_after_secs 为 Retry-After 头秒数，已钳退避上限，无头为 None。
    Throttled {
        info: ErrorInfo,
        retry_after_secs: Option<u64>,
    },
}

/// 显示大类
/// 展示层的六个呈现标题档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Unauthenticated,
    Request,
    Throttled,
    Server,
    Network,
    Empty,
}

impl FetchError {
    /// 大类判定
    /// 状态路径按状态类，信封路径按码族，坏数据归 Server。
    pub fn category(&self) -> ErrorCategory {
        match self {
            FetchError::Unauthenticated(_) => ErrorCategory::Unauthenticated,
            FetchError::Empty => ErrorCategory::Empty,
            FetchError::Network(_) => ErrorCategory::Network,
            FetchError::Throttled { .. } => ErrorCategory::Throttled,
            FetchError::Failure(info) => match info.status {
                200 => match info.code.map(family_of) {
                    Some(CodeFamily::Request) => ErrorCategory::Request,
                    _ => ErrorCategory::Server,
                },
                400..=499 => ErrorCategory::Request,
                _ => ErrorCategory::Server,
            },
        }
    }
}

/// 账户与订阅状态码：欠费、套餐失效或权限不符，恢复只能靠用户侧
/// 动作，速率退避等不来；码族表与 HTTP 429 分流同引此单源。
/// 1309 正常在 inband 与 429 分流处先判空态，落进码族表只是兜底。
pub(crate) fn is_account_state(code: i64) -> bool {
    matches!(code, 1113 | 1309 | 1311 | 1313 | 1314 | 1315)
}

/// 信封错误码的码族
/// inband_error 的变体选择与 category 的显示大类同引此表；
/// 换形态重试只认 401/403 状态，不查此表。429 窗口限额码（会随
/// 重置恢复）归 Throttled，账户状态码归 Request。
fn family_of(code: i64) -> CodeFamily {
    if is_account_state(code) {
        return CodeFamily::Request;
    }
    match code {
        401 | 403 | 1000 | 1001 | 1003 | 1005 | 1220 => CodeFamily::Auth,
        1302 | 1305 | 1308 | 1310 | 1316..=1321 => CodeFamily::Throttled,
        1210..=1215 | 1221 | 1222 | 1261 | 1301 => CodeFamily::Request,
        _ => CodeFamily::Other,
    }
}

/// 码族档位：family_of 的返回值
enum CodeFamily {
    Auth,
    Throttled,
    Request,
    Other,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 有前缀时空格衔接码、冒号衔接提示；空前缀时码与提示直接开头。
        fn with_info(
            prefix: &str,
            info: &ErrorInfo,
            f: &mut std::fmt::Formatter<'_>,
        ) -> std::fmt::Result {
            match (prefix, info.shape()) {
                ("", FieldShape::CodeAndMsg { code, msg }) => write!(f, "{code}: {msg}"),
                ("", FieldShape::CodeOnly { code }) => write!(f, "{code}"),
                ("", FieldShape::MsgOnly { msg }) => write!(f, "{msg}"),
                // 信封失败无码无提示：兜变体名，日志与去重游标不落空串。
                ("", FieldShape::Empty) => write!(f, "failure"),
                (p, FieldShape::CodeAndMsg { code, msg }) => write!(f, "{p} {code}: {msg}"),
                (p, FieldShape::CodeOnly { code }) => write!(f, "{p} {code}"),
                (p, FieldShape::MsgOnly { msg }) => write!(f, "{p}: {msg}"),
                (p, FieldShape::Empty) => write!(f, "{p}"),
            }
        }
        match self {
            FetchError::Unauthenticated(info) => with_info("unauthenticated", info, f),
            FetchError::Empty => write!(f, "empty"),
            FetchError::Failure(info) => match info.status {
                // 本地程序错误无状态码，前缀用 local，不虚构状态。
                0 => with_info("local", info, f),
                // 信封内失败 HTTP 层成功，状态不参与展示。
                200 => with_info("", info, f),
                s => with_info(&format!("HTTP {s}"), info, f),
            },
            FetchError::Network(d) => write!(f, "{d}"),
            FetchError::Throttled { info, .. } => with_info("throttled", info, f),
        }
    }
}

/// unit 单位代码转分钟乘数
/// 3=小时、6=周是配额桶仅见取值，其余代码语义不明，
/// 不作断言，仅保留时长换算兜底。
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

/// limits 数组：data 内优先，顶层数组兜底
fn limits_of(data: &Value) -> Option<&Vec<Value>> {
    data.get("limits")
        .and_then(Value::as_array)
        .or_else(|| data.as_array())
}

/// 条目类型标识
/// type 优先，name 兜底。
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

/// MCP 通道判定
/// TIME_LIMIT 为主，MCP_LIMIT 为别名。
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
/// 绝对值优先重算百分比，percentage 仅在绝对值缺失时兜底。
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
        // 按窗口时长归类：最短归 5h，最长归周。
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

    // 无窗口字段的桶按重置时刻排序补缺。
    unclassified.sort_by_key(|b| (b.resets_at.is_some(), b.resets_at.map(|t| t.timestamp())));
    for bucket in unclassified {
        if five_hour.is_none() {
            five_hour = Some(bucket);
        } else if weekly.is_none() {
            weekly = Some(bucket);
        }
    }

    // 代际判定：credit 桶在即 V3，双桶 V2，单桶 V1。
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
        token_stats: None,
        balance: None,
        queried_at: Local::now(),
    }
}

/// 信封内业务失败分类
/// 鉴权族与鉴权文案归 Unauthenticated，1309 与 coding plan 无套餐归
/// Empty，受限族归 Throttled，其余归 Failure。仅处理 HTTP 200 信封内
/// 错误，HTTP 401/403 由 client.rs 按状态码分类。关键词判定须从严：
/// 凭据词与 invalid/expired/无效 同时出现才判认证，
/// 避免业务文案误触发换钥引导。
fn inband_error(code: Option<i64>, msg: &str) -> FetchError {
    let m = msg.to_ascii_lowercase();
    let mentions_credential = m.contains("token") || m.contains("api key") || m.contains("apikey");
    let says_invalid = m.contains("invalid") || m.contains("expired") || msg.contains("无效");
    let info = || ErrorInfo {
        status: 200,
        code,
        message: msg.chars().take(ERR_BODY_CHARS).collect(),
    };
    if code.is_some_and(|c| matches!(family_of(c), CodeFamily::Auth))
        || m.contains("unauthorized")
        || (mentions_credential && says_invalid)
    {
        FetchError::Unauthenticated(info())
    } else if code == Some(1309) || m.contains("coding plan") {
        FetchError::Empty
    } else if code.is_some_and(|c| matches!(family_of(c), CodeFamily::Throttled)) {
        FetchError::Throttled {
            info: info(),
            retry_after_secs: None,
        }
    } else {
        FetchError::Failure(info())
    }
}

/// 官方业务码解析：兼容数字与字符串形态，统一收 i64。
/// 信封路径与 HTTP 错误体路径同引此函数，防同一码值两条路径归入
/// 不同码族。官方样本皆为整数字，字符串与浮点是未实证的防御分支。
pub(crate) fn code_value(v: &Value) -> Option<i64> {
    match v {
        Value::Number(_) => v.as_i64().or_else(|| {
            v.as_f64().and_then(|f| {
                let i = f as i64;
                (i as f64 == f).then_some(i)
            })
        }),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// 信封校验
/// success=false 或 code 非 200 归入 FetchError。
pub(crate) fn envelope_check(v: &Value) -> Result<(), FetchError> {
    if v.get("success").and_then(Value::as_bool) == Some(false) {
        let msg = v
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        let code = v.get("code").and_then(code_value);
        return Err(inband_error(code, msg));
    }
    if let Some(code) = v.get("code").and_then(code_value)
        && code != 200
    {
        let msg = v.get("msg").and_then(Value::as_str).unwrap_or("");
        return Err(inband_error(Some(code), msg));
    }
    Ok(())
}

/// 解析完整响应体
pub fn parse_response(body: &str) -> Result<UsageSnapshot, FetchError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| FetchError::Failure(ErrorInfo::local(format!("parse failed: {e}"))))?;

    let data = if v.is_array() {
        // 顶层数组是历史响应形态，直接当 limits 用。
        &v
    } else {
        envelope_check(&v)?;
        v.get("data")
            .ok_or_else(|| FetchError::Failure(ErrorInfo::local("missing data field")))?
    };

    // 空 limits 即 Empty：2xx 且信封通过但无数据。
    if limits_of(data).is_none_or(|limits| limits.is_empty()) {
        return Err(FetchError::Empty);
    }
    Ok(parse_usage(data))
}

/// 解析 model-usage 响应取区间 token 总消耗
/// 服务端合计 `data.totalUsage.totalTokensUsage` 优先，缺失时回退把
/// 所有模型所有时间点的 tokensUsage 逐点相加。信封校验与 quota 同源。
pub fn parse_token_total(body: &str) -> Result<f64, FetchError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| FetchError::Failure(ErrorInfo::local(format!("parse failed: {e}"))))?;
    envelope_check(&v)?;
    let data = v.get("data").unwrap_or(&Value::Null);
    if let Some(total) = data
        .get("totalUsage")
        .and_then(|t| t.get("totalTokensUsage"))
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
    {
        return Ok(total);
    }
    let models = data
        .get("modelDataList")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Failure(ErrorInfo::local("missing modelDataList")))?;
    let mut total = 0.0;
    for model in models {
        let Some(points) = model.get("tokensUsage").and_then(Value::as_array) else {
            continue;
        };
        for p in points {
            if let Some(n) = p.as_f64().filter(|v| *v > 0.0) {
                total += n;
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用最小快照：各特征字段按参填，其余取空态。
    /// mcp 传 Some(n)：桶在场且 n 条明细；Some(0)：桶在场明细空；None：桶缺
    fn feature_snap(
        five: bool,
        weekly: bool,
        mcp: Option<usize>,
        stats: bool,
        bal: bool,
    ) -> UsageSnapshot {
        let bucket = || {
            Some(QuotaBucket {
                used_percent: 1.0,
                resets_at: None,
                total: None,
                current: None,
            })
        };
        UsageSnapshot {
            plan_version: PlanVersion::V2,
            tier: PlanTier::Lite,
            plan_label: None,
            five_hour: five.then(bucket).flatten(),
            weekly: weekly.then(bucket).flatten(),
            mcp: mcp.map(|n| McpUsage {
                used_percent: 1.0,
                current_value: 1.0,
                total: 100.0,
                resets_at: None,
                details: (0..n)
                    .map(|i| McpDetail {
                        model_code: format!("m{i}"),
                        usage: 1.0,
                    })
                    .collect(),
            }),
            token_stats: stats.then_some(TokenStats {
                legs: [Some(1.0), Some(2.0), Some(3.0)],
                as_of: Utc::now(),
            }),
            balance: bal.then_some(Balance {
                as_of: Utc::now(),
                ..Balance::default()
            }),
            queried_at: Local::now(),
        }
    }

    /// MCP 构成区陈列判定：明细非空即陈列，与重置时刻无关
    #[test]
    fn mcp_details_presence() {
        let mcp_body = |reset_ms: Option<i64>| {
            let reset = reset_ms
                .map(|ms| format!(r#","nextResetTime":{ms}"#))
                .unwrap_or_default();
            format!(
                r#"{{"success":true,"data":{{"limits":[{{"type":"TIME_LIMIT","percentage":10.0,"usage":100,"currentValue":10{reset},"usageDetails":[{{"modelCode":"m","usage":1}}]}}]}}}}"#
            )
        };
        // 已过与未来的重置时刻同样陈列
        let past = (Utc::now() - chrono::Duration::hours(2)).timestamp_millis();
        let future = (Utc::now() + chrono::Duration::hours(1)).timestamp_millis();
        assert!(
            parse_response(&mcp_body(Some(past)))
                .unwrap()
                .has_mcp_details()
        );
        assert!(
            parse_response(&mcp_body(Some(future)))
                .unwrap()
                .has_mcp_details()
        );
        // 明细空则不陈列
        let empty = r#"{"success":true,"data":{"limits":[{"type":"TIME_LIMIT","percentage":10.0,"usage":100,"currentValue":10}]}}"#;
        assert!(!parse_response(empty).unwrap().has_mcp_details());
    }

    /// 附加块回填：失败块沿用旧值，新值不被覆盖，无旧可兜保持空
    #[test]
    fn sidecar_merge_backfills() {
        // 余额缺与单档缺：沿用旧值；新拉到的档不被旧值覆盖
        let mut new = feature_snap(true, true, None, true, false);
        new.token_stats = Some(TokenStats {
            legs: [Some(1.0), None, Some(3.0)],
            as_of: Utc::now(),
        });
        let mut prev = feature_snap(true, true, None, true, true);
        prev.token_stats = Some(TokenStats {
            legs: [None, Some(2.0), None],
            as_of: Utc::now(),
        });
        merge_sidecar(&mut new, Some(&prev));
        assert!(new.balance.is_some());
        let legs = new.token_stats.unwrap().legs;
        assert_eq!(legs, [Some(1.0), Some(2.0), Some(3.0)]);
        // 统计整块缺：沿用旧整块
        let mut new = feature_snap(true, true, None, false, false);
        merge_sidecar(&mut new, Some(&prev));
        assert!(new.token_stats.is_some());
        // 无旧可兜：保持空
        let mut new = feature_snap(true, true, None, false, false);
        merge_sidecar(&mut new, None);
        assert!(new.balance.is_none() && new.token_stats.is_none());
        // 跨日只挡今日档：旧档属于昨日窗口，今日档宁可占位；周/月是
        // 滚动窗照常沿用；余额不是日期窗口量，跨日仍沿用
        let mut prev = feature_snap(true, true, None, true, true);
        prev.token_stats = Some(TokenStats {
            legs: [Some(1.0), Some(2.0), Some(3.0)],
            as_of: Utc::now(),
        });
        prev.queried_at = Local::now() - chrono::Duration::days(1);
        let mut new = feature_snap(true, true, None, false, false);
        merge_sidecar(&mut new, Some(&prev));
        assert!(new.balance.is_some());
        assert_eq!(new.token_stats.unwrap().legs, [None, Some(2.0), Some(3.0)]);
        // 同日的部分缺档：含今日档在内照常回填
        prev.queried_at = Local::now();
        let mut new = feature_snap(true, true, None, true, false);
        new.token_stats = Some(TokenStats {
            legs: [None, None, Some(3.0)],
            as_of: Utc::now(),
        });
        merge_sidecar(&mut new, Some(&prev));
        assert_eq!(
            new.token_stats.unwrap().legs,
            [Some(1.0), Some(2.0), Some(3.0)]
        );
        // 余额本体超过陈旧界限：不再回填，余额行消失
        let mut prev = feature_snap(true, true, None, true, true);
        if let Some(b) = prev.balance.as_mut() {
            b.as_of = Utc::now() - chrono::Duration::hours(25);
        }
        let mut new = feature_snap(true, true, None, false, false);
        merge_sidecar(&mut new, Some(&prev));
        assert!(new.balance.is_none());
        // 统计块超过陈旧界限：整块不再回填，Token 卡消失
        let mut prev = feature_snap(true, true, None, true, true);
        prev.token_stats = Some(TokenStats {
            legs: [Some(1.0), Some(2.0), Some(3.0)],
            as_of: Utc::now() - chrono::Duration::hours(25),
        });
        let mut new = feature_snap(true, true, None, false, false);
        merge_sidecar(&mut new, Some(&prev));
        assert!(new.token_stats.is_none());
    }

    /// 在场桶序钉位：高度行数、缓存行集与绘制推进同引的绘制序
    #[test]
    fn present_buckets_order() {
        let tags = |s: &UsageSnapshot| {
            s.present_buckets()
                .map(|b| match b {
                    PresentBucket::FiveHour(_) => "5h",
                    PresentBucket::Weekly(_) => "w",
                    PresentBucket::Mcp(_) => "mcp",
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            tags(&feature_snap(true, true, Some(1), false, false)),
            ["5h", "w", "mcp"]
        );
        assert_eq!(tags(&feature_snap(false, true, None, false, false)), ["w"]);
        assert_eq!(tags(&feature_snap(true, false, None, false, false)), ["5h"]);
        assert_eq!(
            tags(&feature_snap(false, false, Some(0), false, false)),
            ["mcp"]
        );
        // 行数特征与桶序同源
        assert_eq!(
            main_features(Some(&feature_snap(true, false, Some(1), false, false))).0,
            2
        );
    }

    /// 高度特征钉位：桶缺失组合 → 四特征。与 sync_main_height /
    /// draw_main 的区块陈列判定同源，漂移即面板高度错位
    #[test]
    fn main_features_matrix() {
        assert_eq!(main_features(None), (0, false, false, false));
        // 三桶齐 + 明细 1 条 + 统计 + 余额
        assert_eq!(
            main_features(Some(&feature_snap(true, true, Some(1), true, true))),
            (3, true, true, true)
        );
        // MCP 桶在场但明细空：行数计它、构成区不陈列
        assert_eq!(
            main_features(Some(&feature_snap(true, false, Some(0), false, false))),
            (2, false, false, false)
        );
        // 只剩周桶与余额
        assert_eq!(
            main_features(Some(&feature_snap(false, true, None, false, true))),
            (1, false, false, true)
        );
        // 空快照（全缺）：零特征
        assert_eq!(
            main_features(Some(&feature_snap(false, false, None, false, false))),
            (0, false, false, false)
        );
    }

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
        assert!(matches!(parse_response(body), Err(FetchError::Failure(_))));
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
    fn business_error_maps_to_failure() {
        let body = r#"{ "success": false, "msg": "boom" }"#;
        let err = parse_response(body).unwrap_err();
        assert!(matches!(err, FetchError::Failure(_)));
        assert!(err.to_string().contains("boom"));
    }

    #[test]
    fn in_band_auth_failures_map_to_unauthenticated() {
        for body in [
            r#"{ "code": 401, "msg": "Unauthorized", "data": null, "success": false }"#,
            r#"{ "success": false, "msg": "token invalid" }"#,
            r#"{ "success": true, "code": 403, "msg": "forbidden" }"#,
            // 业务码族 1000/1001 也是鉴权失败
            r#"{ "success": false, "code": 1001, "msg": "auth required" }"#,
            r#"{ "success": false, "code": 1000, "msg": "" }"#,
            // 1003/1005/1220 同族：归档与修复提示同表判定
            r#"{ "success": false, "code": 1003, "msg": "no auth" }"#,
            r#"{ "success": false, "code": 1005, "msg": "no auth" }"#,
            r#"{ "success": false, "code": 1220, "msg": "no auth" }"#,
        ] {
            let err = parse_response(body).unwrap_err();
            assert!(
                matches!(err, FetchError::Unauthenticated(_)),
                "{body} → {err:?}"
            );
        }
    }

    /// 信封受限族归 Throttled，请求族与账户状态族归 Failure
    #[test]
    fn in_band_throttled_and_request_families() {
        for body in [
            r#"{ "success": false, "code": 1302, "msg": "限速" }"#,
            r#"{ "success": false, "code": 1308, "msg": "上限" }"#,
            r#"{ "success": false, "code": 1310, "msg": "上限" }"#,
            r#"{ "success": false, "code": 1316, "msg": "超额付费关闭" }"#,
        ] {
            let err = parse_response(body).unwrap_err();
            assert!(
                matches!(err, FetchError::Throttled { .. }),
                "{body} → {err:?}"
            );
        }
        for body in [
            r#"{ "success": false, "code": 1213, "msg": "bad" }"#,
            // 账户状态族：欠费与套餐失效恢复靠用户动作，不走退避
            r#"{ "success": false, "code": 1113, "msg": "欠费" }"#,
            r#"{ "success": false, "code": 1314, "msg": "企业套餐已失效" }"#,
        ] {
            let err = parse_response(body).unwrap_err();
            assert!(matches!(err, FetchError::Failure(_)), "{body} → {err:?}");
        }
    }

    #[test]
    fn in_band_credential_word_alone_is_not_auth() {
        for body in [
            r#"{ "success": false, "msg": "token limit exceeded" }"#,
            r#"{ "success": false, "msg": "api key quota exhausted" }"#,
        ] {
            let err = parse_response(body).unwrap_err();
            assert!(matches!(err, FetchError::Failure(_)), "{body} → {err:?}");
        }
    }

    #[test]
    fn in_band_long_message_truncated() {
        let long = "x".repeat(500);
        let body = format!(r#"{{ "success": false, "msg": "{long}" }}"#);
        let FetchError::Failure(info) = parse_response(&body).unwrap_err() else {
            panic!("长 msg 应归 Failure");
        };
        assert_eq!(info.message.chars().count(), ERR_BODY_CHARS);
    }

    /// 大类归属钉位：状态路径按状态类，信封路径按码族
    #[test]
    fn error_category_mapping() {
        let failure = |status: u16, code: Option<i64>| {
            FetchError::Failure(ErrorInfo {
                status,
                code,
                message: String::new(),
            })
        };
        // 状态权威：4xx 归请求档，5xx 与本地解析归服务端档，回显码不改归档
        assert_eq!(failure(400, None).category(), ErrorCategory::Request);
        assert_eq!(failure(404, None).category(), ErrorCategory::Request);
        assert_eq!(failure(503, None).category(), ErrorCategory::Server);
        assert_eq!(failure(0, None).category(), ErrorCategory::Server);
        assert_eq!(failure(400, Some(1003)).category(), ErrorCategory::Request);
        // 信封路径按码族：请求族归请求档，服务族与未知码归服务端档
        assert_eq!(failure(200, Some(1213)).category(), ErrorCategory::Request);
        assert_eq!(failure(200, Some(1301)).category(), ErrorCategory::Request);
        assert_eq!(failure(200, Some(1230)).category(), ErrorCategory::Server);
        assert_eq!(failure(200, Some(9999)).category(), ErrorCategory::Server);
        assert_eq!(failure(200, None).category(), ErrorCategory::Server);
        // 429 窗口限额码随重置恢复，归受限档走指数退避
        for c in [1302, 1305, 1308, 1310, 1316, 1317, 1318, 1319, 1320, 1321] {
            assert_eq!(
                parse_response(&format!(
                    r#"{{ "success": false, "code": {c}, "msg": "limited" }}"#
                ))
                .unwrap_err()
                .category(),
                ErrorCategory::Throttled,
                "code {c} 应归受限档"
            );
        }
        // 账户与订阅状态码：恢复靠用户侧动作，退避等不来，归请求档
        for c in [1113, 1311, 1313, 1314, 1315] {
            assert_eq!(
                parse_response(&format!(
                    r#"{{ "success": false, "code": {c}, "msg": "account" }}"#
                ))
                .unwrap_err()
                .category(),
                ErrorCategory::Request,
                "code {c} 应归请求档"
            );
        }
        // 变体直达
        assert_eq!(
            FetchError::Unauthenticated(ErrorInfo {
                status: 401,
                code: None,
                message: String::new()
            })
            .category(),
            ErrorCategory::Unauthenticated
        );
        assert_eq!(
            FetchError::Throttled {
                info: ErrorInfo {
                    status: 429,
                    code: None,
                    message: String::new()
                },
                retry_after_secs: None
            }
            .category(),
            ErrorCategory::Throttled
        );
        assert_eq!(
            FetchError::Network("timeout".into()).category(),
            ErrorCategory::Network
        );
        assert_eq!(FetchError::Empty.category(), ErrorCategory::Empty);
    }

    /// 1309 = 套餐过期；msg 含 "coding plan" = key 有效但无编码套餐，
    /// 该形态信封 code 为 500，不能按码判
    #[test]
    fn in_band_plan_state_maps_to_empty() {
        for body in [
            r#"{ "success": false, "code": 1309, "msg": "plan expired" }"#,
            r#"{ "success": false, "code": 500, "msg": "No coding plan found for this key" }"#,
        ] {
            let err = parse_response(body).unwrap_err();
            assert!(matches!(err, FetchError::Empty), "{body} → {err:?}");
        }
    }

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
            assert!(matches!(err, FetchError::Empty), "{body} → {err:?}");
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

    #[test]
    fn token_total_sums_all_models_and_points() {
        let body = r#"{
            "success": true,
            "code": 200,
            "data": {
                "x_time": ["08-27 10:00", "08-27 11:00"],
                "modelDataList": [
                    { "modelName": "GLM-5.3", "tokensUsage": [1200, 3400] },
                    { "modelName": "GLM-5.3-Flash", "tokensUsage": [500, 0] },
                    { "modelName": "broken", "tokensUsage": null }
                ]
            }
        }"#;
        assert!((parse_token_total(body).unwrap() - 5100.0).abs() < 1e-9);
    }

    /// 服务端合计字段为权威值，优先于点位求和
    #[test]
    fn token_total_prefers_server_side_summary() {
        let body = r#"{
            "success": true,
            "code": 200,
            "data": {
                "totalUsage": { "totalTokensUsage": 9999, "totalModelCallCount": 42 },
                "modelDataList": [
                    { "modelName": "GLM-5.3", "tokensUsage": [100, 200] }
                ]
            }
        }"#;
        assert_eq!(parse_token_total(body).unwrap(), 9999.0);
    }

    #[test]
    fn token_total_envelope_and_shape_errors() {
        // 信封失败
        let err = parse_token_total(r#"{ "success": false, "msg": "boom" }"#).unwrap_err();
        assert!(matches!(err, FetchError::Failure(_)));
        // 缺 modelDataList
        assert!(parse_token_total(r#"{ "success": true, "data": {} }"#).is_err());
        // 空模型表合法，总和为 0
        let body = r#"{ "success": true, "code": 200, "data": { "modelDataList": [] } }"#;
        assert_eq!(parse_token_total(body).unwrap(), 0.0);
    }
}
