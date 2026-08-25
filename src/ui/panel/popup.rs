//! 账号切换弹窗：独立顶层窗口，贴面板侧边弹出

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HBRUSH, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromWindow, ValidateRect,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use super::render::Renderer;
use crate::platform::wide;

const POPUP_WND_CLASS: &str = "QuotifyAccountPopup";

const POPUP_TIMER_ANIM: usize = 1;
const POPUP_TIMER_OUTSIDE: usize = 2;

/// 弹窗宽度（逻辑像素）
const POPUP_W: i32 = 220;
/// 行高（逻辑像素）
const ROW_H: f32 = 36.0;
/// 首行距顶（逻辑像素）
const ROW_TOP: f32 = 6.0;

pub struct AccountPopup {
    pub hwnd: Option<HWND>,
    pub renderer: Option<Renderer>,
    pub dpi: f32,
    pub hover: Option<usize>,
    class_registered: bool,
}

impl AccountPopup {
    pub fn new() -> Self {
        Self {
            hwnd: None,
            renderer: None,
            dpi: super::FALLBACK_DPI,
            hover: None,
            class_registered: false,
        }
    }

    fn px(&self, logical: i32) -> i32 {
        (logical as f32 * self.dpi).round() as i32
    }

    pub fn is_open(&self) -> bool {
        self.hwnd
            .is_some_and(|h| unsafe { IsWindowVisible(h) }.as_bool())
    }

    /// 已开则关，未开则在面板右侧弹出
    pub fn toggle(&mut self, parent: HWND, panel_hwnd: HWND, rows: usize) {
        if self.is_open() {
            self.close();
            return;
        }
        let Some(hwnd) = self.ensure_window(parent) else {
            return;
        };
        self.place(hwnd, panel_hwnd, rows);
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            if let Some(r) = self.renderer.as_mut() {
                r.anim.appear = Some(super::anim::Tween::now(180));
            }
            SetTimer(Some(hwnd), POPUP_TIMER_ANIM, 16, None);
            SetTimer(Some(hwnd), POPUP_TIMER_OUTSIDE, 400, None);
            let _ = InvalidateRect(Some(hwnd), None, true);
        }
    }

    pub fn close(&mut self) {
        if let Some(h) = self.hwnd {
            unsafe {
                let _ = KillTimer(Some(h), POPUP_TIMER_ANIM);
                let _ = KillTimer(Some(h), POPUP_TIMER_OUTSIDE);
                let _ = ShowWindow(h, SW_HIDE);
            }
        }
        self.hover = None;
    }

    fn ensure_window(&mut self, parent: HWND) -> Option<HWND> {
        if let Some(h) = self.hwnd {
            return Some(h);
        }
        unsafe {
            if !self.class_registered {
                let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok()?;
                let name = wide(POPUP_WND_CLASS);
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(popup_wndproc),
                    hInstance: hinst.into(),
                    lpszClassName: PCWSTR(name.as_ptr()),
                    hbrBackground: HBRUSH(std::ptr::null_mut()),
                    ..Default::default()
                };
                if RegisterClassW(&wc) == 0 {
                    return None;
                }
                self.class_registered = true;
            }
            let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok()?;
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
            .ok()?;
            let pref = windows::Win32::Graphics::Dwm::DWMWCP_DEFAULT;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE>()
                    as u32,
            );
            // 禁用 DWM 位置过渡，弹出瞬时呈现由自绘动画接管
            let disable: i32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWA_TRANSITIONS_FORCEDISABLED,
                &disable as *const i32 as *const core::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            );
            self.hwnd = Some(hwnd);
            Some(hwnd)
        }
    }

    /// 贴面板右侧定位，越右界转左侧，工作区夹取
    fn place(&mut self, hwnd: HWND, panel_hwnd: HWND, rows: usize) {
        unsafe {
            let monitor = MonitorFromWindow(panel_hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut mi);
            self.dpi = super::dpi_of(monitor).unwrap_or(super::FALLBACK_DPI);

            let mut pr = RECT::default();
            let _ = GetWindowRect(panel_hwnd, &mut pr);
            let w = self.px(POPUP_W);
            let h = self.px((ROW_TOP * 2.0 + rows as f32 * ROW_H) as i32);
            let gap = self.px(8);
            let mut x = pr.right + gap;
            if x + w > mi.rcWork.right - 8 {
                x = pr.left - gap - w;
            }
            x = x.clamp(
                mi.rcWork.left + 8,
                (mi.rcWork.right - 8 - w).max(mi.rcWork.left + 8),
            );
            let y = pr.top.clamp(
                mi.rcWork.top + 8,
                (mi.rcWork.bottom - 8 - h).max(mi.rcWork.top + 8),
            );
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                w,
                h,
                SWP_SHOWWINDOW | SWP_NOCOPYBITS,
            );
        }
    }
}

/// y 坐标 → 行索引
fn row_at(rows: usize, y: f32) -> Option<usize> {
    let idx = (y - ROW_TOP) / ROW_H;
    if idx >= 0.0 && (idx as usize) < rows {
        Some(idx as usize)
    } else {
        None
    }
}

extern "system" fn popup_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let _ = ValidateRect(Some(hwnd), None);
                let app = super::app_from_tray(hwnd);
                if let Some(app) = app {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let mut renderer = app.popup.renderer.take().or_else(Renderer::new);
                    if let Some(r) = renderer.as_mut() {
                        let model = super::model::PanelModel::from_app(app);
                        let dpi = app.popup.dpi;
                        let hover = app.popup.hover;
                        r.paint_popup(hwnd, &rect, &model, dpi, hover);
                    }
                    app.popup.renderer = renderer;
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let app = super::app_from_tray(hwnd);
                if let Some(app) = app {
                    let mut tm = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        ..Default::default()
                    };
                    let _ = TrackMouseEvent(&mut tm);
                    let y = super::y_of(lparam) / app.popup.dpi;
                    let row = row_at(app.config.accounts.len(), y);
                    if app.popup.hover != row {
                        app.popup.hover = row;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                let app = super::app_from_tray(hwnd);
                if let Some(app) = app
                    && app.popup.hover.take().is_some()
                {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let app = super::app_from_tray(hwnd);
                if let Some(app) = app {
                    let y = super::y_of(lparam) / app.popup.dpi;
                    if let Some(i) = row_at(app.config.accounts.len(), y) {
                        crate::app::select_account(app, i);
                        app.popup.close();
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => match wparam.0 {
                POPUP_TIMER_ANIM => {
                    let app = super::app_from_tray(hwnd);
                    let done = app
                        .and_then(|a| a.popup.renderer.as_ref())
                        .and_then(|r| r.anim.appear.as_ref())
                        .is_none_or(|t| t.finished());
                    if done {
                        let _ = KillTimer(Some(hwnd), POPUP_TIMER_ANIM);
                    } else {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    LRESULT(0)
                }
                POPUP_TIMER_OUTSIDE => {
                    let app = super::app_from_tray(hwnd);
                    if let Some(app) = app {
                        // 只随面板收起关闭；光标停留/路过不关（选择、点面板他处时另有关闭路径）
                        let panel_visible =
                            app.panel.hwnd.is_some_and(|p| IsWindowVisible(p).as_bool());
                        if !panel_visible {
                            app.popup.close();
                        }
                    }
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            },
            WM_SETCURSOR => {
                let hit_hwnd = HWND(wparam.0 as *mut _);
                if hit_hwnd == hwnd && (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                    let app = super::app_from_tray(hwnd);
                    let hand = app.is_some_and(|a| a.popup.hover.is_some());
                    let cursor = if hand { IDC_HAND } else { IDC_ARROW };
                    if let Ok(c) = LoadCursorW(None, cursor) {
                        let _ = SetCursor(Some(c));
                        LRESULT(1)
                    } else {
                        DefWindowProcW(hwnd, msg, wparam, lparam)
                    }
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            WM_ERASEBKGND => LRESULT(1),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
