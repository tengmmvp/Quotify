//! UI strings

/// Effective language
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Zh,
    En,
}

/// System language detection
pub fn detect_system_lang() -> Lang {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;
    let lang_id = unsafe { GetUserDefaultUILanguage() };
    // 0x0804 = Simplified Chinese (China), 0x0404 = Traditional Chinese (Taiwan),
    // 0x0C04 = Traditional Chinese (Hong Kong)
    matches!(lang_id, 0x0804 | 0x0404 | 0x0C04)
        .then_some(Lang::Zh)
        .unwrap_or(Lang::En)
}

/// Explicit value of the configured language string: Some means an explicit zh/en,
/// None means follow the system.
pub fn explicit_lang(setting: Option<&str>) -> Option<Lang> {
    match setting.map(str::trim) {
        Some(s) if s.eq_ignore_ascii_case("zh") => Some(Lang::Zh),
        Some(s) if s.eq_ignore_ascii_case("en") => Some(Lang::En),
        _ => None,
    }
}

/// Resolves the language setting from the config
pub fn resolve_lang(setting: Option<&str>) -> Lang {
    explicit_lang(setting).unwrap_or_else(detect_system_lang)
}

/// All UI strings.
///
/// Fields are grouped as "common first, view-specific in UI order", with same-family pairs adjacent;
/// ZH / EN keep the same order as this definition — adding or changing a field is kept in
/// sync in three places, and key-set plus placeholder consistency is compared
/// automatically by tests.
#[derive(serde::Serialize)]
pub struct Strings {
    // ── Common buttons ──
    pub cancel: &'static str,
    pub save: &'static str,
    pub apply: &'static str,

    // ── Time units ──
    pub unit_day: &'static str,
    pub unit_hour: &'static str,
    pub unit_minute: &'static str,
    pub unit_second: &'static str,

    // ── Main view · Metrics ──
    pub usage_section: &'static str,
    pub five_hour: &'static str,
    pub weekly: &'static str,
    pub mcp_tools: &'static str,
    pub resets_line: &'static str,
    pub used_of: &'static str,
    pub token_usage_section: &'static str,
    pub today_tokens: &'static str,
    pub week_tokens: &'static str,
    pub month_tokens: &'static str,
    pub balance_label: &'static str,

    // ── Main view · Peak hours ──
    pub peak_badge: &'static str,
    pub peak_tip: &'static str,

    // ── Main view · Status ──
    pub updated_just_now: &'static str,
    pub updated_ago: &'static str,
    pub loading: &'static str,
    pub not_configured_title: &'static str,
    pub not_configured_hint: &'static str,
    pub key_invalid: &'static str,

    // ── Error display ──
    /// Prefix segment of the footer error summary: the data-as-of time;
    /// the separator between segments is joined on the drawing side.
    pub data_as_of: &'static str,
    /// Category title: authentication failed
    pub err_cat_auth: &'static str,
    /// Category title: bad request
    pub err_cat_bad_request: &'static str,
    /// Category title: rate limited
    pub err_cat_rate_limited: &'static str,
    /// Category title: server error
    pub err_cat_server: &'static str,
    /// Category title: network error
    pub err_cat_network: &'static str,
    /// Category title: no data
    pub err_cat_no_data: &'static str,
    /// Error card body: both code and message present
    pub err_code_msg: &'static str,
    /// Error card body: code only
    pub err_code_only: &'static str,
    /// Fallback body when an auth failure has no official message
    pub err_auth_hint: &'static str,
    /// Guidance body for the no-data case
    pub err_empty_hint: &'static str,
    /// Update check failed; corresponds to the Err of `service::update`
    pub err_update: &'static str,

    // ── Tray tooltip ──
    pub tooltip_5h: &'static str,
    pub tooltip_weekly: &'static str,
    pub tooltip_mcp: &'static str,

    // ── Tray menu ──
    pub settings: &'static str,
    pub about: &'static str,
    pub exit: &'static str,

    // ── Settings · Current account ──
    pub accounts_section: &'static str,
    pub platform_section: &'static str,
    pub add_account: &'static str,
    pub account_name: &'static str,
    /// Fallback name when the add form's name is left empty
    pub default_account_name: &'static str,
    pub account_platform: &'static str,
    pub platform_cn: &'static str,
    pub platform_intl: &'static str,
    pub account_type_label: &'static str,
    pub type_personal: &'static str,
    pub type_team: &'static str,
    pub team_badge: &'static str,
    pub org_id_label: &'static str,
    pub project_id_label: &'static str,
    pub api_key_label: &'static str,
    pub switch_account: &'static str,

    // ── Settings · Poll interval ──
    pub poll_interval: &'static str,
    pub interval_1m: &'static str,
    pub interval_5m: &'static str,
    pub interval_15m: &'static str,
    pub interval_30m: &'static str,
    pub interval_custom: &'static str,
    pub interval_custom_unit: &'static str,

    // ── Settings · General ──
    pub settings_general: &'static str,
    pub language: &'static str,
    /// The language options' native names; they do not change with the UI language
    pub lang_name_zh: &'static str,
    pub lang_name_en: &'static str,
    pub follow_system: &'static str,
    pub appearance_section: &'static str,
    pub theme_light: &'static str,
    pub theme_dark: &'static str,
    pub autostart: &'static str,

    // ── Settings · Network proxy ──
    pub network_section: &'static str,
    pub proxy_label: &'static str,
    pub proxy_hint: &'static str,

    // ── Settings · Usage notifications ──
    pub notifications: &'static str,
    pub notify_threshold: &'static str,
    pub notify_threshold_desc: &'static str,
    pub notify_reset_5h_opt: &'static str,
    pub notify_reset_5h_desc: &'static str,
    pub notify_reset_weekly_opt: &'static str,
    pub notify_reset_weekly_desc: &'static str,

    // ── Settings · Peak hours ──
    pub peak_section: &'static str,
    pub peak_start_label: &'static str,
    pub peak_end_label: &'static str,

    // ── Settings · Config management and About ──
    pub backup_section: &'static str,
    pub export_config: &'static str,
    pub export_done: &'static str,
    pub export_failed: &'static str,
    pub export_confirm_title: &'static str,
    pub export_confirm_body: &'static str,
    pub import_config: &'static str,
    pub import_done: &'static str,
    pub import_done_proxy_skipped: &'static str,
    pub import_failed: &'static str,
    /// Import rejection: the file's account count exceeds MAX_ACCOUNTS
    pub import_failed_too_many: &'static str,
    pub import_confirm_title: &'static str,
    pub import_confirm_body: &'static str,
    pub check_update: &'static str,
    pub up_to_date: &'static str,
    pub go_download: &'static str,
    pub version_label: &'static str,
    pub version_new: &'static str,

    // ── About window ──
    pub app_desc: &'static str,
    pub link_repo: &'static str,
    pub link_issues: &'static str,
    pub link_diag: &'static str,
    /// Heading of the recent-logs section appended to the copied diagnostics text
    pub diag_recent_logs: &'static str,
    pub whats_new_section: &'static str,
    /// Quote shown after the easter egg has been fully consumed
    pub egg_quote: &'static str,

    // ── System notification titles ──
    pub notify_threshold_title: &'static str,
    pub notify_reset_5h: &'static str,
    pub notify_reset_weekly: &'static str,
    pub notify_config_broken: &'static str,
    pub notify_diag_copied: &'static str,
    /// Copy-diagnostics failure feedback (clipboard busy, etc.)
    pub notify_copy_failed: &'static str,
    pub notify_interval_invalid: &'static str,
    pub notify_proxy_invalid: &'static str,
    pub notify_team_required: &'static str,
    /// Add-form rejection: the account count is at MAX_ACCOUNTS
    pub notify_account_limit: &'static str,
}

const ZH: Strings = Strings {
    // ── Common buttons ──
    cancel: "取消",
    save: "保存",
    apply: "确定",

    // ── Time units ──
    unit_day: "天",
    unit_hour: "小时",
    unit_minute: "分钟",
    unit_second: "秒",

    // ── Main view · Metrics ──
    usage_section: "额度用量",
    five_hour: "5 小时会话窗口",
    weekly: "周会话窗口",
    mcp_tools: "MCP 工具",
    resets_line: "{t}后重置",
    used_of: "已用 {cur} / {tot}",
    token_usage_section: "Token 消耗",
    today_tokens: "今日",
    week_tokens: "本周",
    month_tokens: "本月",
    balance_label: "账户余额",

    // ── Main view · Peak hours ──
    peak_badge: "高峰",
    peak_tip: "高峰时段：工作日 {r}（UTC+8），模型调用额度消耗更快",

    // ── Main view · Status ──
    updated_just_now: "刚刚更新",
    updated_ago: "数据更新于 {t}前",
    loading: "加载中…",
    not_configured_title: "未配置账号",
    not_configured_hint: "进入设置，添加 GLM Coding Plan 账号即可开始使用",
    key_invalid: "[提示] API key 无效，请在设置中检查",

    // ── Error display ──
    data_as_of: "数据截至 {t}",
    err_cat_auth: "认证失败",
    err_cat_bad_request: "请求出错",
    err_cat_rate_limited: "请求受限",
    err_cat_server: "服务端错误",
    err_cat_network: "网络错误",
    err_cat_no_data: "暂无数据",
    err_code_msg: "错误码 {code}：{msg}",
    err_code_only: "错误码 {code}",
    err_auth_hint: "API key 无效或已失效，请检查后重试",
    err_empty_hint: "未返回额度数据。请确认 API key 属于编码套餐且未到期；团队版需填写组织 ID 与项目 ID",
    err_update: "检查更新失败",

    // ── Tray tooltip ──
    tooltip_5h: "5h",
    tooltip_weekly: "周",
    tooltip_mcp: "MCP",

    // ── Tray menu ──
    settings: "设置",
    about: "关于",
    exit: "退出",

    // ── Settings · Current account ──
    accounts_section: "当前账号",
    platform_section: "账号信息",
    add_account: "添加账号",
    account_name: "名称",
    default_account_name: "默认账号",
    account_platform: "平台",
    platform_cn: "国内版",
    platform_intl: "国际版",
    account_type_label: "类型",
    type_personal: "个人版",
    type_team: "团队版",
    team_badge: "团队",
    org_id_label: "组织 ID",
    project_id_label: "项目 ID",
    api_key_label: "API Key",
    switch_account: "切换账号",

    // ── Settings · Poll interval ──
    poll_interval: "轮询间隔",
    interval_1m: "1 分钟",
    interval_5m: "5 分钟",
    interval_15m: "15 分钟",
    interval_30m: "30 分钟",
    interval_custom: "自定义",
    interval_custom_unit: "分钟",

    // ── Settings · General ──
    settings_general: "通用设置",
    language: "语言",
    lang_name_zh: "中文",
    lang_name_en: "English",
    follow_system: "跟随系统",
    appearance_section: "外观",
    theme_light: "浅色",
    theme_dark: "深色",
    autostart: "开机自启",

    // ── Settings · Network proxy ──
    network_section: "网络代理",
    proxy_label: "代理地址",
    proxy_hint: "留空直连，支持 http:// 或 socks5://",

    // ── Settings · Usage notifications ──
    notifications: "用量通知",
    notify_threshold: "用量预警",
    notify_threshold_desc: "超过 {p}% 时提醒一次，额度恢复后重新计数",
    notify_reset_5h_opt: "5 小时会话窗口重置提醒",
    notify_reset_5h_desc: "进入新 5 小时会话窗口时发送系统通知",
    notify_reset_weekly_opt: "周会话窗口重置提醒",
    notify_reset_weekly_desc: "进入新的周会话窗口时发送系统通知",

    // ── Settings · Peak hours ──
    peak_section: "高峰区间",
    peak_start_label: "开始",
    peak_end_label: "结束",

    // ── Settings · Config management and About ──
    backup_section: "配置管理",
    export_config: "导出配置",
    export_done: "配置已导出：含 API key，请妥善保管",
    export_failed: "导出失败：无法写入所选文件",
    export_confirm_title: "导出配置",
    export_confirm_body: "导出文件将包含明文 API key，请妥善保管。",
    import_config: "导入配置",
    import_done: "配置已导入",
    import_done_proxy_skipped: "已导入；代理地址无效，沿用原代理",
    import_failed: "导入失败：文件不可读或格式无效",
    import_failed_too_many: "导入失败：账号数超上限",
    import_confirm_title: "导入确认",
    import_confirm_body: "导入将整体覆盖当前的账号与设置，代理若不同也会切换。确定继续？",
    check_update: "检查更新",
    up_to_date: "已是最新版本",
    go_download: "前往下载",
    version_label: "当前版本：{v}",
    version_new: "当前版本：{cur} → {new}",

    // ── About window ──
    app_desc: "Windows 任务栏托盘的 GLM Coding Plan 用量小组件",
    link_repo: "开源仓库",
    link_issues: "问题反馈",
    link_diag: "复制诊断",
    diag_recent_logs: "最近日志",
    whats_new_section: "最新动态",
    egg_quote: "人不能两次踏进同一条河流。",

    // ── System notification titles ──
    notify_threshold_title: "额度预警",
    notify_reset_5h: "5 小时窗口已重置",
    notify_reset_weekly: "周额度已重置",
    notify_config_broken: "config.toml 解析失败，已回退默认配置，原文件留档为 config.toml.bak，账号与设置需重新配置",
    notify_diag_copied: "诊断信息已复制到剪贴板",
    notify_copy_failed: "复制失败：剪贴板被占用，请稍后重试",
    notify_interval_invalid: "自定义间隔无效：{v}",
    notify_proxy_invalid: "代理地址无效，未保存",
    notify_team_required: "团队账号需填写组织 ID 与项目 ID",
    notify_account_limit: "账号数已达上限（100）",
};

const EN: Strings = Strings {
    // ── Common buttons ──
    cancel: "Cancel",
    save: "Save",
    apply: "Apply",

    // ── Time units ──
    unit_day: "d",
    unit_hour: "h",
    unit_minute: "m",
    unit_second: "s",

    // ── Main view · Metrics ──
    usage_section: "USAGE",
    five_hour: "Session (5h)",
    weekly: "Session (weekly)",
    mcp_tools: "MCP tools",
    resets_line: "Resets in {t}",
    used_of: "{cur} of {tot} used",
    token_usage_section: "TOKEN",
    today_tokens: "Today",
    week_tokens: "Weekly",
    month_tokens: "Monthly",
    balance_label: "BALANCE",

    // ── Main view · Peak hours ──
    peak_badge: "PEAK",
    peak_tip: "Peak hours: weekdays {r} (UTC+8); model calls drain quota faster",

    // ── Main view · Status ──
    updated_just_now: "Updated just now",
    updated_ago: "Updated {t} ago",
    loading: "Loading…",
    not_configured_title: "No account configured",
    not_configured_hint: "Open Settings and add a GLM Coding Plan account to get started",
    key_invalid: "[Note] Invalid API key. Check it in Settings",

    // ── Error display ──
    data_as_of: "Data as of {t}",
    err_cat_auth: "Authentication failed",
    err_cat_bad_request: "Bad request",
    err_cat_rate_limited: "Rate limited",
    err_cat_server: "Server error",
    err_cat_network: "Network error",
    err_cat_no_data: "No data",
    err_code_msg: "Error {code}: {msg}",
    err_code_only: "Error {code}",
    err_auth_hint: "The API key is invalid or expired. Check it and retry",
    err_empty_hint: "No quota data. Use an API key from an active Coding Plan; team keys need Org & Project IDs.",
    err_update: "Update check failed",

    // ── Tray tooltip ──
    tooltip_5h: "5h",
    tooltip_weekly: "Week",
    tooltip_mcp: "MCP",

    // ── Tray menu ──
    settings: "Settings",
    about: "About",
    exit: "Exit",

    // ── Settings · Current account ──
    accounts_section: "Current account",
    platform_section: "Account details",
    add_account: "Add account",
    account_name: "Name",
    default_account_name: "Default",
    account_platform: "Platform",
    platform_cn: "China",
    platform_intl: "Global",
    account_type_label: "Type",
    type_personal: "Personal",
    type_team: "Team",
    team_badge: "Team",
    org_id_label: "Organization ID",
    project_id_label: "Project ID",
    api_key_label: "API Key",
    switch_account: "Switch account",

    // ── Settings · Poll interval ──
    poll_interval: "Poll interval",
    interval_1m: "1 min",
    interval_5m: "5 min",
    interval_15m: "15 min",
    interval_30m: "30 min",
    interval_custom: "Custom",
    interval_custom_unit: "min",

    // ── Settings · General ──
    settings_general: "General",
    language: "Language",
    lang_name_zh: "中文",
    lang_name_en: "English",
    follow_system: "Follow system",
    appearance_section: "Appearance",
    theme_light: "Light",
    theme_dark: "Dark",
    autostart: "Start at login",

    // ── Settings · Network proxy ──
    network_section: "Network proxy",
    proxy_label: "Proxy address",
    proxy_hint: "Empty = direct; http:// or socks5://",

    // ── Settings · Usage notifications ──
    notifications: "Notifications",
    notify_threshold: "Usage alert",
    notify_threshold_desc: "Alerts once above {p}%, then re-arms",
    notify_reset_5h_opt: "5-hour session reset alerts",
    notify_reset_5h_desc: "Alerts when the 5-hour window resets",
    notify_reset_weekly_opt: "Weekly session reset alerts",
    notify_reset_weekly_desc: "Alerts when the weekly window resets",

    // ── Settings · Peak hours ──
    peak_section: "Peak hours",
    peak_start_label: "Start",
    peak_end_label: "End",

    // ── Settings · Config management and About ──
    backup_section: "Config management",
    export_config: "Export",
    export_done: "Exported: contains API keys — keep it safe",
    export_failed: "Export failed: cannot write to the chosen file",
    export_confirm_title: "Export Config",
    export_confirm_body: "The exported file will contain your API key in plain text. Keep it safe.",
    import_config: "Import",
    import_done: "Configuration imported",
    import_done_proxy_skipped: "Imported; invalid proxy kept the previous one",
    import_failed: "Import failed: unreadable or invalid file",
    import_failed_too_many: "Import failed: too many accounts",
    import_confirm_title: "Import confirmation",
    import_confirm_body: "Importing replaces all current accounts and settings; a different proxy also takes effect. Continue?",
    check_update: "Check for updates",
    up_to_date: "Up to date",
    go_download: "Download",
    version_label: "Version: {v}",
    version_new: "Version: {cur} → {new}",

    // ── About window ──
    app_desc: "A Windows tray widget for GLM Coding Plan usage",
    link_repo: "Repository",
    link_issues: "Feedback",
    link_diag: "Copy diagnostics",
    diag_recent_logs: "Recent logs",
    whats_new_section: "What's New",
    egg_quote: "No man ever steps in the same river twice.",

    // ── System notification titles ──
    notify_threshold_title: "Quota alert",
    notify_reset_5h: "5-hour window has reset",
    notify_reset_weekly: "Weekly quota has reset",
    notify_config_broken: "config.toml failed to parse; defaults restored and the original saved as config.toml.bak. Accounts and settings need to be re-entered",
    notify_diag_copied: "Diagnostics copied to clipboard",
    notify_copy_failed: "Copy failed: the clipboard is busy, try again shortly",
    notify_interval_invalid: "Invalid custom interval: {v}",
    notify_proxy_invalid: "Invalid proxy address; not saved",
    notify_team_required: "Team accounts require Organization ID and Project ID",
    notify_account_limit: "Account limit reached (100)",
};

impl Lang {
    pub fn strings(self) -> &'static Strings {
        match self {
            Lang::Zh => &ZH,
            Lang::En => &EN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extracts placeholder names in `{xxx}` form from the text, sorted and deduplicated
    fn placeholder_set(s: &str) -> Vec<&str> {
        let mut names = Vec::new();
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'{'
                && let Some(end) = s[i + 1..].find('}')
            {
                names.push(&s[i + 1..i + 1 + end]);
                i += end + 2;
                continue;
            }
            i += 1;
        }
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Compares ZH/EN wholesale after serialization: there is no hand-copied field list
    /// to miss anything, and every field's placeholder set
    /// is brought under fully automatic checking [equal key sets are guaranteed
    /// structurally by the identical shape; this is a backstop against drift]
    #[test]
    fn zh_en_parity() {
        let zh = serde_json::to_value(&ZH).unwrap();
        let en = serde_json::to_value(&EN).unwrap();
        let zh = zh.as_object().expect("Strings 序列化为对象");
        let en = en.as_object().expect("Strings 序列化为对象");

        let zh_only: Vec<_> = zh.keys().filter(|k| !en.contains_key(*k)).collect();
        let en_only: Vec<_> = en.keys().filter(|k| !zh.contains_key(*k)).collect();
        assert!(
            zh_only.is_empty() && en_only.is_empty(),
            "ZH/EN 键集合不一致，仅 ZH: {zh_only:?}，仅 EN: {en_only:?}"
        );

        for (key, value) in zh {
            let text = value.as_str().expect("文案字段皆为字符串");
            let other = en
                .get(key)
                .and_then(serde_json::Value::as_str)
                .expect("文案字段皆为字符串");
            assert_eq!(
                placeholder_set(text),
                placeholder_set(other),
                "占位符集合不一致: {key}"
            );
        }
    }

    /// Pins the placeholder-bearing strings against their placeholder-name lists:
    /// renaming a key inside a string (e.g. {t} → {time})
    /// trips here; when changing this, audit the render-side replace call keys in sync
    #[test]
    fn placeholder_names_pinned() {
        let cases: &[(&str, &str, &[&str])] = &[
            ("resets_line", ZH.resets_line, &["t"]),
            ("used_of", ZH.used_of, &["cur", "tot"]),
            ("peak_tip", ZH.peak_tip, &["r"]),
            ("updated_ago", ZH.updated_ago, &["t"]),
            ("data_as_of", ZH.data_as_of, &["t"]),
            ("err_code_msg", ZH.err_code_msg, &["code", "msg"]),
            ("err_code_only", ZH.err_code_only, &["code"]),
            ("notify_threshold_desc", ZH.notify_threshold_desc, &["p"]),
            ("version_label", ZH.version_label, &["v"]),
            ("version_new", ZH.version_new, &["cur", "new"]),
            (
                "notify_interval_invalid",
                ZH.notify_interval_invalid,
                &["v"],
            ),
        ];
        for (name, text, expect) in cases {
            assert_eq!(&placeholder_set(text), expect, "占位符漂移: {name}");
        }
    }
}
