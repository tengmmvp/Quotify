//! Popup panel

pub mod anim;
pub mod caret;
pub mod layout;
pub mod model;
pub mod render;
pub mod text_edit;
pub mod theme;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, MonitorFromWindow, ValidateRect,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForMonitor;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DELETE, VK_END,
    VK_HOME, VK_LEFT, VK_RIGHT, VK_Y, VK_Z,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::ui::panel::model::PanelModel;
use crate::ui::panel::render::Renderer;
use crate::ui::panel::text_edit::EditState;
use crate::ui::panel::theme::PANEL_WIDTH;
use crate::ui::{x_of, y_of};

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

/// Fallback value when DPI probing fails
pub(crate) const FALLBACK_DPI: f32 = 1.5;

/// Logical→physical rounding policy: the one rounding decision every dpi
/// conversion delegates to — Panel::px/pxf and the floating windows'
/// pxf alike, so the panel and the popups can never round apart.
pub(crate) fn dpi_px(dpi: f32, logical: f32) -> i32 {
    (logical * dpi).round() as i32
}

/// Byte cap for a single-field input buffer
const INPUT_MAX_BYTES: usize = 128;
/// Byte cap for the proxy field: may carry userinfo credentials, longer than ordinary fields
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

/// Self-drawn input buffers; field order same as InputField
#[derive(Default)]
pub struct PanelInput {
    pub field: Option<InputField>,
    pub edit: crate::ui::panel::text_edit::EditState,
    pub surrogate: Option<u16>,
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
    pub pending_team: bool,
    pub layout_team: bool,
    pub input: PanelInput,
    pub(crate) key_revealed: bool,
    pub customizing_interval: bool,
    pub layout_customizing: bool,
    pub(crate) caret_ctx: (bool, bool),
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
    pub(crate) account_error: bool,
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
            pending_team: false,
            layout_team: false,
            input: PanelInput::default(),
            key_revealed: false,
            customizing_interval: false,
            layout_customizing: false,
            main_h: layout::EMPTY_VIEW_H as i32,
            account_error: false,
            anim_period: 0,
            height_anim: None,
            minute_period: std::cell::Cell::new(0),
            caret_ctx: (false, false),
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
    fn place_monitor(&self, hwnd: HWND) -> HMONITOR {
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
        self.view_height_for(self.layout_team, self.layout_customizing, accounts)
    }

    /// Catch the effective layout up to the selected values: the shared core
    /// of every transition-termination site (dismissal, drag start, DPI
    /// change, relayout, animation wrap-up). Sites clear height_anim and
    /// reconcile fading focus as their situation requires — register any
    /// future pending/effective layout pair here, not at the call sites.
    pub(crate) fn catch_up_layout(&mut self) {
        self.layout_team = self.pending_team;
        self.layout_customizing = self.customizing_interval;
    }

    /// Catch up render-derived fields: main view height, account error flag,
    /// caret context. The expand and relayout paths share this source;
    /// convergence at a single point prevents drift.
    pub(crate) fn sync_derived(
        &mut self,
        snap: Option<&crate::api::UsageSnapshot>,
        last_error: Option<&crate::api::FetchError>,
        has_accounts: bool,
    ) {
        self.main_h = layout::main_height_of(snap);
        self.account_error =
            last_error.is_some_and(|e| e.category() == crate::api::ErrorCategory::Unauthenticated);
        self.caret_ctx = (has_accounts, self.account_error);
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
                layout::settings_view_height(accounts > 0, self.account_error, customizing)
            }
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
    fn refresh_scroll(&mut self, logical_h: i32, physical_h: i32) {
        let visible = physical_h as f32 / self.dpi;
        self.scroll_max = (logical_h as f32 - visible).max(0.0);
        self.scroll_dy = if self.view == PanelView::Main {
            0.0
        } else {
            self.scroll_dy.min(self.scroll_max)
        };
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
    pub fn show_at(&mut self, parent: HWND, anchor: RECT, accounts: usize) -> Option<HWND> {
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
            let _ = InvalidateRect(Some(hwnd), None, true);
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
        self.input.interval.clear();
        self.input.proxy.clear();
        // The prefill marks reset with their buffers: the next settings
        // session prefills afresh from the config.
        self.input.interval_edited = false;
        self.input.proxy_edited = false;
        self.input.peak_start.clear();
        self.input.peak_end.clear();
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
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
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
                        let _ = InvalidateRect(Some(hwnd), None, false);
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
                    let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
                    // Close the popup only when displacement exceeds the
                    // threshold; below it, just follow. The decision is
                    // copied before the pumps: SetWindowPos can synchronously
                    // dispatch WM_DPICHANGED (cross-monitor), and close's
                    // SW_HIDE re-enters the pump again — both re-derive App,
                    // so the close runs through the re-fetched reference and
                    // nothing here uses this frame's alias afterwards.
                    let close_it =
                        app.popup.is_open() && app.panel.gesture.dragged_far((cursor.x, cursor.y));
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
                                let _ = InvalidateRect(Some(hwnd), None, false);
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
                            let _ = InvalidateRect(Some(hwnd), None, false);
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
                            let _ = InvalidateRect(Some(hwnd), None, false);
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
                        let _ = InvalidateRect(Some(hwnd), None, true);
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
                            let _ = InvalidateRect(Some(hwnd), None, true);
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
            WM_KEYDOWN => {
                // Editing key dispatch: movement, selection, word deletion,
                // and undo/redo go through EditState;
                // characters and combinations are handled by WM_CHAR.
                let vk = (wparam.0 & 0xFF) as u16;
                let app = app_from_tray(hwnd);
                if let Some(app) = app
                    && app.panel.input.field.is_some()
                    && [VK_DELETE, VK_END, VK_HOME, VK_LEFT, VK_RIGHT, VK_Y, VK_Z]
                        .iter()
                        .any(|k| vk == k.0)
                {
                    use windows::Win32::UI::Input::KeyboardAndMouse::{
                        GetKeyState, VK_CONTROL, VK_SHIFT,
                    };
                    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                    let input = &mut app.panel.input;
                    let acted = if vk == VK_LEFT.0 {
                        let t = input.active_str().to_string();
                        input.edit.move_left(&t, ctrl, shift);
                        true
                    } else if vk == VK_RIGHT.0 {
                        let t = input.active_str().to_string();
                        input.edit.move_right(&t, ctrl, shift);
                        true
                    } else if vk == VK_HOME.0 {
                        input.edit.move_home(shift);
                        true
                    } else if vk == VK_END.0 {
                        let t = input.active_str().to_string();
                        input.edit.move_end(&t, shift);
                        true
                    } else if vk == VK_DELETE.0 {
                        apply_edit(input, |e, t| e.delete(t, ctrl));
                        true
                    } else if vk == VK_Z.0 && ctrl {
                        // Shift+Z is a redo convention, same as Y.
                        apply_edit(input, |e, t| {
                            if shift {
                                e.redo(t);
                            } else {
                                e.undo(t);
                            }
                        });
                        true
                    } else if vk == VK_Y.0 && ctrl {
                        apply_edit(input, EditState::redo);
                        true
                    } else {
                        false
                    };
                    if acted {
                        app.panel.update_caret(hwnd, app.panel.renderer.as_ref());
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_CHAR => {
                // Character editing and clipboard combinations;
                // control characters dispatched by Windows edit-control convention.
                let app = app_from_tray(hwnd);
                if let Some(app) = app
                    && app.panel.input.field.is_some()
                {
                    let ch = (wparam.0 & 0xFFFF) as u16;
                    let mut confirm = false;
                    let key_masked =
                        app.panel.input.field == Some(InputField::Key) && !app.panel.key_revealed;
                    // UTF-16 surrogate pairs arrive as high then low half:
                    // stash the high half, and when the low half arrives
                    // assemble the supplementary-plane character; an
                    // ordinary character clears the stash on arrival,
                    // preventing a stale high half from mis-pairing with a
                    // later low half.
                    let decoded = {
                        let cp = ch as u32;
                        if crate::platform::is_utf16_high_surrogate(ch) {
                            app.panel.input.surrogate = Some(ch);
                            None
                        } else if (0xDC00..0xE000).contains(&cp) {
                            app.panel
                                .input
                                .surrogate
                                .take()
                                .map(|hi| 0x10000 + (((hi as u32) - 0xD800) << 10) + cp - 0xDC00)
                                .and_then(char::from_u32)
                        } else {
                            app.panel.input.surrogate = None;
                            char::from_u32(cp)
                        }
                    };
                    {
                        use windows::Win32::UI::Input::KeyboardAndMouse::{
                            GetKeyState, VK_CONTROL,
                        };
                        let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                        // Paste reads the clipboard first: reading is a
                        // cross-process synchronous pump (delayed-render
                        // handshake) and must not straddle the input-state
                        // borrow — an edit re-entering inside the pump
                        // invalidates the old borrow. The unit budget takes
                        // the field's byte-budget tier; an oversized
                        // clipboard is never materialized whole.
                        let pasted = if decoded == Some('\u{16}') {
                            // The field's byte budget doubles as the UTF-16
                            // unit cap: a unit costs up to 3 UTF-8 bytes, so
                            // this reads at most 3x the byte budget — the
                            // cap here only bounds materialization; paste
                            // re-enforces the byte cap downstream.
                            let budget = InputField::max_bytes_for(app.panel.input.field);
                            crate::platform::clipboard::read_text(budget)
                        } else {
                            None
                        };
                        // Clipboard read is a cross-process pump: re-fetch the
                        // reference before touching input state. A KILLFOCUS
                        // dispatched inside the pump clears the field; the
                        // stale reference would paste into active_buf's
                        // None-fallback (the interval buffer). Clipboard
                        // writes collect here and run after the borrow ends —
                        // same pump class.
                        let mut clip_write: Option<String> = None;
                        if let Some(app) = app_from_tray(hwnd) {
                            let input = &mut app.panel.input;
                            // The byte budget takes its tier by field: IME
                            // composition results also pass through here.
                            let max = InputField::max_bytes_for(input.field);
                            match decoded {
                                Some('\r') | Some('\n') => confirm = true,
                                Some('\u{8}') => {
                                    apply_edit(input, |e, t| e.backspace(t, ctrl));
                                }
                                Some('\u{7F}') => {
                                    // Ctrl+Backspace: delete the previous word.
                                    apply_edit(input, |e, t| e.backspace(t, true));
                                }
                                Some('\u{1}') => {
                                    let t = input.active_str().to_string();
                                    input.edit.select_all(&t);
                                }
                                Some('\u{3}') if !key_masked => {
                                    // Single-line convention: with no
                                    // selection, copy everything.
                                    let t = input.active_str().to_string();
                                    clip_write = input
                                        .edit
                                        .copy(&t)
                                        .map(str::to_string)
                                        .or_else(|| (!t.is_empty()).then_some(t));
                                }
                                Some('\u{18}') if !key_masked => {
                                    // Single-line convention: with no
                                    // selection, cut everything.
                                    apply_edit(input, |e, t| {
                                        if e.selection().is_none() {
                                            if t.is_empty() {
                                                return;
                                            }
                                            e.select_all(t);
                                        }
                                        clip_write = e.cut(t);
                                    });
                                }
                                Some('\u{16}') => {
                                    // Paste is void when the field was cleared
                                    // inside the read pump: the buffered text
                                    // belongs to a field that no longer exists.
                                    if input.field.is_some()
                                        && let Some(text) = pasted
                                    {
                                        apply_edit(input, |e, t| e.paste(t, &text, max));
                                    }
                                }
                                Some(c) if !c.is_control() => {
                                    apply_edit(input, |e, t| e.insert(t, c, max));
                                }
                                _ => {}
                            }
                        }
                        if let Some(s) = clip_write {
                            crate::platform::clipboard::write_text(hwnd, &s);
                        }
                    }
                    if confirm && let Some(app) = app_from_tray(hwnd) {
                        crate::app::confirm_panel_input(app, hwnd);
                    }
                    // The confirm path may trigger a system notification
                    // (WinRT calls can re-enter the message pump); the
                    // reference is dead after the pump: the shared tail
                    // re-fetches and re-syncs — a no-op when not confirmed.
                    if sync_caret_after_geometry(hwnd) {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
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
            WM_LBUTTONDOWN => {
                // Pressing any blank spot enters manual dragging so a long
                // settings page can be dragged into view.
                // No system HTCAPTION modal dragging: on Win11 dragging to
                // the screen's top edge forces the snap preview.
                let mut app = app_from_tray(hwnd);
                let (x, y) = if let Some(app) = app.as_ref() {
                    (x_of(lparam) / app.panel.dpi, y_of(lparam) / app.panel.dpi)
                } else {
                    (0.0, 0.0)
                };
                // Compute the hit once: blank detection, press latching, and input-field mapping share it.
                let down_hit = app
                    .as_ref()
                    .and_then(|a| a.panel.renderer.as_ref())
                    .and_then(|r| r.hit_at(x, y));
                let hit_none = down_hit.is_none();
                let mut cursor = POINT::default();
                let _ = GetCursorPos(&mut cursor);
                if let Some(app) = app.as_mut() {
                    app.panel.gesture.record((cursor.x, cursor.y), down_hit);
                    // Press in an input box's text zone: position the caret
                    // and enter drag selection, not blank dragging.
                    // An already-focused box positions directly; an
                    // unfocused box focuses on press then positions at the
                    // click, so the second hit of a double click can already
                    // select a word. Multi-click judged by 500ms/8px: double
                    // click selects a word, triple click selects all; with
                    // Shift held, a click extends the selection instead of
                    // resetting it.
                    let active_zone = app.panel.hit_text_zone(x, y);
                    let target = if active_zone {
                        app.panel.input.field
                    } else {
                        down_hit.and_then(input_field_of_hit)
                    };
                    if let Some(f) = target {
                        if !active_zone {
                            app.panel.focus_input(hwnd, f);
                        }
                        let Some(r) = app.panel.renderer.as_ref() else {
                            return LRESULT(0);
                        };
                        let now = windows::Win32::System::SystemInformation::GetTickCount();
                        let (px, py) = ((x * app.panel.dpi) as i32, (y * app.panel.dpi) as i32);
                        let count = match app.panel.text_clicks {
                            Some((t, cx, cy, n))
                                if now.wrapping_sub(t) <= 500
                                    && manhattan(px, py, cx, cy) <= DRAG_SLOP =>
                            {
                                n.saturating_add(1).min(3)
                            }
                            _ => 1,
                        };
                        app.panel.text_clicks = Some((now, px, py, count));
                        // Hit conversion and caret positioning share one width-table layout.
                        let cl = app.panel.caret_layout(r);
                        let pos = app.panel.caret_hit_test(cl.as_ref(), f, x);
                        use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT};
                        let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                        match count {
                            2 => {
                                // Double click selects a word; Key masked dots carry no word meaning, degrades to select-all.
                                let real = app.panel.input.active_str().to_string();
                                if f == InputField::Key && !app.panel.key_revealed {
                                    app.panel.input.edit.select_all(&real);
                                } else {
                                    let (a, b) = text_edit::word_range_at(&real, pos);
                                    app.panel.input.edit.place(a, false);
                                    app.panel.input.edit.place(b, true);
                                }
                            }
                            3 => {
                                let real = app.panel.input.active_str().to_string();
                                app.panel.input.edit.select_all(&real);
                            }
                            _ => {
                                app.panel.input.edit.place(pos, shift);
                            }
                        }
                        app.panel.update_caret(hwnd, app.panel.renderer.as_ref());
                        app.panel.selecting = true;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        let _ = SetCapture(hwnd);
                        return LRESULT(0);
                    }
                    if hit_none {
                        // Dragging takes over position; an in-progress
                        // transition yields and catches up, and the fading
                        // focus is reconciled; translation relies on
                        // CopyBits without repaint, so an explicit
                        // invalidation is required to avoid stopping in the
                        // originating layout.
                        // Catching up means landing: releasing in place
                        // skips the drag path, and the window would be stuck
                        // at an intermediate animation height.
                        app.panel.height_anim = None;
                        app.panel.catch_up_layout();
                        crate::app::reconcile_fading_focus(app, hwnd);
                        let n = app.config.accounts.len();
                        let logical_h = app.panel.view_height(n);
                        app.panel.place(hwnd, LogPx(logical_h), false);
                        // place's SetWindowPos can synchronously dispatch
                        // WM_DPICHANGED (cross-monitor), whose arm re-derives
                        // App: the drag latch writes go through the
                        // re-fetched reference.
                        if let Some(app) = app_from_tray(hwnd) {
                            let _ = InvalidateRect(Some(hwnd), None, false);
                            let mut wr = RECT::default();
                            let _ = GetWindowRect(hwnd, &mut wr);
                            app.panel.drag_offset = Some((cursor.x - wr.left, cursor.y - wr.top));
                            let _ = SetCapture(hwnd);
                        }
                    } else {
                        // A press on a hit target captures too: releasing
                        // outside after dragging out still routes UP back to
                        // this window, so the gesture latch is necessarily
                        // reset with UP — no left-button state heuristics
                        // needed.
                        let _ = SetCapture(hwnd);
                    }
                }
                LRESULT(0)
            }
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
                    let _ = InvalidateRect(Some(hwnd), None, true);
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
        let mut appear_active = false;
        let mut spin_active = false;
        let mut scroll_frame = false;
        let mut scroll_wake: Option<u32> = None;
        if let Some(r) = app.panel.renderer.as_mut() {
            appear_active = r.anim.appear.is_some();
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
            let footer_active = r.anim.footer.as_ref().is_some_and(|f| !f.tween.finished());
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
                if app.panel.layout_team != app.panel.pending_team
                    || app.panel.layout_customizing != app.panel.customizing_interval
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
        let caret_active = app.panel.input.field.is_some()
            && windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == hwnd;
        if caret_active {
            done = false;
            let (on, wake_in) = app.panel.caret_blink();
            if on != app.panel.blink_drawn.get() {
                app.panel.blink_drawn.set(on);
                repaint = true;
            }
            caret_wake = Some(wake_in);
        }
        if repaint {
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

/// Single entry to get App from any window hwnd of this program: the tray
/// window's USERDATA is App and hits directly; the panel and floating
/// windows go through their parent. There were once two lookup functions,
/// and the tray arm mistakenly using the parent-chain version silently
/// returned None — the single entry eliminates that wrong-pick possibility.
pub(crate) fn app_from_tray(hwnd: HWND) -> Option<&'static mut crate::app::App> {
    unsafe {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if p != 0 {
            return Some(&mut *(p as *mut crate::app::App));
        }
        let parent = GetWindowLongPtrW(hwnd, GWLP_HWNDPARENT);
        if parent == 0 {
            return None;
        }
        let p = GetWindowLongPtrW(HWND(parent as *mut _), GWLP_USERDATA);
        (p != 0).then(|| &mut *(p as *mut crate::app::App))
    }
}

/// Post-pump caret re-sync shared by the geometry-change tails (relayout,
/// DPI change, key input, drag translation): re-fetch the reference and
/// move the caret/IME composition window to the row's new position — a
/// hand-copied tail per site is how the next site forgets. Returns
/// whether a live App answered (callers gate their repaint on it).
pub(crate) fn sync_caret_after_geometry(hwnd: HWND) -> bool {
    match app_from_tray(hwnd) {
        Some(app) => {
            app.panel.update_caret(hwnd, app.panel.renderer.as_ref());
            true
        }
        None => false,
    }
}

/// Pump wrapper for the App reference: f runs a synchronous re-entrant
/// message pump (capture release, layout sync, foreground claim, etc.)
/// and receives the caller's reference as its parameter — the reference
/// is consumed by the move into the call, and the continuation works on
/// the re-fetched reference. One form serves both shapes: closures that
/// need the App inside the pump take it from the parameter, closures
/// that must not touch it ignore the parameter (passing anything else
/// captured would alias). A failed re-fetch returns None; the
/// continuation early-exits on the empty path.
pub(crate) fn pump(
    app: &mut crate::app::App,
    hwnd: HWND,
    f: impl FnOnce(&mut crate::app::App),
) -> Option<&'static mut crate::app::App> {
    f(app);
    app_from_tray(hwnd)
}

/// Run a mutating edit on the active buffer: clone out the text → operate
/// via EditState → write back. EditState and the buffers both live in
/// PanelInput; borrowing both fields directly is mutually exclusive, and
/// the clone decouples them. The edited marks latch only on a real change:
/// no-op edits (all-filtered paste, over-budget insert, empty-stack undo)
/// must not close the prefill gates while the buffer is untouched.
fn apply_edit(input: &mut PanelInput, f: impl FnOnce(&mut EditState, &mut String)) {
    let mut t = input.active_str().to_string();
    f(&mut input.edit, &mut t);
    let changed = t != input.active_str();
    *input.active_buf() = t;
    if changed {
        match input.field {
            Some(InputField::Interval) => input.interval_edited = true,
            Some(InputField::Proxy) => input.proxy_edited = true,
            _ => {}
        }
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
        p.layout_customizing = true;
        assert_eq!(p.view_height(0), 834);
        assert_eq!(p.view_height(1), 882);
        p.account_error = true;
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
}
