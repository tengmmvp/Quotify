//! 应用层：装配各模块、维护全局状态、驱动消息循环。

pub mod config;

use chrono::{DateTime, Utc};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, HMENU, MF_STRING,
    PostQuitMessage, RegisterClassW, SetWindowLongPtrW, SM_CXSMICON, TrackPopupMenu,
    TranslateMessage, WINDOW_EX_STYLE, WNDCLASSW, WS_POPUP, GWLP_USERDATA, MSG,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON, WM_COMMAND, WM_DESTROY,
};

use crate::api::{FetchError, QuotaBucket, UsageSnapshot};
use crate::app::config::Config;
use crate::platform::instance::TRAY_WND_CLASS;
use crate::platform::msg::{
    WM_APP_POLL_RESULT, WM_APP_TRAY, WM_APP_UPDATE_RESULT, WM_APP_WAKE_INSTANCE,
};
use crate::platform::wide;
use crate::service::poller::{PollInterval, PollOutcome, Poller, PollTarget};
use crate::ui::i18n::{Lang, Strings};
use crate::ui::icon;
use crate::ui::panel::Panel;
use crate::ui::tray::{self, TrayIcon};

/// 右键菜单命令 ID。
const IDM_SETTINGS: u16 = 1001;
const IDM_EXIT: u16 = 1002;

/// 气泡通知的固定标题（应用名）。
const NOTIFY_TITLE: &str = "Quotify";

/// 当前账号的展示状态（旧数据 + 最近错误并存，失败时面板保留旧值）。
pub struct AccountData {
    pub snapshot: Option<UsageSnapshot>,
    pub last_error: Option<FetchError>,
}

pub struct App {
    pub config: Config,
    pub lang: Lang,
    pub strings: &'static Strings,
    pub data: AccountData,
    pub tray: Option<TrayIcon>,
    pub poller: Option<Poller>,
    pub poll_target: PollTarget,
    pub poll_interval: PollInterval,
    tray_icon: Option<windows::Win32::UI::WindowsAndMessaging::HICON>,
    pub panel: Panel,
    hwnd: Option<HWND>,
    /// 检查更新结果（设置页显示）
    pub update_status: Option<Result<crate::service::update::ReleaseInfo, String>>,
    /// 检查更新在飞（连点去重，结果回传时清除）
    update_checking: bool,
    /// 开机自启当前状态（启动时读注册表一次，切换时同步；设置页渲染读）
    pub autostart_enabled: bool,
    /// 托盘图标去重键：(取整百分比, 有无快照, 失败灰环)。未变则跳过
    /// 逐像素光栅 + HICON 重建 + NIM_MODIFY 整条开销
    last_icon_key: Option<(i64, bool, bool)>,
    // 通知去重状态
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
            data: AccountData { snapshot: None, last_error: None },
            tray: None,
            poller: None,
            poll_target: std::sync::Arc::new(std::sync::Mutex::new(None)),
            poll_interval: std::sync::Arc::new(std::sync::Mutex::new(300)),
            tray_icon: None,
            panel: Panel::new(),
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

    /// 同步轮询目标与间隔（配置变更 / 账号切换后调用）。
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
        *self.poll_interval.lock().unwrap() = self.config.general.poll_interval_secs.max(10);
    }

    /// 按最新状态重建托盘图标：无数据 → 默认 logo；有数据 → 环形进度；
    /// 拉取失败且无旧数据 → 灰环（有旧数据仍显示旧进度）。
    fn update_tray_icon(&mut self) {
        // 失败且无旧数据才画灰环；有旧数据保留旧进度
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
            // 有快照时 failed 恒为 false（其定义含 snapshot.is_none()）
            Some(_) => icon::ring_icon(px, used, failed),
            // 失败且无旧数据：灰环（区别于「尚未拉取」的 logo）
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

    /// 处理一次轮询结果：更新数据、刷新图标、触发通知。
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
        // 数据/错误状态变化要立即反映到打开的面板（设置页修复提示、
        // 主页动态高度与错误卡都依赖这次重算 + 重绘）
        if let Some(p) = self.panel.hwnd {
            relayout_panel(self, p);
        }
        // 面板未展示时归还 TLS 等运行时页面；面板展示中不归（避免换页抖动）
        if self.panel.mode == crate::ui::panel::PanelMode::Hidden {
            crate::platform::trim_working_set();
        }
    }

    /// 阈值预警与重置通知（开关均默认关闭）。5h / 周两窗走同一套
    /// 检测逻辑（check_reset / check_threshold），差异只在状态位与文案。
    fn check_notifications(&mut self, snap: &UsageSnapshot) {
        let g = &self.config.general;
        let hwnd = self.hwnd();
        let tray_id = self.tray.as_ref().map(|t| t.tray_id()).unwrap_or(1);
        let notify = |body: &str| crate::platform::notify::show(hwnd, tray_id, NOTIFY_TITLE, body);

        // 重置检测：重置时刻变化即认为进入新窗口（两类提醒独立开关）
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

        // 阈值预警：越线提醒一次，回落重新武装
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

    /// 托盘右键菜单（设置 / 退出）。
    ///
    /// 重入不变量：TrackPopupMenu 会进入模态循环，期间持续派发消息
    /// （WM_COMMAND 等）重入 wndproc 取得 `&mut App`。单线程消息循环下
    /// 无真并发，但本方法持有的 `&self` 不得活到循环内——所有借自
    /// self 的值（owner 窗口、两段文案的宽字符）在进入循环前取齐，
    /// 循环期间不再触 self 任何字段，消除别名点。
    fn show_context_menu(&self, pos: windows::Win32::Foundation::POINT) {
        unsafe {
            let owner = self.hwnd();
            let settings = wide(self.strings.settings);
            let exit = wide(self.strings.exit);
            let menu: HMENU = CreatePopupMenu().unwrap_or_default();
            if menu.is_invalid() {
                return;
            }
            let _ = AppendMenuW(menu, MF_STRING, IDM_SETTINGS as usize, PCWSTR(settings.as_ptr()));
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

/// 单个窗口的重置检测：重置时刻变化即视为进入新窗口，按开关提醒并
/// 重新武装阈值。
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
            *armed = true; // 新窗口重新武装阈值
        }
        *last = b.resets_at;
    }
}

/// 单个窗口的阈值预警：越线提醒一次，回落重新武装。
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

/// 托盘隐藏窗口过程。
extern "system" fn tray_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_APP_TRAY => {
            let app = app_from(hwnd);
            let (code, _) = tray::parse_callback(lparam);
            match code {
                tray::NIN_POPUPOPEN => {
                    if let Some(app) = app
                        && let Some(rect) = tray_rect(app) {
                            let n = app.config.accounts.len();
                            sync_main_height(app);
                            app.panel.show_preview(hwnd, rect, n);
                            // 渲染器在 show_at 内才创建：显示后再应用一次，
                            // 保证显式选择的外观从首帧生效
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
                        && let Some(rect) = tray_rect(app) {
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
                // 结果已回：清除在飞标记（成功失败都清）
                app.update_checking = false;
                let boxed = wparam.0 as *mut Result<crate::service::update::ReleaseInfo, String>;
                if !boxed.is_null() {
                    let r = unsafe { Box::from_raw(boxed) };
                    app.update_status = Some(*r);
                    if let Some(p) = app.panel.hwnd {
                        unsafe {
                            let _ = InvalidateRect(
                                Some(p),
                                None,
                                true,
                            );
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_APP_WAKE_INSTANCE => {
            if let Some(app) = app_from(hwnd)
                && let Some(rect) = tray_rect(app) {
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
                        && let Some(rect) = tray_rect(app) {
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

fn app_from(hwnd: HWND) -> Option<&'static mut App> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    (p != 0).then(|| unsafe { &mut *(p as *mut App) })
}

/// 面板命中处理：渲染层 `Hit` → 应用动作。
pub fn handle_panel_hit(app: &mut App, hit: crate::ui::panel::render::Hit, panel_hwnd: HWND) {
    use crate::ui::panel::render::Hit;
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
            // 进入设置是明确的交互意图：锁定面板，不再因鼠标离开收起
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
            // 添加账号页的「取消」回设置页；设置页的「返回」才回主界面
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
            // 预设生效：收起自定义输入行
            app.panel.customizing_interval = false;
            app.panel.clear_input(panel_hwnd);
            relayout_panel(app, panel_hwnd);
        }
        Hit::Language(tag) => {
            app.config.general.language = if tag.is_empty() { None } else { Some(tag.to_string()) };
            app.lang = crate::ui::i18n::resolve_lang(app.config.general.language.as_deref());
            app.strings = app.lang.strings();
            crate::app::config::save(&app.config);
        }
        Hit::Appearance(tag) => {
            app.config.general.appearance = if tag.is_empty() { None } else { Some(tag.to_string()) };
            crate::app::config::save(&app.config);
            apply_appearance(app);
            if let Some(p) = app.panel.hwnd {
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(p), None, true);
                }
            }
        }
        Hit::Platform(tag) => {
            app.panel.pending_platform = if tag == "intl" {
                // 团队版仅国内站：切到国际版时类型同步回个人版
                app.panel.pending_team = false;
                crate::api::client::Platform::Intl
            } else {
                crate::api::client::Platform::Cn
            };
            collapse_team_focus(app, panel_hwnd);
            relayout_panel(app, panel_hwnd);
        }
        Hit::AccountType(tag) => {
            app.panel.pending_team = tag == "team";
            if app.panel.pending_team {
                // 团队版仅国内站：类型切团队时平台同步回国内
                app.panel.pending_platform = crate::api::client::Platform::Cn;
            }
            // 团队输入行收起时清掉残留的输入焦点
            collapse_team_focus(app, panel_hwnd);
            // 团队输入行的显隐影响添加页高度
            relayout_panel(app, panel_hwnd);
        }
        Hit::SaveAccount => save_pending_account(app, panel_hwnd),
        Hit::ToggleThreshold => {
            app.config.general.notify_threshold_enabled = !app.config.general.notify_threshold_enabled;
            crate::app::config::save(&app.config);
        }
        Hit::ToggleReset5h => {
            app.config.general.notify_reset_5h_enabled = !app.config.general.notify_reset_5h_enabled;
            crate::app::config::save(&app.config);
        }
        Hit::ToggleResetWeekly => {
            app.config.general.notify_reset_weekly_enabled = !app.config.general.notify_reset_weekly_enabled;
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
                let removed_id = app.config.accounts[i].id.clone();
                app.config.accounts.remove(i);
                if app.config.selected.as_deref() == Some(removed_id.as_str()) {
                    app.config.selected = app.config.accounts.first().map(|a| a.id.clone());
                }
                // 旧命中区携带列表索引：列表已变立即作废，防止 WM_PAINT
                // 低优先级派发下的第二次点击用旧矩形删掉下一个账号（双击连删）
                if let Some(r) = app.panel.renderer.as_mut() {
                    r.hits.clear();
                    r.hover = None;
                }
                crate::app::config::save(&app.config);
                app.data = AccountData { snapshot: None, last_error: None };
                app.sync_poll_context();
                app.update_tray_icon();
                if let Some(p) = &app.poller {
                    p.refresh_now();
                }
                // 数据已清：立即重算 account_error 与动态高度（提示行、
                // 账号块的伸缩不能等下一轮轮询回来才生效）
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
            app.panel.focus_input(panel_hwnd, crate::ui::panel::InputField::Name);
        }
        Hit::InputName => {
            app.panel.focus_input(panel_hwnd, crate::ui::panel::InputField::Name);
        }
        Hit::InputKey => {
            app.panel.focus_input(panel_hwnd, crate::ui::panel::InputField::Key);
        }
        Hit::InputOrg => {
            app.panel.focus_input(panel_hwnd, crate::ui::panel::InputField::Org);
        }
        Hit::InputProject => {
            app.panel.focus_input(panel_hwnd, crate::ui::panel::InputField::Project);
        }
        Hit::InputInterval => {
            app.panel.focus_input(panel_hwnd, crate::ui::panel::InputField::Interval);
        }
        Hit::CustomizeInterval => {
            app.panel.mode = crate::ui::panel::PanelMode::Pinned;
            app.panel.customizing_interval = true;
            if app.panel.input.interval.trim().is_empty() {
                prefill_interval(app);
            }
            relayout_panel(app, panel_hwnd);
            app.panel.focus_input(panel_hwnd, crate::ui::panel::InputField::Interval);
        }
        Hit::ApplyInterval => apply_interval(app, panel_hwnd),
        Hit::AccountSwitch => {
            let n = app.config.accounts.len();
            if n > 1 {
                let cur = app
                    .config
                    .selected_account()
                    .map(|a| a.id.clone())
                    .unwrap_or_default();
                let idx = app.config.accounts.iter().position(|a| a.id == cur).unwrap_or(0);
                let next = (idx + 1) % n;
                select_account(app, next);
            }
        }
        Hit::CheckUpdate => {
            // 在飞去重：上一轮结果未回不重复发起（连点只查一次）
            if !app.update_checking {
                app.update_checking = true;
                app.update_status = None;
                // HWND 跨线程移动安全（内核对象引用），显式声明 Send
                struct SendHwnd(HWND);
                unsafe impl Send for SendHwnd {}
                let tray = SendHwnd(app.hwnd());
                std::thread::spawn(move || {
                    // 先整体移动 SendHwnd（2021 精准捕获会绕过包装直接捕获裸 HWND）
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
                    // 投递失败（窗口已销毁）：取回 boxed 防泄漏
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

/// 选中账号 i 并立即刷新。
fn select_account(app: &mut App, i: usize) {
    // 只克隆 id：账号结构含 api_key，无需整体复制
    if let Some(id) = app.config.accounts.get(i).map(|a| a.id.clone()) {
        app.config.selected = Some(id);
        crate::app::config::save(&app.config);
        app.data = AccountData { snapshot: None, last_error: None };
        app.sync_poll_context();
        app.update_tray_icon();
        if let Some(p) = &app.poller {
            p.refresh_now();
        }
    }
}

/// 团队输入行收起（切回个人版）时，若焦点还在组织/项目框里则清除，
/// 避免光标残留在已消失的控件位置、键盘输入写入隐藏缓冲。
fn collapse_team_focus(app: &mut App, panel_hwnd: HWND) {
    use crate::ui::panel::InputField;
    if !app.panel.pending_team
        && matches!(app.panel.input.field, Some(InputField::Org) | Some(InputField::Project))
    {
        app.panel.clear_input(panel_hwnd);
    }
}

/// 保存添加账号表单（读取自绘输入缓冲）。
fn save_pending_account(app: &mut App, panel_hwnd: HWND) {
    let name = app.panel.input.name.trim().to_string();
    let key = app.panel.input.key.trim().to_string();
    if key.is_empty() {
        return;
    }
    // 名称留空时默认「Default」，避免空用户名出现在顶栏/账号卡
    let name = if name.is_empty() { "Default".to_string() } else { name };

    let acc = crate::app::config::Account {
        id: app.config.new_account_id(),
        name,
        api_key: key,
        platform: app.panel.pending_platform,
        // 团队版标记与选择头；两 ID 留空时查询自动按个人版降级
        team: app.panel.pending_team,
        org_id: app.panel.input.org.trim().to_string(),
        project_id: app.panel.input.project.trim().to_string(),
    };
    let is_first = app.config.accounts.is_empty();
    app.config.accounts.push(acc.clone());
    if is_first || app.config.selected.is_none() {
        app.config.selected = Some(acc.id);
    }
    // 旧命中区携带列表索引：列表已变立即作废（防旧矩形误触）
    if let Some(r) = app.panel.renderer.as_mut() {
        r.hits.clear();
        r.hover = None;
    }
    crate::app::config::save(&app.config);
    app.panel.adding_account = false;
    app.panel.clear_input(panel_hwnd);
    app.data = AccountData { snapshot: None, last_error: None };
    app.sync_poll_context();
    app.update_tray_icon();
    if let Some(p) = &app.poller {
        p.refresh_now();
    }
    relayout_panel(app, panel_hwnd);
}

/// 应用自定义轮询间隔（分钟）。结果仍为非预设值时保持输入行展开
/// （预填新值，当前值始终可见）；恰好落在预设上则收起。
fn apply_interval(app: &mut App, panel_hwnd: HWND) {
    if let Some(mins) = app.panel.input.interval.trim().parse::<u64>().ok().filter(|m| *m > 0) {
        // 大数乘 60 可能回绕出超小间隔：溢出时保持原值
        if let Some(secs) = mins.checked_mul(60) {
            app.config.general.poll_interval_secs = secs.max(10);
            crate::app::config::save(&app.config);
            app.sync_poll_context();
            if let Some(p) = &app.poller {
                p.reschedule();
            }
        }
    }
    sync_customizing(app);
    app.panel.clear_input(panel_hwnd);
    relayout_panel(app, panel_hwnd);
}

/// 当前间隔非预设值时展开自定义输入行并预填；预设则收起。
fn sync_customizing(app: &mut App) {
    let cur = app.config.general.poll_interval_secs;
    let is_preset = [60u64, 300, 900, 1800].contains(&cur);
    app.panel.customizing_interval = !is_preset;
    if !is_preset {
        prefill_interval(app);
    }
}

/// 用当前配置预填自定义间隔（分钟，向上取整）。
fn prefill_interval(app: &mut App) {
    let cur = app.config.general.poll_interval_secs;
    let mins = cur.div_ceil(60);
    app.panel.input.interval = mins.to_string();
}

/// 面板内按 Enter 的确认行为：名称 → 切到 key；key → 保存；间隔 → 应用。
pub(crate) fn confirm_panel_input(app: &mut App, panel_hwnd: HWND) {
    use crate::ui::panel::InputField;
    match app.panel.input.field {
        Some(InputField::Name) => {
            if !app.panel.input.name.trim().is_empty() {
                app.panel.focus_input(panel_hwnd, InputField::Key);
            }
        }
        Some(InputField::Key) => {
            // 名称可留空（保存时默认 Default），key 必填
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
        None => {}
    }
}

/// 解析生效外观：配置显式指定则用之，否则跟随系统。
fn resolved_appearance(setting: Option<&str>) -> crate::ui::panel::theme::Appearance {
    match setting {
        Some(s) if s.eq_ignore_ascii_case("light") => crate::ui::panel::theme::Appearance::Light,
        Some(s) if s.eq_ignore_ascii_case("dark") => crate::ui::panel::theme::Appearance::Dark,
        _ => crate::ui::panel::theme::Theme::system_appearance(),
    }
}

/// 把生效外观应用到渲染器（即时换肤；渲染器尚未创建时由创建路径兜底）。
fn apply_appearance(app: &mut App) {
    let appearance = resolved_appearance(app.config.general.appearance.as_deref());
    if let Some(r) = app.panel.renderer.as_mut() {
        r.theme = crate::ui::panel::theme::Theme::new(appearance);
    }
}

/// 按数据内容重算主视图高度（指标行数 / 余额 / 副标题都会伸缩面板）。
fn sync_main_height(app: &mut App) {
    app.panel.main_h = match app.data.snapshot.as_ref() {
        None => 300,
        Some(snap) => {
            let rows = [snap.five_hour.is_some(), snap.weekly.is_some(), snap.mcp.is_some()]
                .iter()
                .filter(|b| **b)
                .count() as i32;
            let bal = snap.balance.is_some() as i32;
            // 顶栏 + 刊头 + 指标行 + 余额块（撕线 + 行）+ 底部 footer 区
            16 + if snap.has_meta() { 52 } else { 38 } + 42 + rows * 52 + bal * 40 + 40
        }
    };
}

/// 视图切换后面板重定位重设尺寸。编排：高度重算 → 鉴权错误状态 →
/// 光标上下文 → 外观 → 定位。定位统一走 `Panel::place`（监视器 + DPI +
/// 夹取 + SetWindowPos + 动画基准一行完成）。
fn relayout_panel(app: &mut App, panel_hwnd: HWND) {
    sync_main_height(app);
    app.panel.account_error = matches!(app.data.last_error, Some(crate::api::FetchError::Auth(_)));
    // 自绘输入的光标上下文（设置页间隔输入框 y 随账号区 / 错误行伸缩）
    app.panel.caret_ctx = (!app.config.accounts.is_empty(), app.panel.account_error);
    apply_appearance(app);
    // 面板已收起：只同步状态，不做窗口几何操作（不 SetWindowPos 不 Invalidate）
    if app.panel.mode == crate::ui::panel::PanelMode::Hidden {
        return;
    }
    if app.panel.anchor.is_none() {
        return;
    }
    app.panel.place(panel_hwnd, app.panel.view_height(app.config.accounts.len()), false);
    unsafe {
        let _ = InvalidateRect(Some(panel_hwnd), None, true);
    }
}


/// 应用入口：初始化、装配、消息循环。
pub fn run() -> i32 {
    unsafe {
        // Per-Monitor V2：所有坐标/尺寸统一物理像素体系（缺失会导致
        // 非 DPI-aware 虚拟化，各 API 混用逻辑/物理坐标、面板定位出屏）
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();

        let config = config::load();
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
            0, 0, 0, 0,
            None, None,
            Some(hinst.into()),
            None,
        )
        .unwrap_or_default();

        app.hwnd = Some(hwnd);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *app as *mut App as isize);

        // 初始图标（默认 logo，无数据态）+ 托盘注册
        let px = GetSystemMetrics(SM_CXSMICON).max(16);
        let initial = icon::logo_icon(px);
        if initial.is_none() {
            crate::platform::log(&format!("[Quotify] 初始 logo 图标生成失败 (px={px})"));
        }
        let initial = initial.unwrap_or_default();
        app.tray_icon = Some(initial);
        app.tray = TrayIcon::new(hwnd, initial);

        // 轮询线程
        app.sync_poll_context();
        app.poller = Poller::spawn(hwnd, app.poll_target.clone(), app.poll_interval.clone());
        if let Some(p) = &app.poller {
            p.refresh_now(); // 启动即拉取一次
        }

        // 诊断后门（仅 debug）：QUOTIFY_DIAG=adding 自动弹设置-添加账号页
        #[cfg(debug_assertions)]
        if std::env::var("QUOTIFY_DIAG").as_deref() == Ok("adding") {
            let rect = tray_rect(&app).unwrap_or(RECT { left: 1300, top: 880, right: 1340, bottom: 920 });
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

        // 消息循环
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // 清理
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
