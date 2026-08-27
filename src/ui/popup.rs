//! 账号切换弹窗

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetForegroundWindow, GetWindowRect, IDC_ARROW,
    IDC_HAND, IsWindowVisible, KillTimer, LoadCursorW, RegisterClassW, SW_HIDE, SW_SHOW,
    SWP_NOZORDER, SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    WM_ERASEBKGND, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_SETCURSOR, WM_TIMER, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::PCWSTR;

use crate::platform::wide;
use crate::ui::panel::app_from_tray;
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::render::Renderer;
use crate::ui::panel::render::popup as view;
use crate::ui::panel::{PanelMode, dpi_of, track_leave};
use crate::ui::{x_of, y_of};

const POPUP_WND_CLASS: &str = "QuotifyAccountPopup";

/// 关闭巡检：前台被夺走或面板已收起即收起弹窗
const TIMER_SWEEP: usize = 1;
/// 弹出动画帧时钟
const TIMER_ANIM: usize = 2;

/// 账号切换弹窗
pub struct AccountPopup {
    pub hwnd: Option<HWND>,
    pub renderer: Option<Renderer>,
    pub(crate) dpi: f32,
    class_registered: bool,
}

impl AccountPopup {
    pub fn new() -> Self {
        Self {
            hwnd: None,
            renderer: None,
            dpi: crate::ui::panel::FALLBACK_DPI,
            class_registered: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.hwnd
            .is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() })
    }

    /// 面板右侧弹出；越出工作区右界改放左侧，纵向以面板顶对齐并夹回工作区
    pub fn open(&mut self, tray: HWND, panel_hwnd: HWND, accounts: usize) {
        let Some(h) = self.ensure_window(tray) else {
            return;
        };
        unsafe {
            let monitor = MonitorFromWindow(panel_hwnd, MONITOR_DEFAULTTONEAREST);
            self.dpi = dpi_of(monitor).unwrap_or(crate::ui::panel::FALLBACK_DPI);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut mi);
            let mut pr = RECT::default();
            let _ = GetWindowRect(panel_hwnd, &mut pr);

            let w = (view::POPUP_W * self.dpi).round() as i32;
            let hgt = (view::popup_height(accounts) as f32 * self.dpi).round() as i32;
            let gap = (8.0 * self.dpi).round() as i32;
            let mut x = pr.right + gap;
            if x + w > mi.rcWork.right {
                x = (pr.left - w - gap).max(mi.rcWork.left);
            }
            let y = pr
                .top
                .clamp(mi.rcWork.top, (mi.rcWork.bottom - hgt).max(mi.rcWork.top));

            let _ = SetWindowPos(h, None, x, y, w, hgt, SWP_NOZORDER);
            if let Some(r) = self.renderer.as_mut() {
                r.hover = None;
                r.anim.appear = Some(crate::ui::panel::anim::Tween::now(180));
            }
            let _ = ShowWindow(h, SW_SHOW);
            // 弹窗自身必须成为前台：点击外部才构成关闭信号（同托盘菜单的前台要求）
            let _ = SetForegroundWindow(h);
            SetTimer(Some(h), TIMER_SWEEP, 200, None);
            SetTimer(Some(h), TIMER_ANIM, 16, None);
            let _ = InvalidateRect(Some(h), None, false);
        }
    }

    pub fn close(&mut self) {
        if let Some(h) = self.hwnd {
            unsafe {
                let _ = ShowWindow(h, SW_HIDE);
                let _ = KillTimer(Some(h), TIMER_SWEEP);
                let _ = KillTimer(Some(h), TIMER_ANIM);
            }
        }
    }

    fn ensure_window(&mut self, parent: HWND) -> Option<HWND> {
        if let Some(h) = self.hwnd {
            return Some(h);
        }
        unsafe {
            if !self.class_registered {
                let hinst = GetModuleHandleW(None).ok()?;
                let name = wide(POPUP_WND_CLASS);
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(popup_wndproc),
                    hInstance: hinst.into(),
                    lpszClassName: PCWSTR(name.as_ptr()),
                    hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(std::ptr::null_mut()),
                    hIcon: crate::ui::icon::app_icon(hinst.into())?,
                    ..Default::default()
                };
                if RegisterClassW(&wc) == 0 {
                    return None;
                }
                self.class_registered = true;
            }
            let hinst = GetModuleHandleW(None).ok()?;
            let name = wide(POPUP_WND_CLASS);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                PCWSTR(name.as_ptr()),
                PCWSTR(name.as_ptr()),
                WS_POPUP,
                0,
                0,
                0,
                0,
                Some(parent),
                None,
                Some(hinst.into()),
                None,
            )
            .unwrap_or_else(|e| {
                crate::platform::log(&format!("[Quotify] 账号弹窗创建失败: {e}"));
                HWND::default()
            });
            if hwnd.is_invalid() {
                return None;
            }
            let pref = windows::Win32::Graphics::Dwm::DWMWCP_DEFAULT;
            let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE>()
                    as u32,
            );
            self.hwnd = Some(hwnd);
            Some(hwnd)
        }
    }
}

/// 弹窗的窗口过程
pub extern "system" fn popup_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                // 不走 BeginPaint：验证客户区后直接渲染
                let _ = ValidateRect(Some(hwnd), None);
                if let Some(app) = app_from_tray(hwnd) {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let cached = app.popup.renderer.take();
                    let fresh = cached.is_none();
                    let mut renderer = cached.or_else(|| Renderer::new(hwnd, &rect, app.popup.dpi));
                    let mut keep = true;
                    if let Some(r) = renderer.as_mut() {
                        if fresh {
                            r.theme = crate::ui::panel::theme::Theme::new(
                                crate::app::resolved_appearance(
                                    app.config.general.appearance.as_deref(),
                                ),
                            );
                            // 首开时 renderer 尚未建成，open 里的淡入没设上，这里补
                            r.anim.appear = Some(crate::ui::panel::anim::Tween::now(180));
                        }
                        let model = PanelModel::from_app(app);
                        let dpi = app.popup.dpi;
                        keep = r.paint_popup(hwnd, &rect, &model, dpi);
                    }
                    if keep {
                        app.popup.renderer = renderer;
                    } else {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => match wparam.0 {
                TIMER_SWEEP => {
                    let app = app_from_tray(hwnd);
                    let stale = match app.as_ref() {
                        // 前台被夺走或面板已收起：弹窗失锚，收起
                        Some(app) => {
                            GetForegroundWindow() != hwnd || app.panel.mode == PanelMode::Hidden
                        }
                        None => true,
                    };
                    if stale && let Some(app) = app {
                        app.popup.close();
                    }
                    LRESULT(0)
                }
                TIMER_ANIM => {
                    let app = app_from_tray(hwnd);
                    let done = app
                        .and_then(|a| a.popup.renderer.as_ref())
                        .and_then(|r| r.anim.appear.as_ref().map(|t| t.finished()))
                        .unwrap_or(true);
                    if done {
                        let _ = KillTimer(Some(hwnd), TIMER_ANIM);
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            },
            WM_MOUSEMOVE => {
                // 每次移动重挂 TME_LEAVE：一次性通知，靠反复挂载续命
                track_leave(hwnd);
                if let Some(app) = app_from_tray(hwnd) {
                    let (x, y) = (x_of(lparam) / app.popup.dpi, y_of(lparam) / app.popup.dpi);
                    if let Some(r) = app.popup.renderer.as_mut() {
                        let hit = r.hit_at(x, y);
                        if r.hover != hit {
                            r.hover = hit;
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                // 离窗清除残留高亮
                if let Some(app) = app_from_tray(hwnd)
                    && let Some(r) = app.popup.renderer.as_mut()
                    && r.hover.take().is_some()
                {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if let Some(app) = app_from_tray(hwnd) {
                    let (x, y) = (x_of(lparam) / app.popup.dpi, y_of(lparam) / app.popup.dpi);
                    let hit = app.popup.renderer.as_ref().and_then(|r| r.hit_at(x, y));
                    // 命中分派统一走面板的 handle_panel_hit：PickAccount 臂
                    // 内含选账号、收弹窗与面板重排
                    if let Some(hit) = hit
                        && let Some(p) = app.panel.hwnd
                    {
                        crate::app::handle_panel_hit(app, hit, p);
                    }
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SETCURSOR => {
                let hit_hwnd = HWND(wparam.0 as *mut _);
                if hit_hwnd == hwnd
                    && (lparam.0 & 0xFFFF) as u32
                        == windows::Win32::UI::WindowsAndMessaging::HTCLIENT
                {
                    let hand = app_from_tray(hwnd)
                        .and_then(|a| a.popup.renderer.as_ref())
                        .map(|r| r.hover.is_some())
                        .unwrap_or(false);
                    let cursor = if hand { IDC_HAND } else { IDC_ARROW };
                    if let Ok(c) = LoadCursorW(None, cursor) {
                        let _ = SetCursor(Some(c));
                        return LRESULT(1);
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
