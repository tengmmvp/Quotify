//! Account switcher popup

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{HMONITOR, MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

use crate::ui::float_wnd::{FloatKind, FloatWnd, float_wndproc};
use crate::ui::panel::render::popup as view;
use crate::ui::panel::{dpi_of, monitor_info};

const POPUP_WND_CLASS: &str = "QuotifyAccountPopup";

/// Account switcher popup
pub struct AccountPopup {
    pub wnd: FloatWnd,
    /// Host panel: its chevron is repainted in linkage on open/close.
    panel: Option<HWND>,
    /// Monitor-derived cache: (handle, dpi, work area); invalidated automatically when
    /// the panel changes monitors,
    /// with display-configuration and DPI changes handled by the invalidation hook
    monitor: Option<(HMONITOR, f32, RECT)>,
}

impl AccountPopup {
    pub fn new() -> Self {
        Self {
            wnd: FloatWnd::new(),
            panel: None,
            monitor: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.wnd.is_open()
    }

    /// Invalidates the derived cache after a display-configuration or DPI change;
    /// the next positioning re-queries
    pub fn invalidate_monitor(&mut self) {
        self.monitor = None;
    }

    /// Pops out to the left of the panel.
    /// Positioned next to the trigger button; if it would cross the work area's left edge
    /// it is placed on the right instead, aligned with the panel's top vertically and
    /// clamped back into the work area.
    pub fn open(&mut self, tray: HWND, panel_hwnd: HWND, accounts: usize) {
        let was_open = self.is_open();
        // Clicks do not activate: the foreground stays on the panel, so
        // the marquee does not freeze from clicking the popup.
        let Some(h) =
            self.wnd
                .ensure_window(tray, POPUP_WND_CLASS, Some(popup_wndproc), "账号弹窗", true)
        else {
            return;
        };
        // State recording and the panel repaint go before show_at:
        // its SW_SHOW synchronously reenters the message pump,
        // and this reference must not be used after the pump.
        self.panel = Some(panel_hwnd);
        if !was_open {
            self.repaint_panel();
        }
        unsafe {
            let monitor = MonitorFromWindow(panel_hwnd, MONITOR_DEFAULTTONEAREST);
            // A derived-cache hit spares the dpi and work-area queries: the high-frequency
            // repositioning during dragging and the height animation re-fetches only the panel rectangle.
            let (dpi, work) = match self.monitor {
                Some((m, dpi, work)) if m == monitor => (dpi, work),
                _ => {
                    let dpi = dpi_of(monitor).unwrap_or(crate::ui::panel::FALLBACK_DPI);
                    let work = monitor_info(monitor).rcWork;
                    self.monitor = Some((monitor, dpi, work));
                    (dpi, work)
                }
            };
            self.wnd.dpi = dpi;
            let mut pr = RECT::default();
            let _ = GetWindowRect(panel_hwnd, &mut pr);

            let w = self.wnd.pxf(view::POPUP_W);
            // Height caps at 80% of the work area; longer lists scroll
            let work_h = (work.bottom - work.top) as f32 / dpi;
            let h_log = view::popup_height(accounts, work_h) as f32;
            let hgt = self.wnd.pxf(h_log);
            // Scroll range: uncapped extent minus the viewport; a reposition re-clamps the offset
            self.wnd.scroll_max = (view::popup_content_height(accounts) - h_log).max(0.0);
            self.wnd.scroll_dy = self.wnd.scroll_dy.min(self.wnd.scroll_max);
            let gap = self.wnd.pxf(8.0);
            let mut x = pr.left - w - gap;
            if x < work.left {
                // The right-side fallback is clamped to the left edge as well:
                // on an extremely narrow work area, overlapping the panel beats leaving the screen.
                x = (pr.right + gap).min(work.right - w).max(work.left);
            }
            let y = pr.top.clamp(work.top, (work.bottom - hgt).max(work.top));

            self.wnd.show_at(FloatKind::Popup, h, x, y, w, hgt);
        }
    }

    pub fn close(&mut self) {
        let was_open = self.is_open();
        // The repaint flag goes before hiding: hiding a window that holds capture reenters the message pump.
        if was_open {
            self.repaint_panel();
        }
        self.wnd.close();
    }

    /// Panel repaint linked to open/close: the chevron flips with the popup's visibility.
    fn repaint_panel(&self) {
        if let Some(p) = self.panel {
            crate::ui::panel::invalidate_panel(p);
        }
    }

    /// Repositions with the panel geometry when open; does nothing when closed.
    pub fn reposition(&mut self, tray: HWND, panel_hwnd: HWND, accounts: usize) {
        if self.is_open() {
            self.open(tray, panel_hwnd, accounts);
        }
    }
}

/// The popup's window procedure: common arms go through float_wndproc,
/// with the divergence points injected by FloatKind::Popup.
pub extern "system" fn popup_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    float_wndproc(hwnd, msg, wparam, lparam, FloatKind::Popup)
}
