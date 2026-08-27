//! 关于窗口

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::ui::float_wnd::{FloatKind, FloatWnd, float_wndproc};
use crate::ui::panel::dpi_of;
use crate::ui::panel::render::about as view;

const ABOUT_WND_CLASS: &str = "QuotifyAbout";

/// 关于窗口
pub struct AboutWindow {
    pub wnd: FloatWnd,
    pub news_expanded: Option<usize>,
}

impl AboutWindow {
    pub fn new() -> Self {
        Self {
            wnd: FloatWnd::new(),
            news_expanded: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.wnd.is_open()
    }

    /// 光标所在显示器的工作区居中弹出；`logical_h` 由调用方按动态条目数算好
    pub fn open(&mut self, tray: HWND, logical_h: i32) {
        let Some(h) = self
            .wnd
            .ensure_window(tray, ABOUT_WND_CLASS, Some(about_wndproc), "关于窗")
        else {
            return;
        };
        unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            self.wnd.dpi = dpi_of(monitor).unwrap_or(crate::ui::panel::FALLBACK_DPI);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut mi);
            let w = (view::ABOUT_W * self.wnd.dpi).round() as i32;
            let hgt = (logical_h as f32 * self.wnd.dpi).round() as i32;
            let x = mi.rcWork.left + (mi.rcWork.right - mi.rcWork.left - w) / 2;
            let y =
                (mi.rcWork.top + (mi.rcWork.bottom - mi.rcWork.top - hgt) / 2).max(mi.rcWork.top);

            self.wnd.show_at(h, x, y, w, hgt);
        }
    }

    pub fn close(&mut self) {
        // 收起即放弃：展开态一并复位，重开从折叠档起步
        self.news_expanded = None;
        self.wnd.close();
    }
}

/// 关于窗的窗口过程：公共臂走 float_wndproc，差异点由 FloatKind::About 注入
pub extern "system" fn about_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    float_wndproc(hwnd, msg, wparam, lparam, FloatKind::About)
}
