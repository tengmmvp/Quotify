//! About window

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::ui::float_wnd::{FloatKind, FloatWnd, float_wndproc};
use crate::ui::panel::render::about as view;
use crate::ui::panel::{LogPx, PhysPx, dpi_of, monitor_info};

const ABOUT_WND_CLASS: &str = "QuotifyAbout";

/// About window
pub struct AboutWindow {
    pub wnd: FloatWnd,
    pub news_expanded: Option<usize>,
    pub(crate) egg_clicks: u8,
    pub(crate) egg: Option<crate::ui::panel::anim::Tween>,
    pub(crate) egg_eaten: bool,
}

impl AboutWindow {
    pub fn new() -> Self {
        Self {
            wnd: FloatWnd::new(),
            news_expanded: None,
            egg_clicks: 0,
            egg: None,
            egg_eaten: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.wnd.is_open()
    }

    /// Work-area-centered rectangle: width from ABOUT_W·dpi, height converted from the
    /// logical height; on a short screen
    /// where it exceeds the work area, keep the top and give up the bottom so
    /// the header info is never clipped. The monitor is chosen by the caller — the open path
    /// uses the screen holding the cursor, the relayout path the screen holding the
    /// window. After opening, dpi stops following; before a relayout it is
    /// refreshed from the current screen, so a scale change never reuses a stale value.
    /// The open and relayout paths share this single source.
    pub(crate) fn centered_rect(
        &mut self,
        monitor: windows::Win32::Graphics::Gdi::HMONITOR,
        h: LogPx,
    ) -> (i32, i32, i32, PhysPx) {
        self.wnd.dpi = unsafe { dpi_of(monitor) }.unwrap_or(crate::ui::panel::FALLBACK_DPI);
        let mi = unsafe { monitor_info(monitor) };
        let mi = &mi.rcWork;
        let w = self.wnd.pxf(view::ABOUT_W);
        let hgt = self.wnd.pxf(h.0 as f32);
        // Oversize keeps the left/top and gives up the right/bottom: the
        // header info stays on screen; the unclamped x would spill the
        // window's left edge off-screen on a narrow monitor.
        let x = (mi.left + (mi.right - mi.left - w) / 2).max(mi.left);
        let y = (mi.top + (mi.bottom - mi.top - hgt) / 2).max(mi.top);
        (x, y, w, PhysPx(hgt))
    }

    /// Pops up centered in the work area of the monitor holding the cursor.
    /// `h` is computed by the caller from the number of dynamic entries.
    pub fn open(&mut self, tray: HWND, h: LogPx) {
        // Clicks activate: the About window's keep-alive is anchored to the foreground,
        // and activation is exactly what is wanted.
        let Some(hwnd) =
            self.wnd
                .ensure_window(tray, ABOUT_WND_CLASS, Some(about_wndproc), "关于窗", false)
        else {
            return;
        };
        unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let (x, y, w, hgt) = self.centered_rect(monitor, h);

            self.wnd.show_at(FloatKind::About, hwnd, x, y, w, hgt.0);
        }
    }

    pub fn close(&mut self) {
        // Closing discards everything: the expanded state resets too, so
        // a reopen starts from the collapsed tier.
        self.news_expanded = None;
        self.egg_clicks = 0;
        self.egg = None;
        self.egg_eaten = false;
        self.wnd.close();
    }

    /// TIMER_ANIM tick: a finished easter-egg tween is consumed into the
    /// eaten flag; returns whether an animation is still in flight
    pub(crate) fn anim_tick(&mut self) -> bool {
        if self.egg.as_ref().is_some_and(|t| t.finished()) {
            self.egg = None;
            self.egg_eaten = true;
        }
        self.egg.is_some()
    }
}

/// The About window's window procedure: common arms go through float_wndproc,
/// with the divergence points injected by FloatKind::About.
pub extern "system" fn about_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    float_wndproc(hwnd, msg, wparam, lparam, FloatKind::About)
}
