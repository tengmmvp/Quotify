//! Popup panel

pub mod anim;
pub mod caret;
pub mod geom;
mod input;
pub mod layout;
pub mod model;
pub mod render;
pub mod text_edit;
pub mod theme;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::{app_from_tray, pump, sync_caret_after_geometry};
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::render::Renderer;
use crate::ui::{x_of, y_of};

pub(crate) use geom::{
    FALLBACK_DPI, HeightAnim, LogPx, PhysPx, apply_dpi_change, dpi_of, dpi_px, monitor_info,
    monitor_info_of,
};
pub(crate) use input::apply_edit;

const PANEL_WND_CLASS: &str = "QuotifyPanelWnd";

/// Frame clock for the open/dismiss animation
const TIMER_ANIM: usize = 1;
/// Patrol clock for cursor-off-panel checks in the pinned state
pub(crate) const TIMER_OUTSIDE_CHECK: usize = 3;
/// Minute-level repaint heartbeat
const TIMER_MINUTE_TICK: usize = 4;
/// Wait window from cursor-off-panel to auto-dismiss in the patrol
const OUTSIDE_HIDE_MS: u64 = 2000;
/// Regular patrol period: shares the same source between the patrol arm's re-arm and
/// the post-modal restore
pub(crate) const OUTSIDE_CHECK_MS: u32 = 200;
/// Patrol first-delay grace: cursor grace for a just-opened panel; shares the same source
/// between open and post-modal restore
pub(crate) const OUTSIDE_GRACE_MS: u32 = 1200;

/// Pending flag for popup repositioning: geometry changes arrive at high
/// frequency (one per move while dragging), so they are coalesced into at
/// most one in-flight message; the handler clears it on its first line to
/// release the next one.
static REPOSITION_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Byte cap for a single-field input buffer
const INPUT_MAX_BYTES: usize = 128;
/// Byte cap for the proxy field: may carry userinfo credentials, longer than ordinary fields.
/// Same value as api::client::PROXY_MAX_BYTES (the validation-side gate); two layers
/// hold one copy each — api must not depend on ui
const PROXY_MAX_BYTES: usize = 512;

/// Manhattan distance between two points: shared artifact paired with DRAG_SLOP
pub(crate) fn manhattan(ax: i32, ay: i32, bx: i32, by: i32) -> i32 {
    (ax - bx).abs() + (ay - by).abs()
}

/// Threshold for press→displacement to count as dragging (physical pixels,
/// Manhattan distance): shared by all gesture decisions; independent slop
/// values would make the same gesture yield split verdicts
pub(crate) const DRAG_SLOP: i32 = 8;

/// Press gesture latch: press coordinates and hit are latched together and
/// reset together, shared by panel and floating window; failing to clear
/// either field revives phantom-click class defects
pub(crate) struct PressGesture {
    /// Press screen coordinates; None means no press in progress
    pub at: Option<(i32, i32)>,
    /// Hit item at the press instant: UP judges by the press-time decision —
    /// if the window moved while held, release coordinates are already
    /// misaligned and recomputing would select an adjacent item
    pub hit: Option<crate::ui::panel::render::Hit>,
}

impl PressGesture {
    pub(crate) fn new() -> Self {
        Self {
            at: None,
            hit: None,
        }
    }

    /// Record one press
    pub(crate) fn record(&mut self, at: (i32, i32), hit: Option<crate::ui::panel::render::Hit>) {
        self.at = Some(at);
        self.hit = hit;
    }

    /// Reset as a whole
    pub(crate) fn clear(&mut self) {
        self.at = None;
        self.hit = None;
    }

    /// Dragging when the release point exceeds the threshold distance from the press point
    pub(crate) fn dragged_far(&self, cursor: (i32, i32)) -> bool {
        self.at
            .is_some_and(|(px, py)| manhattan(cursor.0, cursor.1, px, py) > DRAG_SLOP)
    }

    /// Release verdict: holds only when pressed in this window and the
    /// release-point hit matches the press-time latch; a blank-space press
    /// is exempt from re-verification — while dragging, the window is
    /// clamped to the work area and the cursor drifts relative to the
    /// client area, so re-verification would veto the drag wrap-up too.
    /// Displacement beyond the threshold is carried via dragged; click
    /// dispatch and drag wrap-up consume one verdict twice. The verdict
    /// consumes: the latched hit is taken away and the latch is reset as a
    /// whole; the call site must complete the verdict before any pump point
    /// (capture release and hit dispatch), and must not read the latch
    /// after the reset.
    pub(crate) fn release(
        &mut self,
        cursor: (i32, i32),
        release_hit: Option<crate::ui::panel::render::Hit>,
    ) -> Release {
        let pressed = self.at.is_some();
        let hit = self.hit.take();
        let dragged = self.dragged_far(cursor);
        self.clear();
        if pressed && (hit.is_none() || release_hit == hit) {
            Release::Accept { hit, dragged }
        } else {
            Release::Reject
        }
    }
}

/// Result of the release verdict
pub(crate) enum Release {
    /// Verdict holds: the press-time latched hit (None for a blank-space hit)
    /// plus the beyond-threshold displacement flag
    Accept {
        hit: Option<crate::ui::panel::render::Hit>,
        dragged: bool,
    },
    /// Does not hold: not pressed in this window, or the release-point hit of a control
    /// press mismatches the latch
    Reject,
}

/// Selected value vs effective value of one layout-affecting toggle: the
/// click side switches `pending` immediately (instant highlight feedback),
/// while `effective` drives the row set and height and only catches up
/// when the height transition ends or is interrupted. Every catch-up goes
/// through catch_up — no hand-written double assignment at call sites.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DualState<T> {
    pub(crate) pending: T,
    pub(crate) effective: T,
}

impl<T: Copy + PartialEq> DualState<T> {
    /// Click-side write: the selected value switches immediately; the
    /// effective value follows via the transition machinery.
    pub(crate) fn set_pending(&mut self, v: T) {
        self.pending = v;
    }

    /// The effective value catches up to the selected one: the shared core
    /// of every transition-termination site (dismissal, drag start, DPI
    /// change, relayout, animation wrap-up).
    pub(crate) fn catch_up(&mut self) {
        self.effective = self.pending;
    }

    /// Whether the effective layout already matches the selection — the
    /// animation wrap-up's mismatch check.
    pub(crate) fn is_synced(&self) -> bool {
        self.pending == self.effective
    }
}

/// Panel display mode: only two states, pinned and hidden, toggled by left-click
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMode {
    Pinned,
    Hidden,
}

/// View the panel is currently showing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelView {
    Main,
    Settings,
    AddForm,
}

/// Target field of self-drawn input; settings-page fields first, add-page form fields
/// after, each ordered by its section
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputField {
    /// Settings page: custom polling interval
    Interval,
    /// Settings page: network proxy address
    Proxy,
    /// Settings page: peak window start
    PeakStart,
    /// Settings page: peak window end
    PeakEnd,
    /// Add page: account name
    Name,
    /// Add page: API key
    Key,
    /// Add page (team edition): organization ID
    Org,
    /// Add page (team edition): project ID
    Project,
}

impl InputField {
    /// Byte cap of the input buffer: proxy addresses may carry userinfo
    /// credentials, so they get a looser tier.
    /// The match is exhaustive with no fallback, same rule as the other
    /// InputField dispatches: a new variant is a compile error
    pub(crate) fn max_bytes(self) -> usize {
        match self {
            InputField::Proxy => PROXY_MAX_BYTES,
            InputField::Interval
            | InputField::PeakStart
            | InputField::PeakEnd
            | InputField::Name
            | InputField::Key
            | InputField::Org
            | InputField::Project => INPUT_MAX_BYTES,
        }
    }

    /// Byte cap tier with the no-active-field fold: callers holding
    /// Option<InputField> (paste budget, char input, IME commit) all read
    /// the tier here, so the None tier has one owner instead of a
    /// map_or fallback restated per call site.
    pub(crate) fn max_bytes_for(field: Option<Self>) -> usize {
        field.map_or(INPUT_MAX_BYTES, Self::max_bytes)
    }

    /// Prefill copy clamped to the byte cap: the config side can hold more
    /// bytes than typing allows (import bypasses input-side validation), and
    /// a verbatim prefill would leave a buffer no keystroke can extend.
    /// Truncates on a char boundary — the truncation policy single-sources
    /// in platform::truncate_bytes (within-cap input passes through
    /// unchanged, same semantics as the inlined code this replaced).
    pub(crate) fn clamp_prefill(self, s: &str) -> String {
        crate::platform::truncate_bytes(s, self.max_bytes()).to_string()
    }

    /// Slot index into the per-field caret width cache; exhaustive like the
    /// other InputField dispatches so a new variant is a compile error here
    /// (the slot-count pinned test then flags the array size).
    pub(crate) fn slot(self) -> usize {
        match self {
            InputField::Interval => 0,
            InputField::Proxy => 1,
            InputField::PeakStart => 2,
            InputField::PeakEnd => 3,
            InputField::Name => 4,
            InputField::Key => 5,
            InputField::Org => 6,
            InputField::Project => 7,
        }
    }
}

/// Key masking display string: focused or under 12 chars shows per-character
/// dots matching the original length, so caret stepping stays correct and
/// short strings leak no plain text; unfocused with 12+ chars reveals only
/// the first and last 4. Shared by the renderer (what is drawn) and
/// field_display (what the caret width table measures) — one rule, one place.
/// The masking glyph is a named constant: caret.rs's cache-hit predicate
/// recognizes the masked form structurally (length + glyph) without
/// materializing it, and reads the same constant so the two can never
/// drift.
pub(crate) const MASK_GLYPH: char = '•';

pub(crate) fn mask_key(key: &str, active: bool) -> String {
    let n = key.chars().count();
    if active || n < 12 {
        MASK_GLYPH.to_string().repeat(n)
    } else {
        let head: String = key.chars().take(4).collect();
        let tail: String = key.chars().skip(n - 4).collect();
        format!("{head}…{tail}")
    }
}

/// Self-drawn input buffers; field order same as InputField
#[derive(Default)]
pub struct PanelInput {
    pub field: Option<InputField>,
    pub(crate) edit: crate::ui::panel::text_edit::EditState,
    pub(crate) surrogate: Option<u16>,
    pub interval: String,
    pub proxy: String,
    pub peak_start: String,
    pub peak_end: String,
    pub name: String,
    pub key: String,
    pub org: String,
    pub project: String,
    /// User-edited marks for the two prefilled fields: prefilling must
    /// not read an empty buffer as "never edited" — a cleared but
    /// unapplied edit stays put. Set by apply_edit (every user-driven
    /// write, IME included, goes through it); reset wherever the buffer
    /// is cleared.
    pub interval_edited: bool,
    pub proxy_edited: bool,
}

impl PanelInput {
    /// Mutable reference to the active field's buffer; falls back to interval with no active field
    pub(crate) fn active_buf(&mut self) -> &mut String {
        match self.field {
            Some(InputField::Interval) => &mut self.interval,
            Some(InputField::Proxy) => &mut self.proxy,
            Some(InputField::PeakStart) => &mut self.peak_start,
            Some(InputField::PeakEnd) => &mut self.peak_end,
            Some(InputField::Name) => &mut self.name,
            Some(InputField::Key) => &mut self.key,
            Some(InputField::Org) => &mut self.org,
            Some(InputField::Project) => &mut self.project,
            None => &mut self.interval,
        }
    }

    /// Read-only reference to the active field's buffer; falls back to interval with no active field
    pub(crate) fn active_str(&self) -> &str {
        match self.field {
            Some(InputField::Interval) => &self.interval,
            Some(InputField::Proxy) => &self.proxy,
            Some(InputField::PeakStart) => &self.peak_start,
            Some(InputField::PeakEnd) => &self.peak_end,
            Some(InputField::Name) => &self.name,
            Some(InputField::Key) => &self.key,
            Some(InputField::Org) => &self.org,
            Some(InputField::Project) => &self.project,
            None => &self.interval,
        }
    }

    /// Clear one named field's buffer; the edited marks and the edit state
    /// are the callers' semantics and stay untouched here. Exhaustive like
    /// the other InputField dispatches: a new variant is a compile error.
    pub(crate) fn clear_field(&mut self, f: InputField) {
        match f {
            InputField::Interval => self.interval.clear(),
            InputField::Proxy => self.proxy.clear(),
            InputField::PeakStart => self.peak_start.clear(),
            InputField::PeakEnd => self.peak_end.clear(),
            InputField::Name => self.name.clear(),
            InputField::Key => self.key.clear(),
            InputField::Org => self.org.clear(),
            InputField::Project => self.project.clear(),
        }
    }
}

/// Input-slot hit → corresponding edit field; non-input variants are listed
/// explicitly, a new Hit variant missing registration is a compile error.
/// The eye (RevealKey) and the "Customize" button are not part of the text
/// zone and are not mapped
pub(crate) fn input_field_of_hit(hit: crate::ui::panel::render::Hit) -> Option<InputField> {
    use crate::ui::panel::render::Hit;
    match hit {
        Hit::InputInterval => Some(InputField::Interval),
        Hit::InputProxy => Some(InputField::Proxy),
        Hit::InputPeakStart => Some(InputField::PeakStart),
        Hit::InputPeakEnd => Some(InputField::PeakEnd),
        Hit::InputName => Some(InputField::Name),
        Hit::InputKey => Some(InputField::Key),
        Hit::InputOrg => Some(InputField::Org),
        Hit::InputProject => Some(InputField::Project),
        // Hits outside the text zones are never mapped to input fields.
        Hit::Refresh
        | Hit::Settings
        | Hit::AccountSwitch
        | Hit::UsageInfo
        | Hit::Back
        | Hit::ClosePanel
        | Hit::IntervalPreset(_)
        | Hit::CustomizeInterval
        | Hit::ApplyInterval
        | Hit::ApplyProxy
        | Hit::Language(_)
        | Hit::Appearance(_)
        | Hit::ToggleAutostart
        | Hit::ToggleThreshold
        | Hit::ToggleReset5h
        | Hit::ToggleResetWeekly
        | Hit::ApplyPeak
        | Hit::AddAccount
        | Hit::RemoveAccount(_)
        | Hit::PickAccount(_)
        | Hit::AccountType(_)
        | Hit::RevealKey
        | Hit::SaveAccount
        | Hit::Platform(_)
        | Hit::ExportConfig
        | Hit::ImportConfig
        | Hit::CheckUpdate
        | Hit::OpenDownload
        | Hit::LinkRepo
        | Hit::LinkIssues
        | Hit::CopyDiagnostics
        | Hit::NewsItem(_)
        | Hit::AboutLogo => None,
    }
}

pub struct Panel {
    pub hwnd: Option<HWND>,
    pub mode: PanelMode,
    pub view: PanelView,
    pub(crate) scroll_dy: f32,
    pub(crate) scroll_max: f32,
    pub(crate) anchor: Option<RECT>,
    pub renderer: Option<Renderer>,
    pub pending_platform: crate::api::Platform,
    pub(crate) team: DualState<bool>,
    pub input: PanelInput,
    pub(crate) key_revealed: bool,
    pub(crate) customizing: DualState<bool>,
    pub(crate) selecting: bool,
    pub(crate) text_clicks: Option<(u32, i32, i32, u8)>,
    vis_start: std::cell::Cell<usize>,
    last_input_at: std::cell::Cell<std::time::Instant>,
    blink_drawn: std::cell::Cell<bool>,
    ime_owned: bool,
    pub(crate) dragged: bool,
    pub(crate) gesture: PressGesture,
    pub(crate) drag_offset: Option<(i32, i32)>,
    class_registered: bool,
    pub(crate) main_h: i32,
    pub(crate) auth_error: bool,
    /// Caret-line y chain's account-presence input (the settings row
    /// chain's has_account tier), written by sync_derived; the
    /// auth-error input is not mirrored — caret_line_y reads
    /// self.auth_error directly, one source.
    pub(crate) caret_has_account: bool,
    pub(crate) anim_period: u32,
    pub(crate) height_anim: Option<HeightAnim>,
    /// Current period of the minute heartbeat: the final countdown minute switches to a
    /// second tick; remembered across ticks to avoid re-arming.
    minute_period: std::cell::Cell<u32>,
    pub(crate) outside_since: Option<u64>,
    painted: bool,
    caret_cache: std::cell::RefCell<[Option<caret::CaretCacheEntry>; caret::CARET_SLOTS]>,
    dpi: f32,
}

impl Panel {
    pub fn new() -> Self {
        Self {
            hwnd: None,
            mode: PanelMode::Hidden,
            view: PanelView::Main,
            scroll_dy: 0.0,
            scroll_max: 0.0,
            anchor: None,
            renderer: None,
            pending_platform: crate::api::Platform::Cn,
            team: DualState::default(),
            input: PanelInput::default(),
            key_revealed: false,
            customizing: DualState::default(),
            main_h: layout::EMPTY_VIEW_H as i32,
            auth_error: false,
            caret_has_account: false,
            anim_period: 0,
            height_anim: None,
            minute_period: std::cell::Cell::new(0),
            selecting: false,
            text_clicks: None,
            vis_start: std::cell::Cell::new(0),
            last_input_at: std::cell::Cell::new(std::time::Instant::now()),
            blink_drawn: std::cell::Cell::new(true),
            ime_owned: false,
            dragged: false,
            gesture: PressGesture::new(),
            drag_offset: None,
            class_registered: false,
            outside_since: None,
            painted: false,
            caret_cache: std::cell::RefCell::new(std::array::from_fn(|_| None)),
            dpi: FALLBACK_DPI,
        }
    }

    fn ensure_window(&mut self, parent: HWND) -> Option<HWND> {
        if let Some(h) = self.hwnd {
            return Some(h);
        }
        if !self.class_registered {
            if !crate::platform::register_wnd_class(PANEL_WND_CLASS, Some(panel_wndproc)) {
                return None;
            }
            self.class_registered = true;
        }
        let hwnd = crate::platform::create_tool_window(parent, PANEL_WND_CLASS, "面板窗口", false)?;
        unsafe {
            // Strip WS_MAXIMIZEBOX: Win11's "drag to top edge = snap
            // maximize" for maximizable windows would bounce the window back
            // mid-drag; without maximize capability it does not participate.
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            let nosnap = style & !(WS_MAXIMIZEBOX.0 as isize);
            let _ = SetWindowLongPtrW(hwnd, GWL_STYLE, nosnap);
            // Disable DWM position transitions: the system smoothing on
            // open/drag would stack with self-drawn animation into jank.
            let disable: i32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWA_TRANSITIONS_FORCEDISABLED,
                &disable as *const i32 as *const core::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            );
        }
        self.hwnd = Some(hwnd);
        Some(hwnd)
    }

    /// Single source of truth for TIMER_ANIM tuning: touch the system timer
    /// only when the period changes and record it in sync — bare
    /// SetTimer/KillTimer bypassing the record lets the shadow drift, making
    /// the marquee keep-alive and frame-rate re-arm judged against a stale
    /// shadow. 0 removes the clock.
    pub(crate) fn retune_anim(&mut self, hwnd: HWND, period: u32) {
        if self.anim_period == period {
            return;
        }
        unsafe {
            if period == 0 {
                let _ = KillTimer(Some(hwnd), TIMER_ANIM);
            } else {
                SetTimer(Some(hwnd), TIMER_ANIM, period, None);
            }
        }
        self.anim_period = period;
    }

    /// Position and show; the fade-in is advanced by TIMER_ANIM. Returns
    /// the window handle (including one created inside this call) so the
    /// caller never reads self after the trailing pump.
    pub(crate) fn show_at(&mut self, parent: HWND, anchor: RECT, accounts: usize) -> Option<HWND> {
        let hwnd = self.ensure_window(parent)?;
        // Read before place: place's SetWindowPos with SWP_SHOWWINDOW sets the visible bit.
        let fresh = unsafe { !IsWindowVisible(hwnd).as_bool() };
        self.anchor = Some(anchor);
        // New open/close cycle: first-frame fade-in eligibility resets.
        self.painted = false;
        // Dragging is only a temporary look; when the panel re-opens it returns to the
        // tray anchor, and drag leftovers are cleared along the way.
        self.dragged = false;
        self.drag_offset = None;
        // All state writes and timer arms happen before the pumps (place's
        // SetWindowPos, ShowWindow): both can synchronously dispatch
        // WM_DPICHANGED whose arm re-derives App, and writes through this
        // alias after the pump overlap the re-fetched reference.
        self.minute_period.set(60_000);
        unsafe {
            // The patrol's first delay gives a full opening grace, preventing a wrongful
            // dismissal right as the panel opens.
            SetTimer(Some(hwnd), TIMER_OUTSIDE_CHECK, OUTSIDE_GRACE_MS, None);
            SetTimer(Some(hwnd), TIMER_MINUTE_TICK, 60_000, None);
            if fresh && let Some(r) = self.renderer.as_mut() {
                r.anim.appear = Some(anim::Tween::now(180));
            }
        }
        let logical_h = self.view_height(accounts);
        self.place(hwnd, LogPx(logical_h), true);
        // No layered alpha for the whole window —
        // incompatible with HwndRenderTarget's swap-chain presentation.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            invalidate_panel(hwnd);
        }
        // Setting Pinned comes after the window is shown (an earlier
        // return keeps Hidden on window-creation failure) and before the
        // trailing pump: the caller's alias is dead past start_anim.
        self.mode = PanelMode::Pinned;
        // start_anim re-fetches App internally: it must be the last use of
        // this alias.
        if fresh {
            unsafe {
                start_anim(hwnd);
            }
        }
        Some(hwnd)
    }

    /// Local rect test of anchor ±24px — Shell_NotifyIconGetRect is
    /// cross-process synchronous, high-frequency polling would deadlock
    /// The cursor is passed in by the caller: the patrol tick already
    /// queried it once, do not call again
    pub(crate) fn cursor_near_anchor(&self, pt: POINT) -> bool {
        self.anchor
            .map(|a| {
                pt.x >= a.left - 24
                    && pt.x <= a.right + 24
                    && pt.y >= a.top - 24
                    && pt.y <= a.bottom + 24
            })
            .unwrap_or(false)
    }

    pub(crate) fn begin_hide(&mut self, hwnd: HWND) {
        self.mode = PanelMode::Hidden;
        self.key_revealed = false;
        self.outside_since = None;
        self.drag_offset = None;
        self.selecting = false;
        self.gesture.clear();
        self.text_clicks = None;
        self.clear_input(hwnd);
        // All state writes happen before the hiding pump: SW_HIDE
        // synchronously dispatches KILLFOCUS and other messages, and the
        // reference re-fetched inside the pump invalidates this frame's
        // reference.
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_OUTSIDE_CHECK);
            let _ = KillTimer(Some(hwnd), TIMER_MINUTE_TICK);
            self.height_anim = None;
            // Dismissal catches up immediately: the next open's height and
            // row set follow the selected values, carrying no frozen leftovers.
            self.catch_up_layout();
            self.minute_period.set(0);
        }
        // The animation clock is removed with the clock group, and the period
        // record is cleared in sync (the re-arm gate in WM_PAINT's wrap-up
        // judges by 0).
        self.retune_anim(hwnd, 0);
        // Hover highlight, hit zones, and the footer marquee are cleared
        // together so the reopened first frame carries no stale state; the
        // alpha-tiered brush cache does not accumulate across open/close.
        if let Some(r) = self.renderer.as_mut() {
            r.hover = None;
            r.hits.clear();
            r.anim.error_scroll = None;
            r.clear_brush_cache();
        }
        // Hide directly: a self-drawn shrink/fade-out would stack with DWM
        // transitions into flicker; the pump goes last.
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        // After dismissal nothing is drawn; returning the working set pushes
        // idle memory back to the tray tier. self is untouched — safe after
        // the pump.
        crate::platform::trim_working_set();
    }

    /// Expand from hidden: reset to the main view, position and show, set pinned, and
    /// grab the foreground
    pub fn show_from_hidden(&mut self, parent: HWND, anchor: RECT, accounts: usize) {
        // Only reopening from hidden returns to the main view; do not linger on the old
        // scrolled settings page.
        self.reset_to_main();
        // The handle comes back through show_at's return value: on first
        // open the window is created inside it, and reading self after its
        // trailing start_anim pump would go through a dead alias.
        let hwnd = self.show_at(parent, anchor, accounts);
        // Activate immediately after showing — a background window cannot
        // get keyboard focus, and IME misbehaves. hwnd-only, so safe after
        // the pump.
        if let Some(h) = hwnd {
            unsafe {
                let _ = SetForegroundWindow(h);
            }
        }
    }

    /// Reset before opening the panel: back to the main view, clear the
    /// scroll offset, the add form, and transient input state; called when
    /// left-click reopens from hidden, so reopening after a dismissal does
    /// not linger on the old scrolled settings page.
    /// The interval-row expanded state is not cleared: view_height has the
    /// matching CUSTOMIZE_EXTRA_H hook, and the expanded state must survive
    /// across open/close
    fn reset_to_main(&mut self) {
        self.view = PanelView::Main;
        self.scroll_dy = 0.0;
        self.key_revealed = false;
        self.input.clear_field(InputField::Interval);
        self.input.clear_field(InputField::Proxy);
        // The prefill marks reset with their buffers: the next settings
        // session prefills afresh from the config.
        self.input.interval_edited = false;
        self.input.proxy_edited = false;
        self.input.clear_field(InputField::PeakStart);
        self.input.clear_field(InputField::PeakEnd);
        if let Some(h) = self.hwnd {
            self.clear_input(h);
        } else {
            self.input.field = None;
        }
    }

    /// Release the edit state and the key plain-text view; the buffers
    /// keep their text. All four release paths share this one — focus
    /// loss, blank click, dismissal, and config import — so a
    /// release-semantics change (clear_input gained IME-commit behavior
    /// once already) cannot leave paths behaving differently.
    pub(crate) fn end_edit_state(&mut self, hwnd: HWND) {
        // The multi-click latch resets with the edit state: a blank click
        // interleaved with field clicks must not accumulate toward a
        // phantom double/triple click.
        self.text_clicks = None;
        let active = self.input.field.is_some() || self.key_revealed;
        if self.input.field.is_some() {
            self.clear_input(hwnd);
        }
        self.key_revealed = false;
        if active {
            invalidate_panel(hwnd);
        }
    }

    /// Leave the edit state and detach the IME context.
    pub(crate) fn clear_input(&mut self, hwnd: HWND) {
        // Commit the in-flight composition while the field is still
        // active (the commit writes the active buffer): native edit
        // controls commit on focus loss, and the field-switch path in
        // focus_input commits the same way — the two paths must not
        // diverge into keep-text vs drop-text.
        unsafe {
            self.commit_ime_composition(hwnd);
        }
        self.input.field = None;
        self.input.edit.reset();
        self.input.surrogate = None;
        self.vis_start.set(0);
        unsafe {
            // Detach the IME context: what a bare window attached must be taken back, or it dangles.
            use windows::Win32::UI::Input::Ime::{
                HIMC, ImmAssociateContext, ImmDestroyContext, ImmGetContext,
            };
            let ctx = ImmGetContext(hwnd);
            // Reclaim only the self-created context; when the edit state was never
            // entered, the window carries the thread-default context, and
            // destroying it would break global IME.
            if self.ime_owned && !ctx.is_invalid() {
                let _ = ImmAssociateContext(hwnd, HIMC(std::ptr::null_mut()));
                let _ = ImmDestroyContext(ctx);
            }
        }
        self.ime_owned = false;
        // The width table of a dead buffer is pure retention once the edit
        // state ends (the Key slot even carries the masked string's length).
        self.caret_cache
            .borrow_mut()
            .iter_mut()
            .for_each(|s| *s = None);
    }
}

pub(crate) unsafe fn start_anim(hwnd: HWND) {
    if let Some(app) = app_from_tray(hwnd) {
        app.panel.retune_anim(hwnd, 16);
    }
}

/// Marquee wake-up when the foreground reaches any window of this program:
/// the activation arms of panel, tray, and floating window share this —
/// the activation message is only delivered to the window that got the
/// foreground, and while frozen the foreground may come back via the tray
/// menu or a floating window, which the panel itself never receives. owner
/// goes through the app_from_tray single entry, covering tray/panel/
/// floating window alike.
pub(crate) fn wake_frozen_marquee(owner: HWND) {
    if let Some(app) = app_from_tray(owner) {
        let panel = app.panel.hwnd;
        if let Some(r) = app.panel.renderer.as_mut()
            && r.resume_error_scroll()
            && let Some(p) = panel
        {
            unsafe { start_anim(p) };
        }
    }
}

/// The minute heartbeat retunes its period by the nearest reset moment: the
/// final minute switches to a second tick so the readout walks second by
/// second, and after a reset it returns to the minute tick; re-arm only
/// when the period changes.
pub(crate) fn retune_minute(app: &crate::app::App, hwnd: HWND) {
    let now = chrono::Utc::now();
    // The soonest reset goes through present_buckets — the same single
    // source the countdown rows render from — so every bucket variant with
    // a reset (MCP included) retunes the heartbeat, not a hand-kept pair.
    let soonest = app.data.snapshot.as_ref().and_then(|s| {
        s.present_buckets()
            .filter_map(|b| b.resets_at())
            .filter(|t| *t > now)
            .min()
    });
    let want = minute_period(soonest, now);
    if app.panel.minute_period.get() != want {
        unsafe {
            let _ = SetTimer(Some(hwnd), TIMER_MINUTE_TICK, want, None);
        }
        app.panel.minute_period.set(want);
    }
}

/// Period decision decoupled from the time source, for pinned tests. The
/// split point aligns with countdown's display cutoff: a full 60 seconds
/// remaining still shows "N minutes" and keeps the minute tick; from 59
/// seconds it switches to the second tick.
fn minute_period(
    soonest: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> u32 {
    if soonest.is_some_and(|t| t > now && (t - now).num_seconds() < 60) {
        1000
    } else {
        60_000
    }
}

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

/// Re-arm TME_LEAVE leave tracking
pub(crate) unsafe fn track_leave(hwnd: HWND) {
    unsafe {
        let mut tm = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            ..Default::default()
        };
        let _ = TrackMouseEvent(&mut tm);
    }
}

/// Panel window procedure
pub extern "system" fn panel_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                // Validate unconditionally, before the App lookup: a
                // WM_PAINT arriving with USERDATA absent (a forged or
                // cross-posted message, or a teardown path clearing it)
                // must not leave the update region unvalidated — Win32
                // would regenerate WM_PAINT in a tight loop. paint_common
                // validates again, idempotently.
                let _ = ValidateRect(Some(hwnd), None);
                // Invalidation is always whole-window, no partial-rect
                // narrowing: HwndRenderTarget presents the whole window's
                // back buffer, narrowing saves no drawing, and the edit
                // paths likewise InvalidateRect the whole window.
                if let Some(app) = app_from_tray(hwnd) {
                    // Take the renderer out first, then borrow the config:
                    // take's mutable borrow ends first, so the appearance
                    // borrow does not conflict with it — no per-frame clone.
                    let renderer = app.panel.renderer.take();
                    let appearance = app.config.general.appearance.as_deref();
                    let painted = app.panel.painted;
                    let view = app.panel.view;
                    let dpi = app.panel.dpi;
                    let (renderer, keep, need_anim) = crate::ui::panel::render::paint_common(
                        hwnd,
                        renderer,
                        appearance,
                        painted,
                        dpi,
                        |r, rect| {
                            let model = PanelModel::from_app(app);
                            r.paint(hwnd, rect, &app.panel, &model, view, dpi)
                        },
                    );
                    if keep {
                        let drew = renderer.is_some();
                        app.panel.renderer = renderer;
                        if drew {
                            app.panel.painted = true;
                        }
                        // A first-open fade-in arms its clock here, and a
                        // marquee scroll needs the clock the moment it is
                        // created. retune_anim goes through this reference
                        // directly (no re-fetch); start_anim re-fetches and
                        // mutates through a second alias, so it must be the
                        // reference's last use. When the first frame already
                        // scrolls, the retuned marquee clock drives the
                        // fade-in too and start_anim is skipped by the
                        // anim_period check.
                        // Do not re-arm while the clock runs: SetTimer
                        // resets the timing origin; do not re-arm while
                        // frozen — revival belongs to the activation message.
                        let scrolling = app
                            .panel
                            .renderer
                            .as_ref()
                            .is_some_and(|r| r.error_scrolling());
                        if scrolling && app.panel.anim_period == 0 {
                            // The frame rate goes through compose_anim_period
                            // as in on_anim_tick: marquee scrolling uses the
                            // reduced-frame tier.
                            let ms = compose_anim_period(
                                Some(crate::ui::panel::render::ERROR_SCROLL_FRAME_MS),
                                None,
                            );
                            app.panel.retune_anim(hwnd, ms);
                        }
                        if need_anim && app.panel.anim_period == 0 {
                            start_anim(hwnd);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => {
                let id = wparam.0;
                match id {
                    TIMER_ANIM => on_anim_tick(hwnd),
                    // The heartbeat advances all relative-time texts: the
                    // footer's [X minutes ago] sits at the bottom, and the
                    // metric rows' reset countdown sits mid-content — a
                    // banded invalidation would freeze the countdown; and
                    // since WM_PAINT repaints the whole window anyway,
                    // narrowing the invalidation rect saves no drawing.
                    TIMER_MINUTE_TICK => {
                        invalidate_panel(hwnd);
                        if let Some(app) = app_from_tray(hwnd) {
                            retune_minute(app, hwnd);
                        }
                        LRESULT(0)
                    }
                    TIMER_OUTSIDE_CHECK => {
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
                                return LRESULT(0);
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
                                let in_panel =
                                    in_popup || w == hwnd || GetAncestor(w, GA_ROOT) == hwnd;
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
                                let now =
                                    windows::Win32::System::SystemInformation::GetTickCount64();
                                let present = in_panel || typing || near_tray || dragging;
                                if outside_step(present, now, &mut app.panel.outside_since) {
                                    crate::app::hide_panel(app, hwnd);
                                }
                            }
                        }
                        LRESULT(0)
                    }
                    _ => DefWindowProcW(hwnd, msg, wparam, lparam),
                }
            }
            WM_MOUSEMOVE => {
                let mut app = app_from_tray(hwnd);
                // Manual drag follow: mouse pointer minus the press offset, clamped to the work area.
                if let Some(app) = app.as_mut()
                    && let Some((ox, oy)) = app.panel.drag_offset
                {
                    let mut cursor = POINT::default();
                    let _ = GetCursorPos(&mut cursor);
                    let mut wr = RECT::default();
                    let _ = GetWindowRect(hwnd, &mut wr);
                    let w = wr.right - wr.left;
                    let mi = monitor_info_of(hwnd);
                    // The window may go off-screen in all four directions;
                    // only one corner (64 physical pixels) is kept inside
                    // the work area so it stays grabbable, symmetric on all sides.
                    let x = (cursor.x - ox).clamp(
                        mi.rcWork.left - w + 64,
                        (mi.rcWork.right - 64).max(mi.rcWork.left - w + 64),
                    );
                    let y_min = mi.rcWork.top - (wr.bottom - wr.top) + 64;
                    let y = (cursor.y - oy).clamp(y_min, (mi.rcWork.bottom - 64).max(y_min));
                    // Close the popup only when displacement exceeds the
                    // threshold; below it, just follow. The decision is read
                    // before the pumps: SetWindowPos can synchronously
                    // dispatch WM_DPICHANGED (cross-monitor), and close's
                    // SW_HIDE re-enters the pump again — both re-derive App,
                    // so the close runs through the re-fetched reference and
                    // nothing here uses this frame's alias afterwards.
                    let close_it =
                        app.popup.is_open() && app.panel.gesture.dragged_far((cursor.x, cursor.y));
                    let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
                    if close_it && let Some(app) = app_from_tray(hwnd) {
                        app.popup.close();
                    }
                    // The translation (and the close's SW_HIDE) moved the
                    // caret row on screen: the shared tail re-fetches and
                    // re-syncs the IME composition window.
                    sync_caret_after_geometry(hwnd);
                    return LRESULT(0);
                }
                // The drag branch took as_mut's reborrow and returned early;
                // here move the same binding — calling app_from_tray again
                // would construct two live mutable aliases.
                if let Some(app) = app {
                    // Mid drag selection: the caret advances with the mouse
                    // pointer, extending the selection. The width table hits
                    // the cache (buffer and sticky start both unchanged), so
                    // the new caret x is a direct table lookup — no second layout.
                    if app.panel.selecting {
                        let x = x_of(lparam) / app.panel.dpi;
                        if let (Some(f), Some(r)) =
                            (app.panel.input.field, app.panel.renderer.as_ref())
                        {
                            let cl = app.panel.caret_layout(r);
                            let pos = app.panel.caret_hit_test(cl.as_ref(), f, x);
                            if pos != app.panel.input.edit.caret {
                                app.panel.input.edit.place(pos, true);
                                // The caret advancing with the pointer is an interaction
                                // too; zero the phase so it shows immediately.
                                app.panel.note_caret_interaction();
                                // Inside the window, position via table
                                // lookup; past the visible window (capture
                                // makes out-of-box coordinates reachable), a
                                // full re-layout is needed to scroll the
                                // window, otherwise the caret drawn outside
                                // the box never self-heals.
                                let avail = layout::field_avail(f);
                                let cx = cl.seg(cl.vis_start, pos);
                                if cx <= avail {
                                    app.panel.place_caret(hwnd, cx);
                                } else {
                                    app.panel.update_caret(hwnd, app.panel.renderer.as_ref());
                                }
                                invalidate_panel(hwnd);
                            }
                        }
                        return LRESULT(0);
                    }
                    // TME_LEAVE is one-shot and gets interrupted by mouse
                    // capture, so re-arm on every move; the actual leave is
                    // handled by MOUSELEAVE clearing the hover highlight.
                    track_leave(hwnd);
                    // Presence immediately resets the off-panel timer: the
                    // patrol ticks every 200ms, and a brief return visit
                    // between two ticks must not count as off-panel time.
                    app.panel.outside_since = None;
                    let (x, y) = (x_of(lparam) / app.panel.dpi, y_of(lparam) / app.panel.dpi);
                    if let Some(r) = app.panel.renderer.as_mut() {
                        let hit = r.hit_at(x, y);
                        if r.hover != hit {
                            r.hover = hit;
                            invalidate_panel(hwnd);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                let app = app_from_tray(hwnd);
                if let Some(app) = app {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let w = WindowFromPoint(pt);
                    // Correct the transitional state: at the instant TME
                    // fires the cursor may happen to be over a child
                    // control; clearing wrongly would flicker.
                    let still_here = w == hwnd || GetAncestor(w, GA_ROOT) == hwnd;
                    if still_here {
                        // False trigger; re-arm tracking and wait for the real leave.
                        track_leave(hwnd);
                    } else {
                        if let Some(r) = app.panel.renderer.as_mut()
                            && r.hover.is_some()
                        {
                            r.hover = None;
                            invalidate_panel(hwnd);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let app = app_from_tray(hwnd);
                // Only the settings page scrolls; the main view's content is always
                // shorter than the viewport, scrolling would show nothing.
                if let Some(app) = app
                    && app.panel.view != PanelView::Main
                {
                    // The high word is a signed delta; scrolling back (negative)
                    // to see lower content → scroll_dy increases.
                    let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
                    // Three-line scheme: every 120 delta counts 3 lines ≈ 53 logical px.
                    let next = (app.panel.scroll_dy - delta / 120.0 * 53.0)
                        .clamp(0.0, app.panel.scroll_max);
                    if next != app.panel.scroll_dy {
                        app.panel.scroll_dy = next;
                        // When focus is on an input box, the IME composition window translates along with the content.
                        if app.panel.input.field.is_some() {
                            let r = app.panel.renderer.as_ref();
                            app.panel.update_caret(hwnd, r);
                        }
                        invalidate_panel(hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let Some(app) = app_from_tray(hwnd) else {
                    return LRESULT(0);
                };
                // Drag-selection wrap-up: caret positioning is already in
                // place, so skip hit dispatch — if the drag-selection
                // endpoint lands on another input box, that box's hit
                // handling must not fire and cause an accidental field switch.
                if app.panel.selecting {
                    app.panel.selecting = false;
                    // ReleaseCapture synchronously dispatches CAPTURECHANGED;
                    // the pump wrapper swaps in the new reference to reset the latch.
                    let app = pump(app, hwnd, |_| {
                        let _ = ReleaseCapture();
                    });
                    if let Some(app) = app {
                        app.panel.gesture.clear();
                    }
                    track_leave(hwnd);
                    return LRESULT(0);
                }
                // The release verdict goes through the shared PressGesture
                // primitive, same rule as the floating window;
                // release-point hit re-verification: a wheel scroll or
                // repositioning while held can move content under the
                // cursor; a mismatch between latched hit and release point
                // cancels, guarding against accidental activation of adjacent items.
                let release_hit = {
                    let (ux, uy) = (x_of(lparam) / app.panel.dpi, y_of(lparam) / app.panel.dpi);
                    app.panel.renderer.as_ref().and_then(|r| r.hit_at(ux, uy))
                };
                let mut cursor = POINT::default();
                let _ = GetCursorPos(&mut cursor);
                let verdict = app.panel.gesture.release((cursor.x, cursor.y), release_hit);
                // A blank-space press enters the drag state; an in-place click is not a
                // drag. The drag verdict precedes the release:
                // ReleaseCapture synchronously dispatches CAPTURECHANGED, which
                // clears drag_offset — take it before releasing.
                let was_drag = app.panel.drag_offset.take().is_some();
                // Capture return is a pump point: the reference is moved into the pump
                // wrapper; continued use goes through the new reference.
                let app = pump(app, hwnd, |_| {
                    let _ = ReleaseCapture();
                });
                if was_drag {
                    if let Some(app) = app {
                        if let Release::Accept { dragged: true, .. } = verdict {
                            // A real drag ends: restore the full height while keeping the current position.
                            app.panel.dragged = true;
                            let n = app.config.accounts.len();
                            let logical_h = app.panel.view_height(n);
                            app.panel.place(hwnd, LogPx(logical_h), true);
                            invalidate_panel(hwnd);
                        } else {
                            // A blank click ends the edit state and plaintext viewing; buffer text is kept.
                            app.panel.end_edit_state(hwnd);
                        }
                    }
                    return LRESULT(0);
                }
                if let (
                    Release::Accept {
                        hit: Some(pressed_hit),
                        dragged: false,
                    },
                    Some(_),
                ) = (verdict, app)
                {
                    // Dispatch uses the press-time latched hit, only after
                    // the release-point re-verification matches: a window
                    // move or content scroll while held would make
                    // recomputation pick an adjacent item.
                    // Hits that enter/keep the edit state; every other click
                    // ends input so the caret no longer lingers outside the box.
                    // The list converges in Hit::is_input_hit; when adding an
                    // input box, keep it maintained alongside the enum in the
                    // same file.
                    // An undispatched release (release point mismatching the
                    // latch, etc.) leaves the edit state untouched: a stray
                    // touch drift must not clear and interrupt an edit in
                    // progress; blank-click cleanup goes through the was_drag
                    // branch.
                    let refocus = pressed_hit.is_input_hit();
                    // Modal hits are intercepted inside the routing and
                    // direct-called two-phase, returning true: the dialog's
                    // nested pump dispatches other messages that re-fetch
                    // App, so after the pump this arm's reference is
                    // invalidated and must not be touched again; the
                    // clear_input below is likewise unusable. The edit state
                    // does not survive a dialog — the dialog grabbing focus
                    // fires KILLFOCUS which clears input, keeping buffer
                    // text consistent with the old path.
                    if crate::app::dispatch_hit(pressed_hit, hwnd) {
                        return LRESULT(0);
                    }
                    // Hit handling contains pump points; re-fetch the reference before wrapping up the edit state.
                    if !refocus && let Some(app) = app_from_tray(hwnd) {
                        app.panel.end_edit_state(hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_KEYDOWN => input::handle_keydown(hwnd, wparam),
            WM_CHAR => input::handle_char(hwnd, wparam),
            WM_KILLFOCUS => {
                let app = app_from_tray(hwnd);
                if let Some(app) = app
                    // Clear only when focus has truly moved away; if focus wrapped back to this window, keep it.
                    && windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() != hwnd
                {
                    // The fourth release path: plaintext viewing retracts
                    // on focus loss (no shoulder-surfing) and the edit
                    // state ends (an uncleared field would keep the panel
                    // counted as "typing" and never dismissed, with the
                    // IME composition window dangling). The sequence is
                    // shared with the hit-side release paths through
                    // end_edit_state.
                    app.panel.end_edit_state(hwnd);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => input::handle_lbuttondown(hwnd, lparam),
            WM_CAPTURECHANGED => {
                // lParam is the new window that got the capture (wParam unused); null or
                // held by someone else means this window's drag is broken off.
                if HWND(lparam.0 as *mut _) != hwnd {
                    let app = app_from_tray(hwnd);
                    if let Some(app) = app {
                        // After the capture is stolen by the system/another
                        // window there will be no more WM_LBUTTONUP; the drag
                        // state must end here, otherwise the next MOUSEMOVE
                        // still follows as a drag, forming a ghost drag.
                        app.panel.drag_offset = None;
                        app.panel.gesture.clear();
                        app.panel.selecting = false;
                    }
                    // TME leave tracking is canceled while captured; re-arm here; if
                    // the mouse pointer is already outside, MOUSELEAVE arrives immediately.
                    track_leave(hwnd);
                }
                LRESULT(0)
            }
            windows::Win32::UI::WindowsAndMessaging::WM_WINDOWPOSCHANGED => {
                // Default handling first: the window's internal position
                // state must be updated. Geometry changes reposition the
                // popup via an async message — this message is dispatched
                // synchronously with every SetWindowPos, so re-fetching App
                // here would invalidate the outer calling frame's reference
                // (modal pump discipline); the post carries no reference.
                // lParam stays un-dereferenced: a forged cross-process
                // message can carry a non-null garbage pointer, and the
                // realignment itself is idempotent — the pending flag
                // coalesces per-move posts into at most one in flight, the
                // handler's is_open absorbs no-op rounds, and a pure
                // Z-order/visibility change posting one extra round lands
                // the popup where it already is.
                let r = DefWindowProcW(hwnd, msg, wparam, lparam);
                if !REPOSITION_PENDING.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    let posted = PostMessageW(
                        Some(hwnd),
                        crate::platform::msg::WM_APP_REPOSITION_POPUP,
                        WPARAM(0),
                        LPARAM(0),
                    )
                    .is_ok();
                    // On post failure, roll back the pending flag:
                    // without the rollback nobody clears it, realignment
                    // is cut off from then on, and only closing and
                    // reopening the popup would self-heal.
                    if !posted {
                        REPOSITION_PENDING.store(false, std::sync::atomic::Ordering::Release);
                    }
                }
                r
            }
            msg if msg == crate::platform::msg::WM_APP_REPOSITION_POPUP => {
                // Async realignment: no outer reference is alive at queue-dispatch time;
                // if the popup is not open, return idempotently.
                REPOSITION_PENDING.store(false, std::sync::atomic::Ordering::Release);
                if let Some(app) = app_from_tray(hwnd) {
                    crate::app::reposition_popup(app, hwnd);
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                // Geometry first through the shared helper (its SetWindowPos
                // is a pump — no App reference is held across it), then
                // re-fetch for the state updates.
                let old = app_from_tray(hwnd)
                    .map(|app| app.panel.dpi)
                    .unwrap_or(FALLBACK_DPI);
                let applied = apply_dpi_change(hwnd, wparam, old);
                if let Some((dpi, _w, h_phys)) = applied
                    && let Some(app) = app_from_tray(hwnd)
                {
                    app.panel.dpi = dpi;
                    app.popup.invalidate_monitor();
                    // Display scale changed: rebuild the tray icon at the new pixel size.
                    crate::app::queue_tray_refresh(app.hwnd());
                    app.panel.height_anim = None;
                    app.panel.catch_up_layout();
                    crate::app::reconcile_fading_focus(app, hwnd);
                    let logical_h = app.panel.view_height(app.config.accounts.len());
                    app.panel.refresh_scroll(logical_h, h_phys);
                    invalidate_panel(hwnd);
                    // The helper resized from the old top-left
                    // (SWP_NOMOVE), lifting a tray-anchored panel off
                    // the taskbar: re-place from the anchor. Runs last —
                    // its internal SetWindowPos is a pump, and its tail
                    // re-syncs the caret to the new geometry (the scale
                    // change moved every row's y; without that an
                    // in-progress IME composition floats at the old
                    // position until the next keystroke).
                    if let Some(app) = app_from_tray(hwnd) {
                        crate::app::relayout_panel(app, hwnd);
                    }
                }
                LRESULT(0)
            }
            windows::Win32::UI::WindowsAndMessaging::WM_DISPLAYCHANGE => {
                // Resolution or work-area change: the popup's monitor-derived
                // cache is invalidated; the next realignment re-fetches.
                if let Some(app) = app_from_tray(hwnd) {
                    app.popup.invalidate_monitor();
                }
                LRESULT(0)
            }
            windows::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE => {
                // System parameter change: a taskbar move or auto-hide
                // toggle only changes the work area, sends no
                // DISPLAYCHANGE, and leaves the monitor handle unchanged —
                // the cache can only be invalidated by this message.
                if let Some(app) = app_from_tray(hwnd) {
                    app.popup.invalidate_monitor();
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SETCURSOR => {
                let hit_hwnd = HWND(wparam.0 as *mut _);
                if hit_hwnd == hwnd && (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                    let app = app_from_tray(hwnd);
                    let hand = app
                        .and_then(|a| a.panel.renderer.as_ref())
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
            WM_MOUSEACTIVATE => LRESULT(MA_ACTIVATE as isize),
            windows::Win32::UI::WindowsAndMessaging::WM_ACTIVATE => {
                // Activation revives the frozen marquee: this message fires
                // even for foreground switches within the same process
                // (clicking back to the panel from the tray menu), broader
                // coverage than WM_ACTIVATEAPP.
                // A non-zero low word of wparam means activated (click
                // activation or Alt-tab preemption).
                if (wparam.0 & 0xFFFF) != 0 {
                    wake_frozen_marquee(hwnd);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Animation frame
unsafe fn on_anim_tick(hwnd: HWND) -> LRESULT {
    unsafe {
        let app = app_from_tray(hwnd);
        // mut: re-assigned after the wrap-up relayout's pump (see below)
        let Some(mut app) = app else {
            let _ = KillTimer(Some(hwnd), TIMER_ANIM);
            return LRESULT(0);
        };
        let mut done = true;
        let mut footer_only = false;
        let mut repaint = false;
        let mut spin_active = false;
        let mut scroll_frame = false;
        let mut scroll_wake: Option<u32> = None;
        // Band-classification snapshot, read at the tick entry before
        // any animation state below is rewritten: the completion arms
        // clear appear/footer and drop height_anim inside this same
        // tick, so classifying from the post-mutation state would
        // describe the next frame, not this one — a settle frame banded
        // to a lone caret strip (footer tween expired + caret flip in
        // one tick) would freeze the swap animation's last mid-flight
        // pixels with no later frame left to repaint them. Spin is read
        // here too: the spin_remaining call below decrements it.
        let (spin_entry, appear_active, footer_active, footer_final) =
            match app.panel.renderer.as_ref() {
                Some(r) => (
                    r.anim.spin > 0.0,
                    r.anim.appear.is_some(),
                    r.anim.footer.as_ref().is_some_and(|f| !f.tween.finished()),
                    r.anim.footer.as_ref().is_some_and(|f| f.tween.finished()),
                ),
                None => (false, false, false, false),
            };
        // Height-transition presence at the entry: the block below drops
        // height_anim on completion, and its wrap-up pumps can re-arm a
        // different transition — this tick's frame is the entry one.
        let height_active = app.panel.height_anim.is_some();
        if let Some(r) = app.panel.renderer.as_mut() {
            if let Some(t) = &r.anim.appear {
                if t.finished() {
                    r.anim.appear = None;
                    // The final frame still needs a repaint: draw the end state to clear
                    // the intermediate-state ghosting.
                    repaint = true;
                } else {
                    done = false;
                    repaint = true;
                }
            }
            if let Some(f) = &r.anim.footer {
                if f.tween.finished() {
                    r.anim.footer = None;
                    // The final frame draws the static footer, clearing the sliding intermediate state.
                    repaint = true;
                } else {
                    done = false;
                    repaint = true;
                }
            }
            spin_active = r.spin_remaining();
            if spin_active {
                done = false;
                repaint = true;
            }
            // Footer error marquee: frame by frame while scrolling; while
            // paused, jump straight to the resume point. The foreground
            // gate shares its predicate with the floating-window patrol:
            // the foreground being any window of this program (tray menu,
            // popup) counts as present; a real focus loss freezes in place
            // without keep-alive, focus return restarts the clock via the
            // activation message to continue scrolling, and the background
            // gets zero wakes and zero repaints. Non-scrolling and already
            // frozen states skip the foreground query — three system calls
            // should not be paid for nothing on an empty frame.
            let active = r.error_scrolling() && crate::platform::foreground_is_self();
            match r.error_scroll_tick(active) {
                Some(crate::ui::panel::render::ScrollTick::Frame) => {
                    done = false;
                    repaint = true;
                    scroll_frame = true;
                }
                Some(crate::ui::panel::render::ScrollTick::Pause(w)) => {
                    done = false;
                    scroll_wake = Some(w);
                }
                Some(crate::ui::panel::render::ScrollTick::Idle) => {}
                None => {}
            }
            // When the footer change alone is active, the frame rate halves:
            // 30fps is enough for text sliding, halving the whole-window
            // repaint CPU.
            footer_only = footer_active && !appear_active && !spin_active;
        }
        // Height transition: frame-by-frame interpolation, anchoring and
        // clamping as in place, the final frame is exactly the end value;
        // NOCOPYBITS prevents per-frame CopyBits double screen writes from
        // ghosting; expansion eases out, shrink eases in.
        if let Some(a) = app.panel.height_anim {
            done = false;
            footer_only = false;
            let p = if a.to.0 > a.from.0 {
                anim::ease_out_cubic(a.tween.progress())
            } else {
                anim::ease_in_cubic(a.tween.progress())
            };
            let h = a.from.0 + ((a.to.0 - a.from.0) as f32 * p).round() as i32;
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            // The dragged state keeps the current position and only changes
            // the height; anchor-based opening anchors to the tray with top
            // clamping, same as place.
            if app.panel.dragged || app.panel.drag_offset.is_some() {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    wr.left,
                    wr.top,
                    wr.right - wr.left,
                    h,
                    SWP_NOACTIVATE | SWP_NOCOPYBITS,
                );
                repaint = true;
            } else if let Some(anchor) = app.panel.anchor {
                let mi = monitor_info(app.panel.place_monitor(hwnd));
                let y = (anchor.top - h - app.panel.px(8)).max(mi.rcWork.top + 8);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    wr.left,
                    y,
                    wr.right - wr.left,
                    h,
                    SWP_NOACTIVATE | SWP_NOCOPYBITS,
                );
                repaint = true;
            }
            // Either SetWindowPos above can synchronously dispatch
            // WM_DPICHANGED (a mixed-DPI crossing), whose arm re-derives
            // App and re-arms a new transition — this alias is dead past
            // the pump, exactly like the wrap-up branch below re-fetches.
            let Some(refetched) = app_from_tray(hwnd) else {
                return LRESULT(0);
            };
            app = refetched;
            if a.tween.finished() {
                app.panel.height_anim = None;
                // Shrink completed: the effective layout catches up to the
                // selected values; the relayout lands directly with no
                // second animation.
                // If the view height or clamped value changed within the
                // animation window, the comparison mismatches and one make-up
                // relayout runs.
                let want = app.panel.view_height(app.config.accounts.len());
                let moved = app.panel.dragged || app.panel.drag_offset.is_some();
                // Monitor selection shares the same source as place:
                // cross-monitor clamping differences are not a mismatch; the
                // wrap-up comparison only catches real target changes.
                let mi = monitor_info(app.panel.place_monitor(hwnd));
                if !app.panel.team.is_synced()
                    || !app.panel.customizing.is_synced()
                    || app.panel.target_h(&mi, LogPx(want), moved) != a.to
                {
                    app.panel.catch_up_layout();
                    crate::app::reconcile_fading_focus(app, hwnd);
                    crate::app::relayout_panel(app, hwnd);
                    // The wrap-up relayout goes through place → SetWindowPos,
                    // which can synchronously dispatch WM_DPICHANGED
                    // (cross-monitor) whose arm re-fetches App — this alias
                    // is dead past that call, re-fetch before the caret and
                    // clock work below.
                    let Some(a) = app_from_tray(hwnd) else {
                        return LRESULT(0);
                    };
                    app = a;
                }
            }
        }
        // Edit-state caret blink: the clock stays alive, and only a phase
        // crossing the flip point triggers a whole-window repaint;
        // double-gated by focus and edit state — no blinking while unfocused.
        let mut caret_wake: Option<u32> = None;
        let mut caret_flipped = false;
        let caret_active = app.panel.input.field.is_some()
            && windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == hwnd;
        if caret_active {
            done = false;
            let (on, wake_in) = app.panel.caret_blink();
            if on != app.panel.blink_drawn.get() {
                app.panel.blink_drawn.set(on);
                repaint = true;
                caret_flipped = true;
            }
            caret_wake = Some(wake_in);
        }
        if repaint {
            // Narrow-band selection: marquee / footer-swap frames change
            // only the footer strip, a lone caret blink only its row;
            // everything else (and any band-source overlap) repaints the
            // full window. The band is one-shot — consumed by the next
            // paint. An animation ending within this tick paints its
            // final frame unconditionally full: appear/height presence
            // already forces full for their settle frames; a footer swap
            // whose tween expired needs the explicit arm — footer_active
            // is already false and the tick above has torn the animation
            // down, so a same-tick caret flip must not clip the settle
            // frame to the caret strip.
            let full = appear_active || spin_entry || height_active || footer_final;
            let footer_frame = scroll_frame || footer_only;
            let band = if full || (footer_frame && caret_active) {
                None
            } else if footer_frame {
                // Footer strip with room for the pacman's falling crumbs.
                let mut cr = RECT::default();
                let _ = GetClientRect(hwnd, &mut cr);
                let lh = (cr.bottom - cr.top) as f32 / app.panel.dpi;
                Some((lh - layout::FOOTER_BAND_H, lh))
            } else if caret_flipped && let Some(field) = app.panel.input.field {
                let top = app.panel.caret_line_y(field) - layout::CARET_Y_OFFSET;
                Some((top - 6.0, top + layout::INPUT_H + 10.0))
            } else {
                None
            };
            if let Some(r) = app.panel.renderer.as_mut() {
                r.band_clip = band;
            }
            // Bare invalidation: this frame owns the band, no gate raise.
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        if done {
            app.panel.retune_anim(hwnd, 0);
        } else {
            // Frame rate: 16ms for frame-by-frame animations; marquee
            // scrolling and the footer change share the reduced-frame tier;
            // with neither, use precise wake-ups (marquee resume point /
            // caret flip point) — zero spinning during idle.
            let frame = if appear_active || spin_active || app.panel.height_anim.is_some() {
                Some(16u32)
            } else if scroll_frame || footer_only {
                Some(crate::ui::panel::render::ERROR_SCROLL_FRAME_MS)
            } else {
                None
            };
            // When the marquee resume point and the caret flip point coexist,
            // take the earlier one so neither suppresses the other.
            let wake = match (scroll_wake, caret_wake) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let want = compose_anim_period(frame, wake);
            // Re-arm only when the period changes: SetTimer resets the timing origin.
            app.panel.retune_anim(hwnd, want);
        }
        LRESULT(0)
    }
}

/// Animation clock period composition: when the frame tier and a precise
/// wake-up coexist take the earlier; with no wake-ups, fall back to one
/// frame. on_anim_tick's per-tick retuning and WM_PAINT's scroll-period
/// re-arm both draw on this single source; rhythm changes land only here.
fn compose_anim_period(frame: Option<u32>, wake: Option<u32>) -> u32 {
    match (frame, wake) {
        (Some(f), Some(w)) => f.min(w),
        (Some(f), None) => f,
        (None, Some(w)) => w.min(60_000),
        (None, None) => 16,
    }
    .max(1)
}

/// Full-window invalidation for the panel: raises the full-repaint gate
/// so a pending animation band cannot clip this invalidation away on the
/// next paint. Every panel invalidation outside the animation tick goes
/// through here.
pub(crate) fn invalidate_panel(hwnd: HWND) {
    crate::ui::panel::render::FULL_REPAINT.store(true, std::sync::atomic::Ordering::Release);
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

/// Height-chain pinned regression: expected values are derived from
/// draw_settings's y-accumulation chain; layout changes must update them
/// in sync
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_view_height_pinned() {
        let mut p = Panel::new();
        p.view = PanelView::Settings;
        assert_eq!(p.view_height(0), 796);
        assert_eq!(p.view_height(1), 844);
        p.customizing.effective = true;
        assert_eq!(p.view_height(0), 834);
        assert_eq!(p.view_height(1), 882);
        p.auth_error = true;
        assert_eq!(p.view_height(1), 900);
    }

    /// Release verdict matrix: same-hit accept, different-hit reject,
    /// blank press exempt from re-verification, beyond-threshold
    /// displacement carried with Accept, non-this-window press rejected,
    /// verdict consumes (a second release must reject)
    #[test]
    fn press_gesture_release() {
        use crate::ui::panel::render::Hit;
        let h = Hit::Refresh;
        let mut g = PressGesture::new();

        // Same hit released in place: accept and not a drag
        g.record((10, 10), Some(h));
        match g.release((12, 10), Some(h)) {
            Release::Accept { hit, dragged } => {
                assert_eq!(hit, Some(h));
                assert!(!dragged);
            }
            Release::Reject => panic!("同命中应接受"),
        }

        // Release-point hit mismatches the latch: reject
        g.record((10, 10), Some(h));
        assert!(matches!(
            g.release((10, 10), Some(Hit::Back)),
            Release::Reject
        ));

        // Control press, release on blank: mismatch rejects
        g.record((10, 10), Some(h));
        assert!(matches!(g.release((10, 10), None), Release::Reject));

        // Blank press: accept exempt from re-verification
        g.record((10, 10), None);
        assert!(matches!(
            g.release((11, 11), None),
            Release::Accept {
                hit: None,
                dragged: false
            }
        ));

        // Displacement beyond threshold (Manhattan > 8): accept but flag as drag
        g.record((0, 0), Some(h));
        match g.release((9, 0), Some(h)) {
            Release::Accept { dragged, .. } => assert!(dragged),
            Release::Reject => panic!("位移超阈仍属本窗按下"),
        }

        // Not pressed in this window (release without record): reject
        let mut idle = PressGesture::new();
        assert!(matches!(idle.release((5, 5), None), Release::Reject));

        // The verdict consumes: releasing again after a release must reject
        g.record((10, 10), Some(h));
        let _ = g.release((10, 10), Some(h));
        assert!(matches!(g.release((10, 10), Some(h)), Release::Reject));

        // After clear, likewise treated as not pressed in this window
        g.record((10, 10), Some(h));
        g.clear();
        assert!(matches!(g.release((10, 10), Some(h)), Release::Reject));
    }

    /// Heartbeat retuned-period split: a full 60 seconds keeps the minute tick,
    /// 59 seconds switches to the second tick, expired and missing keep the minute tick.
    #[test]
    fn minute_period_boundaries() {
        use chrono::TimeZone;
        let now = chrono::Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap();
        let at = |s: i64| now + chrono::Duration::seconds(s);
        assert_eq!(minute_period(None, now), 60_000);
        assert_eq!(minute_period(Some(at(60)), now), 60_000);
        assert_eq!(minute_period(Some(at(59)), now), 1000);
        assert_eq!(minute_period(Some(at(1)), now), 1000);
        // An expired moment keeps the minute tick, waiting for new data to swap in a new moment
        assert_eq!(minute_period(Some(at(-5)), now), 60_000);
    }

    /// Per-field cache slots: every field maps to a distinct slot inside the
    /// array — a new InputField variant missing from slot() is a compile
    /// error, and this pin catches a slot-count drift.
    #[test]
    fn input_field_slots_distinct() {
        let fields = [
            InputField::Interval,
            InputField::Proxy,
            InputField::PeakStart,
            InputField::PeakEnd,
            InputField::Name,
            InputField::Key,
            InputField::Org,
            InputField::Project,
        ];
        let slots: Vec<usize> = fields.iter().map(|f| f.slot()).collect();
        for s in &slots {
            assert!(*s < caret::CARET_SLOTS, "槽位越界");
        }
        let mut uniq = slots.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), fields.len(), "槽位互异");
    }

    /// Patrol decision: present clears the timer, off-panel counts from the
    /// first tick, strictly-greater-than dismisses, and reopening after a
    /// dismissal counts anew (the old timestamp is not carried over)
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

    /// Prefill clamp: within cap verbatim; over cap truncated at the byte
    /// cap on a char boundary — a multi-byte tail loses at most one char,
    /// never a split sequence
    #[test]
    fn clamp_prefill_char_boundary() {
        let max = InputField::Proxy.max_bytes();
        assert_eq!(
            InputField::Proxy.clamp_prefill(&"a".repeat(max)).len(),
            max,
            "贴上限原样保留"
        );
        // 2-byte CJK tail one byte short of fitting: the whole char goes
        let cjk = "a".repeat(max - 1) + "中中";
        assert_eq!(InputField::Proxy.clamp_prefill(&cjk).len(), max - 1);
        // 4-byte emoji tail: same whole-char retreat
        let emoji = "a".repeat(max - 1) + "😀😀";
        assert_eq!(InputField::Proxy.clamp_prefill(&emoji).len(), max - 1);
    }
}
