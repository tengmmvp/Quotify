//! 应用层：装配各模块、维护全局状态、驱动消息循环。

pub mod config;

use chrono::{DateTime, Utc};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, HMENU,
    MF_STRING, MSG, PostQuitMessage, RegisterClassW, SM_CXSMICON, SetWindowLongPtrW,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    WINDOW_EX_STYLE, WM_COMMAND, WM_DESTROY, WNDCLASSW, WS_POPUP,
};
use windows::core::PCWSTR;

use crate::api::{FetchError, QuotaBucket, UsageSnapshot};
use crate::app::config::{Config, DEFAULT_INTERVAL_SECS, MIN_POLL_SECS};
use crate::platform::instance::TRAY_WND_CLASS;
use crate::platform::msg::{
    WM_APP_POLL_RESULT, WM_APP_TRAY, WM_APP_UPDATE_RESULT, WM_APP_WAKE_INSTANCE,
};
use crate::platform::wide;
use crate::service::poller::{PollInterval, PollOutcome, PollTarget, Poller};
use crate::ui::i18n::{Lang, Strings};
use crate::ui::icon;
use crate::ui::panel::Panel;
use crate::ui::panel::layout::INTERVAL_PRESETS;
use crate::ui::tray::{self, TrayIcon};

const IDM_SETTINGS: u16 = 1001;
const IDM_EXIT: u16 = 1002;

const NOTIFY_TITLE: &str = "Quotify";

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
    pub(crate) popup: crate::ui::panel::popup::AccountPopup,
    hwnd: Option<HWND>,
    pub(crate) update_status: Option<Result<crate::service::update::ReleaseInfo, String>>,
    update_checking: bool,
    pub(crate) autostart_enabled: bool,
    last_icon_key: Option<(i64, bool, bool)>,
    threshold_armed_5h: bool,
    threshold_armed_weekly: bool,
    last_reset_5h: Option<DateTime<Utc>>,
    last_reset_weekly: Option<DateTime<Utc>>,
}

impl App {
    fn new(config: Config) -> Self {
        let lang = crate::ui::i18n::resolve_lang(config.general.language.as_deref());
        Self {
            strings: lang.strings(),
            lang,
            config,
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
            popup: crate::ui::panel::popup::AccountPopup::new(),
            hwnd: None,
            update_status: None,
            update_checking: false,
            autostart_enabled: crate::platform::autostart::is_enabled(),
            last_icon_key: None,
            threshold_armed_5h: true,
            threshold_armed_weekly: true,
            last_reset_5h: None,
            last_reset_weekly: None,
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
            .map(|a| crate::api::client::AccountSpec {
                platform: a.platform,
                api_key: a.api_key.clone(),
                org_id: a.org_id.clone(),
                project_id: a.project_id.clone(),
            });
        *self.poll_target.lock().unwrap() = target;
        *self.poll_interval.lock().unwrap() =
            self.config.general.poll_interval_secs.max(MIN_POLL_SECS);
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
        let px = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
        let new = match &self.data.snapshot {
            Some(_) => icon::ring_icon(px, used, failed),
            None if failed => icon::ring_icon(px, 0.0, true),
            None => icon::logo_icon(px),
        };
        if let Some(new) = new {
            if let Some(old) = self.tray_icon.take() {
                icon::destroy_icon(old);
            }
            if let Some(tray) = &self.tray {
                tray.update_icon(new);
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
            }
            PollOutcome::Failure(e) => {
                self.data.last_error = Some(*e);
            }
        }
        self.update_tray_icon();
        if let Some(p) = self.panel.hwnd {
            relayout_panel(self, p);
        }
    }

    fn check_notifications(&mut self, snap: &UsageSnapshot) {
        let g = &self.config.general;
        let hwnd = self.hwnd();
        let tray_id = self.tray.as_ref().map(|t| t.tray_id()).unwrap_or(1);
        let notify = |body: &str| crate::platform::notify::show(hwnd, tray_id, NOTIFY_TITLE, body);

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
            let _ = AppendMenuW(menu, MF_STRING, IDM_EXIT as usize, PCWSTR(exit.as_ptr()));
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
        }
    }
}

/// 重置时刻变化即新窗口
fn check_reset(
    bucket: Option<&QuotaBucket>,
    last: &mut Option<DateTime<Utc>>,
    armed: &mut bool,
    enabled: bool,
    msg: &'static str,
    notify: &dyn Fn(&str),
) {
    if let Some(b) = bucket {
        if last.is_some_and(|old| old != b.resets_at.unwrap_or(old)) {
            if enabled {
                notify(msg);
            }
            *armed = true;
        }
        *last = b.resets_at;
    }
}

/// 越线提醒一次，回落重新武装
fn check_threshold(
    bucket: Option<&QuotaBucket>,
    armed: &mut bool,
    th: f64,
    label: &'static str,
    title: &'static str,
    notify: &dyn Fn(&str),
) {
    if let Some(b) = bucket {
        if b.used_percent >= th && *armed {
            *armed = false;
            let body = format!("{label} {title} {}%", b.used_percent.round() as i64);
            notify(&body);
        } else if b.used_percent < th {
            *armed = true;
        }
    }
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
                windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP => {
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
            let app = app_from(hwnd);
            if let Some(app) = app {
                let boxed = wparam.0 as *mut PollOutcome;
                if !boxed.is_null() {
                    let outcome = unsafe { Box::from_raw(boxed) };
                    app.handle_poll_result(*outcome);
                }
            }
            LRESULT(0)
        }
        WM_APP_UPDATE_RESULT => {
            let app = app_from(hwnd);
            if let Some(app) = app {
                app.update_checking = false;
                let boxed = wparam.0 as *mut Result<crate::service::update::ReleaseInfo, String>;
                if !boxed.is_null() {
                    let r = unsafe { Box::from_raw(boxed) };
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
                if let Some(p) = app.panel.hwnd {
                    unsafe {
                        let _ = InvalidateRect(Some(p), None, true);
                    }
                }
                if let Some(p) = app.popup.hwnd {
                    unsafe {
                        let _ = InvalidateRect(Some(p), None, true);
                    }
                }
            }
            LRESULT(0)
        }
        WM_APP_WAKE_INSTANCE => {
            if let Some(app) = app_from(hwnd)
                && let Some(rect) = tray_rect(app)
            {
                let n = app.config.accounts.len();
                app.panel.toggle_pin(hwnd, rect, n);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd = (wparam.0 & 0xFFFF) as u16;
            match cmd {
                IDM_EXIT => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
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
    // 弹窗开着时点面板任意处（箭头除外，箭头是开关）即收起弹窗
    if !matches!(hit, Hit::AccountSwitch) && app.popup.is_open() {
        app.popup.close();
    }
    match hit {
        Hit::Refresh | Hit::Retry => {
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
        Hit::Back => {
            let was_adding = app.panel.adding_account;
            app.panel.adding_account = false;
            app.panel.customizing_interval = false;
            app.panel.clear_input(panel_hwnd);
            if !was_adding {
                app.panel.view = crate::ui::panel::PanelView::Main;
            }
            relayout_panel(app, panel_hwnd);
        }
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
        Hit::Language(choice) => {
            app.config.general.language = match choice {
                LanguageChoice::System => None,
                LanguageChoice::Zh => Some("zh".to_string()),
                LanguageChoice::En => Some("en".to_string()),
            };
            app.lang = crate::ui::i18n::resolve_lang(app.config.general.language.as_deref());
            app.strings = app.lang.strings();
            crate::app::config::save(&app.config);
        }
        Hit::Appearance(choice) => {
            app.config.general.appearance = match choice {
                AppearanceChoice::System => None,
                AppearanceChoice::Light => Some("light".to_string()),
                AppearanceChoice::Dark => Some("dark".to_string()),
            };
            crate::app::config::save(&app.config);
            apply_appearance(app);
            if let Some(p) = app.panel.hwnd {
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(p), None, true);
                }
            }
        }
        Hit::Platform(platform) => {
            app.panel.pending_platform = platform;
            if platform == crate::api::client::Platform::Intl {
                // 团队版仅国内站：切到国际版时类型同步回个人版
                app.panel.pending_team = false;
            }
            collapse_team_focus(app, panel_hwnd);
            relayout_panel(app, panel_hwnd);
        }
        Hit::AccountType(scope) => {
            app.panel.pending_team = matches!(scope, ScopeChoice::Team);
            if app.panel.pending_team {
                // 团队版仅国内站：类型切团队时平台同步回国内
                app.panel.pending_platform = crate::api::client::Platform::Cn;
            }
            collapse_team_focus(app, panel_hwnd);
            relayout_panel(app, panel_hwnd);
        }
        Hit::SaveAccount => save_pending_account(app, panel_hwnd),
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
        Hit::ToggleAutostart => {
            let next = !crate::platform::autostart::is_enabled();
            match crate::platform::autostart::set_enabled(next) {
                Ok(()) => app.autostart_enabled = next,
                Err(e) => crate::platform::log(&format!("[Quotify] 开机自启设置失败: {e}")),
            }
        }
        Hit::RemoveAccount(i) => {
            if i < app.config.accounts.len() {
                app.popup.close();
                let removed_id = app.config.accounts[i].id.clone();
                app.config.accounts.remove(i);
                if app.config.selected.as_deref() == Some(removed_id.as_str()) {
                    app.config.selected = app.config.accounts.first().map(|a| a.id.clone());
                }
                if let Some(r) = app.panel.renderer.as_mut() {
                    r.hits.clear();
                    r.hover = None;
                }
                crate::app::config::save(&app.config);
                app.data = AccountData {
                    snapshot: None,
                    last_error: None,
                };
                app.sync_poll_context();
                app.update_tray_icon();
                if let Some(p) = &app.poller {
                    p.refresh_now();
                }
                relayout_panel(app, panel_hwnd);
            }
        }
        Hit::AddAccount => {
            app.panel.mode = crate::ui::panel::PanelMode::Pinned;
            app.panel.adding_account = true;
            app.panel.pending_platform = crate::api::client::Platform::Cn;
            app.panel.pending_team = false;
            app.panel.input.name.clear();
            app.panel.input.key.clear();
            app.panel.input.org.clear();
            app.panel.input.project.clear();
            relayout_panel(app, panel_hwnd);
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Name);
        }
        Hit::InputName => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Name);
        }
        Hit::InputKey => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Key);
        }
        Hit::InputOrg => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Org);
        }
        Hit::InputProject => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Project);
        }
        Hit::InputInterval => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Interval);
        }
        Hit::InputProxy => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Proxy);
        }
        Hit::InputPeakStart => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::PeakStart);
        }
        Hit::InputPeakEnd => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::PeakEnd);
        }
        Hit::ApplyPeak => apply_peak(app, panel_hwnd),
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
        Hit::AccountSwitch => {
            let n = app.config.accounts.len();
            let parent = app.hwnd();
            let panel = app.panel.hwnd;
            if n > 1
                && let Some(panel_hwnd) = panel
            {
                app.popup.toggle(parent, panel_hwnd, n);
            }
        }
        Hit::OpenDownload => {
            if let Some(Ok(info)) = app.update_status.as_ref() {
                crate::platform::open_url(&info.url);
            }
        }
        // 悬停徽标无点击语义
        Hit::UsageInfo => {}
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
    }
    unsafe {
        let _ = InvalidateRect(Some(panel_hwnd), None, true);
    }
}

pub(crate) fn select_account(app: &mut App, i: usize) {
    if let Some(id) = app.config.accounts.get(i).map(|a| a.id.clone()) {
        app.config.selected = Some(id);
        crate::app::config::save(&app.config);
        app.data = AccountData {
            snapshot: None,
            last_error: None,
        };
        app.sync_poll_context();
        app.update_tray_icon();
        if let Some(p) = &app.poller {
            p.refresh_now();
        }
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
        api_key: key,
        platform: app.panel.pending_platform,
        team: app.panel.pending_team,
        org_id: app.panel.input.org.trim().to_string(),
        project_id: app.panel.input.project.trim().to_string(),
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
    crate::app::config::save(&app.config);
    app.panel.adding_account = false;
    app.panel.clear_input(panel_hwnd);
    app.data = AccountData {
        snapshot: None,
        last_error: None,
    };
    app.sync_poll_context();
    app.update_tray_icon();
    if let Some(p) = &app.poller {
        p.refresh_now();
    }
    relayout_panel(app, panel_hwnd);
}

/// 应用自定义轮询间隔（分钟）
fn apply_interval(app: &mut App, panel_hwnd: HWND) {
    if let Some(mins) = app
        .panel
        .input
        .interval
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|m| *m > 0)
        && let Some(secs) = mins.checked_mul(60)
    {
        app.config.general.poll_interval_secs = secs.max(MIN_POLL_SECS);
        crate::app::config::save(&app.config);
        app.sync_poll_context();
        if let Some(p) = &app.poller {
            p.reschedule();
        }
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

/// 应用网络代理；立即重建连接并触发一次拉取验证
fn apply_proxy(app: &mut App, panel_hwnd: HWND) {
    let raw = app.panel.input.proxy.trim().to_string();
    app.config.general.proxy = if raw.is_empty() { None } else { Some(raw) };
    crate::app::config::save(&app.config);
    if let Err(e) = crate::api::client::set_proxy(app.config.general.proxy.clone()) {
        crate::platform::log(&format!("[Quotify] 代理地址无效，保持原连接: {e}"));
    }
    app.panel.input.field = None;
    if let Some(p) = &app.poller {
        p.refresh_now();
    }
    relayout_panel(app, panel_hwnd);
}

/// 导出配置为明文 JSON；文件内容含 API key，由用户自行保管
fn export_config(app: &App) {
    let Some(path) = crate::platform::save_dialog("quotify-config.json") else {
        return;
    };
    let result = serde_json::to_string_pretty(&app.config)
        .map_err(|e| e.to_string())
        .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
    if let Err(e) = result {
        crate::platform::log(&format!("[Quotify] 导出失败: {e}"));
    }
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
    let parsed = picked
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Config>(&text).ok());

    let notify_body = match parsed {
        Some(cfg) => {
            app.popup.close();
            app.config = cfg;
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
            crate::app::config::save(&app.config);
            app.panel.adding_account = false;
            app.panel.input.field = None;
            app.data = AccountData {
                snapshot: None,
                last_error: None,
            };
            app.sync_poll_context();
            app.update_tray_icon();
            if let Some(p) = &app.poller {
                p.refresh_now();
            }
            relayout_panel(app, panel_hwnd);
            app.strings.import_done.to_string()
        }
        None => {
            crate::platform::log("[Quotify] 配置导入失败：文件不可读或格式无效");
            app.strings.import_failed.to_string()
        }
    };
    let hwnd = app.hwnd();
    let tray_id = app.tray.as_ref().map(|t| t.tray_id()).unwrap_or(1);
    crate::platform::notify::show(hwnd, tray_id, NOTIFY_TITLE, &notify_body);
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

fn resolved_appearance(setting: Option<&str>) -> crate::ui::panel::theme::Appearance {
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

/// 应用生效外观；渲染器未创建时由创建路径兜底
fn apply_appearance(app: &mut App) {
    let appearance = resolved_appearance(app.config.general.appearance.as_deref());
    if let Some(r) = app.panel.renderer.as_mut() {
        r.theme = crate::ui::panel::theme::Theme::new(appearance);
    }
    if let Some(r) = app.popup.renderer.as_mut() {
        r.theme = crate::ui::panel::theme::Theme::new(appearance);
    }
}

fn sync_main_height(app: &mut App) {
    app.panel.main_h = match app.data.snapshot.as_ref() {
        None => 300,
        Some(snap) => {
            let rows = [
                snap.five_hour.is_some(),
                snap.weekly.is_some(),
                snap.mcp.is_some(),
            ]
            .iter()
            .filter(|b| **b)
            .count() as i32;
            let bal = snap.balance.is_some() as i32;
            // 顶栏（恒定 52）+ 刊头 + 指标行 + 余额块（撕线 + 行）+ 底部 footer 区
            16 + 52 + 42 + rows * 52 + bal * 40 + 40
        }
    };
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

        let px = GetSystemMetrics(SM_CXSMICON).max(16);
        let initial = icon::logo_icon(px);
        if initial.is_none() {
            crate::platform::log(&format!("[Quotify] 初始 logo 图标生成失败 (px={px})"));
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
                app.panel.pending_platform = crate::api::client::Platform::Cn;
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
            icon::destroy_icon(ic);
        }
        drop(app.poller.take());
        if com {
            CoUninitialize();
        }
        0
    }
}
