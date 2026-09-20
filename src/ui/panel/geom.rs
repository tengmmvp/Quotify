//! Panel pixel geometry: DPI conversion, monitor queries, height units,
//! placement, and height-transition animation — every item that computes
//! geometry in pixels or issues SetWindowPos; window state, message pumping,
//! patrol, and invalidation scheduling stay in the parent module.

use super::Panel;
use super::PanelView;
use super::anim;
use super::layout;
use super::theme::PANEL_WIDTH;
use windows::Win32::Foundation::{HWND, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::GetDpiForMonitor;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, HWND_TOPMOST, IsWindowVisible, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOMOVE,
    SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos,
};

/// Fallback value when DPI probing fails
pub(crate) const FALLBACK_DPI: f32 = 1.5;

/// Logical→physical rounding policy: the one rounding decision every dpi
/// conversion delegates to — Panel::px/pxf and the floating windows'
/// pxf alike, so the panel and the popups can never round apart.
pub(crate) fn dpi_px(dpi: f32, logical: f32) -> i32 {
    (logical * dpi).round() as i32
}

/// Logical-pixel height: the measure on the layout and render side. Height-
/// change chain interfaces are side-tagged with this; a bare i32 cannot
/// enter the chain, and using the wrong side becomes a type error
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LogPx(pub(crate) i32);

/// Physical-pixel height: the measure on the SetWindowPos and GetWindowRect side
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PhysPx(pub(crate) i32);

/// One height transition: start height, target height, timing, and originating view
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeightAnim {
    pub(crate) from: PhysPx,
    pub(crate) to: PhysPx,
    pub(crate) tween: anim::Tween,
    pub(crate) view: PanelView,
}

impl Panel {
    /// Logical pixels → physical pixels; delegates the rounding to
    /// dpi_px — any inline `* dpi` elsewhere drifts silently
    pub(crate) fn px(&self, logical: i32) -> i32 {
        dpi_px(self.dpi, logical as f32)
    }

    /// f32 form of the same rounding policy: float-origin callers (IME
    /// composition-window positioning) go through here instead of
    /// inlining the multiply-round
    pub(crate) fn pxf(&self, logical: f32) -> i32 {
        dpi_px(self.dpi, logical)
    }

    /// Placement reference monitor: when not dragged, pick the monitor by
    /// the anchor center (a first-open window hovers at the primary
    /// monitor's origin, so picking by window would get the wrong monitor);
    /// when dragged, pick by the window's current monitor. Monitor selection
    /// for height and placement uniformly goes through this single source
    pub(crate) fn place_monitor(&self, hwnd: HWND) -> HMONITOR {
        let moved = self.dragged || self.drag_offset.is_some();
        unsafe {
            if !moved && let Some(a) = self.anchor {
                MonitorFromPoint(
                    POINT {
                        x: (a.left + a.right) / 2,
                        y: a.top,
                    },
                    MONITOR_DEFAULTTONEAREST,
                )
            } else {
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)
            }
        }
    }

    /// Logical height → physical target height: the dragged state uses the
    /// full height; anchor-based opening squashes against the work area.
    /// place, shrink-transition startup, and the animation wrap-up
    /// comparison — all three share this source; monitor selection uniformly
    /// goes through place_monitor
    pub(crate) fn target_h(&self, mi: &MONITORINFO, logical: LogPx, moved: bool) -> PhysPx {
        if moved {
            return PhysPx(self.px(logical.0));
        }
        let max_h = (mi.rcWork.bottom - mi.rcWork.top - 16).max(self.px(200));
        PhysPx(self.px(logical.0).min(max_h))
    }

    /// Logical height of the panel's current view
    pub(crate) fn view_height(&self, accounts: usize) -> i32 {
        self.view_height_for(self.team.effective, self.customizing.effective, accounts)
    }

    /// Catch the effective layout up to the selected values: the shared core
    /// of every transition-termination site (dismissal, drag start, DPI
    /// change, relayout, animation wrap-up). Sites clear height_anim and
    /// reconcile fading focus as their situation requires — register any
    /// future pending/effective layout pair here, not at the call sites.
    pub(crate) fn catch_up_layout(&mut self) {
        self.team.catch_up();
        self.customizing.catch_up();
    }

    /// Catch up render-derived fields: main view height, account error
    /// flag, and the caret row chain's account-presence input (the
    /// auth-error side is already a panel field — one source). The
    /// expand and relayout paths share this source; convergence at a
    /// single point prevents drift.
    pub(crate) fn sync_derived(
        &mut self,
        snap: Option<&crate::api::UsageSnapshot>,
        last_error: Option<&crate::api::FetchError>,
        has_accounts: bool,
    ) {
        self.main_h = layout::main_height_of(snap);
        self.auth_error =
            last_error.is_some_and(|e| e.category() == crate::api::ErrorCategory::Unauthenticated);
        self.caret_has_account = has_accounts;
    }

    /// Logical height for a given layout combination: the shrink transition computes its
    /// endpoint from the target layout at startup
    pub(crate) fn view_height_for(&self, team: bool, customizing: bool, accounts: usize) -> i32 {
        match self.view {
            // Dynamic: stretches with metric row count / balance / subtitle, maintained by sync_derived.
            PanelView::Main => self.main_h,
            PanelView::AddForm => layout::add_page_height(team),
            // Whole-page height is the sum of layout's section-constant chain,
            // the same source as draw_settings's y-advance chain.
            PanelView::Settings => {
                layout::settings_view_height(accounts > 0, self.auth_error, customizing)
            }
        }
    }

    /// Compute and apply panel geometry; logical_h uniformly takes logical height
    pub(crate) fn place(&mut self, hwnd: HWND, logical_h: LogPx, show: bool) {
        unsafe {
            let moved = self.dragged || self.drag_offset.is_some();
            let monitor = self.place_monitor(hwnd);
            let mi = monitor_info(monitor);
            self.dpi = dpi_of(monitor).unwrap_or(FALLBACK_DPI);

            // While a height transition is in progress, window geometry is
            // owned exclusively by the animation frames; external relayouts
            // yield. Cross-view transitions have already been terminated by
            // relayout; mid-drag there is no animation, so no check.
            if !show
                && self.drag_offset.is_none()
                && self.height_anim.is_some_and(|a| a.view == self.view)
            {
                return;
            }

            let w = self.px(PANEL_WIDTH);
            // Fetch the current window rect once: drag positioning, the
            // animation start, and jump detection share it.
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let cur_h = wr.bottom - wr.top;
            // Dragged (including mid-drag) keeps the full height and may
            // cover the taskbar — alternating moves and relayouts changing
            // the height would flicker-jump; anchor-based opening still
            // squashes against the work area to stay on screen for short monitors.
            let target_h = self.target_h(&mi, logical_h, moved).0;
            // Expansion goes through a transition animation that gradually
            // uncovers; shrinking collapses the window at the toggle point
            // and never passes through here. Open, mid-drag, and reduced
            // motion land directly; dragged still transitions normally, the
            // frames keep the position.
            let animate = !show
                && self.drag_offset.is_none()
                && cur_h < target_h
                && IsWindowVisible(hwnd).as_bool()
                && anim::animations_allowed();
            if animate {
                self.height_anim = Some(HeightAnim {
                    from: PhysPx(cur_h),
                    to: PhysPx(target_h),
                    tween: anim::Tween::now(150),
                    view: self.view,
                });
                self.retune_anim(hwnd, 16);
            } else {
                self.height_anim = None;
            }
            let h = if animate { cur_h } else { target_h };
            let (x, y) = if moved {
                (wr.left, wr.top)
            } else if let Some(anchor) = self.anchor {
                let ax = (anchor.left + anchor.right) / 2;
                let mut x = ax - w / 2;
                let mut y = anchor.top - h - self.px(8);
                if x < mi.rcWork.left + 8 {
                    x = mi.rcWork.left + 8;
                }
                if x + w > mi.rcWork.right - 8 {
                    x = mi.rcWork.right - 8 - w;
                }
                if y < mi.rcWork.top + 8 {
                    y = mi.rcWork.top + 8;
                }
                (x, y)
            } else {
                (0, 0)
            };
            // Positioning carries SWP_NOCOPYBITS: CopyBits writes the old
            // content to screen first and is then overwritten by the repaint;
            // two misaligned screen writes show as ghost flicker. Drag
            // translation genuinely needs content to follow and is the
            // exception without the flag.
            let flags = if show {
                SWP_SHOWWINDOW | SWP_NOCOPYBITS
            } else if moved {
                SWP_NOACTIVATE
            } else {
                SWP_NOACTIVATE | SWP_NOCOPYBITS
            };
            // The trailing scroll sync runs before the move: every input
            // it reads is known pre-pump, and the SetWindowPos can
            // synchronously dispatch WM_DPICHANGED (mixed-DPI crossing),
            // whose arm re-derives App — writing self after it crosses
            // the alias discipline.
            // Visible height counts the real on-screen viewport; the part
            // past the screen bottom does not count. The expand animation's
            // first frame stays at the old position, so the top edge must be
            // derived backward from the target height to the final state —
            // otherwise an underestimated value releases fake scroll room.
            let final_y = if moved {
                y
            } else if let Some(anchor) = self.anchor {
                (anchor.top - target_h - self.px(8)).max(mi.rcWork.top + 8)
            } else {
                y
            };
            let visible = target_h.min((mi.rcMonitor.bottom - final_y).max(0));
            self.refresh_scroll(logical_h.0, visible);
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, flags);
        }
    }

    /// Start a shrink transition when the target is shorter and animation
    /// is available: the layout keeps its originating state, the window
    /// shrinks first and is gradually clipped; the caller catches up on
    /// finish or interruption. `logical_h` takes logical height.
    pub(crate) fn begin_shrink_anim(&mut self, hwnd: HWND, logical_h: LogPx) -> bool {
        unsafe {
            // Do not start mid-drag: drag frames land immediately and would
            // fight the transition frames; dragged (not mid-drag) still
            // transitions normally, the animation frames keep the current position.
            if self.drag_offset.is_some() {
                return false;
            }
            // The dragged state uses the full height, same as place's moved
            // branch; anchor-based opening squashes against the work area to
            // stay on screen for short monitors. Monitor selection shares the
            // same source as place.
            let mi = monitor_info(self.place_monitor(hwnd));
            let target_h = self.target_h(&mi, logical_h, self.dragged);
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let cur_h = wr.bottom - wr.top;
            if target_h.0 >= cur_h
                || !IsWindowVisible(hwnd).as_bool()
                || !anim::animations_allowed()
            {
                return false;
            }
            self.height_anim = Some(HeightAnim {
                from: PhysPx(cur_h),
                to: target_h,
                tween: anim::Tween::now(150),
                view: self.view,
            });
            self.retune_anim(hwnd, 16);
            true
        }
    }

    /// Recompute the scroll cap after the viewport's physical height changes, and
    /// clamp the offset back into range (visible logical height = physical height / dpi)
    pub(crate) fn refresh_scroll(&mut self, logical_h: i32, physical_h: i32) {
        let visible = physical_h as f32 / self.dpi;
        self.scroll_max = (logical_h as f32 - visible).max(0.0);
        self.scroll_dy = if self.view == PanelView::Main {
            0.0
        } else {
            self.scroll_dy.min(self.scroll_max)
        };
    }
}

/// Work-area info of a given monitor
pub(crate) unsafe fn monitor_info(monitor: HMONITOR) -> MONITORINFO {
    unsafe {
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut mi);
        mi
    }
}

/// Work-area info of the monitor the window is on
pub(crate) unsafe fn monitor_info_of(hwnd: HWND) -> MONITORINFO {
    unsafe { monitor_info(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)) }
}

/// WM_DPICHANGED core shared by the panel and the floating windows:
/// returns (new dpi, new physical width, new physical height). The
/// suggested rect in lparam is deliberately never dereferenced — a forged
/// cross-process message can carry any non-null pointer and reading it is
/// undefined; the replacement size is derived from the window's own rect
/// by ratio, keeping the top-left anchor (no cross-monitor drift). The
/// raw HIWORD scale is clamped: a forged 0 would poison every later
/// division, and absurd values would mis-map clicks. The internal
/// SetWindowPos is a pump — callers re-fetch their references after this.
pub(crate) unsafe fn apply_dpi_change(
    hwnd: HWND,
    wparam: WPARAM,
    old_dpi: f32,
) -> Option<(f32, i32, i32)> {
    unsafe {
        let dpi = ((wparam.0 >> 16) & 0xFFFF) as u32 as f32 / 96.0;
        if !(1.0..=8.0).contains(&dpi) || !old_dpi.is_finite() || old_dpi < 1.0 {
            return None;
        }
        let mut wr = RECT::default();
        let _ = GetWindowRect(hwnd, &mut wr);
        let scale = dpi / old_dpi;
        let w = (((wr.right - wr.left) as f32) * scale).round().max(1.0) as i32;
        let h = (((wr.bottom - wr.top) as f32) * scale).round().max(1.0) as i32;
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            w,
            h,
            // NOCOPYBITS per the repo-wide SetWindowPos rule: the blitted
            // old content and the following D2D present are two misaligned
            // screen writes — the ghost-flash frame.
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOCOPYBITS,
        );
        Some((dpi, w, h))
    }
}

/// Get the monitor's effective DPI (percentage / 96)
pub(crate) unsafe fn dpi_of(monitor: HMONITOR) -> Option<f32> {
    unsafe {
        let mut cx = 0u32;
        let mut cy = 0u32;
        GetDpiForMonitor(
            monitor,
            windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI,
            &mut cx,
            &mut cy,
        )
        .ok()
        .map(|_| cx as f32 / 96.0)
    }
}
