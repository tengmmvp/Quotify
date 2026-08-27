//! 账号切换弹窗

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

use crate::ui::float_wnd::{FloatKind, FloatWnd, float_wndproc};
use crate::ui::panel::dpi_of;
use crate::ui::panel::render::popup as view;

const POPUP_WND_CLASS: &str = "QuotifyAccountPopup";

/// 账号切换弹窗
pub struct AccountPopup {
    pub wnd: FloatWnd,
}

impl AccountPopup {
    pub fn new() -> Self {
        Self {
            wnd: FloatWnd::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.wnd.is_open()
    }

    /// 面板右侧弹出；越出工作区右界改放左侧，纵向以面板顶对齐并夹回工作区
    pub fn open(&mut self, tray: HWND, panel_hwnd: HWND, accounts: usize) {
        let Some(h) =
            self.wnd
                .ensure_window(tray, POPUP_WND_CLASS, Some(popup_wndproc), "账号弹窗")
        else {
            return;
        };
        unsafe {
            let monitor = MonitorFromWindow(panel_hwnd, MONITOR_DEFAULTTONEAREST);
            self.wnd.dpi = dpi_of(monitor).unwrap_or(crate::ui::panel::FALLBACK_DPI);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut mi);
            let mut pr = RECT::default();
            let _ = GetWindowRect(panel_hwnd, &mut pr);

            let w = (view::POPUP_W * self.wnd.dpi).round() as i32;
            let hgt = (view::popup_height(accounts) as f32 * self.wnd.dpi).round() as i32;
            let gap = (8.0 * self.wnd.dpi).round() as i32;
            let mut x = pr.right + gap;
            if x + w > mi.rcWork.right {
                x = (pr.left - w - gap).max(mi.rcWork.left);
            }
            let y = pr
                .top
                .clamp(mi.rcWork.top, (mi.rcWork.bottom - hgt).max(mi.rcWork.top));

            self.wnd.show_at(h, x, y, w, hgt);
        }
    }

    pub fn close(&mut self) {
        self.wnd.close();
    }
}

/// 弹窗的窗口过程：公共臂走 float_wndproc，差异点由 FloatKind::Popup 注入
pub extern "system" fn popup_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    float_wndproc(hwnd, msg, wparam, lparam, FloatKind::Popup)
}
