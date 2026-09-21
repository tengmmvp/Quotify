//! Off-panel patrol: the pinned panel's cursor-presence clock — the
//! TIMER_OUTSIDE_CHECK tick body plus the pure single-step dismissal
//! decision it feeds.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetCursorPos, SetTimer, WindowFromPoint,
};

use super::{PanelMode, TIMER_OUTSIDE_CHECK};
use crate::app::{app_from_tray, hide_panel};

/// Wait window from cursor-off-panel to auto-dismiss in the patrol
const OUTSIDE_HIDE_MS: u64 = 2000;
/// Regular patrol period: the patrol arm re-arms itself with this
const OUTSIDE_CHECK_MS: u32 = 200;

/// Single-step decision of the off-panel patrol: present is any one of the
/// four keep-alive paths hitting (in panel/popup, typing, tray anchor
/// zone, dragging). Present clears the timer; off-panel counts from the
/// first tick, and on timeout returns true (should dismiss) and clears the
/// timer. Pure function; the Win32 side only gathers the booleans and clock
fn outside_step(present: bool, now: u64, since: &mut Option<u64>) -> bool {
    if present {
        *since = None;
        return false;
    }
    since.get_or_insert(now);
    if now - since.unwrap() > OUTSIDE_HIDE_MS {
        *since = None;
        true
    } else {
        false
    }
}

/// TIMER_OUTSIDE_CHECK tick: gather the presence booleans and run the
/// dismissal decision; the wndproc arm is a thin dispatch
pub(crate) unsafe fn on_outside_tick(hwnd: HWND) {
    unsafe {
        let app = app_from_tray(hwnd);
        if let Some(app) = app {
            // During a modal freeze, always short-circuit: no
            // decision and no re-arm. A clock armed by a panel
            // reopened inside the modal's nested pump may only
            // idle to its natural death; deciding would get
            // the panel wrongly dismissed behind the dialog.
            // modal_depth is the live count, covering panels
            // opened after the modal entered.
            if crate::app::modal::modal_depth() > 0 {
                return;
            }
            // Check the mode before re-arming: a leftover tick
            // at the instant of dismissal no longer
            // self-perpetuates; the patrol fully stops while hidden.
            if app.panel.mode == PanelMode::Pinned {
                SetTimer(Some(hwnd), TIMER_OUTSIDE_CHECK, OUTSIDE_CHECK_MS, None);
                // Must not call Shell_NotifyIconGetRect — a cross-process
                // synchronous call; high-frequency polling deadlocks.
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let w = WindowFromPoint(pt);
                // Child controls count as inside the panel
                // too; the account popup is an extension of
                // the panel, and the cursor moving into it
                // must not trigger the panel's dismissal.
                let in_popup = app
                    .popup
                    .wnd
                    .hwnd
                    .is_some_and(|ph| w == ph || GetAncestor(w, GA_ROOT) == ph);
                let in_panel = in_popup || w == hwnd || GetAncestor(w, GA_ROOT) == hwnd;
                // Never dismiss while typing. Trust only the
                // edit state: opening grabs the foreground so
                // GetFocus is always true, which would gut
                // the "auto-dismiss after off-panel timeout" entirely.
                let typing = app.panel.input.field.is_some();
                // Mouse over the tray icon
                let near_tray = app.panel.cursor_near_anchor(pt);
                // Mid-drag counts as present: when the window
                // is clamped at the work-area edge the cursor
                // may already be off the panel; holding the
                // button down must not get dismissed.
                let dragging = app.panel.drag_offset.is_some();
                let now = windows::Win32::System::SystemInformation::GetTickCount64();
                let present = in_panel || typing || near_tray || dragging;
                if outside_step(present, now, &mut app.panel.outside_since) {
                    hide_panel(app, hwnd);
                }
            }
        }
    }
}

/// Patrol decision: present clears the timer, off-panel counts from the
/// first tick, strictly-greater-than dismisses, and reopening after a
/// dismissal counts anew (the old timestamp is not carried over)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_step_timing() {
        let mut since = None;
        // Present: never dismiss and clear the timer
        assert!(!outside_step(true, 100, &mut since));
        assert_eq!(since, None);
        // First off-panel tick: start counting, no dismissal
        assert!(!outside_step(false, 200, &mut since));
        assert_eq!(since, Some(200));
        // Exactly 2000ms: strictly greater than is required to dismiss
        assert!(!outside_step(false, 2200, &mut since));
        // Timed out: dismiss and clear the timer
        assert!(outside_step(false, 2201, &mut since));
        assert_eq!(since, None);
        // After a mid-way return visit clears it, the next off-panel spell counts from the new moment
        assert!(!outside_step(true, 3000, &mut since));
        assert!(!outside_step(false, 3100, &mut since));
        assert_eq!(since, Some(3100));
        assert!(!outside_step(false, 5100, &mut since));
        assert!(outside_step(false, 5101, &mut since));
    }
}
