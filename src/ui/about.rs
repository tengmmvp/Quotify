//! 关于窗口

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetCursorPos, GetForegroundWindow, IDC_ARROW,
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
use crate::ui::panel::render::about as view;
use crate::ui::panel::{dpi_of, track_leave};
use crate::ui::{x_of, y_of};

const ABOUT_WND_CLASS: &str = "QuotifyAbout";

/// 关闭巡检：前台被夺走即收起
const TIMER_SWEEP: usize = 1;
/// 弹出动画帧时钟
const TIMER_ANIM: usize = 2;

/// 关于窗口
pub struct AboutWindow {
    pub hwnd: Option<HWND>,
    pub renderer: Option<Renderer>,
    pub news_expanded: Option<usize>,
    pub(crate) dpi: f32,
    class_registered: bool,
}

impl AboutWindow {
    pub fn new() -> Self {
        Self {
            hwnd: None,
            renderer: None,
            news_expanded: None,
            dpi: crate::ui::panel::FALLBACK_DPI,
            class_registered: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.hwnd
            .is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() })
    }

    /// 光标所在显示器的工作区居中弹出；`logical_h` 由调用方按动态条目数算好
    pub fn open(&mut self, tray: HWND, logical_h: i32) {
        let Some(h) = self.ensure_window(tray) else {
            return;
        };
        unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            self.dpi = dpi_of(monitor).unwrap_or(crate::ui::panel::FALLBACK_DPI);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut mi);
            let w = (view::ABOUT_W * self.dpi).round() as i32;
            let hgt = (logical_h as f32 * self.dpi).round() as i32;
            let x = mi.rcWork.left + (mi.rcWork.right - mi.rcWork.left - w) / 2;
            let y =
                (mi.rcWork.top + (mi.rcWork.bottom - mi.rcWork.top - hgt) / 2).max(mi.rcWork.top);

            let _ = SetWindowPos(h, None, x, y, w, hgt, SWP_NOZORDER);
            if let Some(r) = self.renderer.as_mut() {
                r.hover = None;
                r.anim.appear = Some(crate::ui::panel::anim::Tween::now(180));
            }
            let _ = ShowWindow(h, SW_SHOW);
            let _ = SetForegroundWindow(h);
            SetTimer(Some(h), TIMER_SWEEP, 200, None);
            SetTimer(Some(h), TIMER_ANIM, 16, None);
            let _ = InvalidateRect(Some(h), None, false);
        }
    }

    pub fn close(&mut self) {
        // 收起即放弃：展开态一并复位，重开从折叠档起步
        self.news_expanded = None;
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
                let name = wide(ABOUT_WND_CLASS);
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(about_wndproc),
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
            let name = wide(ABOUT_WND_CLASS);
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
                crate::platform::log(&format!("[Quotify] 关于窗创建失败: {e}"));
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

/// 关于窗的窗口过程
pub extern "system" fn about_wndproc(
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
                    let cached = app.about.renderer.take();
                    let fresh = cached.is_none();
                    let mut renderer = cached.or_else(|| Renderer::new(hwnd, &rect, app.about.dpi));
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
                        let expanded = app.about.news_expanded;
                        let dpi = app.about.dpi;
                        keep = r.paint_about(hwnd, &rect, &model, expanded, dpi);
                    }
                    if keep {
                        app.about.renderer = renderer;
                    } else {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => match wparam.0 {
                TIMER_SWEEP => {
                    let app = app_from_tray(hwnd);
                    let stale = app
                        .as_ref()
                        .map(|_| GetForegroundWindow() != hwnd)
                        .unwrap_or(true);
                    if stale && let Some(app) = app {
                        app.about.close();
                    }
                    LRESULT(0)
                }
                TIMER_ANIM => {
                    let app = app_from_tray(hwnd);
                    let done = app
                        .and_then(|a| a.about.renderer.as_ref())
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
                    let (x, y) = (x_of(lparam) / app.about.dpi, y_of(lparam) / app.about.dpi);
                    if let Some(r) = app.about.renderer.as_mut() {
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
                    && let Some(r) = app.about.renderer.as_mut()
                    && r.hover.take().is_some()
                {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if let Some(app) = app_from_tray(hwnd) {
                    let (x, y) = (x_of(lparam) / app.about.dpi, y_of(lparam) / app.about.dpi);
                    let hit = app.about.renderer.as_ref().and_then(|r| r.hit_at(x, y));
                    // 命中分派统一走面板的 handle_panel_hit；尾部 InvalidateRect
                    // 重绘的是关于窗自身
                    if let Some(hit) = hit {
                        crate::app::handle_panel_hit(app, hit, hwnd);
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
                        .and_then(|a| a.about.renderer.as_ref())
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
