//! 应用层：装配各模块、维护全局状态、驱动消息循环。

pub mod config;

use chrono::{DateTime, Utc};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::Graphics::Gdi::{
    InvalidateRect, MonitorFromWindow, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, HMENU, MF_STRING,
    PostQuitMessage, RegisterClassW, SetWindowLongPtrW, SM_CXSMICON, TrackPopupMenu,
    TranslateMessage, WINDOW_EX_STYLE, WNDCLASSW, WS_POPUP, GWLP_USERDATA, MSG,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON, WM_COMMAND, WM_DESTROY,
};

use crate::api::{FetchError, UsageSnapshot};
use crate::platform::instance::{TRAY_WND_CLASS, WM_APP_WAKEUP};
use crate::service::poller::{
    PollOutcome, Poller, PollInterval, PollTarget, WM_APP_POLL_RESULT,
};
use crate::ui::i18n::{Lang, Strings};
use crate::ui::panel::Panel;
use crate::ui::tray::{self, TrayIcon};
use crate::ui::icon;
use crate::app::config::Config;

/// 右键菜单命令 ID。
const IDM_SETTINGS: u16 = 1001;
const IDM_EXIT: u16 = 1002;

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
            .map(|a| (a.platform, a.api_key.clone()));
        *self.poll_target.lock().unwrap() = target;
        *self.poll_interval.lock().unwrap() = self.config.general.poll_interval_secs.max(10);
    }

    /// 按最新状态重建托盘图标：无数据 → 默认 logo；有数据 → 环形进度。
    fn update_tray_icon(&mut self) {
        let px = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
        let new = match &self.data.snapshot {
            Some(s) => {
                let used = s.five_hour.as_ref().map(|b| b.used_percent).unwrap_or(0.0);
                icon::ring_icon(px, used, false)
            }
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
        // 面板未展示时归还 TLS 等运行时页面；面板展示中不归（避免换页抖动）
        if self.panel.mode == crate::ui::panel::PanelMode::Hidden {
            crate::platform::trim_working_set();
        }
    }

    /// 阈值预警与重置通知（开关均默认关闭）。
    fn check_notifications(&mut self, snap: &UsageSnapshot) {
        let g = &self.config.general;
        let hwnd = self.hwnd();
        let tray_id = self.tray.as_ref().map(|t| t.tray_id()).unwrap_or(1);

        // 重置检测：重置时刻变化即认为进入新窗口（两类提醒独立开关）
        if let Some(fh) = &snap.five_hour {
            if self.last_reset_5h.is_some_and(|old| old != fh.resets_at.unwrap_or(old)) {
                if g.notify_reset_5h_enabled {
                    crate::platform::notify::show(hwnd, tray_id, "Quotify", self.strings.notify_reset_5h);
                }
                self.threshold_armed_5h = true; // 新窗口重新武装阈值
            }
            self.last_reset_5h = fh.resets_at;
        }
        if let Some(w) = &snap.weekly {
            if self.last_reset_weekly.is_some_and(|old| old != w.resets_at.unwrap_or(old)) {
                if g.notify_reset_weekly_enabled {
                    crate::platform::notify::show(hwnd, tray_id, "Quotify", self.strings.notify_reset_weekly);
                }
                self.threshold_armed_weekly = true;
            }
            self.last_reset_weekly = w.resets_at;
        }

        // 阈值预警：越线提醒一次，回落重新武装
        if g.notify_threshold_enabled {
            let th = g.notify_threshold_percent as f64;
            if let Some(fh) = &snap.five_hour {
                if fh.used_percent >= th && self.threshold_armed_5h {
                    self.threshold_armed_5h = false;
                    let body = format!(
                        "{} {} {}%",
                        self.strings.five_hour,
                        self.strings.notify_threshold_title,
                        fh.used_percent.round() as i64
                    );
                    crate::platform::notify::show(hwnd, tray_id, "Quotify", &body);
                } else if fh.used_percent < th {
                    self.threshold_armed_5h = true;
                }
            }
            if let Some(w) = &snap.weekly {
                if w.used_percent >= th && self.threshold_armed_weekly {
                    self.threshold_armed_weekly = false;
                    let body = format!(
                        "{} {} {}%",
                        self.strings.weekly,
                        self.strings.notify_threshold_title,
                        w.used_percent.round() as i64
                    );
                    crate::platform::notify::show(hwnd, tray_id, "Quotify", &body);
                } else if w.used_percent < th {
                    self.threshold_armed_weekly = true;
                }
            }
        }
    }

    /// 托盘右键菜单（设置 / 退出）。
    fn show_context_menu(&self, pos: windows::Win32::Foundation::POINT) {
        unsafe {
            let menu: HMENU = CreatePopupMenu().unwrap_or_default();
            if menu.is_invalid() {
                return;
            }
            let mut buf = wide(self.strings.settings);
            let _ = AppendMenuW(menu, MF_STRING, IDM_SETTINGS as usize, PCWSTR(buf.as_ptr()));
            buf = wide(self.strings.exit);
            let _ = AppendMenuW(menu, MF_STRING, IDM_EXIT as usize, PCWSTR(buf.as_ptr()));
            let _ = TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON,
                pos.x,
                pos.y,
                None,
                self.hwnd(),
                None,
            );
            let _ = DestroyMenu(menu);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// 编译期把 PCWSTR 引入作用域（windows::core）
use windows::core::PCWSTR;

/// 托盘隐藏窗口过程。
extern "system" fn tray_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        crate::ui::tray::WM_APP_TRAY => {
            let app = app_from(hwnd);
            let (code, _) = tray::parse_callback(lparam);
            match code {
                tray::NIN_POPUPOPEN => {
                    if let Some(app) = app {
                        if let Some(rect) = tray_rect(app) {
                            let n = app.config.accounts.len();
                            app.panel.show_preview(hwnd, rect, n);
                        }
                    }
                }
                tray::NIN_POPUPCLOSE => {
                    if let Some(app) = app {
                        app.panel.request_close();
                    }
                }
                windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP => {
                    if let Some(app) = app {
                        if let Some(rect) = tray_rect(app) {
                            let n = app.config.accounts.len();
                            app.panel.toggle_pin(hwnd, rect, n);
                        }
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
        WM_APP_WAKEUP => {
            if let Some(app) = app_from(hwnd) {
                if let Some(rect) = tray_rect(app) {
                    let n = app.config.accounts.len();
                    app.panel.toggle_pin(hwnd, rect, n);
                }
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
                    if let Some(app) = app_from(hwnd) {
                        if let Some(rect) = tray_rect(app) {
                            let n = app.config.accounts.len();
                            app.panel.show_preview(hwnd, rect, n);
                            app.panel.view = crate::ui::panel::PanelView::Settings;
                            // 视图切换后重定尺寸（设置页比主界面高）
                            if let Some(p) = app.panel.hwnd {
                                relayout_panel(app, p);
                            }
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
    let s = app.strings;
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
            relayout_panel(app, panel_hwnd);
        }
        Hit::Back => {
            let was_adding = app.panel.adding_account;
            app.panel.adding_account = false;
            app.panel.destroy_edit_controls_pub();
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
        }
        Hit::Language(tag) => {
            app.config.general.language = if tag.is_empty() { None } else { Some(tag.to_string()) };
            app.lang = crate::ui::i18n::resolve_lang(app.config.general.language.as_deref());
            app.strings = app.lang.strings();
            crate::app::config::save(&app.config);
        }
        Hit::Platform(tag) => {
            app.panel.pending_platform = if tag == "intl" {
                crate::api::client::Platform::Intl
            } else {
                crate::api::client::Platform::Cn
            };
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
            if let Err(e) = crate::platform::autostart::set_enabled(next) {
                eprintln!("{e}");
            }
        }
        Hit::SelectAccount(i) => select_account(app, panel_hwnd, i),
        Hit::RemoveAccount(i) => {
            if i < app.config.accounts.len() {
                let removed_id = app.config.accounts[i].id.clone();
                app.config.accounts.remove(i);
                if app.config.selected.as_deref() == Some(removed_id.as_str()) {
                    app.config.selected = app.config.accounts.first().map(|a| a.id.clone());
                }
                crate::app::config::save(&app.config);
                app.data = AccountData { snapshot: None, last_error: None };
                app.sync_poll_context();
                app.update_tray_icon();
                if let Some(p) = &app.poller {
                    p.refresh_now();
                }
            }
        }
        Hit::AddAccount => {
            app.panel.mode = crate::ui::panel::PanelMode::Pinned;
            app.panel.adding_account = true;
            app.panel.pending_platform = crate::api::client::Platform::Cn;
            relayout_panel(app, panel_hwnd);
            // EDIT 延迟到队列空闲时创建（见 WM_APP_SPAWN_EDIT 注释）
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(panel_hwnd),
                    crate::ui::panel::WM_APP_SPAWN_EDIT,
                    Default::default(),
                    Default::default(),
                );
            }
        }
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
                select_account(app, panel_hwnd, next);
            }
        }
        Hit::CheckUpdate => {
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
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(tray.0),
                        WM_APP_UPDATE_RESULT,
                        WPARAM(boxed as usize),
                        Default::default(),
                    );
                }
            });
        }
    }
    let _ = s;
    unsafe {
        let _ = InvalidateRect(Some(panel_hwnd), None, true);
    }
}

/// 更新结果回传消息（与轮询结果分开）。
pub const WM_APP_UPDATE_RESULT: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 4;

/// 选中账号 i 并立即刷新。
fn select_account(app: &mut App, panel_hwnd: HWND, i: usize) {
    if let Some(acc) = app.config.accounts.get(i).cloned() {
        app.config.selected = Some(acc.id);
        crate::app::config::save(&app.config);
        app.data = AccountData { snapshot: None, last_error: None };
        app.sync_poll_context();
        app.update_tray_icon();
        if let Some(p) = &app.poller {
            p.refresh_now();
        }
    }
    let _ = panel_hwnd;
}

/// 保存添加账号表单（读取 EDIT 内容）。
fn save_pending_account(app: &mut App, panel_hwnd: HWND) {
    let name = read_edit_text(app.panel.edit_name).filter(|s| !s.trim().is_empty());
    let key = read_edit_text(app.panel.edit_key).filter(|s| !s.trim().is_empty());
    let (Some(name), Some(key)) = (name, key) else { return };

    let acc = crate::app::config::Account {
        id: app.config.new_account_id(),
        name: name.trim().to_string(),
        api_key: key.trim().to_string(),
        platform: app.panel.pending_platform,
    };
    let is_first = app.config.accounts.is_empty();
    app.config.accounts.push(acc.clone());
    if is_first || app.config.selected.is_none() {
        app.config.selected = Some(acc.id);
    }
    crate::app::config::save(&app.config);
    app.panel.adding_account = false;
    app.panel.destroy_edit_controls_pub();
    app.data = AccountData { snapshot: None, last_error: None };
    app.sync_poll_context();
    app.update_tray_icon();
    if let Some(p) = &app.poller {
        p.refresh_now();
    }
    relayout_panel(app, panel_hwnd);
}

/// 读取 EDIT 控件文本。
fn read_edit_text(edit: Option<HWND>) -> Option<String> {
    let h = edit?;
    unsafe {
        let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextLengthW(h);
        let mut buf = vec![0u16; len as usize + 1];
        let n = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(h, &mut buf);
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    }
}

/// 视图切换后面板重定位重设尺寸。
fn relayout_panel(app: &App, panel_hwnd: HWND) {
    let Some(anchor) = app.panel.anchor else { return };
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::*;
        let monitor = MonitorFromWindow(panel_hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = windows::Win32::Graphics::Gdi::MONITORINFO {
            cbSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut mi);
        let w = app.panel.px_of(crate::ui::panel::theme::PANEL_WIDTH);
        let h = app.panel.px_of(app.panel.view_height_pub(app.config.accounts.len()));
        let ax = (anchor.left + anchor.right) / 2;
        let mut x = ax - w / 2;
        let mut y = anchor.top - h - app.panel.px_of(8);
        if x < mi.rcWork.left + 8 {
            x = mi.rcWork.left + 8;
        }
        if x + w > mi.rcWork.right - 8 {
            x = mi.rcWork.right - 8 - w;
        }
        if y < mi.rcWork.top + 8 {
            y = mi.rcWork.top + 8;
        }
        let _ = SetWindowPos(panel_hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOCOPYBITS);
        let _ = InvalidateRect(Some(panel_hwnd), None, true);
    }
}

/// 添加账号时创建 name / key 输入框（EDIT 子控件）。
/// 由 WM_APP_SPAWN_EDIT 延迟调用（队列空闲时）。
pub(crate) fn spawn_edit_controls(app: &mut App, panel_hwnd: HWND) {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::*;
        let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .unwrap_or_default();
        let class = wide("EDIT");
        // 逻辑布局与 draw_settings 的 input_field 视觉框对齐：
        // 框 (pad, 126/176, 300×26)，EDIT 内缩 2px
        let dpi = app.panel.dpi_pub();
        let lx = |v: i32| (v as f32 * dpi).round() as i32;
        let x = lx(22);
        let input_w = lx(296);
        let h = lx(22);
        // 15pt Segoe UI（与面板正文字号一致）；HFONT 存 Panel 复用，
        // 避免多次添加账号累积 GDI 句柄
        if app.panel.edit_font.is_none() {
            let face = wide("Segoe UI");
            app.panel.edit_font = Some(windows::Win32::Graphics::Gdi::CreateFontW(
                -lx(20),
                0,
                0,
                0,
                windows::Win32::Graphics::Gdi::FW_NORMAL.0 as i32,
                0,
                0,
                0,
                windows::Win32::Graphics::Gdi::DEFAULT_CHARSET,
                windows::Win32::Graphics::Gdi::OUT_DEFAULT_PRECIS,
                windows::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS,
                windows::Win32::Graphics::Gdi::CLEARTYPE_QUALITY,
                0,
                PCWSTR(face.as_ptr()),
            ));
        }
        let font = app.panel.edit_font;
        for (ey, is_key) in [(lx(128), false), (lx(178), true)] {
            // 平面无边框：深色底由 WM_CTLCOLOREDIT 上色，与面板风格一体
            let edit = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                None,
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                x, ey, input_w, h,
                Some(panel_hwnd),
                None,
                Some(hinst.into()),
                None,
            );
            if let Ok(e) = edit {
                if let Some(f) = font {
                    let _ = SendMessageW(e, WM_SETFONT, Some(WPARAM(f.0 as usize)), Some(LPARAM(1)));
                }
                // 解除 IME 关联：第三方输入法的全局钩子会拖慢发往本窗口
                // 的每次输入消息（打字/点击都卡数秒的根源）。名称/key 以
                // ASCII 为主；需要中文时可在别处粘贴。
                let _ = windows::Win32::UI::Input::Ime::ImmAssociateContext(e, Default::default());
                if is_key {
                    app.panel.set_edit_key(e);
                } else {
                    app.panel.set_edit_name(e);
                }
            }
        }
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
        let _gdiplus = icon::init();

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
            eprintln!("[quotify] 初始 logo 图标生成失败 (px={px}, gdiplus={})", _gdiplus.is_some());
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
                    relayout_panel(&app, p);
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(p),
                        crate::ui::panel::WM_APP_SPAWN_EDIT,
                        Default::default(),
                        Default::default(),
                    );
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
