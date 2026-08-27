//! 渲染视图模型

use crate::api::Platform;
use crate::ui::i18n::{Lang, Strings};

/// 选中账号的展示摘要
#[derive(Debug, Clone, Copy)]
pub struct AccountView<'a> {
    pub index: usize,
    pub name: &'a str,
    pub platform: Platform,
    pub team: bool,
}

/// 一帧渲染所需的全部 app 层数据，引用快照零拷贝
pub struct PanelModel<'a> {
    pub strings: &'static Strings,
    pub lang: Lang,
    pub snapshot: Option<&'a crate::api::UsageSnapshot>,
    pub error: Option<&'a crate::api::FetchError>,
    pub account: Option<AccountView<'a>>,
    pub accounts_count: usize,
    pub accounts: &'a [crate::app::config::Account],
    pub poll_interval_secs: u64,
    pub language: Option<&'a str>,
    pub appearance: Option<&'a str>,
    pub autostart: bool,
    pub threshold_enabled: bool,
    pub threshold_percent: u8,
    pub reset_5h_enabled: bool,
    pub reset_weekly_enabled: bool,
    pub update_available: bool,
    pub peak_range: crate::ui::peak::PeakRange,
    pub peak_start_raw: &'a str,
    pub peak_end_raw: &'a str,
    pub update: Option<&'a Result<crate::service::update::ReleaseInfo, String>>,
    pub news: Option<&'a [crate::service::whatsnew::NewsItem]>,
    pub last_news_read: Option<&'a str>,
}

impl<'a> PanelModel<'a> {
    pub fn from_app(app: &'a crate::app::App) -> Self {
        let g = &app.config.general;
        let account = app
            .config
            .accounts
            .iter()
            .enumerate()
            .find(|(_, a)| Some(a.id.as_str()) == app.config.selected.as_deref())
            .or_else(|| app.config.accounts.first().map(|a| (0, a)))
            .map(|(index, a)| AccountView {
                index,
                name: &a.name,
                platform: a.platform,
                team: a.team,
            });
        Self {
            strings: app.strings,
            lang: app.lang,
            snapshot: app.data.snapshot.as_ref(),
            error: app.data.last_error.as_ref(),
            account,
            accounts_count: app.config.accounts.len(),
            accounts: &app.config.accounts,
            poll_interval_secs: g.poll_interval_secs,
            language: g.language.as_deref(),
            appearance: g.appearance.as_deref(),
            autostart: app.autostart_enabled,
            threshold_enabled: g.notify_threshold_enabled,
            threshold_percent: g.notify_threshold_percent,
            reset_5h_enabled: g.notify_reset_5h_enabled,
            reset_weekly_enabled: g.notify_reset_weekly_enabled,
            update_available: app.panel.update_available,
            peak_range: crate::app::peak_range_of(&app.config),
            peak_start_raw: &app.config.general.peak_start,
            peak_end_raw: &app.config.general.peak_end,
            update: app.update_status.as_ref(),
            news: app.news.as_deref(),
            last_news_read: app.config.general.last_news_read.as_deref(),
        }
    }
}
