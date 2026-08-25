//! 界面文案：中英双语，默认跟随系统语言，可在设置页手动切换。

/// 生效语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

/// 系统语言检测（Win32 `GetUserDefaultUILanguage`）。
pub fn detect_system_lang() -> Lang {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;
    let lang_id = unsafe { GetUserDefaultUILanguage() };
    // 0x0804 = 简体中文（中国），0x0404 = 繁体中文（台湾），0x0C04 = 香港
    matches!(lang_id, 0x0804 | 0x0404 | 0x0C04).then_some(Lang::Zh).unwrap_or(Lang::En)
}

/// 解析配置里的语言设置；空/None 跟随系统。
pub fn resolve_lang(setting: Option<&str>) -> Lang {
    match setting.map(str::trim) {
        Some(s) if s.eq_ignore_ascii_case("zh") => Lang::Zh,
        Some(s) if s.eq_ignore_ascii_case("en") => Lang::En,
        _ => detect_system_lang(),
    }
}

/// 全部界面文案。字段即文案 key，双语各一份常量表；
/// 为保证两语言表结构一致，暂未被渲染消费的字段一并保留。
#[allow(dead_code)]
pub struct Strings {
    // ── 主视图 ──
    pub five_hour: &'static str,
    pub weekly: &'static str,
    pub mcp_tools: &'static str,
    pub resets_in: &'static str,
    /// 指标行脚注：`{t}` 倒计时、`{clock}` 重置钟点（跨天带日期）
    pub resets_line: &'static str,
    /// 绝对值脚注：`{cur}` 当前用量、`{tot}` 总量
    pub used_of: &'static str,
    pub usage_section: &'static str,
    pub balance_label: &'static str,
    pub updated_just_now: &'static str,
    /// 底部更新时间：`{t}` 时长（如「2 分钟」/「2m」）
    pub updated_ago: &'static str,
    pub data_as_of: &'static str,
    pub fetch_failed: &'static str,
    pub retry: &'static str,
    pub refresh: &'static str,
    pub settings: &'static str,
    pub exit: &'static str,
    pub not_configured_title: &'static str,
    pub not_configured_hint: &'static str,
    pub no_data: &'static str,
    pub loading: &'static str,
    pub key_invalid: &'static str,

    // ── 通用 ──
    pub back: &'static str,
    pub cancel: &'static str,
    pub confirm: &'static str,
    pub delete: &'static str,
    pub save: &'static str,

    // ── 设置视图 ──
    pub settings_general: &'static str,
    pub poll_interval: &'static str,
    pub interval_1m: &'static str,
    pub interval_5m: &'static str,
    pub interval_15m: &'static str,
    pub interval_30m: &'static str,
    pub interval_custom: &'static str,
    pub interval_custom_unit: &'static str,
    pub apply: &'static str,
    pub language: &'static str,
    pub follow_system: &'static str,
    /// 外观（主题模式）分段：跟随系统 / 浅色 / 深色
    pub appearance_section: &'static str,
    pub theme_light: &'static str,
    pub theme_dark: &'static str,
    pub notifications: &'static str,
    pub notify_threshold: &'static str,
    pub notify_threshold_desc: &'static str,
    pub notify_reset_5h_opt: &'static str,
    pub notify_reset_5h_desc: &'static str,
    pub notify_reset_weekly_opt: &'static str,
    pub notify_reset_weekly_desc: &'static str,
    pub autostart: &'static str,
    pub accounts_section: &'static str,
    /// 添加账号页的区块标题（平台 + 个人版/团队版 + 凭据）
    pub platform_section: &'static str,
    pub add_account: &'static str,
    pub account_name: &'static str,
    pub account_platform: &'static str,
    pub platform_cn: &'static str,
    pub platform_intl: &'static str,
    /// 账号类型分段标签
    pub account_type_label: &'static str,
    pub type_personal: &'static str,
    pub type_team: &'static str,
    /// 团队版账号卡上的短名牌
    pub team_badge: &'static str,
    /// 团队版选择头输入（组织 / 项目 ID）
    pub org_id_label: &'static str,
    pub project_id_label: &'static str,
    pub api_key_label: &'static str,
    pub check_update: &'static str,
    pub checking_update: &'static str,
    pub up_to_date: &'static str,
    pub update_available: &'static str,
    pub get_update: &'static str,
    pub update_check_failed: &'static str,
    pub version_label: &'static str,

    // ── 通知 ──
    pub notify_threshold_title: &'static str,
    pub notify_reset_5h: &'static str,
    pub notify_reset_weekly: &'static str,

    // ── 倒计时单位 ──
    pub unit_day: &'static str,
    pub unit_hour: &'static str,
    pub unit_minute: &'static str,
    pub unit_second: &'static str,
}

const ZH: Strings = Strings {
    five_hour: "5 小时窗口",
    weekly: "周额度",
    mcp_tools: "MCP 工具",
    resets_in: "后重置",
    resets_line: "{t}后重置",
    used_of: "已用 {cur} / {tot}",
    usage_section: "额度用量",
    balance_label: "账户余额",
    updated_just_now: "刚刚更新",
    updated_ago: "数据更新于 {t}前",
    data_as_of: "数据截至 {t}",
    fetch_failed: "获取失败",
    retry: "重试",
    refresh: "刷新",
    settings: "设置",
    exit: "退出",
    not_configured_title: "未配置账号",
    not_configured_hint: "进入设置，添加 GLM Coding Plan 账号即可开始使用",
    no_data: "暂无数据",
    loading: "加载中…",
    key_invalid: "[提示] API key 无效，请在设置中检查",

    back: "返回",
    cancel: "取消",
    confirm: "确定",
    delete: "删除",
    save: "保存",

    settings_general: "通用设置",
    poll_interval: "轮询间隔",
    interval_1m: "1 分钟",
    interval_5m: "5 分钟",
    interval_15m: "15 分钟",
    interval_30m: "30 分钟",
    interval_custom: "自定义",
    interval_custom_unit: "分钟",
    apply: "确定",
    language: "语言",
    follow_system: "跟随系统",
    appearance_section: "外观",
    theme_light: "浅色",
    theme_dark: "深色",
    notifications: "用量通知",
    notify_threshold: "用量预警",
    notify_threshold_desc: "超过 80% 时提醒一次，额度恢复后重新计数",
    notify_reset_5h_opt: "5 小时额度重置提醒",
    notify_reset_5h_desc: "进入新 5 小时窗口时通知",
    notify_reset_weekly_opt: "周额度重置提醒",
    notify_reset_weekly_desc: "进入新的周周期时通知",
    autostart: "开机自启",
    accounts_section: "账号管理",
    platform_section: "账号信息",
    add_account: "添加账号",
    account_name: "名称",
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
    check_update: "检查更新",
    checking_update: "正在检查…",
    up_to_date: "已是最新版本",
    update_available: "发现新版本 {v}",
    get_update: "前往下载",
    update_check_failed: "检查更新失败",
    version_label: "当前版本：{v}",

    notify_threshold_title: "额度预警",
    notify_reset_5h: "5 小时窗口已重置",
    notify_reset_weekly: "周额度已重置",

    unit_day: "天",
    unit_hour: "小时",
    unit_minute: "分",
    unit_second: "秒",
};

const EN: Strings = Strings {
    // 英文措辞对齐 ai-usagebar（Session (5h) / Weekly / MCP tools）
    five_hour: "Session (5h)",
    weekly: "Weekly",
    mcp_tools: "MCP tools",
    resets_in: "until reset",
    resets_line: "Resets in {t}",
    used_of: "{cur} of {tot} used",
    usage_section: "USAGE",
    balance_label: "BALANCE",
    updated_just_now: "Updated just now",
    updated_ago: "Updated {t} ago",
    data_as_of: "Data as of {t}",
    fetch_failed: "Fetch failed",
    retry: "Retry",
    refresh: "Refresh",
    settings: "Settings",
    exit: "Exit",
    not_configured_title: "No account configured",
    not_configured_hint: "Open Settings and add a Z.AI Coding Plan account to get started",
    no_data: "No data yet",
    loading: "Loading…",
    key_invalid: "[Note] Invalid API key. Check it in Settings",

    back: "Back",
    cancel: "Cancel",
    confirm: "OK",
    delete: "Delete",
    save: "Save",

    settings_general: "General",
    poll_interval: "Poll interval",
    interval_1m: "1 min",
    interval_5m: "5 min",
    interval_15m: "15 min",
    interval_30m: "30 min",
    interval_custom: "Custom",
    interval_custom_unit: "min",
    apply: "Apply",
    language: "Language",
    follow_system: "Follow system",
    appearance_section: "Appearance",
    theme_light: "Light",
    theme_dark: "Dark",
    notifications: "Notifications",
    notify_threshold: "Usage alert",
    notify_threshold_desc: "Alerts once above the threshold",
    notify_reset_5h_opt: "5-hour reset alerts",
    notify_reset_5h_desc: "When the 5-hour window resets",
    notify_reset_weekly_opt: "Weekly reset alerts",
    notify_reset_weekly_desc: "When the weekly quota resets",
    autostart: "Start at login",
    accounts_section: "Accounts",
    platform_section: "Account details",
    add_account: "Add account",
    account_name: "Name",
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
    check_update: "Check for updates",
    checking_update: "Checking…",
    up_to_date: "Up to date",
    update_available: "New version {v} available",
    get_update: "Download",
    update_check_failed: "Update check failed",
    version_label: "Version: {v}",

    notify_threshold_title: "Quota alert",
    notify_reset_5h: "5-hour window has reset",
    notify_reset_weekly: "Weekly quota has reset",

    unit_day: "d",
    unit_hour: "h",
    unit_minute: "m",
    unit_second: "s",
};

impl Lang {
    pub fn strings(self) -> &'static Strings {
        match self {
            Lang::Zh => &ZH,
            Lang::En => &EN,
        }
    }
}
