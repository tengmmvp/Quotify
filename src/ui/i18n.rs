//! 界面文案

/// 生效语言
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

/// 系统语言检测
pub fn detect_system_lang() -> Lang {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;
    let lang_id = unsafe { GetUserDefaultUILanguage() };
    // 0x0804 = 简体中文（中国），0x0404 = 繁体中文（中国台湾），0x0C04 = 繁体中文（中国香港）
    matches!(lang_id, 0x0804 | 0x0404 | 0x0C04)
        .then_some(Lang::Zh)
        .unwrap_or(Lang::En)
}

/// 解析配置里的语言设置
pub fn resolve_lang(setting: Option<&str>) -> Lang {
    match setting.map(str::trim) {
        Some(s) if s.eq_ignore_ascii_case("zh") => Lang::Zh,
        Some(s) if s.eq_ignore_ascii_case("en") => Lang::En,
        _ => detect_system_lang(),
    }
}

/// 全部界面文案
///
/// 字段按「通用前置、视图专属按 UI 顺序」分组，同族成对相邻；
/// ZH / EN / check! 与本定义同序，增改字段四处同步。
pub struct Strings {
    // ── 通用按钮 ──
    pub cancel: &'static str,
    pub save: &'static str,
    pub apply: &'static str,

    // ── 时间单位 ──
    pub unit_day: &'static str,
    pub unit_hour: &'static str,
    pub unit_minute: &'static str,
    pub unit_second: &'static str,

    // ── 主视图 · 指标 ──
    pub usage_section: &'static str,
    pub five_hour: &'static str,
    pub weekly: &'static str,
    pub mcp_tools: &'static str,
    pub resets_line: &'static str,
    pub used_of: &'static str,
    pub balance_label: &'static str,

    // ── 主视图 · 峰谷 ──
    pub peak_badge: &'static str,
    pub peak_tip: &'static str,

    // ── 主视图 · 状态 ──
    pub updated_just_now: &'static str,
    pub updated_ago: &'static str,
    pub data_as_of: &'static str,
    pub loading: &'static str,
    pub fetch_failed: &'static str,
    pub retry: &'static str,
    pub not_configured_title: &'static str,
    pub not_configured_hint: &'static str,
    pub key_invalid: &'static str,

    // ── 错误前缀 ──
    /// 凭据失效（`FetchError::Auth`）：错误卡主文案
    pub err_auth: &'static str,
    /// 空 limits（`FetchError::EmptyLimits`）：key 无套餐权限 / 团队版缺选择头
    pub err_empty: &'static str,
    /// 业务错误前缀（`FetchError::Api`）：与 detail 拼成「前缀: 细节」
    pub err_api: &'static str,
    /// 网络错误前缀（`FetchError::Network`）：拼接方式同上
    pub err_network: &'static str,
    /// 检查更新失败，对应 `service::update` 的 Err
    pub err_update: &'static str,

    // ── 托盘菜单 ──
    pub settings: &'static str,
    pub exit: &'static str,

    // ── 设置 · 当前账号 ──
    pub accounts_section: &'static str,
    pub platform_section: &'static str,
    pub add_account: &'static str,
    pub account_name: &'static str,
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

    // ── 设置 · 轮询间隔 ──
    pub poll_interval: &'static str,
    pub interval_1m: &'static str,
    pub interval_5m: &'static str,
    pub interval_15m: &'static str,
    pub interval_30m: &'static str,
    pub interval_custom: &'static str,
    pub interval_custom_unit: &'static str,

    // ── 设置 · 通用 ──
    pub settings_general: &'static str,
    pub language: &'static str,
    pub follow_system: &'static str,
    pub appearance_section: &'static str,
    pub theme_light: &'static str,
    pub theme_dark: &'static str,
    pub autostart: &'static str,

    // ── 设置 · 网络代理 ──
    pub network_section: &'static str,
    pub proxy_label: &'static str,
    pub proxy_hint: &'static str,

    // ── 设置 · 用量通知 ──
    pub notifications: &'static str,
    pub notify_threshold: &'static str,
    pub notify_threshold_desc: &'static str,
    pub notify_reset_5h_opt: &'static str,
    pub notify_reset_5h_desc: &'static str,
    pub notify_reset_weekly_opt: &'static str,
    pub notify_reset_weekly_desc: &'static str,

    // ── 设置 · 高峰区间 ──
    pub peak_section: &'static str,
    pub peak_start_label: &'static str,
    pub peak_end_label: &'static str,

    // ── 设置 · 配置管理与关于 ──
    pub backup_section: &'static str,
    pub export_config: &'static str,
    pub export_done: &'static str,
    pub export_failed: &'static str,
    pub import_config: &'static str,
    pub import_done: &'static str,
    pub import_failed: &'static str,
    pub import_confirm_title: &'static str,
    pub import_confirm_body: &'static str,
    pub check_update: &'static str,
    pub up_to_date: &'static str,
    pub go_download: &'static str,
    pub version_label: &'static str,
    pub version_new: &'static str,

    // ── 系统通知标题 ──
    pub notify_threshold_title: &'static str,
    pub notify_reset_5h: &'static str,
    pub notify_reset_weekly: &'static str,
}

const ZH: Strings = Strings {
    // ── 通用按钮 ──
    cancel: "取消",
    save: "保存",
    apply: "确定",

    // ── 时间单位 ──
    unit_day: "天",
    unit_hour: "小时",
    unit_minute: "分",
    unit_second: "秒",

    // ── 主视图 · 指标 ──
    usage_section: "额度用量",
    five_hour: "5 小时会话窗口",
    weekly: "周会话窗口",
    mcp_tools: "MCP 工具",
    resets_line: "{t}后重置",
    used_of: "已用 {cur} / {tot}",
    balance_label: "账户余额",

    // ── 主视图 · 峰谷 ──
    peak_badge: "高峰",
    peak_tip: "高峰时段：工作日 {r}（UTC+8），模型调用额度消耗更快",

    // ── 主视图 · 状态 ──
    updated_just_now: "刚刚更新",
    updated_ago: "数据更新于 {t}前",
    data_as_of: "数据截至 {t}",
    loading: "加载中…",
    fetch_failed: "获取失败",
    retry: "重试",
    not_configured_title: "未配置账号",
    not_configured_hint: "进入设置，添加 GLM Coding Plan 账号即可开始使用",
    key_invalid: "[提示] API key 无效，请在设置中检查",

    // ── 错误前缀 ──
    err_auth: "API key 无效或已失效",
    err_empty: "未返回额度数据：请确认 API key 属于编码套餐（团队版需填写组织/项目 ID）",
    err_api: "接口错误",
    err_network: "网络错误",
    err_update: "检查更新失败",

    // ── 托盘菜单 ──
    settings: "设置",
    exit: "退出",

    // ── 设置 · 当前账号 ──
    accounts_section: "当前账号",
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
    switch_account: "切换账号",

    // ── 设置 · 轮询间隔 ──
    poll_interval: "轮询间隔",
    interval_1m: "1 分钟",
    interval_5m: "5 分钟",
    interval_15m: "15 分钟",
    interval_30m: "30 分钟",
    interval_custom: "自定义",
    interval_custom_unit: "分钟",

    // ── 设置 · 通用 ──
    settings_general: "通用设置",
    language: "语言",
    follow_system: "跟随系统",
    appearance_section: "外观",
    theme_light: "浅色",
    theme_dark: "深色",
    autostart: "开机自启",

    // ── 设置 · 网络代理 ──
    network_section: "网络代理",
    proxy_label: "代理地址",
    proxy_hint: "留空直连，支持 http:// 或 socks5://",

    // ── 设置 · 用量通知 ──
    notifications: "用量通知",
    notify_threshold: "用量预警",
    notify_threshold_desc: "超过 {p}% 时提醒一次，额度恢复后重新计数",
    notify_reset_5h_opt: "5 小时会话窗口重置提醒",
    notify_reset_5h_desc: "进入新 5 小时会话窗口时发送系统通知",
    notify_reset_weekly_opt: "周会话窗口重置提醒",
    notify_reset_weekly_desc: "进入新的周会话窗口时发送系统通知",

    // ── 设置 · 高峰区间 ──
    peak_section: "高峰区间",
    peak_start_label: "开始",
    peak_end_label: "结束",

    // ── 设置 · 配置管理与关于 ──
    backup_section: "配置管理",
    export_config: "导出配置",
    export_done: "配置已导出：含 API key，请妥善保管",
    export_failed: "导出失败：无法写入所选文件",
    import_config: "导入配置",
    import_done: "配置已导入",
    import_failed: "导入失败：文件不可读或格式无效",
    import_confirm_title: "导入确认",
    import_confirm_body: "所选文件不含任何账号，导入将清空当前全部账号与设置。确定继续？",
    check_update: "检查更新",
    up_to_date: "已是最新版本",
    go_download: "前往下载",
    version_label: "当前版本：{v}",
    version_new: "当前版本：{cur} → {new}",

    // ── 系统通知标题 ──
    notify_threshold_title: "额度预警",
    notify_reset_5h: "5 小时窗口已重置",
    notify_reset_weekly: "周额度已重置",
};

const EN: Strings = Strings {
    // ── 通用按钮 ──
    cancel: "Cancel",
    save: "Save",
    apply: "Apply",

    // ── 时间单位 ──
    unit_day: "d",
    unit_hour: "h",
    unit_minute: "m",
    unit_second: "s",

    // ── 主视图 · 指标 ──
    usage_section: "USAGE",
    five_hour: "Session (5h)",
    weekly: "Session (weekly)",
    mcp_tools: "MCP tools",
    resets_line: "Resets in {t}",
    used_of: "{cur} of {tot} used",
    balance_label: "BALANCE",

    // ── 主视图 · 峰谷 ──
    peak_badge: "PEAK",
    peak_tip: "Peak hours: weekdays {r} (UTC+8); model calls drain quota faster",

    // ── 主视图 · 状态 ──
    updated_just_now: "Updated just now",
    updated_ago: "Updated {t} ago",
    data_as_of: "Data as of {t}",
    loading: "Loading…",
    fetch_failed: "Fetch failed",
    retry: "Retry",
    not_configured_title: "No account configured",
    not_configured_hint: "Open Settings and add a GLM Coding Plan account to get started",
    key_invalid: "[Note] Invalid API key. Check it in Settings",

    // ── 错误前缀 ──
    err_auth: "Invalid or expired API key",
    err_empty: "No quota data: make sure the API key belongs to a Coding Plan (team accounts need Organization/Project IDs)",
    err_api: "API error",
    err_network: "Network error",
    err_update: "Update check failed",

    // ── 托盘菜单 ──
    settings: "Settings",
    exit: "Exit",

    // ── 设置 · 当前账号 ──
    accounts_section: "Current account",
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
    switch_account: "Switch account",

    // ── 设置 · 轮询间隔 ──
    poll_interval: "Poll interval",
    interval_1m: "1 min",
    interval_5m: "5 min",
    interval_15m: "15 min",
    interval_30m: "30 min",
    interval_custom: "Custom",
    interval_custom_unit: "min",

    // ── 设置 · 通用 ──
    settings_general: "General",
    language: "Language",
    follow_system: "Follow system",
    appearance_section: "Appearance",
    theme_light: "Light",
    theme_dark: "Dark",
    autostart: "Start at login",

    // ── 设置 · 网络代理 ──
    network_section: "Network proxy",
    proxy_label: "Proxy address",
    proxy_hint: "Empty = direct; http:// or socks5://",

    // ── 设置 · 用量通知 ──
    notifications: "Notifications",
    notify_threshold: "Usage alert",
    notify_threshold_desc: "Alerts once above {p}%",
    notify_reset_5h_opt: "5-hour session reset alerts",
    notify_reset_5h_desc: "Alerts when the 5-hour window resets",
    notify_reset_weekly_opt: "Weekly session reset alerts",
    notify_reset_weekly_desc: "Alerts when the weekly window resets",

    // ── 设置 · 高峰区间 ──
    peak_section: "Peak hours",
    peak_start_label: "Start",
    peak_end_label: "End",

    // ── 设置 · 配置管理与关于 ──
    backup_section: "Config management",
    export_config: "Export",
    export_done: "Exported: contains API keys — keep it safe",
    export_failed: "Export failed: cannot write to the chosen file",
    import_config: "Import",
    import_done: "Configuration imported",
    import_failed: "Import failed: unreadable or invalid file",
    import_confirm_title: "Import confirmation",
    import_confirm_body: "The selected file contains no accounts; importing will wipe all current accounts and settings. Continue?",
    check_update: "Check for updates",
    up_to_date: "Up to date",
    go_download: "Download",
    version_label: "Version: {v}",
    version_new: "Version: {cur} → {new}",

    // ── 系统通知标题 ──
    notify_threshold_title: "Quota alert",
    notify_reset_5h: "5-hour window has reset",
    notify_reset_weekly: "Weekly quota has reset",
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

    #[test]
    fn zh_en_placeholders_match() {
        let (zh, en) = (&ZH, &EN);
        macro_rules! check {
            ($($field:ident),* $(,)?) => {
                $(
                    assert_eq!(
                        placeholder_set(zh.$field),
                        placeholder_set(en.$field),
                        concat!("占位符集合不一致: ", stringify!($field)),
                    );
                )*
            };
        }
        check!(
            // ── 通用按钮 ──
            cancel,
            save,
            apply,
            // ── 时间单位 ──
            unit_day,
            unit_hour,
            unit_minute,
            unit_second,
            // ── 主视图 · 指标 ──
            usage_section,
            five_hour,
            weekly,
            mcp_tools,
            resets_line,
            used_of,
            balance_label,
            // ── 主视图 · 峰谷 ──
            peak_badge,
            peak_tip,
            // ── 主视图 · 状态 ──
            updated_just_now,
            updated_ago,
            data_as_of,
            loading,
            fetch_failed,
            retry,
            not_configured_title,
            not_configured_hint,
            key_invalid,
            // ── 错误前缀 ──
            err_auth,
            err_empty,
            err_api,
            err_network,
            err_update,
            // ── 托盘菜单 ──
            settings,
            exit,
            // ── 设置 · 当前账号 ──
            accounts_section,
            platform_section,
            add_account,
            account_name,
            account_platform,
            platform_cn,
            platform_intl,
            account_type_label,
            type_personal,
            type_team,
            team_badge,
            org_id_label,
            project_id_label,
            api_key_label,
            switch_account,
            // ── 设置 · 轮询间隔 ──
            poll_interval,
            interval_1m,
            interval_5m,
            interval_15m,
            interval_30m,
            interval_custom,
            interval_custom_unit,
            // ── 设置 · 通用 ──
            settings_general,
            language,
            follow_system,
            appearance_section,
            theme_light,
            theme_dark,
            autostart,
            // ── 设置 · 网络代理 ──
            network_section,
            proxy_label,
            proxy_hint,
            // ── 设置 · 用量通知 ──
            notifications,
            notify_threshold,
            notify_threshold_desc,
            notify_reset_5h_opt,
            notify_reset_5h_desc,
            notify_reset_weekly_opt,
            notify_reset_weekly_desc,
            // ── 设置 · 高峰区间 ──
            peak_section,
            peak_start_label,
            peak_end_label,
            // ── 设置 · 配置管理与关于 ──
            backup_section,
            export_config,
            export_done,
            export_failed,
            import_config,
            import_done,
            import_failed,
            import_confirm_title,
            import_confirm_body,
            check_update,
            up_to_date,
            go_download,
            version_label,
            version_new,
            // ── 系统通知标题 ──
            notify_threshold_title,
            notify_reset_5h,
            notify_reset_weekly,
        );
    }
}
