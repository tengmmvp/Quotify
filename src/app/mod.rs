//! 应用层：装配各模块、维护全局状态、驱动消息循环。

pub mod config;
mod notify;

use chrono::{DateTime, Utc};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, HMENU,
    MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassW, SM_CXSMICON,
    SetForegroundWindow, SetWindowLongPtrW, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON,
    TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WM_COMMAND, WM_DESTROY, WM_NULL, WNDCLASSW,
    WS_POPUP,
};
use windows::core::PCWSTR;

use crate::api::{FetchError, UsageSnapshot};
use crate::app::config::Config;
use crate::platform::instance::TRAY_WND_CLASS;
use crate::platform::msg::{
    WM_APP_NEWS_RESULT, WM_APP_POLL_RESULT, WM_APP_TRAY, WM_APP_UPDATE_RESULT, WM_APP_WAKE_INSTANCE,
};
use crate::platform::wide;
use crate::service::poller::{
    DEFAULT_INTERVAL_SECS, MAX_POLL_SECS, MIN_POLL_SECS, PollInterval, PollOutcome, PollTarget,
    Poller,
};
use crate::ui::i18n::{Lang, Strings};
use crate::ui::icon;
use crate::ui::panel::Panel;
use crate::ui::panel::layout::INTERVAL_PRESETS;
use crate::ui::tray::{self, TrayIcon};
use notify::{NOTIFY_TITLE, check_reset, check_threshold};

/// 菜单命令 ID，定义序与菜单项顺序一致
const IDM_SETTINGS: u16 = 1001;
const IDM_ABOUT: u16 = 1002;
const IDM_EXIT: u16 = 1003;

/// 导入配置文件大小上限
const IMPORT_MAX_BYTES: u64 = 1024 * 1024;

/// v4 回调的键盘激活通知码
const NIN_KEYSELECT: u32 = 0x0401;

/// 失败时保留旧快照供面板显示。
pub struct AccountData {
    pub(crate) snapshot: Option<UsageSnapshot>,
    pub(crate) last_error: Option<FetchError>,
}

/// 装配层状态机
pub struct App {
    pub(crate) config: Config,
    pub(crate) lang: Lang,
    pub(crate) strings: &'static Strings,
    pub(crate) data: AccountData,
    tray: Option<TrayIcon>,
    poller: Option<Poller>,
    poll_target: PollTarget,
    poll_interval: PollInterval,
    tray_icon: Option<windows::Win32::UI::WindowsAndMessaging::HICON>,
    pub(crate) panel: Panel,
    pub(crate) popup: crate::ui::popup::AccountPopup,
    pub(crate) about: crate::ui::about::AboutWindow,
    hwnd: Option<HWND>,
    pub(crate) update_status: Option<Result<crate::service::update::ReleaseInfo, String>>,
    update_checking: bool,
    pub(crate) news: Option<Vec<crate::service::whatsnew::NewsItem>>,
    news_fetched: bool,
    pub(crate) autostart_enabled: bool,
    last_icon_key: Option<(i64, bool, bool)>,
    threshold_armed_5h: bool,
    last_reset_5h: Option<DateTime<Utc>>,
    threshold_armed_weekly: bool,
    last_reset_weekly: Option<DateTime<Utc>>,
    last_logged_error: Option<String>,
}

impl App {
    fn new(mut config: Config) -> Self {
        normalize_config(&mut config);
        let lang = crate::ui::i18n::resolve_lang(config.general.language.as_deref());
        Self {
            config,
            lang,
            strings: lang.strings(),
            data: AccountData {
                snapshot: None,
                last_error: None,
            },
            tray: None,
            poller: None,
            poll_target: std::sync::Arc::new(std::sync::Mutex::new(None)),
            poll_interval: std::sync::Arc::new(std::sync::Mutex::new(DEFAULT_INTERVAL_SECS)),
            tray_icon: None,
            panel: Panel::new(),
            popup: crate::ui::popup::AccountPopup::new(),
            about: crate::ui::about::AboutWindow::new(),
            hwnd: None,
            update_status: None,
            update_checking: false,
            news: None,
            news_fetched: false,
            autostart_enabled: crate::platform::autostart::is_enabled(),
            last_icon_key: None,
            threshold_armed_5h: true,
            last_reset_5h: None,
            threshold_armed_weekly: true,
            last_reset_weekly: None,
            last_logged_error: None,
        }
    }

    fn hwnd(&self) -> HWND {
        self.hwnd.unwrap_or_default()
    }

    fn sync_poll_context(&self) {
        let target = self
            .config
            .selected_account()
            .filter(|a| !a.api_key.trim().is_empty())
            .map(|a| crate::api::AccountSpec {
                platform: a.platform,
                org_id: a.org_id.clone(),
                project_id: a.project_id.clone(),
                api_key: a.api_key.clone(),
            });
        *borrow(&self.poll_target) = target;
        *borrow(&self.poll_interval) = self
            .config
            .general
            .poll_interval_secs
            .clamp(MIN_POLL_SECS, MAX_POLL_SECS);
    }

    fn update_tray_icon(&mut self) {
        let failed = self.data.last_error.is_some() && self.data.snapshot.is_none();
        let used = self
            .data
            .snapshot
            .as_ref()
            .and_then(|s| s.five_hour.as_ref())
            .map(|b| b.used_percent)
            .unwrap_or(0.0);
        let key = (used.round() as i64, self.data.snapshot.is_some(), failed);
        if self.last_icon_key == Some(key) {
            return;
        }
        // 两倍尺寸给 Shell 缩小显示；无数据态资源优先、手绘兜底
        let px = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) * 2;
        let new = match &self.data.snapshot {
            Some(_) => icon::ring_icon(px, used, failed),
            None if failed => icon::ring_icon(px, 0.0, true),
            None => icon::resource_icon(px).or_else(|| icon::logo_icon(px)),
        };
        if let Some(new) = new {
            // 先换新再销毁旧：成功时 shell 全程不引用已销毁句柄；NIM_MODIFY
            // 失败时旧句柄仍销毁、托盘空到下次更新，与旧顺序结局相同
            if let Some(tray) = &self.tray {
                tray.update_icon(new);
            }
            if let Some(old) = self.tray_icon.take() {
                icon::destroy_owned(old);
            }
            self.tray_icon = Some(new);
            self.last_icon_key = Some(key);
        }
    }

    fn handle_poll_result(&mut self, outcome: PollOutcome) {
        match outcome {
            PollOutcome::Success(snap) => {
                self.check_notifications(&snap);
                self.data.snapshot = Some(*snap);
                self.data.last_error = None;
                self.last_logged_error = None;
            }
            PollOutcome::Failure(e) => {
                // 文案与上次相同的失败只记一次，固定间隔重试不刷屏
                let msg = e.to_string();
                let repeated = self
                    .last_logged_error
                    .as_ref()
                    .is_some_and(|prev| *prev == msg);
                if !repeated {
                    crate::platform::log(&format!("[Quotify] 轮询失败: {msg}"));
                }
                self.last_logged_error = Some(msg);
                self.data.last_error = Some(*e);
            }
        }
        self.update_tray_icon();
        if let Some(p) = self.panel.hwnd {
            relayout_panel(self, p);
        }
        // UI 更新完毕即静止，归还工作集保持低内存
        crate::platform::trim_working_set();
    }

    fn check_notifications(&mut self, snap: &UsageSnapshot) {
        let g = &self.config.general;
        let notify = |body: &str| crate::platform::notify::show(NOTIFY_TITLE, body);

        check_reset(
            snap.five_hour.as_ref(),
            &mut self.last_reset_5h,
            &mut self.threshold_armed_5h,
            g.notify_reset_5h_enabled,
            self.strings.notify_reset_5h,
            &notify,
        );
        check_reset(
            snap.weekly.as_ref(),
            &mut self.last_reset_weekly,
            &mut self.threshold_armed_weekly,
            g.notify_reset_weekly_enabled,
            self.strings.notify_reset_weekly,
            &notify,
        );

        if g.notify_threshold_enabled {
            let th = g.notify_threshold_percent as f64;
            check_threshold(
                snap.five_hour.as_ref(),
                &mut self.threshold_armed_5h,
                th,
                self.strings.five_hour,
                self.strings.notify_threshold_title,
                &notify,
            );
            check_threshold(
                snap.weekly.as_ref(),
                &mut self.threshold_armed_weekly,
                th,
                self.strings.weekly,
                self.strings.notify_threshold_title,
                &notify,
            );
        }
    }

    /// 托盘右键菜单
    fn show_context_menu(&self, pos: windows::Win32::Foundation::POINT) {
        unsafe {
            let owner = self.hwnd();
            let settings = wide(self.strings.settings);
            let about = wide(self.strings.about);
            let exit = wide(self.strings.exit);
            let menu: HMENU = CreatePopupMenu().unwrap_or_default();
            if menu.is_invalid() {
                return;
            }
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                IDM_SETTINGS as usize,
                PCWSTR(settings.as_ptr()),
            );
            let _ = AppendMenuW(menu, MF_STRING, IDM_ABOUT as usize, PCWSTR(about.as_ptr()));
            let _ = AppendMenuW(menu, MF_STRING, IDM_EXIT as usize, PCWSTR(exit.as_ptr()));
            // 弹菜单前先激活前台，否则点菜单外不收起[Shell_NotifyIcon 文档要求]
            let _ = SetForegroundWindow(owner);
            let _ = TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON,
                pos.x,
                pos.y,
                None,
                owner,
                None,
            );
            let _ = DestroyMenu(menu);
            // 官方模板要求的收尾：让菜单模式正确归还激活，否则下次弹菜单可能
            // 立即失焦收不起来
            let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        }
    }
}

/// 毒化锁取回内部数据继续用，与 service::poller 同策略：锁只被短临界区持有，
/// 毒化意味着持锁线程已死，数据本身仍完好
fn borrow<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// lparam 指向的宽字符串是否为 "ImmersiveColorSet"
fn is_immersive_color_set(lparam: LPARAM) -> bool {
    let p = lparam.0 as *const u16;
    if p.is_null() {
        return false;
    }
    let mut expect = "ImmersiveColorSet".encode_utf16();
    unsafe {
        loop {
            match (expect.next(), *p) {
                (Some(a), b) if a == b => {}
                (None, 0) => return true,
                _ => return false,
            }
        }
    }
}

/// 托盘隐藏窗口过程
extern "system" fn tray_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_APP_TRAY => {
            let app = app_from(hwnd);
            let (code, _) = tray::parse_callback(lparam);
            match code {
                tray::NIN_POPUPOPEN => {
                    if let Some(app) = app
                        && let Some(rect) = tray_rect(app)
                    {
                        let n = app.config.accounts.len();
                        sync_main_height(app);
                        app.panel.show_preview(hwnd, rect, n);
                        apply_appearance(app);
                    }
                }
                tray::NIN_POPUPCLOSE => {
                    if let Some(app) = app {
                        app.panel.request_close();
                    }
                }
                NIN_KEYSELECT | windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP => {
                    if let Some(app) = app
                        && let Some(rect) = tray_rect(app)
                    {
                        let n = app.config.accounts.len();
                        sync_main_height(app);
                        app.panel.toggle_pin(hwnd, rect, n);
                        apply_appearance(app);
                    }
                }
                windows::Win32::UI::WindowsAndMessaging::WM_CONTEXTMENU => {
                    if let Some(app) = app {
                        app.show_context_menu(tray::context_menu_pos(wparam));
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_POLL_RESULT => {
            // 先取回 owned 再判 app：app 缺失时指针同样要释放，防泄漏
            let boxed = wparam.0 as *mut PollOutcome;
            let outcome = (!boxed.is_null()).then(|| unsafe { Box::from_raw(boxed) });
            if let (Some(app), Some(outcome)) = (app_from(hwnd), outcome) {
                app.handle_poll_result(*outcome);
            }
            LRESULT(0)
        }
        WM_APP_UPDATE_RESULT => {
            // 先取回 owned 再判 app：app 缺失时指针同样要释放，防泄漏
            let boxed = wparam.0 as *mut Result<crate::service::update::ReleaseInfo, String>;
            let result = (!boxed.is_null()).then(|| unsafe { Box::from_raw(boxed) });
            if let Some(app) = app_from(hwnd) {
                app.update_checking = false;
                if let Some(r) = result {
                    app.panel.update_available = match r.as_ref() {
                        Ok(info) => {
                            crate::service::update::is_newer(&info.tag, env!("CARGO_PKG_VERSION"))
                        }
                        Err(_) => false,
                    };
                    app.update_status = Some(*r);
                    if let Some(p) = app.panel.hwnd {
                        relayout_panel(app, p);
                        unsafe {
                            let _ = InvalidateRect(Some(p), None, true);
                        }
                    }
                }
            }
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE => {
            // 外观未显式指定时，系统主题切换即时重绘面板
            if is_immersive_color_set(lparam)
                && let Some(app) = app_from(hwnd)
                && !app.config.general.appearance.as_deref().is_some_and(|s| {
                    s.eq_ignore_ascii_case("light") || s.eq_ignore_ascii_case("dark")
                })
            {
                apply_appearance(app);
                // 面板与打开中的弹窗、关于窗一并按新主题重绘
                invalidate_ui(app);
            }
            LRESULT(0)
        }
        WM_APP_NEWS_RESULT => {
            // 先取回 owned 再判 app：app 缺失时指针同样要释放，防泄漏
            let boxed = wparam.0 as *mut Result<Vec<crate::service::whatsnew::NewsItem>, String>;
            let result = (!boxed.is_null()).then(|| unsafe { Box::from_raw(boxed) });
            if let Some(app) = app_from(hwnd)
                && let Some(Ok(news)) = result.map(|b| *b)
            {
                app.news = Some(news);
                // 慢网络下关于窗可能已按基础高度打开：动态到达后重排窗高
                refit_about(app);
            }
            LRESULT(0)
        }
        WM_APP_WAKE_INSTANCE => {
            if let Some(app) = app_from(hwnd)
                && let Some(rect) = tray_rect(app)
            {
                let n = app.config.accounts.len();
                sync_main_height(app);
                app.panel.toggle_pin(hwnd, rect, n);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd = (wparam.0 & 0xFFFF) as u16;
            match cmd {
                IDM_SETTINGS => {
                    if let Some(app) = app_from(hwnd)
                        && let Some(rect) = tray_rect(app)
                    {
                        let n = app.config.accounts.len();
                        app.panel.show_preview(hwnd, rect, n);
                        app.panel.view = crate::ui::panel::PanelView::Settings;
                        if let Some(p) = app.panel.hwnd {
                            sync_customizing(app);
                            relayout_panel(app, p);
                        }
                    }
                }
                IDM_ABOUT => {
                    if let Some(app) = app_from(hwnd) {
                        // 首次开关于窗才拉；不随启动拉，弱网下 raw 域名
                        // 慢解析会在进程内排队，拖住启动轮询、延后面板首屏
                        if app.news.is_none() && !app.news_fetched {
                            app.news_fetched = true;
                            struct SendHwnd(HWND);
                            unsafe impl Send for SendHwnd {}
                            let tray = SendHwnd(hwnd);
                            std::thread::spawn(move || {
                                let tray = tray;
                                let r = crate::service::whatsnew::fetch_latest();
                                let boxed = Box::into_raw(Box::new(r));
                                let posted = unsafe {
                                    PostMessageW(
                                        Some(tray.0),
                                        WM_APP_NEWS_RESULT,
                                        WPARAM(boxed as usize),
                                        Default::default(),
                                    )
                                };
                                if posted.is_err() {
                                    drop(unsafe { Box::from_raw(boxed) });
                                }
                            });
                        }
                        let h = crate::ui::panel::render::about::about_height(
                            app.news.as_deref(),
                            app.about.news_expanded,
                        );
                        app.about.open(hwnd, h);
                    }
                }
                IDM_EXIT => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn tray_rect(app: &App) -> Option<RECT> {
    app.tray.as_ref().and_then(|t| t.rect())
}

/// App 装箱后存活到进程退出，GWLP_USERDATA 指针全程有效，可转写为
/// `&'static mut`；别名安全依赖单线程消息循环。
fn app_from(hwnd: HWND) -> Option<&'static mut App> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    (p != 0).then(|| unsafe { &mut *(p as *mut App) })
}

/// 面板命中处理
pub fn handle_panel_hit(app: &mut App, hit: crate::ui::panel::render::Hit, panel_hwnd: HWND) {
    use crate::ui::panel::render::{AppearanceChoice, Hit, LanguageChoice, ScopeChoice};
    match hit {
        Hit::Refresh | Hit::Retry => {
            // 主动重试开启新一轮记录，同文案失败再现时也重记日志
            app.last_logged_error = None;
            if let Some(p) = &app.poller {
                p.refresh_now();
            }
            if let Some(r) = app.panel.renderer.as_mut() {
                r.start_spin();
            }
        }
        Hit::Settings => {
            app.panel.mode = crate::ui::panel::PanelMode::Pinned;
            app.panel.view = crate::ui::panel::PanelView::Settings;
            sync_customizing(app);
            relayout_panel(app, panel_hwnd);
        }
        Hit::AccountSwitch => {
            // 弹窗与面板间有间隙，Preview 下离面防抖会连带误收；置 Pinned 锁定
            app.panel.mode = crate::ui::panel::PanelMode::Pinned;
            if let Some(p) = app.panel.hwnd {
                app.popup.open(app.hwnd(), p, app.config.accounts.len());
            }
        }
        // 悬停徽标无点击语义
        Hit::UsageInfo => {}

        // ── 导航 ──
        Hit::Back => {
            let was_adding = app.panel.adding_account;
            app.panel.adding_account = false;
            app.panel.key_revealed = false;
            app.panel.customizing_interval = false;
            app.panel.clear_input(panel_hwnd);
            if !was_adding {
                app.panel.view = crate::ui::panel::PanelView::Main;
            }
            relayout_panel(app, panel_hwnd);
        }
        Hit::ClosePanel => {
            app.panel.begin_hide(panel_hwnd);
        }

        // ── 设置 · 轮询间隔 ──
        Hit::IntervalPreset(secs) => {
            app.config.general.poll_interval_secs = secs;
            crate::app::config::save(&app.config);
            app.sync_poll_context();
            if let Some(p) = &app.poller {
                p.reschedule();
            }
            app.panel.customizing_interval = false;
            app.panel.clear_input(panel_hwnd);
            relayout_panel(app, panel_hwnd);
        }
        Hit::CustomizeInterval => {
            app.panel.mode = crate::ui::panel::PanelMode::Pinned;
            app.panel.customizing_interval = true;
            if app.panel.input.interval.trim().is_empty() {
                prefill_interval(app);
            }
            relayout_panel(app, panel_hwnd);
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Interval);
        }
        Hit::ApplyInterval => apply_interval(app, panel_hwnd),
        Hit::InputInterval => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Interval);
        }

        // ── 设置 · 通用 ──
        Hit::Language(choice) => {
            app.config.general.language = match choice {
                LanguageChoice::System => None,
                LanguageChoice::Zh => Some("zh".to_string()),
                LanguageChoice::En => Some("en".to_string()),
            };
            app.lang = crate::ui::i18n::resolve_lang(app.config.general.language.as_deref());
            app.strings = app.lang.strings();
            crate::app::config::save(&app.config);
            invalidate_ui(app);
        }
        Hit::Appearance(choice) => {
            app.config.general.appearance = match choice {
                AppearanceChoice::System => None,
                AppearanceChoice::Light => Some("light".to_string()),
                AppearanceChoice::Dark => Some("dark".to_string()),
            };
            crate::app::config::save(&app.config);
            apply_appearance(app);
            invalidate_ui(app);
        }
        Hit::ToggleAutostart => {
            let next = !crate::platform::autostart::is_enabled();
            match crate::platform::autostart::set_enabled(next) {
                Ok(()) => app.autostart_enabled = next,
                Err(e) => crate::platform::log(&format!("[Quotify] 开机自启设置失败: {e}")),
            }
        }

        // ── 设置 · 网络代理 ──
        Hit::InputProxy => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Proxy);
        }

        // ── 设置 · 用量通知 ──
        Hit::ToggleThreshold => {
            app.config.general.notify_threshold_enabled =
                !app.config.general.notify_threshold_enabled;
            crate::app::config::save(&app.config);
        }
        Hit::ToggleReset5h => {
            app.config.general.notify_reset_5h_enabled =
                !app.config.general.notify_reset_5h_enabled;
            crate::app::config::save(&app.config);
        }
        Hit::ToggleResetWeekly => {
            app.config.general.notify_reset_weekly_enabled =
                !app.config.general.notify_reset_weekly_enabled;
            crate::app::config::save(&app.config);
        }

        // ── 设置 · 高峰区间 ──
        Hit::InputPeakStart => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::PeakStart);
        }
        Hit::InputPeakEnd => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::PeakEnd);
        }
        Hit::ApplyPeak => apply_peak(app, panel_hwnd),

        // ── 设置 · 账号 ──
        Hit::AddAccount => {
            app.panel.mode = crate::ui::panel::PanelMode::Pinned;
            app.panel.adding_account = true;
            app.panel.pending_platform = crate::api::Platform::Cn;
            app.panel.pending_team = false;
            app.panel.input.name.clear();
            app.panel.input.key.clear();
            app.panel.input.org.clear();
            app.panel.input.project.clear();
            relayout_panel(app, panel_hwnd);
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Name);
        }
        Hit::RemoveAccount(i) => {
            if i < app.config.accounts.len() {
                let removed_id = app.config.accounts[i].id.clone();
                app.config.accounts.remove(i);
                if app.config.selected.as_deref() == Some(removed_id.as_str()) {
                    app.config.selected = app.config.accounts.first().map(|a| a.id.clone());
                }
                if let Some(r) = app.panel.renderer.as_mut() {
                    r.hits.clear();
                    r.hover = None;
                }
                switch_poll_source(app);
                relayout_panel(app, panel_hwnd);
            }
        }
        Hit::PickAccount(i) => {
            if i < app.config.accounts.len() {
                select_account(app, i);
                app.popup.close();
                app.panel.view = crate::ui::panel::PanelView::Main;
                // 选完光标还停在弹窗原位（面板外），重起离面计时
                app.panel.outside_since = None;
                relayout_panel(app, panel_hwnd);
            }
        }
        Hit::AccountType(scope) => {
            app.panel.pending_team = matches!(scope, ScopeChoice::Team);
            if app.panel.pending_team {
                // 团队版仅国内站：类型切团队时平台同步回国内
                app.panel.pending_platform = crate::api::Platform::Cn;
            }
            collapse_team_focus(app, panel_hwnd);
            relayout_panel(app, panel_hwnd);
        }
        Hit::InputName => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Name);
        }
        Hit::InputKey => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Key);
        }
        // key 明暗切换；点击不夺输入焦点，输入态保持
        Hit::RevealKey => {
            app.panel.key_revealed = !app.panel.key_revealed;
            if let Some(p) = app.panel.hwnd {
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(p), None, false);
                }
            }
        }
        Hit::InputOrg => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Org);
        }
        Hit::InputProject => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Project);
        }
        Hit::SaveAccount => save_pending_account(app, panel_hwnd),
        Hit::Platform(platform) => {
            app.panel.pending_platform = platform;
            if platform == crate::api::Platform::Intl {
                // 团队版仅国内站：切到国际版时类型同步回个人版
                app.panel.pending_team = false;
            }
            collapse_team_focus(app, panel_hwnd);
            relayout_panel(app, panel_hwnd);
        }

        // ── 设置 · 配置管理与关于 ──
        Hit::ExportConfig => export_config(app),
        Hit::ImportConfig => import_config(app, panel_hwnd),
        Hit::CheckUpdate => {
            if !app.update_checking {
                app.update_checking = true;
                app.update_status = None;
                app.panel.update_available = false;
                struct SendHwnd(HWND);
                unsafe impl Send for SendHwnd {}
                let tray = SendHwnd(app.hwnd());
                std::thread::spawn(move || {
                    let tray = tray;
                    let r = crate::service::update::check_latest();
                    let boxed = Box::into_raw(Box::new(r));
                    let posted = unsafe {
                        windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                            Some(tray.0),
                            WM_APP_UPDATE_RESULT,
                            WPARAM(boxed as usize),
                            Default::default(),
                        )
                    };
                    if posted.is_err() {
                        drop(unsafe { Box::from_raw(boxed) });
                    }
                });
            }
        }
        Hit::OpenDownload => {
            if let Some(Ok(info)) = app.update_status.as_ref() {
                crate::platform::open_url(&info.url);
            }
        }

        // ── 关于窗 ──
        Hit::LinkRepo => {
            crate::platform::open_url(crate::ui::panel::render::about::REPO_URL);
        }
        Hit::LinkIssues => {
            crate::platform::open_url(crate::ui::panel::render::about::ISSUES_URL);
        }
        Hit::NewsItem(i) => {
            // 展开/收起互斥；展开即视为已读，已读标记推进到该条日期并落盘
            let Some(item) = app.news.as_ref().and_then(|n| n.get(i)) else {
                return;
            };
            let need_read = app
                .config
                .general
                .last_news_read
                .as_deref()
                .is_none_or(|r| item.date.as_str() > r);
            if need_read {
                app.config.general.last_news_read = Some(item.date.clone());
                crate::app::config::save(&app.config);
            }
            app.about.news_expanded = if app.about.news_expanded == Some(i) {
                None
            } else {
                Some(i)
            };
            // 展开改变窗高：重定位并重绘关于窗
            refit_about(app);
        }
    }
    unsafe {
        let _ = InvalidateRect(Some(panel_hwnd), None, true);
    }
}

/// 换账号后旧快照的重置基线作废，重置提醒状态防新账号首个快照误报
fn reset_notify_state(app: &mut App) {
    app.threshold_armed_5h = true;
    app.last_reset_5h = None;
    app.threshold_armed_weekly = true;
    app.last_reset_weekly = None;
}

/// 账号切换/增删/导入共用的换源序列：落盘、清快照与提醒基线、重置失败
/// 日志游标、重排轮询并立即拉取、刷新托盘图标。
/// renderer 命中清理与面板 relayout 仅部分路径需要，留在各调用点。
fn switch_poll_source(app: &mut App) {
    crate::app::config::save(&app.config);
    app.data = AccountData {
        snapshot: None,
        last_error: None,
    };
    app.last_logged_error = None;
    reset_notify_state(app);
    app.sync_poll_context();
    app.update_tray_icon();
    if let Some(p) = &app.poller {
        p.refresh_now();
    }
}

pub(crate) fn select_account(app: &mut App, i: usize) {
    if let Some(id) = app.config.accounts.get(i).map(|a| a.id.clone()) {
        app.config.selected = Some(id);
        switch_poll_source(app);
    }
}

/// 团队输入行收起时清除残留焦点，避免键盘输入写入已隐藏的缓冲。
fn collapse_team_focus(app: &mut App, panel_hwnd: HWND) {
    use crate::ui::panel::InputField;
    if !app.panel.pending_team
        && matches!(
            app.panel.input.field,
            Some(InputField::Org) | Some(InputField::Project)
        )
    {
        app.panel.clear_input(panel_hwnd);
    }
}

/// 保存添加账号表单
fn save_pending_account(app: &mut App, panel_hwnd: HWND) {
    let name = app.panel.input.name.trim().to_string();
    let key = app.panel.input.key.trim().to_string();
    if key.is_empty() {
        return;
    }
    // 名称留空默认 Default
    let name = if name.is_empty() {
        "Default".to_string()
    } else {
        name
    };

    let acc = crate::app::config::Account {
        id: app.config.new_account_id(),
        name,
        platform: app.panel.pending_platform,
        team: app.panel.pending_team,
        org_id: app.panel.input.org.trim().to_string(),
        project_id: app.panel.input.project.trim().to_string(),
        api_key: key,
    };
    let is_first = app.config.accounts.is_empty();
    app.config.accounts.push(acc.clone());
    if is_first || app.config.selected.is_none() {
        app.config.selected = Some(acc.id);
    }
    if let Some(r) = app.panel.renderer.as_mut() {
        r.hits.clear();
        r.hover = None;
    }
    switch_poll_source(app);
    app.panel.adding_account = false;
    app.panel.key_revealed = false;
    app.panel.clear_input(panel_hwnd);
    relayout_panel(app, panel_hwnd);
}

/// 应用自定义轮询间隔（分钟）
fn apply_interval(app: &mut App, panel_hwnd: HWND) {
    let secs = app
        .panel
        .input
        .interval
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|m| *m > 0)
        .and_then(|m| m.checked_mul(60))
        .map(|s| s.clamp(MIN_POLL_SECS, MAX_POLL_SECS));
    let Some(secs) = secs else {
        crate::platform::log("[Quotify] 自定义间隔无效");
        return;
    };
    app.config.general.poll_interval_secs = secs;
    crate::app::config::save(&app.config);
    app.sync_poll_context();
    if let Some(p) = &app.poller {
        p.reschedule();
    }
    sync_customizing(app);
    app.panel.clear_input(panel_hwnd);
    relayout_panel(app, panel_hwnd);
}

/// 应用高峰区间：未编辑的框沿用当前配置值；两值合法且不相等才写入
fn apply_peak(app: &mut App, panel_hwnd: HWND) {
    let start = if app.panel.input.peak_start.trim().is_empty() {
        app.config.general.peak_start.clone()
    } else {
        app.panel.input.peak_start.trim().to_string()
    };
    let end = if app.panel.input.peak_end.trim().is_empty() {
        app.config.general.peak_end.clone()
    } else {
        app.panel.input.peak_end.trim().to_string()
    };
    if let (Some(s), Some(e)) = (
        crate::ui::peak::parse_hhmm(&start),
        crate::ui::peak::parse_hhmm(&end),
    ) && s != e
    {
        app.config.general.peak_start = start;
        app.config.general.peak_end = end;
        crate::app::config::save(&app.config);
        app.panel.clear_input(panel_hwnd);
    } else {
        crate::platform::log("[Quotify] 高峰区间格式无效，应为 HH:MM 且两端不相等");
    }
    relayout_panel(app, panel_hwnd);
}

/// 面板与打开中的弹窗、关于窗一并失效重绘；外观、语言切换路径共用
fn invalidate_ui(app: &App) {
    let hwnds = [
        app.panel.hwnd,
        app.popup.hwnd.filter(|_| app.popup.is_open()),
        app.about.hwnd.filter(|_| app.about.is_open()),
    ];
    for h in hwnds.into_iter().flatten() {
        unsafe {
            let _ = InvalidateRect(Some(h), None, true);
        }
    }
}

/// 关于窗按当前动态与展开态重排窗高并重绘；未打开时不动。
/// 按新高重新居中——展开使窗体增长，钉在原顶会在矮屏越出工作区底边
fn refit_about(app: &mut App) {
    let Some(a) = app.about.hwnd.filter(|_| app.about.is_open()) else {
        return;
    };
    let h =
        crate::ui::panel::render::about::about_height(app.news.as_deref(), app.about.news_expanded);
    unsafe {
        let monitor = windows::Win32::Graphics::Gdi::MonitorFromWindow(
            a,
            windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
        );
        let mut mi = windows::Win32::Graphics::Gdi::MONITORINFO {
            cbSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = windows::Win32::Graphics::Gdi::GetMonitorInfoW(monitor, &mut mi);
        let w = (crate::ui::panel::render::about::ABOUT_W * app.about.dpi).round() as i32;
        let hgt = (h as f32 * app.about.dpi).round() as i32;
        let x = mi.rcWork.left + (mi.rcWork.right - mi.rcWork.left - w) / 2;
        // 矮屏窗高超出工作区时保顶弃底，头部信息不裁
        let y = (mi.rcWork.top + (mi.rcWork.bottom - mi.rcWork.top - hgt) / 2).max(mi.rcWork.top);
        let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
            a,
            None,
            x,
            y,
            w,
            hgt,
            windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER,
        );
        let _ = InvalidateRect(Some(a), None, true);
    }
}

/// 应用网络代理；立即重建连接并触发一次拉取验证
fn apply_proxy(app: &mut App, panel_hwnd: HWND) {
    let raw = app.panel.input.proxy.trim().to_string();
    app.config.general.proxy = if raw.is_empty() { None } else { Some(raw) };
    crate::app::config::save(&app.config);
    if let Err(e) = crate::api::client::set_proxy(app.config.general.proxy.clone()) {
        crate::platform::log(&format!("[Quotify] 代理地址无效，保持原连接: {e}"));
    }
    app.panel.input.field = None;
    // 换代理后的拉取是新尝试轮次，同文案失败再现时也重记日志
    app.last_logged_error = None;
    if let Some(p) = &app.poller {
        p.refresh_now();
    }
    relayout_panel(app, panel_hwnd);
}

/// 导出配置为明文 JSON
fn export_config(app: &App) {
    use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

    // 模态对话框期间冻结面板巡检，防止面板被误判收起
    if let Some(panel) = app.panel.hwnd {
        unsafe {
            let _ = KillTimer(Some(panel), crate::ui::panel::TIMER_OUTSIDE_CHECK);
        }
    }
    let picked = crate::platform::save_dialog("quotify-config.json");
    if let Some(panel) = app.panel.hwnd {
        unsafe {
            let _ = SetTimer(
                Some(panel),
                crate::ui::panel::TIMER_OUTSIDE_CHECK,
                200,
                None,
            );
        }
    }
    let Some(path) = picked else {
        return;
    };
    let body = match serde_json::to_string_pretty(&app.config)
        .map_err(|e| e.to_string())
        .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()))
    {
        Ok(()) => app.strings.export_done.to_string(),
        Err(e) => {
            crate::platform::log(&format!("[Quotify] 导出失败: {e}"));
            app.strings.export_failed.to_string()
        }
    };
    crate::platform::notify::show(NOTIFY_TITLE, &body);
}

/// 导入的文件不含账号时等于清空现有配置，弹框确认防误删
fn confirm_import_wipe(app: &App) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW};
    let title = wide(app.strings.import_confirm_title);
    let body = wide(app.strings.import_confirm_body);
    let r = unsafe {
        MessageBoxW(
            Some(app.hwnd()),
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OKCANCEL | MB_ICONWARNING,
        )
    };
    r == IDOK
}

/// 导入配置 JSON；模态期间冻结面板巡检防止误收起
fn import_config(app: &mut App, panel_hwnd: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

    unsafe {
        let _ = KillTimer(Some(panel_hwnd), crate::ui::panel::TIMER_OUTSIDE_CHECK);
    }
    let picked = crate::platform::open_dialog();
    unsafe {
        let _ = SetTimer(
            Some(panel_hwnd),
            crate::ui::panel::TIMER_OUTSIDE_CHECK,
            200,
            None,
        );
    }
    let Some(path) = picked else {
        // 用户取消选择文件，无事发生
        return;
    };
    // 外部文件不可信：先限大小防超大文件撑内存；三类失败分别落日志，区分
    // 超限 / 读取事故 / 坏文件，用户 toast 统一为导入失败
    let parsed = match std::fs::metadata(&path) {
        Ok(m) if m.len() > IMPORT_MAX_BYTES => {
            crate::platform::log(&format!(
                "[Quotify] 配置导入放弃：文件 {} 字节超上限",
                m.len()
            ));
            None
        }
        meta => {
            let _ = meta; // Err(metadata) 随读取一并报错
            std::fs::read_to_string(&path)
                .map_err(|e| {
                    crate::platform::log(&format!("[Quotify] 配置导入读取失败: {e}"));
                })
                .ok()
                .and_then(|text| {
                    serde_json::from_str::<Config>(&text)
                        .map_err(|e| {
                            crate::platform::log(&format!("[Quotify] 配置导入解析失败: {e}"));
                        })
                        .ok()
                })
        }
    };

    let notify_body = match parsed {
        Some(cfg) => {
            if cfg.accounts.is_empty()
                && !app.config.accounts.is_empty()
                && !confirm_import_wipe(app)
            {
                return;
            }
            app.config = cfg;
            // 外部文件的值域不可信，归一化与启动加载保持一致
            normalize_config(&mut app.config);
            app.lang = crate::ui::i18n::resolve_lang(app.config.general.language.as_deref());
            app.strings = app.lang.strings();
            if app.config.selected_account().is_none() {
                app.config.selected = None;
            }
            if let Err(e) = crate::api::client::set_proxy(app.config.general.proxy.clone()) {
                crate::platform::log(&format!("[Quotify] 导入的代理地址无效，保持原连接: {e}"));
            }
            if let Some(r) = app.panel.renderer.as_mut() {
                r.hits.clear();
                r.hover = None;
            }
            switch_poll_source(app);
            app.panel.adding_account = false;
            app.panel.key_revealed = false;
            app.panel.input.field = None;
            app.update_status = None;
            app.update_checking = false;
            app.panel.update_available = false;
            sync_customizing(app);
            relayout_panel(app, panel_hwnd);
            app.strings.import_done.to_string()
        }
        None => {
            crate::platform::log("[Quotify] 配置导入失败：文件不可读或格式无效");
            app.strings.import_failed.to_string()
        }
    };
    crate::platform::notify::show(NOTIFY_TITLE, &notify_body);
}

/// 非预设间隔展开自定义行并预填；预设则收起
fn sync_customizing(app: &mut App) {
    let cur = app.config.general.poll_interval_secs;
    let is_preset = INTERVAL_PRESETS.contains(&cur);
    app.panel.customizing_interval = !is_preset;
    if !is_preset {
        prefill_interval(app);
    }
}

/// 预填自定义间隔（分钟），向上取整
fn prefill_interval(app: &mut App) {
    let cur = app.config.general.poll_interval_secs;
    let mins = cur.div_ceil(60);
    app.panel.input.interval = mins.to_string();
}

/// 面板内按 Enter 的确认行为：名称 → 切到 key；key → 保存；间隔 → 应用
pub(crate) fn confirm_panel_input(app: &mut App, panel_hwnd: HWND) {
    use crate::ui::panel::InputField;
    match app.panel.input.field {
        Some(InputField::Name) => {
            if !app.panel.input.name.trim().is_empty() {
                app.panel.focus_input(panel_hwnd, InputField::Key);
            }
        }
        Some(InputField::Key) => {
            // key 必填，名称可留空
            if !app.panel.input.key.trim().is_empty() {
                if app.panel.pending_team {
                    app.panel.focus_input(panel_hwnd, InputField::Org);
                } else {
                    save_pending_account(app, panel_hwnd);
                }
            }
        }
        Some(InputField::Org) => {
            if !app.panel.input.org.trim().is_empty() {
                app.panel.focus_input(panel_hwnd, InputField::Project);
            }
        }
        Some(InputField::Project) => {
            if !app.panel.input.key.trim().is_empty() {
                save_pending_account(app, panel_hwnd);
            }
        }
        Some(InputField::Interval) => apply_interval(app, panel_hwnd),
        Some(InputField::Proxy) => apply_proxy(app, panel_hwnd),
        Some(InputField::PeakStart) => {
            app.panel.focus_input(panel_hwnd, InputField::PeakEnd);
        }
        Some(InputField::PeakEnd) => apply_peak(app, panel_hwnd),
        None => {}
    }
}

pub(crate) fn resolved_appearance(setting: Option<&str>) -> crate::ui::panel::theme::Appearance {
    match setting {
        Some(s) if s.eq_ignore_ascii_case("light") => crate::ui::panel::theme::Appearance::Light,
        Some(s) if s.eq_ignore_ascii_case("dark") => crate::ui::panel::theme::Appearance::Dark,
        _ => crate::ui::panel::theme::Theme::system_appearance(),
    }
}

/// 解析配置中的高峰区间；格式无效或两端相等回退官方默认，start > end 视为跨午夜
pub fn peak_range_of(config: &Config) -> crate::ui::peak::PeakRange {
    let s = crate::ui::peak::parse_hhmm(&config.general.peak_start);
    let e = crate::ui::peak::parse_hhmm(&config.general.peak_end);
    match (s, e) {
        (Some(s), Some(e)) if s != e => (s, e),
        _ => crate::ui::peak::DEFAULT_PEAK,
    }
}

/// 配置值域归一化，启动加载与导入共用；阈值越界夹回 1–100，
/// 间隔夹进合法区间，保证 UI 预填值与实际轮询行为一致
fn normalize_config(c: &mut Config) {
    c.general.notify_threshold_percent = c.general.notify_threshold_percent.clamp(1, 100);
    c.general.poll_interval_secs = c
        .general
        .poll_interval_secs
        .clamp(MIN_POLL_SECS, MAX_POLL_SECS);
}

/// 应用生效外观
fn apply_appearance(app: &mut App) {
    let appearance = resolved_appearance(app.config.general.appearance.as_deref());
    // 托盘菜单是系统绘制的，须在进程级单独设模式；此后弹出的菜单才带上主题
    crate::platform::menu_theme::apply(matches!(
        appearance,
        crate::ui::panel::theme::Appearance::Dark
    ));
    if let Some(r) = app.panel.renderer.as_mut() {
        r.theme = crate::ui::panel::theme::Theme::new(appearance);
    }
    // 弹窗、关于窗与面板同步换肤
    for r in [&mut app.popup.renderer, &mut app.about.renderer]
        .into_iter()
        .flatten()
    {
        r.theme = crate::ui::panel::theme::Theme::new(appearance);
    }
}

fn sync_main_height(app: &mut App) {
    let (rows, stats, bal) = match app.data.snapshot.as_ref() {
        None => (0, false, false),
        Some(snap) => (
            [
                snap.five_hour.is_some(),
                snap.weekly.is_some(),
                snap.mcp.is_some(),
            ]
            .iter()
            .filter(|b| **b)
            .count(),
            snap.token_stats.is_some(),
            snap.balance.is_some(),
        ),
    };
    app.panel.main_h =
        crate::ui::panel::layout::main_view_height(app.data.snapshot.is_some(), rows, stats, bal);
}

/// 同步状态并重定位面板
fn relayout_panel(app: &mut App, panel_hwnd: HWND) {
    sync_main_height(app);
    app.panel.account_error = matches!(app.data.last_error, Some(crate::api::FetchError::Auth));
    app.panel.caret_ctx = (!app.config.accounts.is_empty(), app.panel.account_error);
    apply_appearance(app);
    if app.panel.mode == crate::ui::panel::PanelMode::Hidden {
        return;
    }
    if app.panel.anchor.is_none() {
        return;
    }
    app.panel.place(
        panel_hwnd,
        app.panel.view_height(app.config.accounts.len()),
        false,
    );
    unsafe {
        let _ = InvalidateRect(Some(panel_hwnd), None, true);
    }
}

/// 应用入口
pub fn run() -> i32 {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        crate::platform::notify::ensure_aumid();

        let config = config::load();
        // 配置的代理在首次请求前生效；地址无效仅记录，不阻断启动
        if let Err(e) = crate::api::client::set_proxy(config.general.proxy.clone()) {
            crate::platform::log(&format!("[Quotify] 代理地址无效，保持直连: {e}"));
        }
        let app = Box::new(App::new(config));
        let mut app = app;

        let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok();
        let hinst = hinst.unwrap_or_default();

        let class_name = wide(TRAY_WND_CLASS);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(tray_wndproc),
            hInstance: hinst.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            hIcon: icon::app_icon(hinst.into()).unwrap_or_default(),
            ..Default::default()
        };
        let atom = RegisterClassW(&wc);
        if atom == 0 {
            return 1;
        }

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap_or_default();

        app.hwnd = Some(hwnd);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *app as *mut App as isize);

        // 两倍尺寸给 Shell 缩小显示；资源优先，手绘兜底
        let px = GetSystemMetrics(SM_CXSMICON).max(16) * 2;
        let initial = icon::resource_icon(px).or_else(|| icon::logo_icon(px));
        if initial.is_none() {
            crate::platform::log(&format!("[Quotify] 初始图标加载失败 (px={px})"));
        }
        let initial = initial.unwrap_or_default();
        app.tray_icon = Some(initial);
        app.tray = TrayIcon::new(hwnd, initial);

        app.sync_poll_context();
        app.poller = Poller::spawn(hwnd, app.poll_target.clone(), app.poll_interval.clone());
        if let Some(p) = &app.poller {
            p.refresh_now();
        }

        // 诊断后门，仅 debug 生效：QUOTIFY_DIAG=adding 自动弹设置-添加账号页
        #[cfg(debug_assertions)]
        if std::env::var("QUOTIFY_DIAG").as_deref() == Ok("adding") {
            let rect = tray_rect(&app).unwrap_or(RECT {
                left: 1300,
                top: 880,
                right: 1340,
                bottom: 920,
            });
            {
                let n = app.config.accounts.len();
                app.panel.show_preview(hwnd, rect, n);
                app.panel.view = crate::ui::panel::PanelView::Settings;
                app.panel.mode = crate::ui::panel::PanelMode::Pinned;
                app.panel.adding_account = true;
                app.panel.pending_platform = crate::api::Platform::Cn;
                if let Some(p) = app.panel.hwnd {
                    relayout_panel(&mut app, p);
                    app.panel.focus_input(p, crate::ui::panel::InputField::Name);
                }
            }
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        if let Some(t) = app.tray.take() {
            drop(t);
        }
        if let Some(ic) = app.tray_icon.take() {
            icon::destroy_owned(ic);
        }
        drop(app.poller.take());
        if com {
            CoUninitialize();
        }
        0
    }
}
