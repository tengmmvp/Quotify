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
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
    TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WM_COMMAND, WM_DESTROY,
    WM_NULL, WM_POWERBROADCAST, WM_TIMER, WNDCLASSW, WS_POPUP,
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
    DEFAULT_INTERVAL_SECS, MAX_POLL_SECS, MIN_POLL_SECS, PollGeneration, PollInterval, PollMessage,
    PollOutcome, PollTarget, Poller,
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

/// 托盘注册重试定时器：首注册失败每 2s 重试，上限 15 次，成功或
/// 超限即撤；explorer 重启的兜底另走 TaskbarCreated。
const TIMER_TRAY_RETRY: usize = 1;
const TRAY_RETRY_MS: u32 = 2000;
const TRAY_RETRY_MAX: u32 = 15;

/// 更新检查结果的回传通道
static UPDATE_SLOT: crate::platform::post::Slot<
    Result<crate::service::update::ReleaseInfo, String>,
> = crate::platform::post::Slot::new();

/// 仓库动态拉取结果的回传通道
static NEWS_SLOT: crate::platform::post::Slot<
    Result<Vec<crate::service::whatsnew::NewsItem>, String>,
> = crate::platform::post::Slot::new();

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
    tray_retries: u32,
    poller: Option<Poller>,
    poll_target: PollTarget,
    poll_interval: PollInterval,
    poll_gen: PollGeneration,
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
    last_icon_key: Option<(i32, i64, bool, bool)>,
    /// tooltip 的内容指纹，不变则跳过文本构建
    last_tip: Option<String>,
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
            tray_retries: 0,
            poller: None,
            poll_target: std::sync::Arc::new(std::sync::Mutex::new(None)),
            poll_interval: std::sync::Arc::new(std::sync::Mutex::new(DEFAULT_INTERVAL_SECS)),
            poll_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
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
            last_tip: None,
            threshold_armed_5h: false,
            last_reset_5h: None,
            threshold_armed_weekly: false,
            last_reset_weekly: None,
            last_logged_error: None,
        }
    }

    pub(crate) fn hwnd(&self) -> HWND {
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

    /// 刷新托盘 tooltip 与图标
    /// NIM_MODIFY 是跨线程同步调用，可能重入消息泵使引用失效：每段
    /// 下发后经重取引用重算决策再续走。内容键只在下发成功后推进，
    /// 失败由下一次刷新重试。
    pub(crate) fn update_tray_icon(&mut self) {
        let tray_hwnd = self.hwnd();
        // tooltip 先行：与上次下发的文本比对，不变即跳过下发。文本级
        // 比对天然跟随文案增改，指纹键漏段会冻结旧文本。
        let tip = tooltip_text(self);
        // 未注册期 NIM_MODIFY 必失败：跳过下发，键不推进，重注册后
        // 的全量重发自愈。
        let tray_up = self.tray.as_ref().is_some_and(|tray| tray.registered);
        if tray_up && self.last_tip.as_deref() != Some(tip.as_str()) {
            let tip_set = self
                .tray
                .as_ref()
                .is_some_and(|tray| tray.set_tooltip(&tip));
            if tip_set && let Some(app) = app_from(tray_hwnd) {
                app.last_tip = Some(tip);
            }
        }
        // set_tooltip 的 NIM_MODIFY 是泵点：重取引用并重算决策，
        // 防泵内到达的新状态被泵前旧值覆盖。
        let Some(app) = app_from(tray_hwnd) else {
            return;
        };
        // 图标同样只对已注册托盘下发：未注册期建图必随失败销毁，
        // 纯 GDI 空转。
        if !app.tray.as_ref().is_some_and(|tray| tray.registered) {
            return;
        }
        let failed = app.data.last_error.is_some() && app.data.snapshot.is_none();
        let used = app
            .data
            .snapshot
            .as_ref()
            .and_then(|s| s.five_hour.as_ref())
            .map(|b| b.used_percent)
            .unwrap_or(0.0);
        // 键含像素尺寸：显示缩放变化后按新尺寸重建，不沿用旧位图。
        let px = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) * 2;
        let key = (px, used.round() as i64, app.data.snapshot.is_some(), failed);
        if app.last_icon_key == Some(key) {
            return;
        }
        let new = match &app.data.snapshot {
            Some(_) => icon::ring_icon(px, used, failed),
            None if failed => icon::ring_icon(px, 0.0, true),
            None => icon::resource_icon(px).or_else(|| icon::logo_icon(px)),
        };
        let Some(new) = new else {
            return;
        };
        // 下发成功才销毁旧句柄并推进键，失败由下一次刷新重试。
        let set = app.tray.as_ref().is_some_and(|tray| tray.update_icon(new));
        if !set {
            icon::destroy_owned(new);
            return;
        }
        if let Some(app) = app_from(tray_hwnd) {
            // update_icon 的 NIM_MODIFY 是泵点：泵内嵌套刷新可能已装入
            // 更新图标并推进键，键已变即让位——销毁在用句柄、回写旧键
            // 都会吞掉新内容，只回收自建句柄。
            if app.last_icon_key != Some(key) {
                icon::destroy_owned(new);
                return;
            }
            if let Some(old) = app.tray_icon.take() {
                icon::destroy_owned(old);
            }
            app.tray_icon = Some(new);
            app.last_icon_key = Some(key);
        }
    }

    fn handle_poll_result(&mut self, msg: PollMessage) {
        // 迟到的旧世代结果直接丢弃：换源后旧账号数据不得冒充当前账号。
        if msg.generation != self.poll_gen.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        match msg.outcome {
            PollOutcome::Success(mut snap) => {
                // 快照时间戳推进才播换装动画；页脚当前是错误摘要时不播：
                // 吃豆人起点是定宽静态文案，接滚动中的文本会跳变，错误
                // 恢复直接切正常文案。
                let prev_at = self.data.snapshot.as_ref().map(|s| s.queried_at);
                let anim_ok = self.data.last_error.is_none()
                    && prev_at.is_some_and(|t| t != snap.queried_at)
                    && self.panel.mode == crate::ui::panel::PanelMode::Pinned
                    && self.panel.view == crate::ui::panel::PanelView::Main;
                let anim_texts = anim_ok.then(|| {
                    let prev = prev_at.unwrap();
                    let s = self.strings;
                    // 旧文案与静态页脚同源：动画起点必须逐字节等于上一帧。
                    let old_text = crate::ui::fmt::updated_text(s, self.lang, prev);
                    (old_text, s.updated_just_now.to_string())
                });
                self.check_notifications(&snap);
                // 附加块回填：本轮失败的余额与 Token 档沿用上一份快照。
                let prev = self.data.snapshot.take();
                crate::api::merge_sidecar(&mut snap, prev.as_ref());
                self.data.snapshot = Some(*snap);
                self.data.last_error = None;
                self.last_logged_error = None;
                if let Some((old_text, new_text)) = anim_texts
                    && old_text != new_text
                {
                    let ph = self.panel.hwnd;
                    if let (Some(p), Some(r)) = (ph, self.panel.renderer.as_mut()) {
                        // 动画开关用 renderer 的缓存判定，与 appear/spin
                        // 同一来源。
                        if r.animations_on() {
                            let (ow, nw) = unsafe {
                                (
                                    r.measure(&old_text, 12.0, 400, false),
                                    r.measure(&new_text, 12.0, 400, false),
                                )
                            };
                            r.start_footer_anim(old_text, ow, new_text, nw);
                            unsafe { crate::ui::panel::start_anim(p) };
                        }
                    }
                }
            }
            PollOutcome::Failure(e) => {
                // 文案与上次相同的失败只记一次，固定间隔重试不刷屏。
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
        // 托盘刷新异步投递：NIM_MODIFY 泵点收进 wndproc 臂，此处引用不跨泵。
        queue_tray_refresh(self.hwnd());
        if let Some(p) = self.panel.hwnd {
            relayout_panel(self, p);
        }
        // UI 可见期间跳过修剪：刚用过的页面被立即换出、下轮再缺页换回，
        // 往返纯亏；静止归还交给收起路径兜底。
        let ui_visible = self.panel.mode != crate::ui::panel::PanelMode::Hidden
            || self.popup.is_open()
            || self.about.is_open();
        if !ui_visible {
            crate::platform::trim_working_set();
        }
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

    /// 左键开关面板的唯一入口：锁定态收起、隐藏态展开。展开序列
    /// 调高度 → 弹面板 → 套外观，分支内部先复位主视图再显示，视图切换
    /// 必须在其后——顺序错则开门停在错页。只想展开不收起的调用方须先
    /// 自行确认处于 Hidden。收起路径无须重算高度
    pub(crate) fn toggle_panel(&'static mut self, tray: HWND, anchor: RECT) {
        let n = self.config.accounts.len();
        match self.panel.mode {
            // 隐藏会重入消息泵，之后不再碰引用。
            crate::ui::panel::PanelMode::Pinned => {
                if let Some(p) = self.panel.hwnd {
                    hide_panel(self, p);
                }
                return;
            }
            crate::ui::panel::PanelMode::Hidden => {
                sync_main_height(self);
                self.panel.show_from_hidden(tray, anchor, n);
            }
        }
        // 展开序列内嵌消息泵，泵后重取引用收尾。
        // 已处窗口末分钟时立即切秒拍，不等首个 tick。
        if let Some(app) = app_from(tray) {
            apply_appearance(app);
            if let Some(h) = app.panel.hwnd {
                crate::ui::panel::retune_minute(app, h);
            }
        }
    }

    /// 托盘右键菜单。文案取 &'static 不借 App：TrackPopupMenu 的嵌套泵
    /// 会派发其他消息并重取 App，泵后不得再使用进入前的引用。
    fn show_context_menu(owner: HWND, s: &'static Strings, pos: windows::Win32::Foundation::POINT) {
        unsafe {
            let settings = wide(s.settings);
            let about = wide(s.about);
            let exit = wide(s.exit);
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
            // [Shell_NotifyIcon 文档要求] 弹菜单前先激活前台，否则点菜单外不收起。
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
            // 让菜单模式正确归还激活，否则下次弹菜单可能立即失焦收不起来。
            let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        }
    }
}

/// 毒化锁取回内部数据继续用：锁只被短临界区持有，
/// 毒化意味着持锁线程已死，数据本身仍完好。
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

/// 托盘内容刷新的异步入口
/// NIM_MODIFY 泵点收进 wndproc 臂，调用方引用不跨泵
pub(crate) fn queue_tray_refresh(tray_hwnd: HWND) {
    unsafe {
        let _ = PostMessageW(
            Some(tray_hwnd),
            crate::platform::msg::WM_APP_TRAY_REFRESH,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// 托盘重注册的自愈序列：TaskbarCreated 与重试定时器两路同引。
/// NIM_ADD 与 NIM_MODIFY 都是泵点，进出各自重取引用；成功即回写注册
/// 态、清内容键并投递全量刷新——用量环立即接管，不等下一轮轮询回包。
/// 失败返回 false，重试节奏归调用方。
fn rearm_tray(hwnd: HWND) -> bool {
    let Some((icon, tip)) = app_from(hwnd).map(|app| (app.tray_icon, tooltip_text(app))) else {
        return false;
    };
    let Some(tray) = app_from(hwnd).and_then(|app| app.tray.as_ref()) else {
        return false;
    };
    let ok = icon.is_some_and(|icon| tray.readd(icon, &tip));
    if ok && let Some(tray) = app_from(hwnd).and_then(|app| app.tray.as_mut()) {
        tray.registered = true;
        if let Some(app) = app_from(hwnd) {
            app.last_tip = None;
            app.last_icon_key = None;
        }
        queue_tray_refresh(hwnd);
    }
    ok
}

/// 托盘隐藏窗口过程
extern "system" fn tray_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        crate::platform::msg::WM_APP_TRAY_REFRESH => {
            if let Some(app) = app_from(hwnd) {
                app.update_tray_icon();
            }
            LRESULT(0)
        }
        WM_APP_TRAY => {
            let app = app_from(hwnd);
            let code = tray::parse_callback(lparam);
            match code {
                // 悬停显示 tooltip 摘要，左键打开面板。
                // tray_rect 是跨进程同步调用即泵点：先拷判定，泵后重取再动面板。
                windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP => {
                    let rect = app.and_then(|app| tray_rect(app));
                    if let Some(rect) = rect
                        && let Some(app) = app_from(hwnd)
                    {
                        app.toggle_panel(hwnd, rect);
                    }
                }
                windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONUP => {
                    let ctx = app.map(|a| (a.hwnd(), a.strings));
                    if let Some((owner, s)) = ctx {
                        // 菜单窗口同属本进程，前台判定不受弹窗巡检影响：
                        // 进菜单前显式收掉，免得两件 UI 并存；close 的隐藏
                        // 泵由 helper 消费引用，泵后不续用。
                        if let Some(app) = app_from(hwnd) {
                            let _ = close_popup(app, hwnd);
                        }
                        App::show_context_menu(owner, s, tray::context_menu_pos());
                    }
                }
                // 键盘激活走 WM_CONTEXTMENU 且不带坐标（右键另有
                // RBUTTONUP 臂）；光标可能在任意处，菜单定位用图标矩形中心。
                windows::Win32::UI::WindowsAndMessaging::WM_CONTEXTMENU => {
                    if let Some(app) = app {
                        let owner = app.hwnd();
                        let s = app.strings;
                        let pos = tray_rect(app)
                            .map(|r| windows::Win32::Foundation::POINT {
                                x: (r.left + r.right) / 2,
                                y: (r.top + r.bottom) / 2,
                            })
                            .unwrap_or_else(tray::context_menu_pos);
                        // 同 RBUTTONUP：菜单前台不受巡检影响，先收弹窗；
                        // tray_rect 的跨进程同步调用是泵点，收起前重取。
                        if let Some(app) = app_from(hwnd) {
                            let _ = close_popup(app, hwnd);
                        }
                        App::show_context_menu(owner, s, pos);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        msg if msg == *tray::TASKBAR_CREATED => {
            // explorer 重启：托盘图标随任务栏进程消亡，广播到达即重注册。
            // readd 与 tray_rect 都是泵点：决策前置纯读，泵后重取。
            let pinned = app_from(hwnd)
                .is_some_and(|app| app.panel.mode == crate::ui::panel::PanelMode::Pinned);
            rearm_tray(hwnd);
            // 任务栏重建后图标可能换位，面板还开着时同步刷新锚点，
            // near_tray 保活与下次重排不按旧矩形误判。
            if pinned
                && let Some(rect) = app_from(hwnd).and_then(|app| tray_rect(app))
                && let Some(app) = app_from(hwnd)
            {
                app.panel.anchor = Some(rect);
            }
            // 补回成功撤掉还在跑的重试定时器；失败则挂起重试接管，
            // 不让图标缺席到下次 explorer 重启。
            if let Some(app) = app_from(hwnd) {
                if app.tray.as_ref().is_some_and(|t| t.registered) {
                    unsafe {
                        use windows::Win32::UI::WindowsAndMessaging::KillTimer;
                        let _ = KillTimer(Some(hwnd), TIMER_TRAY_RETRY);
                    }
                } else {
                    app.tray_retries = 0;
                    unsafe {
                        SetTimer(Some(hwnd), TIMER_TRAY_RETRY, TRAY_RETRY_MS, None);
                    }
                }
            }
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_ACTIVATE => {
            // 前台到达托盘窗（右键菜单流程）：面板冻结的跑马灯一并
            // 唤醒——激活消息只发给本窗口，面板自己收不到。
            if (wparam.0 & 0xFFFF) != 0 {
                crate::ui::panel::wake_frozen_marquee(hwnd);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_POWERBROADCAST => {
            // 唤醒即补拉：等待计时与墙钟在睡眠期间脱钩。只匹配
            // AUTOMATIC，任何恢复都广播它，用户触发的恢复还追加
            // SUSPEND，双接会双触发。
            use windows::Win32::UI::WindowsAndMessaging::PBT_APMRESUMEAUTOMATIC;
            if wparam.0 == PBT_APMRESUMEAUTOMATIC as usize
                && let Some(app) = app_from(hwnd)
                && let Some(p) = app.poller.as_ref()
            {
                p.refresh_now();
            }
            LRESULT(1)
        }
        WM_TIMER => {
            // 托盘首注册失败的短程重试；成功或超限即撤，TaskbarCreated 兜底。
            // 未知定时器 id 落回默认处理，未来新增带回调的定时器不被吞。
            if wparam.0 == TIMER_TRAY_RETRY {
                let registered = app_from(hwnd).map(|app| {
                    let registered = app.tray.as_ref().is_some_and(|t| t.registered);
                    if !registered {
                        // 本拍已用掉一次重试：计数推进在泵前完成。
                        app.tray_retries += 1;
                    }
                    registered
                });
                if let Some(registered) = registered {
                    let mut done = registered;
                    if !registered {
                        done = rearm_tray(hwnd);
                    }
                    if done || app_from(hwnd).is_some_and(|app| app.tray_retries >= TRAY_RETRY_MAX)
                    {
                        unsafe {
                            use windows::Win32::UI::WindowsAndMessaging::KillTimer;
                            let _ = KillTimer(Some(hwnd), TIMER_TRAY_RETRY);
                        }
                    }
                }
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }
        WM_APP_POLL_RESULT => {
            // 无载荷唤醒码：结果在通道里排队空取，伪造消息最多落空。
            if let Some(app) = app_from(hwnd) {
                while let Some(msg) = crate::service::poller::POLL_SLOT.take() {
                    app.handle_poll_result(msg);
                }
            }
            LRESULT(0)
        }
        WM_APP_UPDATE_RESULT => {
            if let Some(app) = app_from(hwnd) {
                app.update_checking = false;
                while let Some(r) = UPDATE_SLOT.take() {
                    app.update_status = Some(r);
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
            // 外观未显式指定时，系统主题切换即时重绘面板。
            if is_immersive_color_set(lparam)
                && let Some(app) = app_from(hwnd)
                && crate::ui::panel::theme::explicit_appearance(
                    app.config.general.appearance.as_deref(),
                )
                .is_none()
            {
                apply_appearance(app);
                invalidate_ui(app);
            }
            LRESULT(0)
        }
        WM_APP_NEWS_RESULT => {
            if let Some(app) = app_from(hwnd) {
                while let Some(result) = NEWS_SLOT.take() {
                    match result {
                        Ok(news) => {
                            app.news = Some(news);
                            // 慢网络下关于窗可能已按基础高度打开：动态到达后重排窗高。
                            refit_about(app);
                        }
                        Err(e) => {
                            crate::platform::log(&format!("[Quotify] 动态拉取失败: {e}"));
                            // 复位闸门，下次打开关于窗可重试。
                            app.news_fetched = false;
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_APP_WAKE_INSTANCE => {
            // 第二实例唤醒的语义是「弹出来」：已显示时不动作，仅隐藏时开门。
            // tray_rect 是泵点：先拷判定，泵后重取再动面板。
            let rect = app_from(hwnd)
                .filter(|app| app.panel.mode == crate::ui::panel::PanelMode::Hidden)
                .and_then(|app| tray_rect(app));
            if let Some(rect) = rect
                && let Some(app) = app_from(hwnd)
            {
                app.toggle_panel(hwnd, rect);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd = (wparam.0 & 0xFFFF) as u16;
            match cmd {
                IDM_SETTINGS => {
                    // 已在设置页时不再重进：重进会预填代理，覆盖用户清空
                    // 未应用的编辑。tray_rect 是泵点：先拷判定，泵后重取。
                    let decision = app_from(hwnd).map(|app| {
                        let showing = app.panel.mode != crate::ui::panel::PanelMode::Hidden
                            && app.panel.view == crate::ui::panel::PanelView::Settings;
                        let open_rect = (app.panel.mode == crate::ui::panel::PanelMode::Hidden)
                            .then(|| tray_rect(app))
                            .flatten();
                        (!showing, open_rect)
                    });
                    if let Some((_, rect)) = decision.filter(|(act, _)| *act) {
                        if let Some(rect) = rect
                            && let Some(app) = app_from(hwnd)
                        {
                            app.toggle_panel(hwnd, rect);
                        }
                        if let Some(app) = app_from(hwnd)
                            && let Some(p) = app.panel.hwnd
                        {
                            enter_settings(app, p);
                        }
                    }
                }
                IDM_ABOUT => {
                    if let Some(app) = app_from(hwnd) {
                        // 首次开关于窗才拉；不随启动拉，弱网下 raw 域名
                        // 慢解析会在进程内排队，拖住启动轮询、延后面板首屏。
                        if app.news.is_none() && !app.news_fetched {
                            app.news_fetched = true;
                            if !crate::platform::post::spawn_post(
                                &NEWS_SLOT,
                                hwnd,
                                WM_APP_NEWS_RESULT,
                                crate::service::whatsnew::fetch_latest,
                            ) {
                                app.news_fetched = false;
                            }
                        }
                        let h = crate::ui::panel::render::about::about_height(
                            app.news.as_deref(),
                            app.about.news_expanded,
                        );
                        // 关弹窗先于开关于窗：SW_HIDE 同步重入消息泵，
                        // 引用 move 进 helper 换泵后新引用；两浮窗不同时在场。
                        if let Some(app) = close_popup(app, hwnd) {
                            app.about.open(hwnd, h);
                        }
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

/// 托盘 tooltip 文本：应用名 + 各桶用量摘要，无数据仅应用名。
/// 桶集与面板行集同引 present_buckets，摘要与面板不再各写一份。
fn tooltip_text(app: &App) -> String {
    let Some(snap) = app.data.snapshot.as_ref() else {
        return "Quotify".to_string();
    };
    let names = [
        app.strings.tooltip_5h,
        app.strings.tooltip_weekly,
        app.strings.tooltip_mcp,
    ];
    let segs: Vec<String> = snap
        .present_buckets()
        .zip(names)
        .map(|(b, name)| format!("{name} {}", crate::ui::fmt::percent(b.used_percent())))
        .collect();
    if segs.is_empty() {
        "Quotify".to_string()
    } else {
        format!("Quotify | {}", segs.join(" | "))
    }
}

/// App 装箱后存活到进程退出，GWLP_USERDATA 指针全程有效，可转写为
/// `&'static mut`；别名安全依赖单线程消息循环。
fn app_from(hwnd: HWND) -> Option<&'static mut App> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    (p != 0).then(|| unsafe { &mut *(p as *mut App) })
}

/// 进设置页的公共入口：切视图、同步布局、预填代理、重排、关弹窗。
/// 关弹窗放最后：close 的 SW_HIDE 同步重入消息泵，之后不得再用本引用
fn enter_settings(app: &mut App, panel_hwnd: HWND) {
    app.panel.view = crate::ui::panel::PanelView::Settings;
    sync_customizing(app);
    // 代理框预填当前生效值，用户可改可清空；清空应用即直连。
    if app.panel.input.proxy.is_empty()
        && let Some(p) = app.config.general.proxy.clone()
    {
        app.panel.input.proxy = p;
    }
    relayout_panel(app, panel_hwnd);
    app.popup.close();
}

/// 面板命中处理
/// 引用按 'static 收：ClosePanel 臂经 hide_panel move 消费过泵，
/// 其余臂泵后由调用方重取。
pub fn handle_panel_hit(
    app: &'static mut App,
    hit: crate::ui::panel::render::Hit,
    panel_hwnd: HWND,
) {
    use crate::ui::panel::render::{Hit, ScopeChoice};
    match hit {
        Hit::Refresh => {
            // 主动重试开启新一轮记录，同文案失败再现时也重记日志。
            app.last_logged_error = None;
            if let Some(p) = &app.poller {
                p.refresh_now();
            }
            if let Some(r) = app.panel.renderer.as_mut() {
                r.start_spin();
                // 旋转的驱动时钟在此统一补挂：appear 收尾会撤 TIMER_ANIM，
                // 调用点无须知晓此坑。
                unsafe { crate::ui::panel::start_anim(panel_hwnd) };
            }
        }
        Hit::Settings => enter_settings(app, panel_hwnd),
        Hit::AccountSwitch => {
            if app.popup.is_open() {
                app.popup.close();
            } else if let Some(p) = app.panel.hwnd {
                app.popup.open(app.hwnd(), p, app.config.accounts.len());
            }
        }
        // 悬停徽标无点击语义。
        Hit::UsageInfo => {}

        // ── 导航 ──
        Hit::Back => {
            app.panel.key_revealed = false;
            app.panel.customizing_interval = false;
            app.panel.clear_input(panel_hwnd);
            // 表单态的返回是退回设置页，设置页的返回才是回主视图。
            app.panel.view = if app.panel.view == crate::ui::panel::PanelView::AddForm {
                crate::ui::panel::PanelView::Settings
            } else {
                crate::ui::panel::PanelView::Main
            };
            relayout_panel(app, panel_hwnd);
        }
        Hit::ClosePanel => {
            hide_panel(app, panel_hwnd);
        }

        // ── 设置 · 轮询间隔 ──
        Hit::IntervalPreset(secs) => {
            app.config.general.poll_interval_secs = secs;
            crate::app::config::save(&app.config);
            app.sync_poll_context();
            if let Some(p) = &app.poller {
                p.refresh_now();
            }
            app.panel.customizing_interval = false;
            app.panel.clear_input(panel_hwnd);
            sync_layout_effective(app, panel_hwnd);
        }
        Hit::CustomizeInterval => {
            app.panel.customizing_interval = true;
            if app.panel.input.interval.trim().is_empty() {
                prefill_interval(app);
            }
            sync_layout_effective(app, panel_hwnd);
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
            app.config.general.language = choice.to_config();
            app.lang = crate::ui::i18n::resolve_lang(app.config.general.language.as_deref());
            app.strings = app.lang.strings();
            crate::app::config::save(&app.config);
            invalidate_ui(app);
            // 语言变了 tooltip 要重建。
            queue_tray_refresh(app.hwnd());
        }
        Hit::Appearance(choice) => {
            app.config.general.appearance = choice.to_config();
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
        Hit::ApplyProxy => apply_proxy(app, panel_hwnd),

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
            app.panel.view = crate::ui::panel::PanelView::AddForm;
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
                // 仅删中当前选中才换源重拉；无关账号的删除不动轮询与快照。
                let selected_gone = app.config.selected.as_deref() == Some(removed_id.as_str());
                if selected_gone {
                    app.config.selected = app.config.accounts.first().map(|a| a.id.clone());
                }
                if let Some(r) = app.panel.renderer.as_mut() {
                    r.hits.clear();
                    r.hover = None;
                }
                if selected_gone {
                    switch_poll_source(app);
                } else {
                    crate::app::config::save(&app.config);
                }
                relayout_panel(app, panel_hwnd);
            }
        }
        Hit::PickAccount(i) => {
            if i < app.config.accounts.len() {
                select_account(app, i);
                app.panel.view = crate::ui::panel::PanelView::Main;
                // 选完光标还停在弹窗原位（面板外），重起离面计时。
                app.panel.outside_since = None;
                relayout_panel(app, panel_hwnd);
                // close 放最后：SW_HIDE 同步重入消息泵，泵后不用本引用。
                app.popup.close();
            }
        }
        Hit::AccountType(scope) => {
            app.panel.pending_team = matches!(scope, ScopeChoice::Team);
            if app.panel.pending_team {
                // 团队版仅国内站：类型切团队时平台同步回国内。
                app.panel.pending_platform = crate::api::Platform::Cn;
            }
            reconcile_fading_focus(app, panel_hwnd);
            sync_layout_effective(app, panel_hwnd);
        }
        Hit::InputName => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Name);
        }
        Hit::InputKey => {
            app.panel
                .focus_input(panel_hwnd, crate::ui::panel::InputField::Key);
        }
        // key 明暗切换；点击不夺输入焦点，输入态保持。明暗切换改变显示串
        // 宽度，光标与 IME 组合窗须按新宽度重定位。
        Hit::RevealKey => {
            app.panel.key_revealed = !app.panel.key_revealed;
            if let Some(p) = app.panel.hwnd {
                app.panel.update_caret(p, app.panel.renderer.as_ref());
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
                // 团队版仅国内站：切到国际版时类型同步回个人版。
                app.panel.pending_team = false;
            }
            reconcile_fading_focus(app, panel_hwnd);
            sync_layout_effective(app, panel_hwnd);
        }

        // ── 设置 · 配置管理与关于 ──
        // 模态两臂由窗口过程层分派以防泵后引用失效，经本函数到达即接线错误。
        Hit::ExportConfig | Hit::ImportConfig => {
            crate::platform::log("[Quotify] 模态命中误入 handle_panel_hit");
        }
        Hit::CheckUpdate => {
            if !app.update_checking {
                app.update_checking = true;
                app.update_status = None;
                if !crate::platform::post::spawn_post(
                    &UPDATE_SLOT,
                    app.hwnd(),
                    WM_APP_UPDATE_RESULT,
                    crate::service::update::check_latest,
                ) {
                    app.update_checking = false;
                }
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
        Hit::CopyDiagnostics => {
            let text = diagnostics_text(app);
            if crate::ui::panel::write_clipboard_text(&text) {
                crate::platform::notify::show(NOTIFY_TITLE, app.strings.notify_diag_copied);
            }
        }
        Hit::NewsItem(i) => {
            // 展开/收起互斥；展开即视为已读，已读标记推进到该条日期并落盘。
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
            refit_about(app);
        }
        Hit::AboutLogo => {
            // 单向彩蛋：吃掉后点击不再响应，关闭关于窗后复位。
            let about = &mut app.about;
            if about.egg.is_none() && !about.egg_eaten {
                about.egg_clicks = about.egg_clicks.saturating_add(1);
                if about.egg_clicks >= 5 {
                    about.egg_clicks = 0;
                    let anim = about
                        .wnd
                        .renderer
                        .as_ref()
                        .is_some_and(|r| r.animations_on());
                    if anim {
                        about.egg = Some(crate::ui::panel::anim::Tween::now(1500));
                        // 动画时钟可能已被 appear 收尾撤掉，彩蛋期间重挂。
                        use windows::Win32::UI::WindowsAndMessaging::SetTimer;
                        unsafe {
                            SetTimer(Some(panel_hwnd), crate::ui::float_wnd::TIMER_ANIM, 16, None);
                        }
                    } else {
                        about.egg_eaten = true;
                    }
                }
            }
        }
    }
    unsafe {
        let _ = InvalidateRect(Some(panel_hwnd), None, true);
    }
}

/// 关于窗「复制诊断信息」的拼装
/// 版本、系统、config 路径、代理、最近错误各一行。
fn diagnostics_text(app: &App) -> String {
    let os = windows_registry::LOCAL_MACHINE
        .open("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
        .map(|k| {
            let display = k.get_string("DisplayVersion").unwrap_or_default();
            let build = k.get_string("CurrentBuildNumber").unwrap_or_default();
            if build.is_empty() {
                "unknown".to_string()
            } else if display.is_empty() {
                build
            } else {
                format!("{display} (build {build})")
            }
        })
        .unwrap_or_else(|_| "unknown".into());
    [
        format!("Quotify v{}", env!("CARGO_PKG_VERSION")),
        format!("OS: Windows {os}"),
        format!("Config: {}", crate::app::config::config_path().display()),
        match &app.config.general.proxy {
            Some(p) => format!("Proxy: {p}"),
            None => "Proxy: direct".into(),
        },
        match &app.data.last_error {
            Some(e) => format!("Last error: {e}"),
            None => "Last error: none".into(),
        },
    ]
    .join("\n")
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
    app.poll_gen
        .fetch_add(1, std::sync::atomic::Ordering::Release);
    queue_tray_refresh(app.hwnd());
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

/// 焦点落在已收起的输入行上时清输入态，防孤儿光标与误写入；
/// 切换即时与过渡追平两处调用
pub(crate) fn reconcile_fading_focus(app: &mut App, panel_hwnd: HWND) {
    use crate::ui::panel::InputField;
    let fading_team = !app.panel.pending_team
        && matches!(
            app.panel.input.field,
            Some(InputField::Org) | Some(InputField::Project)
        );
    let fading_custom =
        !app.panel.customizing_interval && app.panel.input.field == Some(InputField::Interval);
    if fading_team || fading_custom {
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
    if app.panel.pending_team {
        let missing = if app.panel.input.org.trim().is_empty() {
            Some(crate::ui::panel::InputField::Org)
        } else if app.panel.input.project.trim().is_empty() {
            Some(crate::ui::panel::InputField::Project)
        } else {
            None
        };
        if let Some(field) = missing {
            crate::platform::notify::show(NOTIFY_TITLE, app.strings.notify_team_required);
            app.panel.focus_input(panel_hwnd, field);
            return;
        }
    }
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
    app.panel.view = crate::ui::panel::PanelView::Settings;
    app.panel.key_revealed = false;
    app.panel.clear_input(panel_hwnd);
    relayout_panel(app, panel_hwnd);
}

/// 自定义间隔文本（分钟）→ 秒：0/非数字/溢出拒绝，越界夹进合法区间
fn parse_interval_secs(s: &str) -> Option<u64> {
    s.trim()
        .parse::<u64>()
        .ok()
        .filter(|m| *m > 0)
        .and_then(|m| m.checked_mul(60))
        .map(|s| s.clamp(MIN_POLL_SECS, MAX_POLL_SECS))
}

/// 应用自定义轮询间隔（分钟）
fn apply_interval(app: &mut App, panel_hwnd: HWND) {
    let Some(secs) = parse_interval_secs(&app.panel.input.interval) else {
        crate::platform::log("[Quotify] 自定义间隔无效");
        return;
    };
    app.config.general.poll_interval_secs = secs;
    crate::app::config::save(&app.config);
    app.sync_poll_context();
    if let Some(p) = &app.poller {
        p.refresh_now();
    }
    sync_customizing(app);
    app.panel.clear_input(panel_hwnd);
    sync_layout_effective(app, panel_hwnd);
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
        app.popup.wnd.hwnd.filter(|_| app.popup.is_open()),
        app.about.wnd.hwnd.filter(|_| app.about.is_open()),
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
    let Some(a) = app.about.wnd.hwnd.filter(|_| app.about.is_open()) else {
        return;
    };
    let h =
        crate::ui::panel::render::about::about_height(app.news.as_deref(), app.about.news_expanded);
    unsafe {
        let mi = crate::ui::panel::monitor_info_of(a);
        let w = (crate::ui::panel::render::about::ABOUT_W * app.about.wnd.dpi).round() as i32;
        let hgt = (h as f32 * app.about.wnd.dpi).round() as i32;
        let x = mi.rcWork.left + (mi.rcWork.right - mi.rcWork.left - w) / 2;
        // 矮屏窗高超出工作区时保顶弃底，头部信息不裁。
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
    let next = if raw.is_empty() { None } else { Some(raw) };
    // 校验先行：无效地址不落盘——坏值一旦持久化，此后每次启动都
    // 静默降级直连；失败保留输入框现场供修改。
    if let Some(p) = next.as_deref()
        && let Err(e) = crate::api::client::proxy_valid(p)
    {
        crate::platform::log(&format!("[Quotify] 代理地址无效，未保存: {e}"));
        crate::platform::notify::show(NOTIFY_TITLE, app.strings.notify_proxy_invalid);
        return;
    }
    if let Err(e) = crate::api::client::set_proxy(next.clone()) {
        crate::platform::log(&format!("[Quotify] 代理设置失败，未保存: {e}"));
        crate::platform::notify::show(NOTIFY_TITLE, app.strings.notify_proxy_invalid);
        return;
    }
    app.config.general.proxy = next;
    crate::app::config::save(&app.config);
    app.panel.clear_input(panel_hwnd);
    // 换代理后的拉取是新尝试轮次，同文案失败再现时也重记日志。
    app.last_logged_error = None;
    if let Some(p) = &app.poller {
        p.refresh_now();
    }
    relayout_panel(app, panel_hwnd);
}

/// 模态期间的巡检冻结：面板在弹框背后离面属正常，巡检须停防误收；
/// Drop 恢复时钟，早退不漏挂。
struct ModalGuard(HWND);

impl ModalGuard {
    fn new(panel: HWND) -> Self {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::KillTimer;
            let _ = KillTimer(Some(panel), crate::ui::panel::TIMER_OUTSIDE_CHECK);
        }
        Self(panel)
    }
}

impl Drop for ModalGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = SetTimer(
                Some(self.0),
                crate::ui::panel::TIMER_OUTSIDE_CHECK,
                200,
                None,
            );
        }
    }
}

/// 警告级 OK/Cancel 确认框，取消返 false，导出与导入共用；只收
/// owner 与文案不借 App——嵌套泵返回后调用方引用已失效。
fn confirm_box(owner: HWND, title: &str, body: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW};
    let title = wide(title);
    let body = wide(body);
    let r = unsafe {
        MessageBoxW(
            Some(owner),
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OKCANCEL | MB_ICONWARNING,
        )
    };
    r == IDOK
}

/// 模态类命中的直调分派：此类命中进 handle_panel_hit 会因嵌套泵
/// 引用失效，由分派层按 is_modal 拦截转此。新增模态变体时与
/// is_modal 同步登记，误入仅留日志。
pub fn dispatch_modal(hit: crate::ui::panel::render::Hit, panel_hwnd: HWND) {
    match hit {
        crate::ui::panel::render::Hit::ExportConfig => export_config(panel_hwnd),
        crate::ui::panel::render::Hit::ImportConfig => import_config(panel_hwnd),
        _ => crate::platform::log("[Quotify] 非模态命中误入 dispatch_modal"),
    }
}

/// 命中分派路由：模态类拦截直调两段式并返回 true，其余重取引用
/// 分派返回 false。面板与浮窗两处分派层同引，模态拦截的同步只剩
/// 此一处消费。
pub(crate) fn dispatch_hit(hit: crate::ui::panel::render::Hit, owner: HWND) -> bool {
    if hit.is_modal() {
        dispatch_modal(hit, owner);
        return true;
    }
    if let Some(app) = crate::ui::panel::app_from_tray(owner) {
        handle_panel_hit(app, hit, owner);
    }
    false
}

/// 导出配置为明文 JSON。两段式取引用：模态对话框跑嵌套消息泵，泵内
/// 其他消息会重取 App 使本函数前段的引用失效——模态前只取 &'static
/// 数据，模态后重取引用落盘。
pub fn export_config(panel_hwnd: HWND) {
    let (owner, title, body) = {
        let Some(app) = crate::ui::panel::app_from_tray(panel_hwnd) else {
            return;
        };
        (
            app.hwnd(),
            app.strings.export_confirm_title,
            app.strings.export_confirm_body,
        )
    };
    // 风险知情先于选位置：敏感内容的告知应前置。
    // 取消则连保存对话框都不出，静默中止导出、无 toast。
    let picked = {
        let _modal = ModalGuard::new(panel_hwnd);
        confirm_box(owner, title, body)
            .then(|| crate::platform::save_dialog("quotify-config.json"))
            .flatten()
    };
    let Some(path) = picked else {
        return;
    };
    let Some(app) = crate::ui::panel::app_from_tray(panel_hwnd) else {
        return;
    };
    let body = match serde_json::to_string_pretty(&app.config)
        .map_err(|e| e.to_string())
        .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()))
    {
        Ok(()) => {
            // 导出文件同为明文 key 落盘，与 config 同标准收紧。
            crate::platform::secure_file_acl(&path);
            app.strings.export_done.to_string()
        }
        Err(e) => {
            crate::platform::log(&format!("[Quotify] 导出失败: {e}"));
            app.strings.export_failed.to_string()
        }
    };
    crate::platform::notify::show(NOTIFY_TITLE, &body);
}

/// 导入配置 JSON，两段式同 export_config：模态段不持 App 引用，应用段重取。
pub fn import_config(panel_hwnd: HWND) {
    let picked = {
        let _modal = ModalGuard::new(panel_hwnd);
        crate::platform::open_dialog()
    };
    let Some(path) = picked else {
        return;
    };
    // 外部文件不可信，先限大小防超大文件撑内存。
    // 三类失败分别落日志：超限、读取事故、坏文件；用户 toast 统一为导入失败。
    let parsed = match std::fs::metadata(&path) {
        Ok(m) if m.len() > IMPORT_MAX_BYTES => {
            crate::platform::log(&format!(
                "[Quotify] 配置导入放弃：文件 {} 字节超上限",
                m.len()
            ));
            None
        }
        meta => {
            let _ = meta; // Err(metadata) 随读取一并报错。
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
    let Some(cfg) = parsed else {
        crate::platform::log("[Quotify] 配置导入失败：文件不可读或格式无效");
        let Some(app) = crate::ui::panel::app_from_tray(panel_hwnd) else {
            return;
        };
        crate::platform::notify::show(NOTIFY_TITLE, app.strings.import_failed);
        return;
    };
    // 导入会清掉现有账号时先确认。
    // 现状判定与文案一次取用拷出，确认框模态泵返回后重取。
    if cfg.accounts.is_empty() {
        let confirm = {
            let Some(app) = crate::ui::panel::app_from_tray(panel_hwnd) else {
                return;
            };
            (!app.config.accounts.is_empty()).then(|| {
                (
                    app.hwnd(),
                    app.strings.import_confirm_title,
                    app.strings.import_confirm_body,
                )
            })
        };
        if let Some((owner, title, body)) = confirm {
            let _modal = ModalGuard::new(panel_hwnd);
            if !confirm_box(owner, title, body) {
                return;
            }
        }
    }
    let Some(app) = crate::ui::panel::app_from_tray(panel_hwnd) else {
        return;
    };
    app.config = cfg;
    // 外部文件的值域不可信，归一化与启动加载保持一致。
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
    app.panel.key_revealed = false;
    app.panel.clear_input(panel_hwnd);
    app.update_status = None;
    app.update_checking = false;
    sync_customizing(app);
    relayout_panel(app, panel_hwnd);
    crate::platform::notify::show(NOTIFY_TITLE, app.strings.import_done);
}

/// 非预设间隔展开自定义行并预填；预设则收起
fn sync_customizing(app: &mut App) {
    let cur = app.config.general.poll_interval_secs;
    let is_preset = INTERVAL_PRESETS.contains(&cur);
    app.panel.customizing_interval = !is_preset;
    // 缓冲非空说明用户正在输入，预填会覆盖未应用的编辑。
    if !is_preset && app.panel.input.interval.is_empty() {
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
            // key 必填，名称可留空。
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
    crate::ui::panel::theme::resolved(setting)
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
    // 托盘菜单由系统绘制，须在进程级单独设模式，此后弹出的菜单才带上主题。
    crate::platform::menu_theme::apply(matches!(
        appearance,
        crate::ui::panel::theme::Appearance::Dark
    ));
    if let Some(r) = app.panel.renderer.as_mut() {
        r.theme = crate::ui::panel::theme::Theme::new(appearance);
    }
    for r in [&mut app.popup.wnd.renderer, &mut app.about.wnd.renderer]
        .into_iter()
        .flatten()
    {
        r.theme = crate::ui::panel::theme::Theme::new(appearance);
    }
}

fn sync_main_height(app: &mut App) {
    app.panel.main_h = main_height_of(app.data.snapshot.as_ref());
}

/// 主视图高度的派生单源：sync_main_height 的高度计算同引
pub(crate) fn main_height_of(snap: Option<&UsageSnapshot>) -> i32 {
    let (rows, comp, stats, bal) = crate::api::main_features(snap);
    crate::ui::panel::layout::main_view_height(snap.is_some(), rows, comp, stats, bal)
}

/// 生效布局切换：展开立即生效渐扩揭露；收缩保持出发布局收窗渐裁，结束追平。
fn sync_layout_effective(app: &mut App, panel_hwnd: HWND) {
    app.panel.height_anim = None;
    let target_h = app.panel.view_height_for(
        app.panel.pending_team,
        app.panel.customizing_interval,
        app.config.accounts.len(),
    );
    if app.panel.begin_shrink_anim(panel_hwnd, target_h) {
        unsafe {
            let _ = InvalidateRect(Some(panel_hwnd), None, true);
        }
        return;
    }
    relayout_panel(app, panel_hwnd);
}

/// 已开弹窗随面板几何重对位；由面板的 WM_WINDOWPOSCHANGED 统一驱动
pub(crate) fn reposition_popup(app: &mut App, panel_hwnd: HWND) {
    app.popup
        .reposition(app.hwnd(), panel_hwnd, app.config.accounts.len());
}

/// 收起面板并连带关弹窗的统一入口，漏走此处的收起只剩巡检兜底。
/// 两步各含隐藏泵点：弹窗段由 close_popup 组合，引用 move 消费换泵
/// 后新引用——begin_hide 尾部的 SW_HIDE 又是一轮嵌套泵，此后再取的
/// 引用一律不可外传，续用方自行 app_from 重取。
pub(crate) fn hide_panel(app: &'static mut App, panel_hwnd: HWND) {
    let tray = app.hwnd();
    let mut app = close_popup(app, tray);
    if let Some(app) = &mut app {
        app.panel.begin_hide(panel_hwnd);
    }
}

/// 关闭账号弹窗并换回泵后新引用：close 的 SW_HIDE 同步重入消息泵，
/// 调用方持有的 App 引用经此 move 消费——泵后误用旧引用是编译错误。
/// 弹窗未开时 close 幂等，等价一次重取。重取失败返 None，续用方
/// 按空路径早退。
pub(crate) fn close_popup(app: &'static mut App, hwnd: HWND) -> Option<&'static mut App> {
    app.popup.close();
    app_from(hwnd)
}

/// 同步布局状态并重定位面板
pub(crate) fn relayout_panel(app: &mut App, panel_hwnd: HWND) {
    // 生效布局默认随重排追平；同视图收缩过渡例外——出发布局归动画
    // 独占，跨视图过渡终止后照常追平。
    let frozen = app
        .panel
        .height_anim
        .is_some_and(|a| a.view == app.panel.view);
    if !frozen {
        app.panel.height_anim = None;
        app.panel.layout_team = app.panel.pending_team;
        app.panel.layout_customizing = app.panel.customizing_interval;
        reconcile_fading_focus(app, panel_hwnd);
    }
    sync_main_height(app);
    app.panel.account_error = app
        .data
        .last_error
        .as_ref()
        .is_some_and(|e| e.category() == crate::api::ErrorCategory::Unauthenticated);
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
        // 配置的代理在首次请求前生效。地址无效仅记录，不阻断启动。
        if let Err(e) = crate::api::client::set_proxy(config.general.proxy.clone()) {
            crate::platform::log(&format!("[Quotify] 代理地址无效，保持直连: {e}"));
        }
        let app = Box::new(App::new(config));
        let mut app = app;
        // 启动即套外观，未开面板前的首次右键菜单也带主题。
        // 此刻 renderer 均未建立，无副作用。
        apply_appearance(&mut app);

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

        let px = GetSystemMetrics(SM_CXSMICON).max(16) * 2;
        let initial = icon::resource_icon(px).or_else(|| icon::logo_icon(px));
        if initial.is_none() {
            crate::platform::log(&format!("[Quotify] 初始图标加载失败 (px={px})"));
        }
        let initial = initial.unwrap_or_default();
        app.tray_icon = Some(initial);
        app.tray = Some(TrayIcon::new(hwnd, initial));
        // 开机自启可能撞上任务栏未就绪，失败挂短程重试直至图标就位。
        if !app.tray.as_ref().is_some_and(|t| t.registered) {
            app.tray_retries = 0;
            SetTimer(Some(hwnd), TIMER_TRAY_RETRY, TRAY_RETRY_MS, None);
        }

        app.sync_poll_context();
        app.poller = Poller::spawn(
            hwnd,
            app.poll_target.clone(),
            app.poll_interval.clone(),
            app.poll_gen.clone(),
        );
        if let Some(p) = &app.poller {
            p.refresh_now();
        }

        // 诊断后门，仅 debug 生效：QUOTIFY_DIAG=adding 自动弹设置-添加账号页。
        #[cfg(debug_assertions)]
        if std::env::var("QUOTIFY_DIAG").as_deref() == Ok("adding") {
            let rect = tray_rect(&app).unwrap_or(RECT {
                left: 1300,
                top: 880,
                right: 1340,
                bottom: 920,
            });
            // move 进 toggle_panel 须用重取引用，外层引用留给退出清理。
            if let Some(app) = app_from(hwnd) {
                app.toggle_panel(hwnd, rect);
            }
            if let Some(app) = app_from(hwnd) {
                app.panel.view = crate::ui::panel::PanelView::AddForm;
                app.panel.pending_platform = crate::api::Platform::Cn;
                if let Some(p) = app.panel.hwnd {
                    relayout_panel(app, p);
                    if let Some(app) = app_from(hwnd) {
                        app.panel.focus_input(p, crate::ui::panel::InputField::Name);
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_minutes_parse_to_secs() {
        assert_eq!(parse_interval_secs("5"), Some(300));
        // trim 后再解析
        assert_eq!(parse_interval_secs("  10 "), Some(600));
    }

    #[test]
    fn interval_rejects_zero_and_non_numeric() {
        assert_eq!(parse_interval_secs("0"), None);
        assert_eq!(parse_interval_secs(""), None);
        assert_eq!(parse_interval_secs("abc"), None);
        // 间隔粒度是整分钟，小数不收
        assert_eq!(parse_interval_secs("1.5"), None);
    }

    #[test]
    fn interval_overflow_rejected() {
        assert_eq!(parse_interval_secs(&u64::MAX.to_string()), None);
    }

    #[test]
    fn interval_above_cap_clamped() {
        // 1441 分钟 = 86460 秒，超一天上限
        assert_eq!(parse_interval_secs("1441"), Some(MAX_POLL_SECS));
    }

    #[test]
    fn normalize_clamps_out_of_range() {
        let mut c = Config::default();
        c.general.notify_threshold_percent = 0;
        c.general.poll_interval_secs = 0;
        normalize_config(&mut c);
        assert_eq!(c.general.notify_threshold_percent, 1);
        assert_eq!(c.general.poll_interval_secs, MIN_POLL_SECS);

        c.general.notify_threshold_percent = 200;
        c.general.poll_interval_secs = u64::MAX;
        normalize_config(&mut c);
        assert_eq!(c.general.notify_threshold_percent, 100);
        assert_eq!(c.general.poll_interval_secs, MAX_POLL_SECS);
    }

    #[test]
    fn normalize_keeps_valid_values() {
        let mut c = Config::default();
        c.general.notify_threshold_percent = 80;
        c.general.poll_interval_secs = 300;
        normalize_config(&mut c);
        assert_eq!(c.general.notify_threshold_percent, 80);
        assert_eq!(c.general.poll_interval_secs, 300);
    }

    #[test]
    fn appearance_resolved_from_setting() {
        use crate::ui::panel::theme::{Appearance, Theme};
        assert_eq!(resolved_appearance(Some("light")), Appearance::Light);
        // 大小写不敏感
        assert_eq!(resolved_appearance(Some("DARK")), Appearance::Dark);
        // 未知值与未设置同为跟随系统
        assert_eq!(
            resolved_appearance(Some("blue")),
            Theme::system_appearance()
        );
        assert_eq!(resolved_appearance(None), Theme::system_appearance());
    }
}
