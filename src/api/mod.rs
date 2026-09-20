//! GLM Coding Plan usage data model and response parsing

use chrono::{DateTime, Local, Utc};
use serde_json::Value;

pub mod client;

/// Diagnostic log sink: the data layer does not depend on platform implementations;
/// main injects it at assembly time. Silently dropped when not injected (unit tests).
static LOG_HOOK: std::sync::OnceLock<fn(&str)> = std::sync::OnceLock::new();

/// Inject the platform log implementation.
pub fn set_log_hook(f: fn(&str)) {
    let _ = LOG_HOOK.set(f);
}

pub(crate) fn log(msg: &str) {
    if let Some(f) = LOG_HOOK.get() {
        f(msg);
    }
}

/// Character cap on response text carried in error details.
pub(crate) const ERR_BODY_CHARS: usize = 160;

/// API platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// Domestic edition, open.bigmodel.cn.
    Cn,
    /// International edition, api.z.ai.
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

/// Account parameters for one query.
/// Team edition exists only on the domestic site; the API key, type=2, and
/// both selection headers are all mandatory —
/// with a selection header missing, the server still replies success with empty limits.
#[derive(Debug, Clone)]
pub struct AccountSpec {
    pub platform: Platform,
    /// Team-edition flag: the deciding fact for the request shape. It used to be inferred from
    /// non-empty fields — a personal account with leftover organization/project strings would take
    /// the team query by mistake and always get empty limits.
    pub team: bool,
    /// Team edition: organization ID (`Bigmodel-Organization`).
    pub org_id: String,
    /// Team edition: project ID (`Bigmodel-Project`).
    pub project_id: String,
    pub api_key: String,
}

impl AccountSpec {
    /// A team query needs the flag, the CN site, and both selection headers;
    /// team edition exists only on the CN site, and the request shaper is
    /// the last line of defense — upstream normalization and the UI toggle
    /// arms keep the combination out, but any future writer that bypasses
    /// them would otherwise send team params and selection headers to the
    /// international base URL and get empty limits every round. If any
    /// requirement is missing, treat as personal edition.
    fn team_scope(&self) -> Option<(&str, &str)> {
        if !self.team || self.platform != Platform::Cn {
            return None;
        }
        let org = self.org_id.trim();
        let project = self.project_id.trim();
        (!org.is_empty() && !project.is_empty()).then_some((org, project))
    }
}

/// Plan generation.
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

/// Plan tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanTier {
    Lite,
    Pro,
    Max,
    Unknown,
}

impl PlanTier {
    /// Parse a plan label: the single classification ladder — the render
    /// side's tier badge colors key off it and must not re-derive
    /// contains() rules from display text.
    pub fn from_label(label: &str) -> PlanTier {
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

/// One quota bucket.
#[derive(Debug, Clone)]
pub struct QuotaBucket {
    /// Used percentage, 0–100.
    pub used_percent: f64,
    /// Reset time.
    pub resets_at: Option<DateTime<Utc>>,
    /// Total.
    pub total: Option<f64>,
    /// Current usage.
    pub current: Option<f64>,
}

/// MCP tool usage broken down by model.
#[derive(Debug, Clone)]
pub struct McpDetail {
    pub model_code: String,
    pub usage: f64,
}

/// MCP tool usage, monthly.
#[derive(Debug, Clone)]
pub struct McpUsage {
    pub used_percent: f64,
    /// Current usage.
    pub current_value: f64,
    /// Total.
    pub total: f64,
    /// Monthly reset time.
    pub resets_at: Option<DateTime<Utc>>,
    /// Per-model breakdown.
    pub details: Vec<McpDetail>,
}

/// Usage snapshot from one successful query.
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
    /// Whether to show the MCP composition section.
    /// Shown whenever details are non-empty; the height side and the render side share this source.
    pub fn has_mcp_details(&self) -> bool {
        self.mcp.as_ref().is_some_and(|m| !m.details.is_empty())
    }

    /// Bucket priority for the tray ring progress: the 5h hard gate wins — with the short-window
    /// quota exhausted, no amount of weekly headroom helps; only when 5h is absent does the
    /// weekly bucket represent the state (MCP is an independent tool quota and
    /// takes no part in the ring).
    pub fn ring_percent(&self) -> f64 {
        match (self.five_hour.as_ref(), self.weekly.as_ref()) {
            (Some(fh), _) => fh.used_percent,
            (None, Some(wk)) => wk.used_percent,
            (None, None) => 0.0,
        }
    }

    /// Enumerates present buckets in draw order: the single source for row-set composition,
    /// countdown rows, and render progression; the height side's row-count feature shares this order
    pub(crate) fn present_buckets(&self) -> impl Iterator<Item = PresentBucket<'_>> {
        self.five_hour
            .as_ref()
            .map(PresentBucket::FiveHour)
            .into_iter()
            .chain(self.weekly.as_ref().map(PresentBucket::Weekly))
            .chain(self.mcp.as_ref().map(PresentBucket::Mcp))
    }

    /// Tier display label.
    /// Prefers the tier label, falling back to the plan label when missing.
    pub fn tier_label(&self) -> &str {
        let t = self.tier.label();
        if t.is_empty() {
            self.plan_label.as_deref().unwrap_or("")
        } else {
            t
        }
    }
}

/// Draw-order variants of present metric buckets: five-hour → weekly → MCP.
pub(crate) enum PresentBucket<'a> {
    FiveHour(&'a QuotaBucket),
    Weekly(&'a QuotaBucket),
    Mcp(&'a McpUsage),
}

impl PresentBucket<'_> {
    /// Reset time: feeds the countdown rows.
    pub(crate) fn resets_at(&self) -> Option<DateTime<Utc>> {
        match self {
            PresentBucket::FiveHour(b) | PresentBucket::Weekly(b) => b.resets_at,
            PresentBucket::Mcp(m) => m.resets_at,
        }
    }

    /// Used percentage: feeds the tooltip summary and metric rows.
    pub(crate) fn used_percent(&self) -> f64 {
        match self {
            PresentBucket::FiveHour(b) | PresentBucket::Weekly(b) => b.used_percent,
            PresentBucket::Mcp(m) => m.used_percent,
        }
    }
}

/// Sidecar backfill.
/// When balance or Token tiers fail to fetch this round, carry over the previous
/// snapshot's value — never empty;
/// blocks never once fetched stay empty. Balance stops being backfilled after
/// a full day without a new value — when the
/// balance endpoint is persistently broken, the panel's main data keeps refreshing while
/// the frozen fake number does not tag along; better a noticeable disappearance.
pub(crate) fn merge_sidecar(new: &mut UsageSnapshot, prev: Option<&UsageSnapshot>) {
    let Some(prev) = prev else { return };
    // Backfill only when the age is non-negative and within one day: treating a negative age (clock
    // set back) as always fresh would stretch the frozen window by the rollback amount.
    let fresh = |as_of: DateTime<Utc>| {
        let age = Utc::now() - as_of;
        age >= chrono::Duration::zero() && age <= chrono::Duration::hours(24)
    };
    if new.balance.is_none() && prev.balance.as_ref().is_some_and(|b| fresh(b.as_of)) {
        new.balance = prev.balance.clone();
    }
    // The same-day check constrains only the today tier (legs[0]): past midnight the old value
    // belongs to yesterday's window, and backfilling it would pin yesterday's total onto the today
    // tier; week/month are rolling windows, so old values still represent them. When the whole
    // block is missing, age by the block's as-of time; partially missing tiers mean the endpoint
    // is alive, so backfill carries no age limit.
    let same_day = prev.queried_at.date_naive() == new.queried_at.date_naive();
    match (&mut new.token_stats, prev.token_stats) {
        (Some(n), Some(p)) => {
            // Per-tier age bound: the block's as_of refreshes whenever any
            // tier lands, so without leg_at a persistently failing tier
            // would be re-injected forever while a sibling keeps landing.
            for i in 0..n.legs.len() {
                if n.legs[i].is_none()
                    && (same_day || i > 0)
                    && p.legs[i].is_some()
                    && fresh(p.leg_at[i])
                {
                    n.legs[i] = p.legs[i];
                    // The backfilled value carries its own landing time.
                    n.leg_at[i] = p.leg_at[i];
                }
            }
        }
        (None, Some(p)) if fresh(p.as_of) => {
            let mut legs = p.legs;
            let leg_at = p.leg_at;
            if !same_day {
                legs[0] = None;
            }
            // The per-tier bound applies here too, not just in the partial
            // arm: the block gate (as_of) is refreshed by any sibling
            // landing and can outlive an individual tier by a full day,
            // which would resurrect it past twice the designed bound.
            for i in 0..legs.len() {
                if legs[i].is_some() && !fresh(leg_at[i]) {
                    legs[i] = None;
                }
            }
            // After the day-crossing clears the today tier, no tier may survive: backfilling an
            // all-empty stats block would leave the Token card present yet contentless — better absent.
            if legs.iter().any(Option::is_some) {
                new.token_stats = Some(TokenStats {
                    legs,
                    leg_at,
                    as_of: p.as_of,
                });
            }
        }
        _ => {}
    }
}

/// Main-view height features.
/// The single source of truth for the four features — metric row count, composition section,
/// Token stats, balance; sync_derived's height input derives from here, and block display
/// decisions must share this source — a missed sync means clipped content or
/// blank space at the bottom.
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

/// Token stats tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenLeg {
    Today,
    Week,
    Month,
}

/// The three Token stats tiers.
/// Tier, start-offset days, and log label; tier order is card order.
pub(crate) const TOKEN_LEGS: [(TokenLeg, u64, &str); 3] = [
    (TokenLeg::Today, 0, "today"),
    (TokenLeg::Week, 6, "week"),
    (TokenLeg::Month, 29, "month"),
];

/// Token consumption totals.
/// Today since local midnight plus the last 7 / 30 calendar days; legs tier order matches
/// TOKEN_LEGS.
/// Tiers are independent — one failed tier is None and does not drag down the rest. as_of is the
/// block's as-of time: refreshed whenever any tier lands. leg_at keeps each tier's own landing
/// time — the block's as_of cannot age tiers individually, so partial backfill would otherwise
/// re-inject a frozen tier indefinitely while a sibling tier keeps landing.
#[derive(Debug, Clone, Copy)]
pub struct TokenStats {
    pub legs: [Option<f64>; TOKEN_LEGS.len()],
    /// Last successful landing time per tier; for a None leg the field is
    /// placeholder — the backfilled value's age comes from the previous
    /// block's leg_at.
    pub leg_at: [DateTime<Utc>; TOKEN_LEGS.len()],
    pub as_of: DateTime<Utc>,
}

/// Account balance, domestic edition only.
#[derive(Debug, Clone, Default)]
pub struct Balance {
    pub available: f64,
    /// Fetch time of the value: kept at the block's own time along the backfill chain,
    /// used for staleness checks.
    pub as_of: DateTime<Utc>,
    #[allow(dead_code)]
    pub recharged: Option<f64>,
    #[allow(dead_code)]
    pub granted: Option<f64>,
    #[allow(dead_code)]
    pub spent: Option<f64>,
}

/// Official error info triple.
/// Missing code/message become None/empty string; the display side handles them via the
/// fallback chain.
/// PartialEq backs FetchError's cache-key full-value comparison.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ErrorInfo {
    /// HTTP status code; filled with 200 on the HTTP-200 envelope-failure path.
    pub status: u16,
    /// Official business code: official samples mix string and number forms, so parsing normalizes
    /// to i64; the code-family table and the display share this one copy
    pub code: Option<i64>,
    /// Official message; falls back to truncated raw text when
    /// the response shape mismatches; may be empty.
    pub message: String,
}

impl ErrorInfo {
    /// Local program error.
    /// No HTTP status or business code; categorization falls through to the server bucket.
    pub(crate) fn local(msg: impl Into<String>) -> Self {
        ErrorInfo {
            status: 0,
            code: None,
            message: msg.into(),
        }
    }

    /// Material for the fallback chain.
    /// Both code and message → code only → raw message → error status echo
    /// → last-resort fallback, decided here in one place. The error card
    /// body and the footer tail share this; the display side only formats.
    pub(crate) fn detail(&self) -> ErrorDetail<'_> {
        match (self.code, self.message.is_empty()) {
            (Some(c), false) => ErrorDetail::CodeAndMsg {
                code: c,
                msg: &self.message,
            },
            (Some(c), true) => ErrorDetail::CodeOnly { code: c },
            (None, false) => ErrorDetail::Msg(std::borrow::Cow::Borrowed(&self.message)),
            // Neither code nor message: error statuses echo the status; the
            // rest falls to the caller's fallback
            (None, true) if (400..600).contains(&self.status) => {
                ErrorDetail::Msg(std::borrow::Cow::Owned(format!("HTTP {}", self.status)))
            }
            (None, true) => ErrorDetail::Fallback,
        }
    }
}

/// Material tiers for error display.
/// The message tier includes the status echo; text mixes borrowed and owned.
pub(crate) enum ErrorDetail<'a> {
    CodeAndMsg { code: i64, msg: &'a str },
    CodeOnly { code: i64 },
    Msg(std::borrow::Cow<'a, str>),
    Fallback,
}

/// Query failure classification.
/// Hash/PartialEq act as a content fingerprint: footer summaries and error-card cache keys compare
/// full values against it, with no intermediate Display string allocation.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum FetchError {
    /// Credentials invalid.
    /// HTTP 401/403 or the envelope auth family; shape-switch retry applies only when status-sourced.
    Unauthenticated(ErrorInfo),
    /// Not a failure, just no data.
    /// 2xx with the envelope passing but empty limits, or envelope 1309 with the plan-missing
    /// wording, or HTTP 429 with body code 1309 (judged the same as the envelope path).
    Empty,
    /// Response failure.
    /// Non-2xx error status or envelope business failure; the broad category sorts into
    /// two buckets by status or code family.
    Failure(ErrorInfo),
    /// Local network-layer failure.
    /// Timeout, disconnection; old data is kept and retry stays allowed.
    Network(String),
    /// HTTP 429.
    /// retry_after_secs is the Retry-After header in seconds, already clamped to the
    /// backoff cap; None without the header.
    Throttled {
        info: ErrorInfo,
        retry_after_secs: Option<u64>,
    },
}

/// Display category.
/// Six heading tiers presented by the display layer.
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
    /// Clamping constructor for Throttled: retry_after_secs enters the
    /// enum only through here, so the ≤-backoff-cap invariant has a
    /// single production owner and consumers (backoff_wait) read the
    /// value without re-clamping.
    pub(crate) fn throttled(info: ErrorInfo, retry_after_secs: Option<u64>) -> Self {
        FetchError::Throttled {
            info,
            retry_after_secs: retry_after_secs
                .map(|s| s.min(crate::api::client::RATE_LIMIT_BACKOFF_MAX_SECS)),
        }
    }

    /// Broad-category decision.
    /// The status path sorts by status class, the envelope path by code family; bad data goes to Server.
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

/// Account and subscription state codes: arrears, expired plan, or permission mismatch — recovery
/// only comes from user-side action, never from rate backoff; the code-family table and the HTTP 429
/// split share this single source. 1309 is normally caught as the empty state at the inband and 429
/// splits; landing in the family table is only a backstop.
pub(crate) fn is_account_state(code: i64) -> bool {
    matches!(code, 1113 | 1309 | 1311 | 1313 | 1314 | 1315)
}

/// Empty-state business code: plan expired or missing — the envelope path and the HTTP 429 body-code
/// path share this decision; forking them would give one code different classifications
/// on the two paths
pub(crate) fn is_empty_state(code: i64) -> bool {
    code == 1309
}

/// Code families of envelope error codes.
/// inband_error's variant choice and category's display class share this table;
/// shape-switch retry only recognizes 401/403 statuses and never consults it. 429 window-quota
/// codes (which recover on reset) go to Throttled; account-state codes go to Request.
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

/// Code-family tiers: family_of's return values.
enum CodeFamily {
    Auth,
    Throttled,
    Request,
    Other,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The prefix join reads the fields directly — the log string and the
        // card material (detail) are two different renderings of the same
        // code/message pair, with no intermediate enum between them.
        fn with_info(
            prefix: &str,
            info: &ErrorInfo,
            f: &mut std::fmt::Formatter<'_>,
        ) -> std::fmt::Result {
            let msg = info.message.as_str();
            let (code, nonempty): (Option<i64>, bool) = (info.code, !msg.is_empty());
            match (prefix, code, nonempty) {
                ("", Some(c), true) => write!(f, "{c}: {msg}"),
                ("", Some(c), false) => write!(f, "{c}"),
                ("", None, true) => write!(f, "{msg}"),
                // Envelope failure with neither code nor message:
                // fall back to the variant name so logs and dedup cursors
                // never land on an empty string.
                ("", None, false) => write!(f, "failure"),
                (p, Some(c), true) => write!(f, "{p} {c}: {msg}"),
                (p, Some(c), false) => write!(f, "{p} {c}"),
                (p, None, true) => write!(f, "{p}: {msg}"),
                (p, None, false) => write!(f, "{p}"),
            }
        }
        match self {
            FetchError::Unauthenticated(info) => with_info("unauthenticated", info, f),
            FetchError::Empty => write!(f, "empty"),
            FetchError::Failure(info) => match info.status {
                // Local program errors carry no status code; the prefix is "local" — no fabricated status.
                0 => with_info("local", info, f),
                // In-envelope failure with a successful HTTP layer; the status takes no part in display.
                200 => with_info("", info, f),
                s => with_info(&format!("HTTP {s}"), info, f),
            },
            FetchError::Network(d) => write!(f, "{d}"),
            FetchError::Throttled { info, .. } => with_info("throttled", info, f),
        }
    }
}

/// unit code to minute multiplier.
/// 3=hour and 6=week are the only values seen on quota buckets; other codes have unclear
/// semantics — no assertion is made, the duration conversion stays as a fallback.
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

/// limits array: prefer inside data, fall back to the top-level array.
fn limits_of(data: &Value) -> Option<&Vec<Value>> {
    data.get("limits")
        .and_then(Value::as_array)
        .or_else(|| data.as_array())
}

/// Entry kind identifier.
/// type first, name as fallback.
fn entry_kind(item: &Value) -> &str {
    item.get("type")
        .or_else(|| item.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Quota bucket predicate.
fn is_quota_entry(item: &Value) -> bool {
    let t = entry_kind(item);
    t.eq_ignore_ascii_case("TOKENS_LIMIT") || t.eq_ignore_ascii_case("CREDIT_LIMIT")
}

fn is_credit_entry(item: &Value) -> bool {
    entry_kind(item).eq_ignore_ascii_case("CREDIT_LIMIT")
}

/// MCP channel predicate.
/// TIME_LIMIT is primary; MCP_LIMIT is an alias.
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

/// Parse a quota bucket.
/// Absolute values take priority to recompute the percentage; percentage is only a
/// fallback when absolutes are missing.
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

/// Unified usage snapshot parsing.
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
        // Classify by window duration: shortest becomes 5h, longest becomes weekly.
        timed.sort_by_key(|&(_, minutes)| minutes);
        if timed.len() == 1 {
            let (bucket, minutes) = timed.remove(0);
            if minutes == 300 {
                five_hour = Some(bucket);
            } else {
                weekly = Some(bucket);
            }
        } else {
            weekly = Some(timed.pop().expect("非单桶即 len ≥ 2").0);
            five_hour = Some(timed.remove(0).0);
        }
    }

    // Buckets without window fields backfill empty slots, ordered by reset time.
    unclassified.sort_by_key(|b| (b.resets_at.is_some(), b.resets_at.map(|t| t.timestamp())));
    for bucket in unclassified {
        if five_hour.is_none() {
            five_hour = Some(bucket);
        } else if weekly.is_none() {
            weekly = Some(bucket);
        }
    }

    // Generation decision: a credit bucket present means V3; two buckets V2; one bucket V1.
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

/// In-envelope business failure classification.
/// The auth family and auth wording go to Unauthenticated; 1309 and coding-plan-missing go to
/// Empty; the throttled family to Throttled; everything else to Failure. Handles only in-envelope
/// errors on HTTP 200 — HTTP 401/403 are classified by client.rs on the status code. Keyword
/// matching must stay strict: authentication is decided only when a credential word co-occurs with
/// invalid/expired/无效, so ordinary business copy can't falsely trigger the change-your-key guidance.
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
    } else if code.is_some_and(is_empty_state) || m.contains("coding plan") {
        FetchError::Empty
    } else if code.is_some_and(|c| matches!(family_of(c), CodeFamily::Throttled)) {
        FetchError::throttled(info(), None)
    } else {
        FetchError::Failure(info())
    }
}

/// Official business code parsing: accepts both number and string forms, normalized to i64.
/// The envelope path and the HTTP error-body path share this function, so one code value can't
/// land in different families on the two paths. Official samples are all integers; string and
/// float are defensive branches not yet seen in practice.
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

/// Envelope check.
/// success=false or code other than 200 becomes a FetchError.
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

/// Parse the full response body.
pub fn parse_response(body: &str) -> Result<UsageSnapshot, FetchError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| FetchError::Failure(ErrorInfo::local(format!("parse failed: {e}"))))?;

    let data = if v.is_array() {
        // A top-level array is a legacy response shape; use it directly as limits.
        &v
    } else {
        envelope_check(&v)?;
        v.get("data")
            .ok_or_else(|| FetchError::Failure(ErrorInfo::local("missing data field")))?
    };

    // Empty limits means Empty: 2xx with the envelope passing but no data.
    if limits_of(data).is_none_or(|limits| limits.is_empty()) {
        return Err(FetchError::Empty);
    }
    Ok(parse_usage(data))
}

/// Parse a model-usage response into the interval's total token consumption.
/// The server-side summary `data.totalUsage.totalTokensUsage` wins; when missing, fall back to
/// summing tokensUsage point by point across all models and time points.
/// Envelope check shares the quota side's source.
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

    /// Pins the CN-only line of defense in the request shaper: a team+Intl
    /// spec (only constructible by a config writer that bypassed
    /// normalization and the UI arms) must not take the team query shape.
    #[test]
    fn team_scope_rejects_intl() {
        let spec = |platform| AccountSpec {
            platform,
            team: true,
            org_id: "org".into(),
            project_id: "proj".into(),
            api_key: "k".into(),
        };
        assert!(spec(Platform::Cn).team_scope().is_some());
        assert!(spec(Platform::Intl).team_scope().is_none());
    }

    /// Minimal test snapshot: feature fields filled per argument, the rest left empty.
    /// mcp Some(n): bucket present with n details; Some(0): bucket present with empty
    /// details; None: bucket absent
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
                leg_at: [Utc::now(); TOKEN_LEGS.len()],
                as_of: Utc::now(),
            }),
            balance: bal.then_some(Balance {
                as_of: Utc::now(),
                ..Balance::default()
            }),
            queried_at: Local::now(),
        }
    }

    /// MCP composition section display: shown whenever details are non-empty, regardless of reset time.
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
        // Past and future reset times both display.
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
        // Empty details mean not displayed.
        let empty = r#"{"success":true,"data":{"limits":[{"type":"TIME_LIMIT","percentage":10.0,"usage":100,"currentValue":10}]}}"#;
        assert!(!parse_response(empty).unwrap().has_mcp_details());
    }

    /// Tray ring priority: a present 5h bucket beats everything, absence falls back to the weekly
    /// bucket, both absent is zero; MCP usage, however large, takes no part
    #[test]
    fn ring_percent_priority() {
        let mut snap = feature_snap(true, true, Some(3), false, false);
        snap.five_hour.as_mut().unwrap().used_percent = 40.0;
        snap.weekly.as_mut().unwrap().used_percent = 80.0;
        snap.mcp.as_mut().unwrap().used_percent = 95.0;
        assert_eq!(snap.ring_percent(), 40.0);
        snap.five_hour = None;
        assert_eq!(snap.ring_percent(), 80.0);
        snap.weekly = None;
        assert_eq!(snap.ring_percent(), 0.0);
    }

    /// Sidecar backfill: failed blocks carry old values, new values are not overwritten,
    /// and with nothing old to fall back on they stay empty
    #[test]
    fn sidecar_merge_backfills() {
        // Missing balance and a missing tier: carry old values; freshly fetched tiers are
        // not overwritten by old values
        let mut new = feature_snap(true, true, None, true, false);
        new.token_stats = Some(TokenStats {
            legs: [Some(1.0), None, Some(3.0)],
            leg_at: [Utc::now(); TOKEN_LEGS.len()],
            as_of: Utc::now(),
        });
        let mut prev = feature_snap(true, true, None, true, true);
        prev.token_stats = Some(TokenStats {
            legs: [None, Some(2.0), None],
            leg_at: [Utc::now(); TOKEN_LEGS.len()],
            as_of: Utc::now(),
        });
        merge_sidecar(&mut new, Some(&prev));
        assert!(new.balance.is_some());
        let legs = new.token_stats.unwrap().legs;
        assert_eq!(legs, [Some(1.0), Some(2.0), Some(3.0)]);
        // Whole stats block missing: carry the old block.
        let mut new = feature_snap(true, true, None, false, false);
        merge_sidecar(&mut new, Some(&prev));
        assert!(new.token_stats.is_some());
        // Nothing old to fall back on: stays empty.
        let mut new = feature_snap(true, true, None, false, false);
        merge_sidecar(&mut new, None);
        assert!(new.balance.is_none() && new.token_stats.is_none());
        // Crossing a day blocks only the today tier: the old tier belongs to yesterday's window,
        // so the today tier is better left as a placeholder; week/month are rolling windows and
        // carry over as usual; balance is not a date-windowed quantity and still carries over across days
        let mut prev = feature_snap(true, true, None, true, true);
        prev.token_stats = Some(TokenStats {
            legs: [Some(1.0), Some(2.0), Some(3.0)],
            leg_at: [Utc::now(); TOKEN_LEGS.len()],
            as_of: Utc::now(),
        });
        prev.queried_at = Local::now() - chrono::Duration::days(1);
        let mut new = feature_snap(true, true, None, false, false);
        merge_sidecar(&mut new, Some(&prev));
        assert!(new.balance.is_some());
        assert_eq!(new.token_stats.unwrap().legs, [None, Some(2.0), Some(3.0)]);
        // Partially missing tiers on the same day: backfilled as usual, today tier included.
        prev.queried_at = Local::now();
        let mut new = feature_snap(true, true, None, true, false);
        new.token_stats = Some(TokenStats {
            legs: [None, None, Some(3.0)],
            leg_at: [Utc::now(); TOKEN_LEGS.len()],
            as_of: Utc::now(),
        });
        merge_sidecar(&mut new, Some(&prev));
        assert_eq!(
            new.token_stats.unwrap().legs,
            [Some(1.0), Some(2.0), Some(3.0)]
        );
        // Balance itself past the staleness bound: no longer backfilled, the balance row disappears.
        let mut prev = feature_snap(true, true, None, true, true);
        if let Some(b) = prev.balance.as_mut() {
            b.as_of = Utc::now() - chrono::Duration::hours(25);
        }
        let mut new = feature_snap(true, true, None, false, false);
        merge_sidecar(&mut new, Some(&prev));
        assert!(new.balance.is_none());
        // Stats block past the staleness bound: the whole block no longer backfilled,
        // the Token card disappears.
        let mut prev = feature_snap(true, true, None, true, true);
        prev.token_stats = Some(TokenStats {
            legs: [Some(1.0), Some(2.0), Some(3.0)],
            leg_at: [Utc::now() - chrono::Duration::hours(25); TOKEN_LEGS.len()],
            as_of: Utc::now() - chrono::Duration::hours(25),
        });
        let mut new = feature_snap(true, true, None, false, false);
        merge_sidecar(&mut new, Some(&prev));
        assert!(new.token_stats.is_none());
        // Old block has a value only in the today tier and crosses a day:
        // nothing survives the clearing, backfill absent.
        let mut prev = feature_snap(true, true, None, true, true);
        prev.token_stats = Some(TokenStats {
            legs: [Some(1.0), None, None],
            leg_at: [Utc::now(); TOKEN_LEGS.len()],
            as_of: Utc::now(),
        });
        prev.queried_at = Local::now() - chrono::Duration::days(1);
        let mut new = feature_snap(true, true, None, false, false);
        merge_sidecar(&mut new, Some(&prev));
        assert!(new.token_stats.is_none());
        // Per-tier age bound: the today tier keeps landing (block as_of is
        // fresh) while the week tier last landed over a day ago — the frozen
        // week value must not be re-injected as a live number; the month
        // tier's fresh landing still backfills.
        let mut prev = feature_snap(true, true, None, true, true);
        prev.token_stats = Some(TokenStats {
            legs: [Some(1.0), Some(2.0), Some(3.0)],
            leg_at: [
                Utc::now(),
                Utc::now() - chrono::Duration::hours(25),
                Utc::now(),
            ],
            as_of: Utc::now(),
        });
        let mut new = feature_snap(true, true, None, true, false);
        new.token_stats = Some(TokenStats {
            legs: [Some(1.0), None, None],
            leg_at: [Utc::now(); TOKEN_LEGS.len()],
            as_of: Utc::now(),
        });
        merge_sidecar(&mut new, Some(&prev));
        assert_eq!(new.token_stats.unwrap().legs, [Some(1.0), None, Some(3.0)]);
        // Whole-block backfill honors the per-tier bound too: a tier whose
        // own landing is past the bound does not resurrect even while the
        // block's as_of stays fresh via sibling landings.
        let mut prev = feature_snap(true, true, None, true, true);
        prev.token_stats = Some(TokenStats {
            legs: [Some(1.0), Some(2.0), Some(3.0)],
            leg_at: [
                Utc::now(),
                Utc::now() - chrono::Duration::hours(25),
                Utc::now(),
            ],
            as_of: Utc::now(),
        });
        let mut new = feature_snap(true, true, None, false, false);
        merge_sidecar(&mut new, Some(&prev));
        assert_eq!(new.token_stats.unwrap().legs, [Some(1.0), None, Some(3.0)]);
    }

    /// Pinned present-bucket order: the draw order shared by height row counts,
    /// cached row sets, and render progression.
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
        // The row-count feature shares the bucket order's source.
        assert_eq!(
            main_features(Some(&feature_snap(true, false, Some(1), false, false))).0,
            2
        );
    }

    /// Pinned height features: bucket-absence combinations → the four features. Shares the block
    /// display decisions of sync_derived / draw_main; any drift means a misaligned panel height
    #[test]
    fn main_features_matrix() {
        assert_eq!(main_features(None), (0, false, false, false));
        // All three buckets + 1 detail + stats + balance.
        assert_eq!(
            main_features(Some(&feature_snap(true, true, Some(1), true, true))),
            (3, true, true, true)
        );
        // MCP bucket present with empty details: the row count includes it,
        // the composition section stays hidden.
        assert_eq!(
            main_features(Some(&feature_snap(true, false, Some(0), false, false))),
            (2, false, false, false)
        );
        // Only the weekly bucket and balance remain.
        assert_eq!(
            main_features(Some(&feature_snap(false, true, None, false, true))),
            (1, false, false, true)
        );
        // Empty snapshot (all absent): zero features.
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
            // Business code family 1000/1001 is also an auth failure.
            r#"{ "success": false, "code": 1001, "msg": "auth required" }"#,
            r#"{ "success": false, "code": 1000, "msg": "" }"#,
            // 1003/1005/1220 same family: archival and remediation hints judged by the same table.
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

    /// Envelope throttled family goes to Throttled; the request family and
    /// account-state family go to Failure.
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
            // Account-state family: arrears and expired plans recover through user action, not backoff.
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

    /// Pinned broad-category mapping: the status path by status class, the envelope path by code family.
    #[test]
    fn error_category_mapping() {
        let failure = |status: u16, code: Option<i64>| {
            FetchError::Failure(ErrorInfo {
                status,
                code,
                message: String::new(),
            })
        };
        // Status is authoritative: 4xx to the request tier, 5xx and local parse errors to
        // the server tier; an echoed code doesn't change the category.
        assert_eq!(failure(400, None).category(), ErrorCategory::Request);
        assert_eq!(failure(404, None).category(), ErrorCategory::Request);
        assert_eq!(failure(503, None).category(), ErrorCategory::Server);
        assert_eq!(failure(0, None).category(), ErrorCategory::Server);
        assert_eq!(failure(400, Some(1003)).category(), ErrorCategory::Request);
        // Envelope path by code family: the request family to the request tier,
        // the server family and unknown codes to the server tier.
        assert_eq!(failure(200, Some(1213)).category(), ErrorCategory::Request);
        assert_eq!(failure(200, Some(1301)).category(), ErrorCategory::Request);
        assert_eq!(failure(200, Some(1230)).category(), ErrorCategory::Server);
        assert_eq!(failure(200, Some(9999)).category(), ErrorCategory::Server);
        assert_eq!(failure(200, None).category(), ErrorCategory::Server);
        // 429 window-quota codes recover on reset; they go to the throttled tier with exponential backoff.
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
        // Account and subscription state codes: recovery needs user-side action that
        // backoff can't wait into being; they go to the request tier.
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
        // Direct variant access.
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
            FetchError::throttled(
                ErrorInfo {
                    status: 429,
                    code: None,
                    message: String::new()
                },
                None
            )
            .category(),
            ErrorCategory::Throttled
        );
        assert_eq!(
            FetchError::Network("timeout".into()).category(),
            ErrorCategory::Network
        );
        assert_eq!(FetchError::Empty.category(), ErrorCategory::Empty);
    }

    /// 1309 = plan expired; msg containing "coding plan" = key valid but no coding plan —
    /// in that shape the envelope code is 500, so it must not be judged by code
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

    /// Real response shape: a single 5h bucket + MCP details, level lowercase "max".
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

    /// The server-side summary field is authoritative, preferred over summing the points.
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
        // Envelope failure.
        let err = parse_token_total(r#"{ "success": false, "msg": "boom" }"#).unwrap_err();
        assert!(matches!(err, FetchError::Failure(_)));
        // Missing modelDataList.
        assert!(parse_token_total(r#"{ "success": true, "data": {} }"#).is_err());
        // An empty model list is valid; the total is 0.
        let body = r#"{ "success": true, "code": 200, "data": { "modelDataList": [] } }"#;
        assert_eq!(parse_token_total(body).unwrap(), 0.0);
    }
}
