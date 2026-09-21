//! Floating window skeleton

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetCursorPos, GetWindowRect, HTCLIENT, IDC_ARROW, IDC_HAND, IsWindowVisible,
    KillTimer, LoadCursorW, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOZORDER,
    SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, WM_CAPTURECHANGED,
    WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT,
    WM_SETCURSOR, WM_TIMER, WNDPROC,
};

use crate::app::{app_from_tray, pump};
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::render::Renderer;
use crate::ui::panel::{PanelMode, PanelView, Release, WHEEL_STEP_PX, track_leave};
use crate::ui::{x_of, y_of};

/// Close patrol
const TIMER_SWEEP: usize = 1;
/// Popup animation frame clock
pub(crate) const TIMER_ANIM: usize = 2;

/// Kind of floating window.
/// Close policy and foreground keep-alive differ by kind; the divergence points are
/// TIMER_SWEEP, TIMER_ANIM, and show_at,
/// and the paint call plus click dispatch target also branch on kind.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FloatKind {
    /// Account popup.
    /// Closed explicitly; torn down with the panel when it collapses or
    /// leaves the main view, and also closed when the foreground switches to another app.
    Popup,
    /// About window.
    /// Closes as soon as it loses its foreground anchor.
    About,
}

impl FloatKind {
    /// Whether to claim the foreground: the About window's keep-alive anchor is the
    /// foreground, so the first show and every reopen must seize it,
    /// or the next patrol tick will close it. The popup does not grab it;
    /// its close check compares the foreground against the process.
    fn grabs_foreground(self) -> bool {
        match self {
            FloatKind::Popup => false,
            FloatKind::About => true,
        }
    }
}

/// Floating window skeleton: hwnd, renderer, dpi, and the class-registration flag,
/// carried by common lifecycle methods.
pub struct FloatWnd {
    pub hwnd: Option<HWND>,
    pub renderer: Option<Renderer>,
    pub(crate) dpi: f32,
    /// Press gesture latch: UP is dispatched only when the press originated in this
    /// window; a press started in another window sliding in does not trigger.
    pub(crate) gesture: crate::ui::panel::PressGesture,
    /// Wheel-scroll offset and cap (logical px); a zero cap leaves the wheel
    /// inert — only the account popup arms them
    pub(crate) scroll_dy: f32,
    pub(crate) scroll_max: f32,
    class_registered: bool,
    /// Has drawn successfully at least once in this open lifecycle:
    /// on a device-loss rebuild the content is already on screen,
    /// so the fade-in is not replayed; the same rule as the panel's painted guard
    painted: bool,
}

impl FloatWnd {
    pub fn new() -> Self {
        Self {
            hwnd: None,
            renderer: None,
            dpi: crate::ui::panel::FALLBACK_DPI,
            gesture: crate::ui::panel::PressGesture::new(),
            scroll_dy: 0.0,
            scroll_max: 0.0,
            class_registered: false,
            painted: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.hwnd
            .is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() })
    }

    /// Logical→physical through the shared dpi_px rounding policy: the
    /// panel's conversion points self-describe as the sole rounding
    /// source, and inline `* dpi` elsewhere would silently drift the day
    /// that policy changes.
    pub(crate) fn pxf(&self, logical: f32) -> i32 {
        crate::ui::panel::dpi_px(self.dpi, logical)
    }

    /// Registers the window class and creates the window; the class is registered once,
    /// and the registration flag is reused after window destruction.
    /// noactivate controls whether clicking activates the window:
    /// clicking a non-activating window leaves the foreground untouched.
    pub(crate) fn ensure_window(
        &mut self,
        parent: HWND,
        class: &str,
        wndproc: WNDPROC,
        log_tag: &str,
        noactivate: bool,
    ) -> Option<HWND> {
        if let Some(h) = self.hwnd {
            return Some(h);
        }
        if !self.class_registered {
            if !crate::platform::register_wnd_class(class, wndproc) {
                return None;
            }
            self.class_registered = true;
        }
        let hwnd = crate::platform::create_tool_window(parent, class, log_tag, noactivate)?;
        self.hwnd = Some(hwnd);
        Some(hwnd)
    }

    /// Positioning and showing: when already visible, only re-position;
    /// the first show resets the latch and hover, fades in,
    /// seizes the foreground when required, and starts the patrol and animation timers.
    pub(crate) fn show_at(&mut self, kind: FloatKind, h: HWND, x: i32, y: i32, w: i32, hgt: i32) {
        let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS;
        unsafe {
            if self.is_open() {
                let mut cur = RECT::default();
                let _ = GetWindowRect(h, &mut cur);
                let target = RECT {
                    left: x,
                    top: y,
                    right: x + w,
                    bottom: y + hgt,
                };
                if cur == target {
                    // Reassert the foreground even when the rectangle is unchanged:
                    // the About window's keep-alive is anchored to it;
                    // return immediately after the pump without touching self.
                    if kind.grabs_foreground() {
                        let _ = SetForegroundWindow(h);
                    }
                    return;
                }
                // Same size with no hover highlight takes the pure-translation path:
                // CopyBits carries the old content along with the window,
                // and a full repaint is reserved for cases that must clear the highlight or change size.
                let same_size = cur.right - cur.left == w && cur.bottom - cur.top == hgt;
                let had_hover = self.renderer.as_ref().is_some_and(|r| r.hover.is_some());
                if let Some(r) = self.renderer.as_mut() {
                    // The window moving while the cursor stays still produces no move
                    // message, so clear the old highlight to avoid a stale row.
                    r.hover = None;
                }
                // Foreground claiming is a synchronous pump point: do it after
                // the self writes and never touch self after the pump.
                if kind.grabs_foreground() {
                    let _ = SetForegroundWindow(h);
                }
                if same_size && !had_hover {
                    let _ = SetWindowPos(h, None, x, y, w, hgt, SWP_NOZORDER | SWP_NOACTIVATE);
                } else {
                    let _ = SetWindowPos(h, None, x, y, w, hgt, flags);
                    let _ = InvalidateRect(Some(h), None, false);
                }
                return;
            }
            // The latch resets only on the first show: re-positioning must not interrupt
            // an in-progress press. An animation frame
            // or poll result arriving during the hold would swallow this click if it cleared the latch.
            self.gesture.clear();
            // Renderer state writes land before the pumps (SetWindowPos /
            // ShowWindow / the foreground claim): all can synchronously
            // dispatch WM_DPICHANGED whose arm re-derives App, and writes
            // through this alias after the pump overlap the re-fetched
            // reference.
            if let Some(r) = self.renderer.as_mut() {
                r.hover = None;
                r.anim.appear = Some(crate::ui::panel::anim::Tween::now(180));
            }
            let _ = SetWindowPos(h, None, x, y, w, hgt, flags);
            let _ = ShowWindow(h, SW_SHOW);
            if kind.grabs_foreground() {
                let _ = SetForegroundWindow(h);
            }
            SetTimer(Some(h), TIMER_SWEEP, 200, None);
            start_float_anim(h);
            let _ = InvalidateRect(Some(h), None, false);
        }
    }

    pub fn close(&mut self) {
        self.gesture.clear();
        // The scroll position resets with the open lifecycle
        self.scroll_dy = 0.0;
        // The alpha-tiered brushes built during the animation are cleared on close, so
        // the cache does not accumulate across open/close cycles;
        // painted resets as well, so the next genuine open replays the fade-in.
        // The cleanup goes before the hide pump point: touching self again after
        // SW_HIDE's synchronous reentrancy is unreliable.
        self.painted = false;
        if let Some(r) = self.renderer.as_mut() {
            r.clear_brush_cache();
        }
        if let Some(h) = self.hwnd {
            unsafe {
                let _ = ShowWindow(h, SW_HIDE);
                let _ = KillTimer(Some(h), TIMER_SWEEP);
                let _ = KillTimer(Some(h), TIMER_ANIM);
            }
        }
    }
}

/// Mount the float-window animation clock: the timer id and frame period
/// have this single owner — show_at, the paint-side device-loss re-arm,
/// and external restarts (the about easter egg) all go through here; an
/// outside bare SetTimer would drift silently when the id or period
/// changes.
pub(crate) fn start_float_anim(hwnd: HWND) {
    unsafe {
        let _ = SetTimer(Some(hwnd), TIMER_ANIM, 16, None);
    }
}

/// Where the skeleton lands on App differs by kind: the popup hangs off popup,
/// the About window off about.
fn float_of(app: &mut crate::app::App, kind: FloatKind) -> &mut FloatWnd {
    match kind {
        FloatKind::Popup => &mut app.popup.wnd,
        FloatKind::About => &mut app.about.wnd,
    }
}

/// Collapsing goes through the outer close: the About window must also reset its expanded
/// state, not just fold the skeleton.
fn close_float(hwnd: HWND, app: &mut crate::app::App, kind: FloatKind) {
    match kind {
        FloatKind::Popup => app.popup.close(),
        FloatKind::About => app.about.close(),
    }
    // Hiding a window that holds capture reenters the message pump; the reference must be
    // re-taken after the pump.
    if let Some(app) = app_from_tray(hwnd) {
        // When all floating windows are closed and the panel is hidden too,
        // return the working set immediately: poll results
        // trigger this only when data arrives, and under long intervals a quiet window
        // would keep holding memory. The visibility predicate shares
        // App::ui_visible with the poll arm's trim gate.
        if !app.ui_visible() {
            crate::platform::trim_working_set();
        }
    }
}

/// One wheel notch's scroll step: every 120 of delta counts three lines ≈
/// WHEEL_STEP_PX logical px, the panel's scheme; positive delta scrolls
/// back up. Clamped to [0, max]
pub(crate) fn wheel_scroll(cur: f32, delta: f32, max: f32) -> f32 {
    (cur - delta / 120.0 * WHEEL_STEP_PX).clamp(0.0, max)
}

/// Common window-procedure arms for floating windows, with the divergence points
/// branching on kind in place: the stale check,
/// the paint call, and the click dispatch target. The two windows' wndprocs are thin wrappers.
pub(crate) fn float_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    kind: FloatKind,
) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                // Validate unconditionally, before the App lookup: a
                // WM_PAINT arriving with USERDATA absent must not leave
                // the update region unvalidated — Win32 would regenerate
                // WM_PAINT in a tight loop. paint_common validates again,
                // idempotently.
                let _ = ValidateRect(Some(hwnd), None);
                if let Some(app) = app_from_tray(hwnd) {
                    // Scalars are copied first, the renderer taken next,
                    // the config string borrowed last: the three borrow scopes do not overlap.
                    let (dpi, painted, scroll_dy) = {
                        let f = float_of(app, kind);
                        (f.dpi, f.painted, f.scroll_dy)
                    };
                    let renderer = float_of(app, kind).renderer.take();
                    let appearance = app.config.general.appearance.as_deref();
                    let (renderer, keep, need_anim) = crate::ui::panel::render::paint_common(
                        hwnd,
                        renderer,
                        appearance,
                        painted,
                        dpi,
                        |r, rect| match kind {
                            FloatKind::Popup => r.paint_popup(
                                hwnd,
                                rect,
                                &PanelModel::from_app(app),
                                scroll_dy,
                                dpi,
                            ),
                            FloatKind::About => {
                                let expanded = app.about.news_expanded;
                                let egg = app.about.egg.as_ref().map(|t| t.progress());
                                let eaten = app.about.egg_eaten;
                                r.paint_about(
                                    hwnd,
                                    rect,
                                    &PanelModel::from_app(app),
                                    expanded,
                                    egg,
                                    eaten,
                                    dpi,
                                )
                            }
                        },
                    );
                    if keep {
                        let drew = renderer.is_some();
                        let f = float_of(app, kind);
                        f.renderer = renderer;
                        if drew {
                            f.painted = true;
                        }
                    }
                    // A device-loss rebuild re-arms the first-open fade-in
                    // (paint_common's fresh branch) at a moment when the
                    // TIMER_ANIM arm above may have already killed the
                    // timer — WM_TIMER outranks WM_PAINT in the queue, so
                    // the sweep sees renderer==None, reads the fade as done,
                    // and unmounts. Re-mount whenever the flag is set;
                    // idempotent with the timer show_at already mounted.
                    if need_anim {
                        start_float_anim(hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => match wparam.0 {
                TIMER_SWEEP => {
                    if let Some(app) = app_from_tray(hwnd) {
                        let pressing = float_of(app, kind).gesture.at.is_some();
                        let stale = match kind {
                            // The About window also survives while any window of
                            // this program holds the foreground — the tray menu
                            // takes it when opened over an About window, and a
                            // same-process handoff is not a user departure.
                            FloatKind::About => !pressing && !crate::platform::foreground_is_self(),
                            // The foreground check is waived while pressing:
                            // foreground ownership is unsettled at the click instant,
                            // and a false verdict would swallow the click into a flicker.
                            FloatKind::Popup => {
                                app.panel.mode == PanelMode::Hidden
                                    || app.panel.view != PanelView::Main
                                    || (!pressing && !crate::platform::foreground_is_self())
                            }
                        };
                        if stale {
                            close_float(hwnd, app, kind);
                        }
                    }
                    LRESULT(0)
                }
                TIMER_ANIM => {
                    let mut app = app_from_tray(hwnd);
                    // Per-kind animation tick: the About window's easter egg lives there
                    let mut anim_alive = false;
                    if let Some(a) = app.as_mut() {
                        anim_alive = match kind {
                            FloatKind::Popup => false,
                            FloatKind::About => a.about.anim_tick(),
                        };
                    }
                    let appear_done = app.as_ref().is_none_or(|a| {
                        let f = match kind {
                            FloatKind::Popup => &a.popup.wnd,
                            FloatKind::About => &a.about.wnd,
                        };
                        f.renderer
                            .as_ref()
                            .and_then(|r| r.anim.appear.as_ref().map(|t| t.finished()))
                            .unwrap_or(true)
                    });
                    if appear_done && !anim_alive {
                        let _ = KillTimer(Some(hwnd), TIMER_ANIM);
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            },
            WM_MOUSEMOVE => {
                // Re-arm TME_LEAVE on every move: it is a one-shot notification kept alive by repeated arming.
                track_leave(hwnd);
                if let Some(app) = app_from_tray(hwnd) {
                    let dpi = float_of(app, kind).dpi;
                    let (x, y) = (x_of(lparam) / dpi, y_of(lparam) / dpi);
                    if let Some(r) = float_of(app, kind).renderer.as_mut() {
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
                // Clear any leftover highlight on leaving the window
                if let Some(app) = app_from_tray(hwnd)
                    && let Some(r) = float_of(app, kind).renderer.as_mut()
                    && r.hover.take().is_some()
                {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                // Row scrolling for oversize content; the cap is zero on windows that never scroll
                if let Some(app) = app_from_tray(hwnd) {
                    let f = float_of(app, kind);
                    let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
                    let next = wheel_scroll(f.scroll_dy, delta, f.scroll_max);
                    if next != f.scroll_dy {
                        f.scroll_dy = next;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
            windows::Win32::UI::WindowsAndMessaging::WM_DPICHANGED => {
                // Per-monitor v2 top-level windows are not auto-resized by
                // DefWindowProc: resize through the shared helper and
                // refresh the captured dpi — otherwise every later frame
                // renders mis-scaled until the window is reopened. The
                // helper's SetWindowPos is a pump, so the reference is
                // taken after it; the next paint's ensure_target sees the
                // dpi change and rebuilds.
                let old = app_from_tray(hwnd)
                    .map(|app| float_of(app, kind).dpi)
                    .unwrap_or(crate::ui::panel::FALLBACK_DPI);
                let applied = crate::ui::panel::apply_dpi_change(hwnd, wparam, old);
                if let Some((dpi, _w, _h)) = applied
                    && let Some(app) = app_from_tray(hwnd)
                {
                    float_of(app, kind).dpi = dpi;
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
                // The helper resizes from the old top-left (SWP_NOMOVE),
                // which leaves the window off its anchor: re-place — the
                // popup realigns to the panel, the About window re-centers
                // through refit_about's shared rect source.
                if let Some(app) = app_from_tray(hwnd) {
                    // The popup's monitor cache keys by HMONITOR, which
                    // survives a scale change: without invalidating it here
                    // a cache hit would write the old dpi/size back over the
                    // just-rescaled window. The panel's own DPICHANGED arm
                    // does the same invalidation first.
                    app.popup.invalidate_monitor();
                    match kind {
                        // reposition_popup anchors to the panel, not the
                        // popup itself: its second parameter is the panel's
                        // hwnd. A popup can only be open while the panel
                        // window exists; the None case needs no realignment.
                        FloatKind::Popup => {
                            if let Some(p) = app.panel.hwnd {
                                crate::app::reposition_popup(app, p);
                            }
                        }
                        FloatKind::About => crate::app::refit_about(app),
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                if let Some(app) = app_from_tray(hwnd) {
                    let dpi = float_of(app, kind).dpi;
                    let (x, y) = (x_of(lparam) / dpi, y_of(lparam) / dpi);
                    let mut cursor = POINT::default();
                    let _ = GetCursorPos(&mut cursor);
                    let hit = float_of(app, kind)
                        .renderer
                        .as_ref()
                        .and_then(|r| r.hit_at(x, y));
                    float_of(app, kind)
                        .gesture
                        .record((cursor.x, cursor.y), hit);
                    // Hold capture: releasing outside the window still delivers UP here, or the latch could never reset.
                    let _ = SetCapture(hwnd);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                // The verdict goes through the shared PressGesture primitive,
                // the same rule as the panel; capture release
                // is a pump point: the reference moves into the pump wrapper in exchange
                // for a fresh one, and the dispatch route re-takes it internally.
                let mut cursor = POINT::default();
                let _ = GetCursorPos(&mut cursor);
                if let Some(app) = app_from_tray(hwnd) {
                    let f = float_of(app, kind);
                    let (ux, uy) = (x_of(lparam) / f.dpi, y_of(lparam) / f.dpi);
                    let release_hit = f.renderer.as_ref().and_then(|r| r.hit_at(ux, uy));
                    let verdict = f.gesture.release((cursor.x, cursor.y), release_hit);
                    let target = match kind {
                        FloatKind::Popup => app.panel.hwnd,
                        FloatKind::About => Some(hwnd),
                    };
                    let dispatch = match verdict {
                        Release::Accept {
                            hit: Some(hit),
                            dragged: false,
                        } => target.map(|t| (hit, t)),
                        _ => None,
                    };
                    let app = pump(app, hwnd, |_| {
                        let _ = ReleaseCapture();
                    });
                    if let (Some((hit, t)), Some(_)) = (dispatch, app) {
                        let _ = crate::app::dispatch_hit(hit, t);
                    }
                } else {
                    let _ = ReleaseCapture();
                }
                LRESULT(0)
            }
            windows::Win32::UI::WindowsAndMessaging::WM_ACTIVATE => {
                // Foreground arrived at the floating window: wake the panel's frozen
                // marquee as well — the panel itself
                // never receives this message.
                if (wparam.0 & 0xFFFF) != 0 {
                    crate::ui::panel::wake_frozen_marquee(hwnd);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_CAPTURECHANGED => {
                // After capture is stolen there is no paired UP, so the latch must reset here.
                if let Some(app) = app_from_tray(hwnd) {
                    float_of(app, kind).gesture.clear();
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SETCURSOR => {
                let hit_hwnd = HWND(wparam.0 as *mut _);
                if hit_hwnd == hwnd && (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                    let hand = app_from_tray(hwnd)
                        .and_then(|a| float_of(a, kind).renderer.as_ref())
                        .and_then(|r| r.hover)
                        .is_some_and(|h| h != crate::ui::panel::render::Hit::AboutLogo);
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

#[cfg(test)]
mod tests {
    use super::wheel_scroll;
    use crate::ui::panel::WHEEL_STEP_PX;

    #[test]
    fn wheel_scroll_steps_and_clamps() {
        // One notch down = one wheel step; back up returns
        assert_eq!(wheel_scroll(0.0, -120.0, 100.0), WHEEL_STEP_PX);
        assert_eq!(wheel_scroll(WHEEL_STEP_PX, 120.0, 100.0), 0.0);
        // Past either end: clamped
        assert_eq!(wheel_scroll(0.0, 120.0, 100.0), 0.0);
        assert_eq!(wheel_scroll(90.0, -240.0, 100.0), 100.0);
        // Zero cap (non-scrolling window): always 0
        assert_eq!(wheel_scroll(0.0, -120.0, 0.0), 0.0);
    }
}
