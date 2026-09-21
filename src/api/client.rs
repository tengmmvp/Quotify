//! Usage query HTTP client

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
    AccountSpec, Balance, ERR_BODY_CHARS, ErrorInfo, FetchError, Platform, TOKEN_LEGS, TokenLeg,
    TokenStats, UsageSnapshot, parse_response, parse_token_total,
};

/// Hard cap on response body reads.
pub(crate) const MAX_BODY_BYTES: u64 = 1024 * 1024;
/// Read cap for error response bodies.
/// Error-body read cap: failure paths keep only a message-prefix worth of
/// body, so a huge error response never allocates a full success-sized
/// buffer. Shared by the usage client's error path and the update check.
pub(crate) const ERR_BODY_BYTES: u64 = 64 * 1024;
/// Character cap for official messages.
const ERR_MSG_CHARS: usize = 100;

/// Dual-agent cache with short and long timeouts.
struct AgentCache {
    proxy: Option<String>,
    short: ureq::Agent,
    long: ureq::Agent,
}

static AGENTS: RwLock<Option<AgentCache>> = RwLock::new(None);

fn agents_read() -> RwLockReadGuard<'static, Option<AgentCache>> {
    crate::locks::borrow_read(&AGENTS)
}

fn agents_write() -> RwLockWriteGuard<'static, Option<AgentCache>> {
    crate::locks::borrow_write(&AGENTS)
}

impl AgentCache {
    /// Cheap clone of the agent cache.
    /// Internally these are Arc handles; cloning only copies the references.
    fn cloned(&self) -> AgentCache {
        AgentCache {
            proxy: self.proxy.clone(),
            short: self.short.clone(),
            long: self.long.clone(),
        }
    }
}

/// Get the global agent cache.
/// Built without a proxy when missing.
fn agents() -> AgentCache {
    if let Some(c) = agents_read().as_ref() {
        return c.cloned();
    }
    let mut guard = agents_write();
    // Double-checked: another thread may have built it while we waited for the write
    // lock; reuse it directly to avoid duplicate agents.
    if let Some(c) = guard.as_ref() {
        return c.cloned();
    }
    let cache = AgentCache {
        proxy: None,
        short: build_agent(SHORT_TIMEOUT_SECS, None),
        long: build_agent(LONG_TIMEOUT_SECS, None),
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

/// Split credentials out of a proxy string's authority segment: returns (userinfo, host).
/// Split at the last @ — passwords may contain @, and splitting at the first @ would push
/// the password tail into the host.
/// Empty userinfo is kept as Some(""): the stray @ is stripped off the host, so '@:1080' lands on an
/// empty host and is rejected by validation instead of reaching ureq as an invalid authority.
/// Validation and diagnostic redaction share this single source; a semantic fork means
/// misjudgment or leaks.
pub fn proxy_split_userinfo(rest: &str) -> (Option<&str>, &str) {
    match rest.rfind('@') {
        Some(at) => (Some(&rest[..at]), &rest[at + 1..]),
        None => (None, rest),
    }
}

/// Byte-length cap for proxy strings — the sole source: the ui layer's
/// input buffer limit (panel/fields.rs) references this constant directly,
/// so an over-cap value from an imported file is rejected here rather than
/// silently truncated by one settings-page Apply.
pub(crate) const PROXY_MAX_BYTES: usize = 512;

/// Agent timeout tiers: short covers small-body endpoints, long covers the
/// body-largest ones (main usage and the month token window); poller's Drop
/// join bound follows LONG.
const SHORT_TIMEOUT_SECS: u64 = 5;
const LONG_TIMEOUT_SECS: u64 = 15;

/// Validate proxy address shape.
/// Explicit http:// or socks5:// prefix, non-empty host, port 1–65535,
/// byte length capped at PROXY_MAX_BYTES.
/// ureq fills in a default prefix and never validates the port, so parse it ourselves.
pub fn proxy_valid(s: &str) -> Result<(), String> {
    if s.len() > PROXY_MAX_BYTES {
        return Err(format!(
            "too long: {} bytes (max {PROXY_MAX_BYTES})",
            s.len()
        ));
    }
    let lower = s.trim().to_ascii_lowercase();
    let socks = lower.starts_with("socks5://");
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("socks5://"))
        .ok_or("must start with http:// or socks5://")?;
    let (userinfo, hostport) = proxy_split_userinfo(rest);
    // socks5 credentials are hard-cut at the last ':'; with more than one ':' in userinfo the
    // username and password become ambiguous and unrecoverable. HTTP proxies reassemble the
    // Basic header losslessly and are unrestricted.
    if socks && userinfo.is_some_and(|u| u.matches(':').count() > 1) {
        return Err("ambiguous ':' in socks5 credentials".into());
    }
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

/// Set the proxy and rebuild both agents.
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
    let short = build_agent(SHORT_TIMEOUT_SECS, parsed.as_ref());
    let long = build_agent(LONG_TIMEOUT_SECS, parsed.as_ref());
    *agents_write() = Some(AgentCache { proxy, short, long });
    Ok(())
}

/// Unified agent configuration.
/// Global timeout and https_only. Status codes are not turned into errors;
/// classification is centralized in `http_get_text`.
fn build_agent(timeout_secs: u64, proxy: Option<&ureq::Proxy>) -> ureq::Agent {
    let mut builder = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .https_only(true)
        .http_status_as_error(false)
        // Idle connections are reclaimed after 15s by default — shorter than the poll interval, so
        // every round rebuilt the connection; relax it so the pool survives across polls.
        .max_idle_age(Duration::from_secs(86400));
    builder = match proxy {
        Some(p) => builder.proxy(Some(p.clone())),
        None => builder.proxy(None),
    };
    builder.build().into()
}

/// Per-key memory: the Bearer-shape retry state and the balance-log dedup
/// slot live on one entry, so a key's memory lifecycle has a single owner
/// — account deletion clears everything in one removal, and any future
/// per-key state joins this struct instead of a second parallel map.
#[derive(Default)]
struct KeyMemory {
    /// Bearer prefix needed: remembered after a 401 shape-switch retry
    /// succeeds, so the next round's first attempt already uses the
    /// working credential shape.
    needs_bearer: bool,
    /// Balance log dedup slot: the last logged line, None when the last
    /// state was healthy — unchanged states log nothing.
    balance_log: Option<String>,
}

/// key hash → per-key memory. On lock poisoning the data is still intact;
/// take the inner data and keep using it.
///
/// Keyed by `key_hash`, never the plaintext: a process-lifetime map, and
/// hashed keys keep key material out of it (logs print no keys either;
/// nothing is persisted, so a deterministic hasher is all it needs to be).
/// Written by fetch rounds, shrunk by `retain_keys` with the account set —
/// an orphaned entry would hand a re-added key its old auth order.
static KEY_MEMORY: LazyLock<Mutex<std::collections::HashMap<u64, KeyMemory>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// key → stable hash.
fn key_hash(key: &str) -> u64 {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

/// Query whether a key is remembered as needing the Bearer shape.
fn needs_bearer(key: &str) -> bool {
    crate::locks::borrow(&KEY_MEMORY)
        .get(&key_hash(key))
        .is_some_and(|m| m.needs_bearer)
}

/// Remember or forget a key's Bearer shape.
fn set_needs_bearer(key: &str, on: bool) {
    let mut mem = crate::locks::borrow(&KEY_MEMORY);
    mem.entry(key_hash(key)).or_default().needs_bearer = on;
}

/// Drop the memory of every key absent from the alive set: the
/// account-deletion and config-import paths share this. KEY_MEMORY's
/// owner does the diff itself — callers only list the still-existing
/// accounts, so no future edit path can leave a deleted key's memory
/// behind by forgetting to thread the old set through.
/// One removal per key clears the Bearer shape and the balance dedup
/// slot together — a key's memory has the single owner KEY_MEMORY, so
/// re-adding the same key starts from a clean slate.
pub fn retain_keys(alive: &[&str]) {
    // Known edge: an in-flight request's trailing write at deletion time
    // may recreate an entry, so re-adding the same key costs at most one
    // extra 401 retry, self-healing within a round.
    crate::locks::borrow(&KEY_MEMORY).retain(|h, _| alive.iter().any(|k| key_hash(k) == *h));
}

/// Attempt order for credential shapes: if Bearer is remembered it goes first, otherwise the bare
/// key goes first, and the retry round uses the other. A successful shape-switch retry flips the
/// memory, so the next round's first attempt uses the effective shape.
fn auth_order<'a>(plain: &'a str, bearer: &'a str, remembered: bool) -> (&'a str, &'a str) {
    if remembered {
        (bearer, plain)
    } else {
        (plain, bearer)
    }
}

/// Join a scoped thread.
/// A thread panic propagates as-is, equivalent to direct propagation without threads.
fn join<T: Send>(handle: Option<std::thread::ScopedJoinHandle<'_, T>>) -> Option<T> {
    handle.map(|h| match h.join() {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    })
}

/// Log the balance endpoint only on state change: no duplicate entries for the same account in
/// the same state; recovery clears the slot and a later failure logs again; one slot per
/// account, so toggling back and forth doesn't re-log the same failure
fn log_balance_state(key: &str, line: Option<String>) {
    let mut mem = crate::locks::borrow(&KEY_MEMORY);
    let m = mem.entry(key_hash(key)).or_default();
    if m.balance_log.as_deref() == line.as_deref() {
        return;
    }
    if let Some(line) = line.as_deref() {
        crate::api::log(line);
    }
    m.balance_log = line;
}

/// Fetch one usage snapshot.
pub fn fetch_usage(spec: &AccountSpec) -> Result<UsageSnapshot, FetchError> {
    let team = spec.team_scope();
    // quota is the primary data, with independent failure semantics. Auth retries with the other
    // credential shape; all other errors propagate directly.
    let plain = spec.trimmed_key();
    let bearer = format!("Bearer {plain}");
    let remembered = needs_bearer(plain);
    let (first, second) = auth_order(plain, &bearer, remembered);
    let TokenWindows { mut starts, end } = token_params(chrono::Local::now());
    // Five parallel paths: the primary data is never held up by slow sidecar endpoints; a sidecar
    // failure only drops its block without dragging down main usage. Timeout tiers follow
    // criticality and response-body size together — the month leg is a sidecar yet rides the long
    // agent for its 30-day body. On the shape-switch round, sidecar endpoints may 401 and drop
    // blocks; the next round recovers once the shape is remembered.
    let (quota, legs, balance) = std::thread::scope(|scope| {
        let quota = scope.spawn(|| match fetch_quota(spec.platform, first, team) {
            Ok(snap) => Ok(snap),
            // Shape-switch retry only recognizes 401/403 statuses: envelope auth failures arrive
            // on HTTP 200, meaning the credential shape was already accepted and retrying is pointless.
            Err(FetchError::Unauthenticated(info)) if is_auth_status(info.status) => {
                let snap = fetch_quota(spec.platform, second, team)?;
                // Shape-switch retry succeeded: remember the shape that
                // worked this round; next round's first attempt uses it directly.
                set_needs_bearer(plain, !remembered);
                Ok(snap)
            }
            Err(e) => Err(e),
        });
        let end = end.as_str();
        // Single source of the tier mechanism: adding/removing tiers only touches TOKEN_LEGS.
        let legs: [_; TOKEN_LEGS.len()] = std::array::from_fn(|i| {
            let leg = TOKEN_LEGS[i].0;
            starts[i].take().map(|s| {
                scope.spawn(move || fetch_token_window(spec.platform, first, team, leg, &s, end))
            })
        });
        // The balance endpoint exists only on the domestic edition.
        let balance = (spec.platform == Platform::Cn).then(|| scope.spawn(|| fetch_balance(first)));
        let quota = quota
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p));
        (quota, legs.map(join), join(balance))
    });
    let mut snap = quota?;
    // Tiers are independent: one failed tier doesn't drag down the others; the block hides only
    // when all three are missing. Failed tiers aggregate into one log line, so
    // persistent failures don't flood the log.
    let failed: Vec<String> = TOKEN_LEGS
        .iter()
        .zip(&legs)
        .filter_map(|((_, _, tag), leg)| match leg {
            Some(Err(e)) => Some(format!("{tag}: {e}")),
            _ => None,
        })
        .collect();
    if !failed.is_empty() {
        crate::api::log(&format!("Token 档拉取失败: {}", failed.join("; ")));
    }
    // A sidecar 429 doesn't trigger separate backoff: while the main endpoint is healthy, the
    // regular cadence stands and the next round retries naturally. Any present tier sets
    // the block's as-of time; failed tiers are None with a placeholder leg_at — their
    // backfill age comes from the previous block's leg_at.
    let now = chrono::Utc::now();
    let stats = TokenStats {
        legs: legs.map(|l| l.and_then(Result::ok)),
        leg_at: [now; TOKEN_LEGS.len()],
        as_of: now,
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
            // Envelope failure and shape mismatch are both handled as no data:
            // the log keeps a record, and the backfill carries the old value.
            Ok(None) => log_balance_state(plain, Some("余额响应无有效数据".into())),
            Err(e) => log_balance_state(plain, Some(format!("余额拉取失败: {e}"))),
        }
    }
    Ok(snap)
}

/// The three-interval time windows for Token stats.
/// Three starts in TOKEN_LEGS tier order, one shared end. A start failing to localize only
/// blanks that window, without implicating the others.
struct TokenWindows {
    starts: [Option<String>; TOKEN_LEGS.len()],
    end: String,
}

/// Token stats interval parameters.
/// Starts at midnight of today, 6 days ago, and 29 days ago, plus an end-of-day terminal;
/// week/month windows span today included, 7 / 30 calendar days total — month is not a calendar
/// month. Starts must subtract days on the date side before localizing; subtracting a Duration
/// from a DateTime is a UTC instant operation and drifts an hour across DST boundaries.
fn token_params(now: chrono::DateTime<chrono::Local>) -> TokenWindows {
    // When a DST fall-back makes midnight ambiguous, take the early side:
    // an hour early beats losing a whole window.
    let day_start = |offset: u64, tag: &str| {
        let t = (now.date_naive() - chrono::Days::new(offset))
            .and_hms_opt(0, 0, 0)
            .and_then(|t| chrono::TimeZone::from_local_datetime(&chrono::Local, &t).earliest());
        if t.is_none() {
            crate::api::log(&format!("Token {tag} 窗口起点本地化失败，跳过"));
        }
        t
    };
    // When a DST fall-back makes end-of-day ambiguous, take the late side:
    // an hour late beats shortening the window.
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

/// Timeout tier per token window: the month window's 30-day body is the
/// largest — slow links need the long timeout; today/week stay short.
fn leg_timeout_secs(leg: TokenLeg) -> u64 {
    match leg {
        TokenLeg::Today | TokenLeg::Week => SHORT_TIMEOUT_SECS,
        TokenLeg::Month => LONG_TIMEOUT_SECS,
    }
}

/// Cached agent for a token window, picked by its timeout tier — a fresh
/// build would leave the pool empty every round.
fn leg_agent(leg: TokenLeg) -> ureq::Agent {
    match leg_timeout_secs(leg) {
        LONG_TIMEOUT_SECS => agent_long(),
        _ => agent_short(),
    }
}

/// Fetch one interval's total token consumption.
/// Prefers the server-side totalUsage for the total.
fn fetch_token_window(
    platform: Platform,
    api_key: &str,
    team: Option<(&str, &str)>,
    leg: TokenLeg,
    start: &str,
    end: &str,
) -> Result<f64, FetchError> {
    let mut url = format!(
        "{}/api/monitor/usage/model-usage?startTime={start}&endTime={end}",
        platform.base_url()
    );
    let extra = team_parts(&mut url, "3", team);
    let body = http_get_text(&leg_agent(leg), &url, api_key, &extra)?;
    parse_token_total(&body)
}

/// Format a timestamp as `YYYY-MM-DD HH:mm:ss`.
/// Spaces percent-encoded so it can go straight into a URL.
fn stamp(dt: &chrono::DateTime<chrono::Local>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S")
        .to_string()
        .replace(' ', "%20")
}

/// Account balance query [domestic edition only]. Open question: this
/// console-report endpoint carries no team scope (no type parameter, no
/// selection headers) while the sibling quota/token endpoints route
/// through team_parts — whether a team key gets the team's report, a
/// personal one, or empty here is unverified against the server. The
/// endpoint also predates Coding Plan (a pay-as-you-go console report),
/// so the semantic of a balance row under a team plan is itself murky;
/// route it through team_parts only after live behavior is confirmed.
fn fetch_balance(api_key: &str) -> Result<Option<Balance>, FetchError> {
    let url = "https://www.bigmodel.cn/api/biz/account/query-customer-account-report";
    let body = http_get(url, api_key)?;
    Ok(parse_balance(&body))
}

/// Balance report response parsing.
/// Envelope check shares the quota side's envelope_check; a flagged failure is discarded, and
/// the persist backfill carries the old value. availableBalance wins with balance as fallback;
/// non-finite numbers and both values missing are handled as no data.
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

/// GET and parse JSON.
fn http_get(url: &str, api_key: &str) -> Result<Value, FetchError> {
    let body = http_get_text(&agent_short(), url, api_key, &[])?;
    serde_json::from_str(&body)
        .map_err(|e| FetchError::Failure(ErrorInfo::local(format!("parse failed: {e}"))))
}

/// Send a GET and return the response body text.
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
        // 429 gets its own classification; the Retry-After header must be read before into_body consumes it.
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

/// Field extraction from non-2xx response bodies.
/// The official shape is `{"error":{"code","message"}}`; code parsing shares the single
/// code_value source, message truncated at the character cap. Shape mismatch falls back
/// to truncated raw text.
fn error_body_fields(body: &str) -> (Option<i64>, String) {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            // A non-object error shape falls back to raw text, same as missing.
            let e = v.get("error")?.as_object()?;
            let code = e.get("code").and_then(crate::api::code_value);
            // A missing message becomes an empty string and takes the code-only fallback.
            let message: String = e
                .get("message")
                .and_then(Value::as_str)
                .map(|m| m.chars().take(ERR_MSG_CHARS).collect())
                .unwrap_or_default();
            // An empty shell with neither code nor message falls back to raw text, same as a shape mismatch.
            (code.is_some() || !message.is_empty()).then_some((code, message))
        })
        .unwrap_or_else(|| (None, body.chars().take(ERR_BODY_CHARS).collect()))
}

/// Cap in seconds for 429 backoff waits.
pub(crate) const RATE_LIMIT_BACKOFF_MAX_SECS: u64 = 300;

/// Retry-After header parsing.
/// Supports both integer-seconds and HTTP-date formats; negative offsets clamp to zero
/// and to the backoff cap.
fn parse_retry_after(v: &str) -> Option<u64> {
    if let Ok(secs) = v.trim().parse::<i64>() {
        return Some(secs.clamp(0, RATE_LIMIT_BACKOFF_MAX_SECS as i64) as u64);
    }
    DateTime::parse_from_rfc2822(v.trim()).ok().map(|t| {
        (t.timestamp() - Utc::now().timestamp()).clamp(0, RATE_LIMIT_BACKOFF_MAX_SECS as i64) as u64
    })
}

/// Auth status predicate: 401/403.
/// The shape-switch retry guard and classify_status's variant choice share this predicate;
/// once the two sides' status sets diverge, the retry silently stops working for newly
/// effective statuses.
fn is_auth_status(status: u16) -> bool {
    matches!(status, 401 | 403)
}

/// Non-2xx status classification: the single point where 429 is fully classified.
/// 401/403 go to Unauthenticated; 429 looks at the body code first — account and subscription
/// state codes are not rate limits, and backoff can't wait them back to health; 1309 is judged
/// empty-state the same as the envelope path, other account states go to Failure, and no code
/// or any other code becomes Throttled carrying Retry-After backoff; all other statuses go to
/// Failure, with the broad category decided by status class in category. Auth is judged by
/// status alone: a non-401/403 failure never triggers a shape-switch retry, even if
/// it echoes an auth code.
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
        if code.is_some_and(crate::api::is_empty_state) {
            return FetchError::Empty;
        }
        if !code.is_some_and(crate::api::is_account_state) {
            return FetchError::throttled(
                ErrorInfo {
                    status,
                    code,
                    message,
                },
                retry_after_secs,
            );
        }
    }
    FetchError::Failure(ErrorInfo {
        status,
        code,
        message,
    })
}

/// Read the body up to a cap.
/// Invalid UTF-8 is replaced with `?`, matching ureq's default behavior.
/// Shared by the usage endpoints and the GitHub fetchers: a read-policy
/// fork here would leave the endpoints reading differently.
pub(crate) fn read_body_capped(body: ureq::Body, limit: u64) -> Result<String, ureq::Error> {
    body.into_with_config()
        .limit(limit)
        .lossy_utf8(true)
        .read_to_string()
}

/// Capped JSON read: the same single read policy for success bodies that
/// deserialize — the GitHub release check routes through here instead of
/// an inline chain, so read-policy changes reach every endpoint.
pub(crate) fn read_json_capped<T: serde::de::DeserializeOwned>(
    body: ureq::Body,
    limit: u64,
) -> Result<T, ureq::Error> {
    body.into_with_config().limit(limit).read_json()
}

/// Read an error body's prefix up to a cap.
/// On hitting the cap, keep what was read and continue extraction;
/// never discard everything for exceeding the limit.
pub(crate) fn read_body_prefix(body: ureq::Body, limit: u64) -> String {
    use std::io::Read;
    let mut buf = Vec::new();
    let _ = body.into_reader().take(limit).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Common section of team-edition requests: appends the type parameter to the URL (quota/limit=2,
/// model-usage=3, separator chosen by the existing query) and returns the organization/project
/// selection headers; personal edition does neither.
fn team_parts<'a>(
    url: &mut String,
    type_val: &str,
    team: Option<(&'a str, &'a str)>,
) -> Vec<(&'static str, &'a str)> {
    let Some((org, project)) = team else {
        return Vec::new();
    };
    url.push(if url.contains('?') { '&' } else { '?' });
    url.push_str("type=");
    url.push_str(type_val);
    vec![
        ("Bigmodel-Organization", org),
        ("Bigmodel-Project", project),
    ]
}

/// Fetch main usage from quota/limit.
fn fetch_quota(
    platform: Platform,
    auth: &str,
    team: Option<(&str, &str)>,
) -> Result<UsageSnapshot, FetchError> {
    let mut url = format!("{}/api/monitor/usage/quota/limit", platform.base_url());
    let extra = team_parts(&mut url, "2", team);
    let body_text = http_get_text(&agent_long(), &url, auth, &extra)?;
    parse_response(&body_text)
}

#[cfg(test)]
mod tests {
    use super::{
        LONG_TIMEOUT_SECS, PROXY_MAX_BYTES, SHORT_TIMEOUT_SECS, auth_order, is_auth_status,
        leg_timeout_secs, parse_balance, proxy_valid, team_parts, token_params,
    };
    use crate::api::FetchError;
    use crate::api::TokenLeg;
    use serde_json::json;

    /// Credential shape order: bare key first by default, Bearer first once remembered as needed.
    #[test]
    fn auth_order_by_memory() {
        assert_eq!(auth_order("k", "Bearer k", false), ("k", "Bearer k"));
        assert_eq!(auth_order("k", "Bearer k", true), ("Bearer k", "k"));
    }

    /// Timeout tier per token window pinned: the month window's 30-day body
    /// rides the long agent; today/week keep the short one.
    #[test]
    fn token_window_timeout_tiers() {
        assert_ne!(SHORT_TIMEOUT_SECS, LONG_TIMEOUT_SECS);
        assert_eq!(leg_timeout_secs(TokenLeg::Today), SHORT_TIMEOUT_SECS);
        assert_eq!(leg_timeout_secs(TokenLeg::Week), SHORT_TIMEOUT_SECS);
        assert_eq!(leg_timeout_secs(TokenLeg::Month), LONG_TIMEOUT_SECS);
    }

    /// Team request shape pinned: the type param (quota/limit=2,
    /// model-usage=3) joins with the separator the existing query dictates,
    /// plus both selection headers; personal edition gets neither.
    #[test]
    fn team_parts_shape() {
        let mut url = String::from("https://open.bigmodel.cn/api/monitor/usage/quota/limit");
        let extra = team_parts(&mut url, "2", Some(("org-9", "proj-8")));
        assert_eq!(
            url,
            "https://open.bigmodel.cn/api/monitor/usage/quota/limit?type=2"
        );
        assert_eq!(
            extra,
            vec![
                ("Bigmodel-Organization", "org-9"),
                ("Bigmodel-Project", "proj-8")
            ]
        );
        // model-usage already carries a query: the separator is '&'.
        let mut url = String::from(
            "https://open.bigmodel.cn/api/monitor/usage/model-usage?startTime=a&endTime=b",
        );
        let extra = team_parts(&mut url, "3", Some(("o", "p")));
        assert!(url.ends_with("&type=3"));
        assert_eq!(extra.len(), 2);
        // Personal edition: URL untouched, no headers.
        let mut url = String::from("https://open.bigmodel.cn/api/monitor/usage/quota/limit");
        let before = url.clone();
        assert!(team_parts(&mut url, "2", None).is_empty());
        assert_eq!(url, before);
    }

    /// Shape-switch retry only recognizes status-side auth failures:
    /// neither a 200 envelope auth failure nor 429 switches.
    #[test]
    fn auth_retry_status_gate() {
        assert!(is_auth_status(401));
        assert!(is_auth_status(403));
        assert!(!is_auth_status(200));
        assert!(!is_auth_status(429));
    }

    #[test]
    fn balance_report_shapes() {
        // All fields present: availableBalance wins, the three secondary fields come along.
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
        // Without availableBalance, fall back to balance.
        let b = parse_balance(&json!({ "data": { "balance": 7.0 } })).unwrap();
        assert_eq!(b.available, 7.0);
        assert_eq!(b.recharged, None);
        // Both missing: shape unknown — better absent than fabricated.
        assert!(parse_balance(&json!({ "data": {} })).is_none());
        // data non-object / missing likewise None.
        assert!(parse_balance(&json!({ "data": 3 })).is_none());
        assert!(parse_balance(&json!({})).is_none());
        // An explicit envelope success/code failure counts as no data;
        // the persist backfill carries the old value.
        assert!(parse_balance(&json!({ "success": false, "data": { "balance": 7.0 } })).is_none());
        assert!(
            parse_balance(&json!({ "code": 500, "success": true, "data": { "balance": 7.0 } }))
                .is_none()
        );
        assert!(parse_balance(&json!({ "success": true, "data": { "balance": 7.0 } })).is_some());
        // Pinned real response shape: null secondary fields, scientific notation, and
        // string fields coexisting; recharge + granted − spent = available.
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
        // Non-finite numbers treated as missing.
        let b =
            parse_balance(&json!({ "data": { "balance": 3.0, "rechargeAmount": null } })).unwrap();
        assert_eq!(b.recharged, None);
    }

    #[test]
    fn proxy_valid_shapes() {
        // Only an explicit prefix + valid structure pass.
        assert!(proxy_valid("http://127.0.0.1:7890").is_ok());
        assert!(proxy_valid("socks5://proxy.example.com:1080").is_ok());
        assert!(proxy_valid("HTTP://host:80").is_ok());
        assert!(proxy_valid("  http://host:80  ").is_ok());
        assert!(proxy_valid("http://user:pass@host:8080").is_ok());
        assert!(proxy_valid("socks5://[::1]:1080").is_ok());
        assert!(proxy_valid("http://localhost").is_ok());
        // Bare hostnames that ureq would default-scheme: all rejected.
        assert!(proxy_valid("127").is_err());
        assert!(proxy_valid("127.0.0.1:7890").is_err());
        assert!(proxy_valid("localhost").is_err());
        // Right prefix but broken structure: caught by our own parsing.
        assert!(proxy_valid("http://").is_err());
        assert!(proxy_valid("http://@:1080").is_err());
        assert!(proxy_valid("http://host:notaport").is_err());
        // socks5 credentials with an ambiguous colon are rejected (ureq hard-cuts at the last ':');
        // http reassembles the Basic header losslessly and is unrestricted
        assert!(proxy_valid("socks5://u:pa:ss@h:1").is_err());
        assert!(proxy_valid("http://u:pa:ss@h:1").is_ok());
        assert!(proxy_valid("socks5://u:p@h:1").is_ok());
        assert!(proxy_valid("http://host:0").is_err());
        assert!(proxy_valid("http://host:99999").is_err());
        assert!(proxy_valid("ftp://host:21").is_err());
        // Bare IPv6 without brackets slips past shape validation;
        // ureq's Uri parsing rejects it as a backstop.
        assert!(proxy_valid("http://::1").is_ok());
        assert!(ureq::Proxy::new("http://::1").is_err());
    }

    /// Length gate: 511/512 bytes pass, 513 is rejected and the reason
    /// carries both the actual length and the cap.
    #[test]
    fn proxy_valid_length_gate() {
        // "http://" + host + ":80" = 10 fixed bytes; padding goes into the host.
        let mk = |total: usize| format!("http://{}:80", "h".repeat(total - 10));
        assert_eq!(mk(511).len(), 511);
        assert!(proxy_valid(&mk(511)).is_ok());
        assert!(proxy_valid(&mk(512)).is_ok());
        let err = proxy_valid(&mk(513)).unwrap_err();
        assert!(err.contains("513"), "reason carries the length: {err}");
        assert!(
            err.contains(&PROXY_MAX_BYTES.to_string()),
            "reason carries the cap: {err}"
        );
    }

    #[test]
    fn token_window_params_shape() {
        use chrono::TimeZone;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 8, 29, 20, 15, 30)
            .unwrap();
        let w = token_params(now);
        // All four values percent-encoded; tier order = TOKEN_LEGS today/week/month, end at the
        // day's close, week/month starts span today included, 7 / 30 calendar days total
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
        // 429 single-point split: 1309 judged empty-state like the envelope path, account states
        // skip backoff, the rest including no code become Throttled carrying Retry-After
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
        // Other non-2xx go to Failure; the broad category is decided by status class in category.
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
        // Official error shape: code and message extracted as a pair, string form normalized to a number.
        assert_eq!(
            error_body_fields(
                r#"{"error":{"code":"1113","message":"您的账户已欠费，请充值后重试"}}"#
            ),
            (Some(1113), "您的账户已欠费，请充值后重试".to_string())
        );
        // The numeric code form is also accepted.
        assert_eq!(
            error_body_fields(r#"{"error":{"code":1309,"message":"expired"}}"#),
            (Some(1309), "expired".to_string())
        );
        // Non-JSON / shape mismatch: fall back to truncated raw text.
        assert_eq!(
            error_body_fields("gateway html"),
            (None, "gateway html".into())
        );
        assert_eq!(error_body_fields(""), (None, String::new()));
        // error without a message field: code-only fallback, not a wholesale fallback.
        assert_eq!(
            error_body_fields(r#"{"error":{"code":"1000"}}"#),
            (Some(1000), String::new())
        );
        // Non-object error: raw-text fallback, response content preserved.
        assert_eq!(
            error_body_fields(r#"{"error":"bad gateway"}"#),
            (None, r#"{"error":"bad gateway"}"#.to_string())
        );
        assert_eq!(
            error_body_fields(r#"{"error":null}"#),
            (None, r#"{"error":null}"#.to_string())
        );
        // Overlong messages truncated at the character cap.
        let long = "x".repeat(300);
        let body = format!(r#"{{"error":{{"code":"1113","message":"{long}"}}}}"#);
        let (_, msg) = error_body_fields(&body);
        assert_eq!(msg.chars().count(), super::ERR_MSG_CHARS);
    }

    #[test]
    fn retry_after_shapes() {
        use super::parse_retry_after;
        // Integer seconds, with surrounding whitespace.
        assert_eq!(parse_retry_after("30"), Some(30));
        assert_eq!(parse_retry_after(" 120 \r\n"), Some(120));
        // Negative offsets clamp to zero: retry immediately.
        assert_eq!(parse_retry_after("-5"), Some(0));
        // Pathologically large values clamp to the backoff cap.
        assert_eq!(parse_retry_after("999999999"), Some(300));
        assert_eq!(parse_retry_after("17600000000000"), Some(300));
        // Garbage beyond i64 fails to parse as None: header-less backoff.
        assert_eq!(parse_retry_after("99999999999999999999"), None);
        // Unparseable.
        assert_eq!(parse_retry_after("soon"), None);
        assert_eq!(parse_retry_after(""), None);
        // HTTP-date (RFC 1123) absolute times: far past clamps to zero, far future clamps by the difference.
        // The weekday must match the date — chrono's RFC 2822 parser checks consistency
        assert_eq!(parse_retry_after("Mon, 01 Jan 2001 00:00:00 GMT"), Some(0));
        assert_eq!(
            parse_retry_after("Sat, 15 Sep 2096 12:00:00 GMT"),
            Some(300)
        );
    }
}
