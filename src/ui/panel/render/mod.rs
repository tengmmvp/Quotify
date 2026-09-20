//! Panel content rendering

#![allow(unsafe_op_in_unsafe_fn)]

pub mod about;
mod main;
pub mod popup;
mod settings;
mod widgets;

use std::collections::HashMap;

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_NO_SNAP,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_ROUNDED_RECT, D2D1CreateFactory, ID2D1Factory,
    ID2D1HwndRenderTarget, ID2D1PathGeometry, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWriteCreateFactory,
    IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::core::PCWSTR;
use windows_numerics::Vector2;

use super::anim::{Tween, animations_allowed};
use super::model::PanelModel;
use super::theme::Theme;
use super::{Panel, PanelView};
use crate::platform::wide;

/// Appearance mode choice: System = follow the system
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppearanceChoice {
    System,
    Light,
    Dark,
}

impl AppearanceChoice {
    /// Config string → choice: unset / unknown values both map to "follow the system",
    /// the same semantics as explicit_appearance so the control highlight never lands on
    /// the wrong preset.
    pub fn from_setting(setting: Option<&str>) -> Self {
        match crate::ui::panel::theme::explicit_appearance(setting) {
            Some(crate::ui::panel::theme::Appearance::Light) => AppearanceChoice::Light,
            Some(crate::ui::panel::theme::Appearance::Dark) => AppearanceChoice::Dark,
            None => AppearanceChoice::System,
        }
    }

    /// Choice → config string: System writes None (follow the system).
    pub fn to_config(self) -> Option<String> {
        match self {
            AppearanceChoice::System => None,
            AppearanceChoice::Light => Some("light".to_string()),
            AppearanceChoice::Dark => Some("dark".to_string()),
        }
    }
}

/// UI language choice: System = follow the system
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageChoice {
    System,
    Zh,
    En,
}

impl LanguageChoice {
    /// Config string → choice: unset / unknown values both map to "follow the system",
    /// the same semantics as explicit_lang so the control highlight never lands on the wrong preset.
    pub fn from_setting(setting: Option<&str>) -> Self {
        match crate::ui::i18n::explicit_lang(setting) {
            Some(crate::ui::i18n::Lang::Zh) => LanguageChoice::Zh,
            Some(crate::ui::i18n::Lang::En) => LanguageChoice::En,
            None => LanguageChoice::System,
        }
    }

    /// Choice → config string: System writes None (follow the system).
    pub fn to_config(self) -> Option<String> {
        match self {
            LanguageChoice::System => None,
            LanguageChoice::Zh => Some("zh".to_string()),
            LanguageChoice::En => Some("en".to_string()),
        }
    }
}

/// Scope choice when adding an account: personal / team; team is domestic-site only
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeChoice {
    Personal,
    Team,
}

/// Clickable element identifier used for hit testing; grouped by UI scene,
/// mirroring the match-arm grouping of handle_panel_hit
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    // ── Main view ──
    Refresh,
    Settings,
    AccountSwitch,
    UsageInfo,

    // ── Navigation ──
    Back,
    ClosePanel,

    // ── Settings · Poll interval ──
    IntervalPreset(u64),
    CustomizeInterval,
    ApplyInterval,
    InputInterval,

    // ── Settings · General ──
    Language(LanguageChoice),
    Appearance(AppearanceChoice),
    ToggleAutostart,

    // ── Settings · Network proxy ──
    InputProxy,
    ApplyProxy,

    // ── Settings · Usage notifications ──
    ToggleThreshold,
    ToggleReset5h,
    ToggleResetWeekly,

    // ── Settings · Peak hours ──
    InputPeakStart,
    InputPeakEnd,
    ApplyPeak,

    // ── Settings · Accounts ──
    AddAccount,
    RemoveAccount(usize),
    PickAccount(usize),
    AccountType(ScopeChoice),
    InputName,
    InputKey,
    RevealKey,
    InputOrg,
    InputProject,
    SaveAccount,
    Platform(crate::api::Platform),

    // ── Settings · Config management & about ──
    ExportConfig,
    ImportConfig,
    CheckUpdate,
    OpenDownload,

    // ── About window ──
    LinkRepo,
    LinkIssues,
    CopyDiagnostics,
    NewsItem(usize),
    AboutLogo,
}

/// Exhaustive registry of non-slot hits that keep the input state after
/// a click (apply buttons keep the field for editing when validation
/// fails, same semantics as the Enter path): every Hit variant is listed,
/// so a new variant that must keep the input state fails to compile here
/// instead of silently clearing it (caret gone, in-flight IME
/// composition dropped).
pub(crate) const fn keeps_input_state_hit(h: Hit) -> bool {
    match h {
        Hit::CustomizeInterval
        | Hit::AddAccount
        | Hit::RevealKey
        | Hit::ApplyProxy
        | Hit::ApplyInterval
        | Hit::ApplyPeak
        | Hit::SaveAccount => true,
        Hit::Refresh
        | Hit::Settings
        | Hit::AccountSwitch
        | Hit::UsageInfo
        | Hit::Back
        | Hit::ClosePanel
        | Hit::IntervalPreset(_)
        | Hit::InputInterval
        | Hit::Language(_)
        | Hit::Appearance(_)
        | Hit::ToggleAutostart
        | Hit::InputProxy
        | Hit::ToggleThreshold
        | Hit::ToggleReset5h
        | Hit::ToggleResetWeekly
        | Hit::InputPeakStart
        | Hit::InputPeakEnd
        | Hit::RemoveAccount(_)
        | Hit::PickAccount(_)
        | Hit::AccountType(_)
        | Hit::InputName
        | Hit::InputKey
        | Hit::InputOrg
        | Hit::InputProject
        | Hit::Platform(_)
        | Hit::ExportConfig
        | Hit::ImportConfig
        | Hit::CheckUpdate
        | Hit::OpenDownload
        | Hit::LinkRepo
        | Hit::LinkIssues
        | Hit::CopyDiagnostics
        | Hit::NewsItem(_)
        | Hit::AboutLogo => false,
    }
}

/// Exhaustive modal registry: every Hit variant is listed, so a new
/// modal variant fails to compile here until registered — and this
/// match's true arm and crate::app::dispatch_modal's dispatch arm must
/// be extended in the same change (a dispatch arm missed after
/// registration is still caught by dispatch_modal's fallback log).
pub(crate) const fn is_modal_hit(h: Hit) -> bool {
    match h {
        Hit::ExportConfig | Hit::ImportConfig => true,
        Hit::Refresh
        | Hit::Settings
        | Hit::AccountSwitch
        | Hit::UsageInfo
        | Hit::Back
        | Hit::ClosePanel
        | Hit::IntervalPreset(_)
        | Hit::CustomizeInterval
        | Hit::ApplyInterval
        | Hit::InputInterval
        | Hit::Language(_)
        | Hit::Appearance(_)
        | Hit::ToggleAutostart
        | Hit::InputProxy
        | Hit::ApplyProxy
        | Hit::ToggleThreshold
        | Hit::ToggleReset5h
        | Hit::ToggleResetWeekly
        | Hit::InputPeakStart
        | Hit::InputPeakEnd
        | Hit::ApplyPeak
        | Hit::AddAccount
        | Hit::RemoveAccount(_)
        | Hit::PickAccount(_)
        | Hit::AccountType(_)
        | Hit::InputName
        | Hit::InputKey
        | Hit::RevealKey
        | Hit::InputOrg
        | Hit::InputProject
        | Hit::SaveAccount
        | Hit::Platform(_)
        | Hit::CheckUpdate
        | Hit::OpenDownload
        | Hit::LinkRepo
        | Hit::LinkIssues
        | Hit::CopyDiagnostics
        | Hit::NewsItem(_)
        | Hit::AboutLogo => false,
    }
}

/// Hit predicates are kept next to the enum; new input-field variants must be registered
/// here in sync
impl Hit {
    /// Hits that enter/keep the input state or the key plain-text view state after a click;
    /// WM_LBUTTONUP uses this to decide whether to end the input state after the click.
    /// Input-field slots are decided exhaustively via input_field_of_hit;
    /// the non-slot additions come from the exhaustive keeps_input_state_hit.
    pub(crate) fn is_input_hit(&self) -> bool {
        crate::ui::panel::input_field_of_hit(*self).is_some() || keeps_input_state_hit(*self)
    }

    /// Modal hits: dispatch must go through the window-procedure-layer interception into
    /// the direct two-stage call;
    /// reaching handle_panel_hit is a wiring error. The registry is the
    /// exhaustive is_modal_hit; when adding a modal variant, register it in
    /// crate::app::dispatch_modal in sync.
    pub(crate) fn is_modal(&self) -> bool {
        is_modal_hit(*self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Align {
    Left,
    Center,
}

/// Interpolation state of the panel's animations: fade-in, refresh spin, footer swap, error marquee
pub struct AnimState {
    pub appear: Option<Tween>,
    pub spin: f32,
    pub footer: Option<FooterAnim>,
    pub error_scroll: Option<ErrorScroll>,
}

/// Single-run state of the footer swap animation: old and new text are held as
/// prebuilt layouts, zero layout creation on animation frames
pub struct FooterAnim {
    pub tween: Tween,
    pub old: TextBlock,
    pub new: TextBlock,
}

/// Full-value form of the marquee rebuild-decision key: the snapshot time leads —
/// being equal across frames short-circuits the
/// remaining field comparisons. The visible width is stored by bits (f32 semantic
/// equality is bitwise equality).
pub(super) type ScrollKey = (
    chrono::DateTime<chrono::Local>,
    crate::api::FetchError,
    u32,
    crate::ui::i18n::Lang,
    bool,
);

/// Scroll state of the footer error summary: when the summary exceeds the panel width it
/// loops conveyor-belt style, pausing a few seconds at the
/// start and after each lap returns to the beginning. Layouts and widths are computed at
/// creation; scroll frames do zero measuring, zero allocation,
/// and zero COM object rebuilds
pub struct ErrorScroll {
    /// Content key: a full-value tuple of error payload, snapshot time, visible width,
    /// language, and the animation toggle;
    /// any change triggers a rebuild; full-value comparison has no hash-collision missed updates
    key: ScrollKey,
    /// Three content blocks (prefix / category / tail): layout and width are held as
    /// a pair, consumers do no re-layout;
    /// under reduced motion the over-wide case stores the truncated string's layout
    blocks: [ScrollBlock; 3],
    /// Left and right brackets delimiting the blocks: bold weight, shared by the three blocks
    bracket: ScrollBlock,
    bracket_end: ScrollBlock,
    /// Whether the summary is over-wide and scrolls: the non-scrolling state is
    /// cache-only, tick does not keep it alive
    scrolling: bool,
    /// Frozen while unfocused: the timer is already removed and reactivation wakes it;
    /// both the re-mount decision and advancement bypass it
    frozen: bool,
    /// Current offset, 0..total_w
    offset: f32,
    /// Pause deadline; a past instant while scrolling
    resume_at: std::time::Instant,
    /// Last frame's time; the frame rate varies with animation priority,
    /// advance by the actual elapsed time
    last: std::time::Instant,
}

/// Text block with a prebuilt layout: layout and width/height are held as one,
/// the cache and direct drawing share the same copy; marquee
/// content blocks share this structure with main-view frame-cache text.
/// The height serves the vertical-centering math in direct drawing.
/// Clone is a COM refcount bump — cached blocks hand out cheap copies to
/// callers that cannot hold the cache's borrow across &mut self drawing.
#[derive(Clone)]
pub(crate) struct TextBlock {
    pub(crate) layout: Option<IDWriteTextLayout>,
    pub(crate) w: f32,
    pub(crate) h: f32,
}

impl TextBlock {
    /// Build a block and read its metrics from the layout; a layout that
    /// fails to build yields an empty block
    pub(crate) unsafe fn new(layout: Option<IDWriteTextLayout>) -> Self {
        layout.as_ref().map_or(
            Self {
                layout: None,
                w: 0.0,
                h: 0.0,
            },
            |l| {
                let (w, h) = Renderer::layout_metrics(l);
                Self {
                    layout: Some(l.clone()),
                    w,
                    h,
                }
            },
        )
    }
}

/// Drawn-text tier of an input field: the placeholder (12/400 non-mono)
/// and the body (INPUT_FONT mono) are distinct cache tiers — keying by
/// string alone would hand one tier's font to the other when both draw
/// the same text (a peak preset pasted back verbatim).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum InputStyle {
    Placeholder,
    Body,
}

/// input_blocks cache entry: tier, display string, prebuilt layout.
pub(super) struct InputBlockEntry {
    style: InputStyle,
    text: String,
    block: TextBlock,
}

/// Marquee content blocks reuse the TextBlock structure
type ScrollBlock = TextBlock;

impl ErrorScroll {
    /// Scroll period: total width of the on-screen blocks plus the inter-segment gap of the loop
    pub(crate) fn total_w(&self) -> f32 {
        belt_width(
            &self.blocks.each_ref().map(|b| b.w),
            self.bracket.w + self.bracket_end.w,
        ) + ERROR_SCROLL_GAP
    }
}

/// Total width of the on-screen blocks (content width + bracket pair):
/// empty blocks contribute zero, the period carries no trailing blank.
/// Overflow detection and the scroll period share this source; block-structure changes
/// only touch this one place
pub(crate) fn belt_width(blocks: &[f32], pair: f32) -> f32 {
    blocks.iter().filter(|w| **w > 0.0).map(|w| *w + pair).sum()
}

/// Marquee frame interval
/// At 30px/s, 33ms advances about 1px per frame, halving full-window redraw cost to the
/// same tier as the footer swap.
pub(crate) const ERROR_SCROLL_FRAME_MS: u32 = 33;
/// Marquee scroll speed (logical px/second)
pub(crate) const ERROR_SCROLL_SPEED: f32 = 30.0;
/// Pause duration after a lap completes
pub(crate) const ERROR_SCROLL_PAUSE: std::time::Duration = std::time::Duration::from_secs(3);
/// Inter-segment gap in the looping tiling (logical px)
pub(crate) const ERROR_SCROLL_GAP: f32 = 48.0;
/// Marquee text font size: the three content blocks and the brackets share one size
pub(crate) const ERROR_SCROLL_SIZE: f32 = 12.0;
/// Block-delimiting bracket weight (bold): content uses regular weight 400
pub(crate) const ERROR_SCROLL_BRACKET_WEIGHT: u16 = 700;

/// Single-tick result of the marquee
pub(crate) enum ScrollTick {
    /// Scroll frame, frame interval ERROR_SCROLL_FRAME_MS
    Frame,
    /// Paused, milliseconds until resumption
    Pause(u32),
    /// Frozen by the foreground gate, no advancing or redrawing; not kept alive,
    /// refocus restarts the timer via the activation message
    Idle,
}

impl AnimState {
    pub fn new() -> Self {
        Self {
            appear: None,
            spin: 0.0,
            footer: None,
            error_scroll: None,
        }
    }
}

impl Renderer {
    /// Whether the refresh-button animation is still running
    pub fn spin_remaining(&mut self) -> bool {
        if !self.anim_allowed {
            self.anim.spin = 0.0;
            return false;
        }
        if self.anim.spin > 0.0 {
            self.anim.spin = (self.anim.spin - 0.15).max(0.0);
            true
        } else {
            false
        }
    }

    /// Trigger one refresh-button spin
    pub fn start_spin(&mut self) {
        self.anim.spin = std::f32::consts::TAU;
    }

    /// Start the footer swap animation: the old and new text blocks are prepared by the
    /// caller (the start point takes the on-screen frozen block,
    /// the end point is prebuilt once); teardown on error departure is handled uniformly
    /// by the draw entry from the data
    pub fn start_footer_anim(&mut self, old: TextBlock, new: TextBlock) {
        self.anim.footer = Some(FooterAnim {
            tween: crate::ui::panel::anim::Tween::now(3600),
            old,
            new,
        });
    }

    /// Take the current footer text block: the start point of the swap animation (the
    /// on-screen frozen block); None when absent
    pub(crate) fn take_footer_block(&mut self) -> Option<TextBlock> {
        let empty = || unsafe { TextBlock::new(None) };
        self.main_texts
            .as_mut()
            .map(|t| std::mem::replace(&mut t.footer, empty()))
    }

    /// Footer typography (12/400): frame-cache construction, minute refresh, and
    /// the swap animation all share this one source,
    /// the layout parameters are written in exactly one place
    pub(crate) unsafe fn make_footer_block(&self, s: &str) -> TextBlock {
        TextBlock::new(self.make_layout(s, 12.0, 400, false))
    }

    /// Advance the marquee by one frame
    /// Focus loss is checked before the pause decision: freezing takes effect
    /// immediately, no extra keep-alive tick just to wait for the resume point;
    /// during a pause the timer is set once to the resume point with zero idle spinning;
    /// the non-scrolling and reduced-motion states are not kept alive;
    /// refocus restarts the timer via the activation message, resuming without a jump.
    pub fn error_scroll_tick(&mut self, active: bool) -> Option<ScrollTick> {
        let sc = self.anim.error_scroll.as_mut()?;
        if !sc.scrolling || !self.anim_allowed {
            return None;
        }
        let now = std::time::Instant::now();
        if !active {
            sc.frozen = true;
            // While inside the end-of-lap pause, last stays aligned to the
            // resume point (a past instant) so the remaining pause is not
            // paid out as displacement on the first post-resume frame; the
            // reset only applies mid-scroll, where the frozen stretch must
            // not count toward the next frame either.
            if now >= sc.resume_at {
                sc.last = now;
            }
            return Some(ScrollTick::Idle);
        }
        // During a pause last is not advanced: when a lap completes,
        // last is already aligned to the resume point, so the first frame after
        // resumption counts only one frame interval of displacement.
        if now < sc.resume_at {
            return Some(ScrollTick::Pause((sc.resume_at - now).as_millis() as u32));
        }
        sc.frozen = false;
        let dt = now.saturating_duration_since(sc.last).as_secs_f32();
        sc.last = now;
        sc.offset += ERROR_SCROLL_SPEED * dt;
        if sc.offset >= sc.total_w() {
            // A full lap scrolled: the text start returns to the left and
            // scrolling pauses before resuming; last is synced to the resume point,
            // so the pause duration is excluded from the first frame's displacement.
            sc.offset = 0.0;
            sc.resume_at = now + ERROR_SCROLL_PAUSE;
            sc.last = sc.resume_at;
        }
        Some(ScrollTick::Frame)
    }

    /// The marquee is scrolling and not frozen: the predicate for whether the timer
    /// should be present, shared by the re-mount points.
    /// The frozen state does not re-mount — a mounted timer would just idle one tick and
    /// be removed again.
    pub fn error_scrolling(&self) -> bool {
        self.anim
            .error_scroll
            .as_ref()
            .is_some_and(|sc| sc.scrolling && !sc.frozen)
    }

    /// The footer's current frozen string (the frame-cache value): byte-identical to the
    /// swap animation's on-screen
    /// start point, no jump across minute boundaries
    pub(crate) fn cached_footer(&self) -> Option<&str> {
        self.main_texts.as_ref().map(|t| t.footer_text.as_str())
    }

    /// Reset the hit table and hover together: after the account set changes,
    /// all old rectangles are invalid
    pub(crate) fn reset_hits(&mut self) {
        self.hits.clear();
        self.hover = None;
    }

    /// Revive the frozen marquee on refocus: last is re-stamped to now,
    /// preventing the first frame from advancing the entire
    /// stalled duration in one jump — except while the end-of-lap pause is
    /// still running, where last keeps its alignment to the resume point;
    /// returns whether the timer needs re-mounting.
    pub fn resume_error_scroll(&mut self) -> bool {
        if let Some(sc) = self.anim.error_scroll.as_mut()
            && sc.scrolling
            && sc.frozen
        {
            sc.frozen = false;
            let now = std::time::Instant::now();
            if now >= sc.resume_at {
                sc.last = now;
            }
            return true;
        }
        false
    }

    /// The clickable element hit by the mouse position (logical pixels);
    /// returns None when nothing is hit
    pub fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .find(|(_, rc)| x >= rc.left && x <= rc.right && y >= rc.top && y <= rc.bottom)
            .map(|(h, _)| *h)
    }

    /// Whether animations are allowed
    pub fn animations_on(&self) -> bool {
        self.anim_allowed
    }

    /// Fade-in prologue shared by every surface's draw: content rises and
    /// fades in; settles to (0, 1) once the tween ends or animation is
    /// disallowed. The background must still be drawn opaquely over the
    /// full area — the first frame would otherwise expose the swap chain.
    pub(crate) fn appear_offset(&self) -> (f32, f32) {
        match &self.anim.appear {
            Some(t) if self.anim_allowed => {
                let p = crate::ui::panel::anim::ease_out_cubic(t.progress());
                ((1.0 - p) * 6.0, p)
            }
            _ => (0.0, 1.0),
        }
    }
}

/// D2D/DWrite renderer
pub struct Renderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: ID2D1HwndRenderTarget,
    black: ID2D1SolidColorBrush,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
    formats: HashMap<(u32, u16, bool), IDWriteTextFormat>,
    ro_formats: std::cell::RefCell<HashMap<(&'static str, u32, u16), IDWriteTextFormat>>,
    frame_measures: HashMap<(usize, usize, u32, u16, bool), f32>,
    pub theme: Theme,
    pub hits: Vec<(Hit, D2D_RECT_F)>,
    pub hover: Option<Hit>,
    pub anim: AnimState,
    pending_tip: Option<(f32, f32, f32)>,
    /// Peak-hours tip copy, keyed (language, range): hover frames rebuild
    /// no strings; the Rc clone follows cached_error_body's borrow escape
    peak_tip: Option<(
        crate::ui::i18n::Lang,
        crate::ui::peak::PeakRange,
        std::rc::Rc<str>,
    )>,
    /// Drawn-text block per input field, keyed by (style tier, display
    /// string): the placeholder, active visible slice, and unfocused tail
    /// slice reuse one layout per distinct string — caret blink and
    /// drag-selection frames create zero layouts and zero intermediate
    /// strings. One slot per field; a tier switch replaces the entry.
    input_blocks:
        std::cell::RefCell<[Option<InputBlockEntry>; crate::ui::panel::caret::CARET_SLOTS]>,
    font_fallback: bool,
    target_dpi: f32,
    anim_allowed: bool,
    logo_geo: Option<(ID2D1PathGeometry, ID2D1PathGeometry)>,
    bolt_geo: Option<ID2D1PathGeometry>,
    eye_geo: Option<ID2D1PathGeometry>,
    pacman_geo: Option<(ID2D1PathGeometry, ID2D1PathGeometry)>,
    refresh_geo: Option<ID2D1PathGeometry>,
    u16_buf: std::cell::RefCell<Vec<u16>>,
    dash_style: Option<ID2D1StrokeStyle>,
    // ── Frame caches, grouped per view in draw order (main → settings →
    // popup → about): the struct reads as the render flow ──
    main_texts: Option<main::MainTexts>,
    /// Main-view constant-label layout cache: the content changes only with the language,
    /// and the key is the language
    main_labels: Option<(crate::ui::i18n::Lang, main::MainLabels)>,
    mcp_cache: Option<main::McpCompCache>,
    token_cells: Option<main::TokenCells>,
    /// Error-card body cache: keyed by the full-value pair of error content and language;
    /// marquee frames of the no-snapshot error
    /// view rebuild nothing
    err_card: Option<(
        (crate::api::FetchError, crate::ui::i18n::Lang),
        std::rc::Rc<str>,
    )>,
    /// Top-bar title truncation cache: key (source string, available width)
    /// → prebuilt layout of the truncated string.
    /// Changes only with the account name and width, independent of snapshots, survives across snapshots
    title_cache: Option<((String, f32), TextBlock)>,
    /// Settings-view frame cache (threshold description, buttons, about
    /// section, account card) keyed by a full-value key over (language,
    /// config, data, width); caret-blink, hover, and height-animation
    /// frames rebuild zero layouts
    settings_texts: Option<settings::SettingsTexts>,
    /// Settings-view constant-label layout cache: the content changes
    /// only with the language, kept apart from the volatile settings
    /// cache so account switches and update results rebuild neither —
    /// mirrors main_labels
    settings_labels: Option<(crate::ui::i18n::Lang, settings::SettingsLabels)>,
    /// Account-popup text cache: prebuilt heading/name/badge layouts keyed
    /// by (language, width, selected index, per-row name/platform/team);
    /// fade-in and hover frames build zero layouts and allocate zero
    /// strings
    popup_texts: Option<popup::PopupTexts>,
    /// About-window text cache: constant labels and ellipsized news
    /// artifacts as prebuilt layouts; the key bundles language, width,
    /// news set, expanded item, and read marker
    about_texts: Option<about::AboutTexts>,
}

impl Renderer {
    /// Create the renderer; the target and the black brush are created as a pair —
    /// either failing abandons the whole renderer,
    /// and the caller skips this frame and retries on the next paint message —
    /// this is the origin of the contract that black is always a live handle
    pub fn new(hwnd: HWND, rect_phys: &RECT, dpi: f32) -> Option<Self> {
        unsafe {
            let factory: ID2D1Factory =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            let dpi = sane_dpi(dpi);
            let (target, black) = create_hwnd_target(&factory, hwnd, rect_phys, dpi)?;
            Some(Self {
                factory,
                dwrite,
                target,
                black,
                brushes: HashMap::new(),
                formats: HashMap::new(),
                ro_formats: std::cell::RefCell::new(HashMap::new()),
                frame_measures: HashMap::new(),
                theme: Theme::new(
                    super::theme::system_appearance().unwrap_or(super::theme::Appearance::Light),
                ),
                hits: Vec::new(),
                hover: None,
                anim: AnimState::new(),
                pending_tip: None,
                peak_tip: None,
                input_blocks: std::cell::RefCell::new(std::array::from_fn(|_| None)),
                font_fallback: false,
                target_dpi: dpi,
                anim_allowed: animations_allowed(),
                logo_geo: None,
                bolt_geo: None,
                eye_geo: None,
                pacman_geo: None,
                refresh_geo: None,
                u16_buf: std::cell::RefCell::new(Vec::new()),
                dash_style: None,
                main_texts: None,
                main_labels: None,
                mcp_cache: None,
                token_cells: None,
                err_card: None,
                title_cache: None,
                settings_texts: None,
                settings_labels: None,
                popup_texts: None,
                about_texts: None,
            })
        }
    }

    /// Create a text format
    fn format(&mut self, size: f32, weight: u16, mono: bool) -> Option<IDWriteTextFormat> {
        let key = (size.to_bits(), weight, mono);
        if let Some(f) = self.formats.get(&key) {
            return Some(f.clone());
        }
        unsafe {
            let make = |family: &str| -> Option<IDWriteTextFormat> {
                self.dwrite
                    .CreateTextFormat(
                        PCWSTR(wide(family).as_ptr()),
                        None,
                        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT(weight as i32),
                        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STYLE_NORMAL,
                        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STRETCH_NORMAL,
                        size,
                        PCWSTR(wide("").as_ptr()),
                    )
                    .ok()
            };
            let fmt = if mono {
                make("Consolas")?
            } else {
                make("Segoe UI Variable Text").or_else(|| {
                    self.font_fallback = true;
                    make("Segoe UI")
                })?
            };
            let _ = fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
            let _ = fmt.SetWordWrapping(
                windows::Win32::Graphics::DirectWrite::DWRITE_WORD_WRAPPING_NO_WRAP,
            );
            self.formats.insert(key, fmt.clone());
            Some(fmt)
        }
    }

    /// Real DWrite width measurement
    pub(crate) unsafe fn measure(&mut self, s: &str, size: f32, weight: u16, mono: bool) -> f32 {
        let Some(fmt) = self.format(size, weight, mono) else {
            return 0.0;
        };
        self.measure_with(&fmt, s)
    }

    /// Format lookup for read-only contexts: cached by (family, size, weight), built once when absent.
    /// Shared by the &self measure paths (the prefix width table) — creating a COM object
    /// per call on the
    /// high-frequency drag-selection path is pure waste
    unsafe fn ro_format(&self, size: f32, weight: u16, mono: bool) -> Option<IDWriteTextFormat> {
        unsafe {
            let family = if mono {
                "Consolas"
            } else if self.font_fallback {
                "Segoe UI"
            } else {
                "Segoe UI Variable Text"
            };
            let key = (family, size.to_bits(), weight);
            if let Some(f) = self.ro_formats.borrow().get(&key) {
                return Some(f.clone());
            }
            let fmt = self
                .dwrite
                .CreateTextFormat(
                    PCWSTR(wide(family).as_ptr()),
                    None,
                    windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT(weight as i32),
                    windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STYLE_NORMAL,
                    windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    PCWSTR(wide("").as_ptr()),
                )
                .ok()?;
            self.ro_formats.borrow_mut().insert(key, fmt.clone());
            Some(fmt)
        }
    }

    /// Prefix width table: one CreateTextLayout + GetClusterMetrics for the whole string,
    /// spreading each cluster's
    /// width onto character positions by its UTF-16 length and accumulating — P[i]
    /// = the cumulative width of the first i
    /// characters, table length = character count + 1. Any "prefix width query"
    /// becomes a table lookup (index or
    /// binary search), replacing per-prefix TextLayout creation for measurement;
    /// character boundaries inside a cluster (surrogate pairs /
    /// combining marks merged into a cluster) take the cluster-end cumulative width,
    /// compatible with the visual-slice / truncation usage that only takes widths at character
    /// boundaries. When DWrite is unavailable it degrades to an all-zero table,
    /// matching the old behavior of returning 0 on measurement failure
    pub(crate) unsafe fn prefix_widths(
        &self,
        chars: &[char],
        size: f32,
        weight: u16,
        mono: bool,
    ) -> Vec<f32> {
        unsafe {
            let n = chars.len();
            let mut table = vec![0.0f32; n + 1];
            if n == 0 {
                return table;
            }
            let Some(fmt) = self.ro_format(size, weight, mono) else {
                return table;
            };
            // encode_utf16's &mut [u16] parameter checks capacity against the slice length; a bare Vec
            // has length 0 and will always panic. Catch each character in an on-stack
            // two-unit buffer, then append it to the vector.
            let mut w16: Vec<u16> = Vec::with_capacity(n);
            for c in chars {
                let mut buf = [0u16; 2];
                w16.extend_from_slice(c.encode_utf16(&mut buf));
            }
            let Ok(layout) = self.layout_sentinel(&w16, &fmt) else {
                return table;
            };
            let mut cms = vec![
                windows::Win32::Graphics::DirectWrite::DWRITE_CLUSTER_METRICS::default();
                w16.len()
            ];
            let mut count = 0u32;
            if layout
                .GetClusterMetrics(Some(&mut cms), &mut count)
                .is_err()
            {
                return table;
            }
            // Each character's UTF-16 starting unit position; cluster widths are recorded
            // at the character boundary where the cluster ends.
            let mut starts: Vec<usize> = Vec::with_capacity(n + 1);
            let mut off = 0usize;
            for c in chars {
                starts.push(off);
                off += c.len_utf16();
            }
            starts.push(off);
            let mut w = 0.0f32;
            let mut units = 0usize;
            let mut ci = 0usize;
            for cm in &cms[..count as usize] {
                w += cm.width;
                units += cm.length as usize;
                while ci < n && starts[ci + 1] <= units {
                    table[ci + 1] = w;
                    ci += 1;
                }
            }
            // When the metrics fail to cover the whole string (abnormal shortage),
            // flatten the tail to preserve monotonic non-decrease.
            for t in table.iter_mut().skip(ci + 1) {
                *t = w;
            }
            table
        }
    }

    /// Per-frame dedup measurement: within the same frame, identical ('static text, size,
    /// weight, mono) builds only one
    /// layout [the family is proxied by mono; the actual family argument has only two or
    /// three values]. Constant text recurring every frame — the ellipsis in ellipsize, section
    /// titles, and the like — goes through here; see the frame_measures field comment for
    /// the narrowing rationale
    unsafe fn measure_static(
        &mut self,
        s: &'static str,
        size: f32,
        weight: u16,
        mono: bool,
    ) -> f32 {
        let key = (s.as_ptr() as usize, s.len(), size.to_bits(), weight, mono);
        if let Some(&w) = self.frame_measures.get(&key) {
            return w;
        }
        let w = self.measure(s, size, weight, mono);
        self.frame_measures.insert(key, w);
        w
    }

    /// Encode into the shared staging buffer and return the borrow
    fn encode_u16(&self, s: &str) -> std::cell::RefMut<'_, Vec<u16>> {
        let mut buf = self.u16_buf.borrow_mut();
        buf.clear();
        buf.extend(s.encode_utf16());
        buf
    }

    /// Layout at sentinel extents (unlimited width): the single creation
    /// shape shared by measuring, prefix tables, ellipsize, and prebuilt
    /// blocks — widths from the different callers already combine in one
    /// layout math, so a drifting copy would make reserved widths stop
    /// matching drawn widths.
    unsafe fn layout_sentinel(
        &self,
        w16: &[u16],
        fmt: &IDWriteTextFormat,
    ) -> windows::core::Result<IDWriteTextLayout> {
        self.dwrite.CreateTextLayout(w16, fmt, 1.0e6, 1.0e6)
    }

    /// Build a layout from a format and take the width
    unsafe fn measure_with(&self, fmt: &IDWriteTextFormat, s: &str) -> f32 {
        unsafe {
            if s.is_empty() {
                return 0.0;
            }
            let w16 = self.encode_u16(s);
            let Ok(layout) = self.layout_sentinel(&w16, fmt) else {
                return 0.0;
            };
            Self::layout_metrics(&layout).0
        }
    }

    /// Head-preserving truncation with real metrics; returns (truncated string,
    /// its actual width) — the call site lays out the elements
    /// that follow (chevron, NEW badge) and need not re-measure. The width comes from
    /// the cumulative cluster widths: untruncated it is the sum over the whole string,
    /// truncated it adds the ellipsis width
    unsafe fn ellipsize(
        &mut self,
        s: &str,
        size: f32,
        max_w: f32,
        weight: u16,
        mono: bool,
    ) -> (String, f32) {
        if s.is_empty() {
            return (String::new(), 0.0);
        }
        // An exhausted budget yields a zero-width empty string, not a bare
        // ellipsis: callers skip zero-width blocks, while a lone ellipsis
        // would render dangling bracket pairs in the marquee cascade.
        if max_w <= 0.0 {
            return (String::new(), 0.0);
        }
        // The ellipsis width goes through per-frame dedup: multiple ellipsize calls with
        // the same parameters in one frame build the layout only once.
        let ell_w = self.measure_static("…", size, weight, mono);
        let budget = (max_w - ell_w).max(0.0);
        let Some(fmt) = self.format(size, weight, mono) else {
            return (s.to_string(), 0.0);
        };
        let (layout, w16_len) = {
            let w16 = self.encode_u16(s);
            let len = w16.len();
            (self.layout_sentinel(&w16, &fmt), len)
        };
        let Ok(layout) = layout else {
            // The degraded path falls back to whole-string measurement;
            // a zero width would stack the following elements onto the text head.
            let w = self.measure_with(&fmt, s);
            return (s.to_string(), w);
        };
        let mut cms =
            vec![windows::Win32::Graphics::DirectWrite::DWRITE_CLUSTER_METRICS::default(); w16_len];
        let mut n = 0u32;
        if layout.GetClusterMetrics(Some(&mut cms), &mut n).is_err() {
            // Without cluster metrics there is no basis for truncation;
            // the width falls back to whole-string measurement.
            let w = self.measure(s, size, weight, mono);
            return (s.to_string(), w);
        }
        cms.truncate(n as usize);
        let mut w = 0.0f32;
        let mut units = 0usize;
        for cm in &cms {
            if w + cm.width > budget {
                break;
            }
            w += cm.width;
            units += cm.length as usize;
        }
        if units >= w16_len {
            return (s.to_string(), w);
        }
        // The staging buffer has not been overwritten since the layout was created —
        // the calls that would overwrite it all sit on failure paths
        // that return early, so reading the slice back is safe.
        let mut out: Vec<u16> = self.u16_buf.borrow()[..units].to_vec();
        out.push(0x2026);
        let disp = String::from_utf16_lossy(&out);
        (disp, w + ell_w)
    }

    /// Build a text layout by size and weight: takes the format in a read-only context,
    /// shared by cached direct-draw text
    /// for prebuilt layouts; the footer swap start point also builds its block through this
    pub(crate) unsafe fn make_layout(
        &self,
        s: &str,
        size: f32,
        weight: u16,
        mono: bool,
    ) -> Option<IDWriteTextLayout> {
        if s.is_empty() {
            return None;
        }
        let fmt = self.ro_format(size, weight, mono)?;
        let w16 = self.encode_u16(s);
        self.layout_sentinel(&w16, &fmt).ok()
    }

    /// Cached drawn-text block for one input field (see input_blocks).
    /// The style tier is part of the key: the placeholder (12/400
    /// non-mono) and the body (INPUT_FONT mono) must never share an entry
    /// even for identical text — e.g. a peak preset pasted back verbatim.
    /// `matches` compares structurally against the cached string so a hit
    /// materializes nothing; `materialize` runs only on a real miss.
    #[allow(clippy::too_many_arguments)]
    fn input_block(
        &self,
        style: InputStyle,
        field: crate::ui::panel::InputField,
        matches: impl FnOnce(&str) -> bool,
        materialize: impl FnOnce() -> String,
        size: f32,
        weight: u16,
        mono: bool,
    ) -> TextBlock {
        {
            let cache = self.input_blocks.borrow();
            if let Some(e) = cache[field.slot()].as_ref()
                && e.style == style
                && matches(&e.text)
            {
                return e.block.clone();
            }
        }
        let text = materialize();
        let b = unsafe { TextBlock::new(self.make_layout(&text, size, weight, mono)) };
        self.input_blocks.borrow_mut()[field.slot()] = Some(InputBlockEntry {
            style,
            text,
            block: b.clone(),
        });
        b
    }

    /// Direct draw of a prebuilt layout: reuses the cached layout, bypassing DrawText's
    /// internal COM object rebuild on every call;
    /// clipping is applied uniformly by the caller's outer layer. nosnap disables pixel
    /// snapping (used by chain-advanced badges)
    unsafe fn draw_layout(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        layout: &IDWriteTextLayout,
        x: f32,
        y: f32,
        brush: &ID2D1SolidColorBrush,
        nosnap: bool,
    ) {
        let opts = if nosnap {
            D2D1_DRAW_TEXT_OPTIONS_NO_SNAP
        } else {
            D2D1_DRAW_TEXT_OPTIONS_NONE
        };
        target.DrawTextLayout(Vector2 { X: x, Y: y }, layout, brush, opts);
    }

    /// Parameterized core of prebuilt-layout direct drawing — the single
    /// guard/brush/draw contract every view's block helpers wrap with their
    /// clip and centering math (main's draw_block family, settings'
    /// block_at family, popup_block, about_text): no-op without a layout,
    /// one brush lookup, one direct draw.
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn draw_block_at(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        b: &TextBlock,
        x: f32,
        y: f32,
        color: [f32; 4],
        alpha: f32,
        nosnap: bool,
    ) {
        if let Some(l) = b.layout.as_ref() {
            let brush = self.brush(target, color, alpha);
            self.draw_layout(target, l, x, y, &brush, nosnap);
        }
    }

    /// Badge box drawing a cached label block: rounded outline (edge
    /// dimmed to 0.9) + centered label — the settings account card and
    /// the popup rows share this one geometry (BADGE_H, radius, stroke
    /// weight), so badge tweaks cannot drift between the two views;
    /// mono lives in the block's format, not a parameter
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn badge_blk(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &TextBlock,
        x: f32,
        y: f32,
        w: f32,
        edge_color: [f32; 4],
        fg: [f32; 4],
        alpha: f32,
    ) {
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + widgets::BADGE_H,
            },
            radiusX: 2.5,
            radiusY: 2.5,
        };
        let edge = self.brush(target, edge_color, alpha * 0.9);
        target.DrawRoundedRectangle(&r, &edge, 1.0, None);
        self.draw_block_at(
            target,
            label,
            x + (w - label.w) / 2.0,
            y + (widgets::BADGE_H - label.h) / 2.0,
            fg,
            alpha,
            false,
        );
    }

    /// Rect-anchored placement over the shared draw_block_at: the
    /// horizontal origin by Align, the vertical by top-vs-midline,
    /// clipping optional — every view's rect-based block wrapper
    /// (settings' block family, about's about_text pair) reduces to this
    /// one placement math, so centering and clip semantics cannot drift
    /// between views.
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn draw_block_in(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        b: &TextBlock,
        rect: &D2D_RECT_F,
        color: [f32; 4],
        alpha: f32,
        align: Align,
        vcenter: bool,
        clip: bool,
    ) {
        let x = match align {
            Align::Center => rect.left + (rect.right - rect.left - b.w) / 2.0,
            Align::Left => rect.left,
        };
        let y = if vcenter {
            rect.top + (rect.bottom - rect.top - b.h) / 2.0
        } else {
            rect.top
        };
        if clip {
            target.PushAxisAlignedClip(rect, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.draw_block_at(target, b, x, y, color, alpha, false);
            target.PopAxisAlignedClip();
        } else {
            self.draw_block_at(target, b, x, y, color, alpha, false);
        }
    }

    /// Left + top-aligned block in a rect, clipped to it — the text()
    /// replacement family shared by the settings and about views; the
    /// placement math is the shared draw_block_in
    unsafe fn block_l(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        b: &TextBlock,
        rect: &D2D_RECT_F,
        color: [f32; 4],
        alpha: f32,
    ) {
        self.draw_block_in(target, b, rect, color, alpha, Align::Left, false, true);
    }

    /// Horizontally centered + top-aligned block in a rect, clipped to
    /// it
    unsafe fn block_c(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        b: &TextBlock,
        rect: &D2D_RECT_F,
        color: [f32; 4],
        alpha: f32,
    ) {
        self.draw_block_in(target, b, rect, color, alpha, Align::Center, false, true);
    }

    /// Centered + vertically centered block in a rect, clipped to it;
    /// the center math matches what DrawText's vcenter path performs on
    /// its internal layout
    unsafe fn block_vc(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        b: &TextBlock,
        rect: &D2D_RECT_F,
        color: [f32; 4],
        alpha: f32,
    ) {
        self.draw_block_in(target, b, rect, color, alpha, Align::Center, true, true);
    }

    /// Left + vertically centered block in a rect, clipped to it
    unsafe fn block_lv(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        b: &TextBlock,
        rect: &D2D_RECT_F,
        color: [f32; 4],
        alpha: f32,
    ) {
        self.draw_block_in(target, b, rect, color, alpha, Align::Left, true, true);
    }

    /// Metrics of a built layout (width including trailing whitespace, height):
    /// cache blocks and truncation measurement share this source,
    /// the width and direct drawing come from the same source, size and
    /// weight are written in one place only
    unsafe fn layout_metrics(layout: &IDWriteTextLayout) -> (f32, f32) {
        let mut m = windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS::default();
        if layout.GetMetrics(&mut m).is_ok() {
            (m.widthIncludingTrailingWhitespace, m.height)
        } else {
            (0.0, 0.0)
        }
    }

    /// One draw frame's skeleton shared by every surface: target
    /// (re)creation, the per-frame cache clears, Begin/EndDraw, and the
    /// device-loss verdict. `draw` receives the live target plus the
    /// logical size; a failed full rebuild reports success and skips the
    /// frame (the Renderer stays healthy for the next paint message).
    /// Returning false means device loss — the caller must drop the whole
    /// Renderer. Future frame-lifecycle fixes (new caches to clear,
    /// device-loss brush handling) land in this one place.
    unsafe fn paint_frame(
        &mut self,
        hwnd: HWND,
        rect_phys: &RECT,
        dpi: f32,
        log_tag: &str,
        draw: impl FnOnce(&mut Self, &ID2D1HwndRenderTarget, f32, f32),
    ) -> bool {
        unsafe {
            let Some(target) = self.ensure_target(hwnd, rect_phys, dpi) else {
                return true;
            };
            let w = (rect_phys.right - rect_phys.left) as f32 / dpi;
            let h = (rect_phys.bottom - rect_phys.top) as f32 / dpi;
            self.hits.clear();
            self.frame_measures.clear();
            target.BeginDraw();
            draw(self, &target, w, h);
            match target.EndDraw(None, None) {
                Ok(()) => true,
                Err(e) => {
                    crate::platform::log(&format!("[Quotify] {log_tag} EndDraw 失败: {e}"));
                    false
                }
            }
        }
    }

    /// Paint one frame; `rect_phys` is in physical pixels while
    /// drawing uses logical-pixel DIPs throughout.
    /// Returning false means device loss; the caller must discard the whole Renderer and
    /// fully rebuild it on the next frame
    pub fn paint(
        &mut self,
        hwnd: HWND,
        rect_phys: &RECT,
        panel: &Panel,
        model: &PanelModel,
        view: PanelView,
        dpi: f32,
    ) -> bool {
        unsafe {
            self.paint_frame(hwnd, rect_phys, dpi, "面板", |s, t, w, h| {
                s.draw(t, panel, model, view, w, h)
            })
        }
    }

    /// Draw one frame
    unsafe fn draw(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        panel: &Panel,
        model: &PanelModel,
        view: PanelView,
        w: f32,
        h: f32,
    ) {
        // Pop animation: content rises and fades in; the background must be drawn
        // opaquely over the full area, otherwise the first frame exposes the swap chain
        // and jitters out of place.
        let (dy, alpha) = self.appear_offset();
        let dy = dy - panel.scroll_dy;

        // Single point for footer animation lifetime: the swap animation departs with the
        // error (left uncleared it would idle
        // hidden behind the summary and resume the stale copy mid-way after
        // the error clears); the marquee survives only on the main view with
        // both the error and the snapshot present — teardown follows the data and
        // the view, so the data side and each
        // view no longer maintain it themselves.
        if model.error.is_some() {
            self.anim.footer = None;
        }
        if view != PanelView::Main || model.error.is_none() || model.snapshot.is_none() {
            self.anim.error_scroll = None;
        }

        let bg = self.theme.bg;
        let bg_brush = self.brush(target, bg, 1.0);
        let bg_rect = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: w,
            bottom: h,
        };
        target.FillRectangle(&bg_rect, &bg_brush);

        match view {
            PanelView::Main => {
                let content_h = panel.main_h as f32;
                self.draw_main(target, model, w, h, content_h, dy, alpha)
            }
            // The add form and the settings page share draw_settings; it branches by view internally.
            PanelView::Settings | PanelView::AddForm => {
                self.draw_settings(target, panel, model, w, dy, alpha)
            }
        }

        // The peak-hours tip card is drawn last, covering the data rows.
        if let Some((x, y, tw)) = self.pending_tip.take() {
            let tip = self.cached_peak_tip(model);
            self.tip_card(target, x, y, tw, alpha, &tip);
        }
    }

    /// Peak tip copy keyed (language, range): hover frames rebuild no
    /// strings; the Rc clone follows cached_error_body's borrow escape —
    /// tip_card needs &mut self for hit registration mid-draw.
    fn cached_peak_tip(&mut self, model: &PanelModel) -> std::rc::Rc<str> {
        if let Some((l, r, t)) = self.peak_tip.as_ref()
            && *l == model.lang
            && *r == model.peak_range
        {
            return std::rc::Rc::clone(t);
        }
        let t: std::rc::Rc<str> = model
            .strings
            .peak_tip
            .replace("{r}", &crate::ui::peak::fmt_range(model.peak_range))
            .into();
        self.peak_tip = Some((model.lang, model.peak_range, std::rc::Rc::clone(&t)));
        t
    }

    // ── Text primitives ──

    #[allow(clippy::too_many_arguments)]
    unsafe fn text(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
    ) {
        let rect = D2D_RECT_F {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        };
        self.text_rect(target, s, &rect, size, weight, color, alpha);
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn text_rect(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        rect: &D2D_RECT_F,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
    ) {
        self.text_aligned(
            target,
            s,
            rect,
            size,
            weight,
            color,
            alpha,
            Align::Left,
            false,
        );
    }

    /// Text drawing primitive: temporarily changes the shared format's
    /// alignment/wrapping/centering and restores it after use — once the
    /// fmt is obtained there must be no early return; leaking the modified state would
    /// mis-lay-out subsequent text with the same parameters
    #[allow(clippy::too_many_arguments)]
    unsafe fn text_raw(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        rect: &D2D_RECT_F,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
        align: Align,
        mono: bool,
        wrap: bool,
        vcenter: bool,
        nosnap: bool,
    ) {
        let Some(fmt) = self.format(size, weight, mono) else {
            return;
        };
        let align_set = match align {
            Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
        };
        let _ = fmt.SetTextAlignment(align_set);
        if wrap {
            let _ = fmt
                .SetWordWrapping(windows::Win32::Graphics::DirectWrite::DWRITE_WORD_WRAPPING_WRAP);
        }
        if vcenter {
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
        let mut opts = D2D1_DRAW_TEXT_OPTIONS_CLIP;
        if nosnap {
            opts |= D2D1_DRAW_TEXT_OPTIONS_NO_SNAP;
        }
        // Empty string skips: nothing to draw, and it saves one brush creation; the brush is created first —
        // while the encoding staging buffer's borrow is alive, &mut self must not be borrowed again.
        if !s.is_empty() {
            let brush = self.brush(target, color, alpha);
            let w16 = self.encode_u16(s);
            target.DrawText(
                &w16,
                &fmt,
                rect as *const D2D_RECT_F,
                &brush,
                opts,
                windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        // Restore the format's default state (LEADING/NO_WRAP/NEAR),
        // matching what was set at cache creation.
        if wrap {
            let _ = fmt.SetWordWrapping(
                windows::Win32::Graphics::DirectWrite::DWRITE_WORD_WRAPPING_NO_WRAP,
            );
        }
        if vcenter {
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        }
        let _ = fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
    }

    /// Draw text aligned per the enum, paragraph alignment stays top; mono selects the monospace font.
    #[allow(clippy::too_many_arguments)]
    unsafe fn text_aligned(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        rect: &D2D_RECT_F,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
        align: Align,
        mono: bool,
    ) {
        self.text_raw(
            target, s, rect, size, weight, color, alpha, align, mono, false, false, false,
        );
    }

    /// Multi-line copy: temporarily enables word wrapping; long text folds to the box width
    #[allow(clippy::too_many_arguments)]
    unsafe fn text_wrapped(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        rect: &D2D_RECT_F,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
        align: Align,
        mono: bool,
    ) {
        self.text_raw(
            target, s, rect, size, weight, color, alpha, align, mono, true, false, false,
        );
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn line(
        &self,
        target: &ID2D1HwndRenderTarget,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        brush: &ID2D1SolidColorBrush,
        width: f32,
    ) {
        target.DrawLine(
            Vector2 { X: x0, Y: y0 },
            Vector2 { X: x1, Y: y1 },
            brush,
            width,
            None,
        );
    }

    /// Solid-color brush: cached and reused by quantized color, cloned on a hit;
    /// alpha is multiplied into the color components.
    /// A separate instance is cached per color — reuse via SetColor on a single brush
    /// gets alternately overwritten, and the sliders icon goes entirely invisible
    unsafe fn brush(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        c: [f32; 4],
        alpha: f32,
    ) -> ID2D1SolidColorBrush {
        let a = (c[3] * alpha).clamp(0.0, 1.0);
        let key = pack_color(c[0], c[1], c[2], a);
        if let Some(b) = self.brushes.get(&key) {
            return b.clone();
        }
        let color = D2D1_COLOR_F {
            r: c[0],
            g: c[1],
            b: c[2],
            a,
        };
        match target.CreateSolidColorBrush(&color, None) {
            Ok(b) => {
                self.brushes.insert(key, b.clone());
                b
            }
            // Creation failure only happens when the device is near death: black,
            // living alongside the target, always has a live handle; drawing with it
            // defers the error to EndDraw, and this frame's color deviation is
            // acceptable; the Renderer is then discarded and
            // rebuilt as a whole — never let panic=abort escalate transient device loss
            // into process termination.
            Err(_) => self.black.clone(),
        }
    }

    /// Clear the brush cache when collapsing: the key includes quantized steps of
    /// animation alpha, one per fade-in frame,
    /// and without clearing it slowly accumulates with each open/close;
    /// the first frame of the next open rebuilds on demand at negligible cost
    pub(crate) fn clear_brush_cache(&mut self) {
        self.brushes.clear();
    }

    /// Create or reuse the HwndRenderTarget; returns None on failure
    unsafe fn ensure_target(
        &mut self,
        hwnd: HWND,
        rect_phys: &RECT,
        dpi: f32,
    ) -> Option<ID2D1HwndRenderTarget> {
        let dpi = sane_dpi(dpi);
        let w_px = (rect_phys.right - rect_phys.left).max(1) as u32;
        let h_px = (rect_phys.bottom - rect_phys.top).max(1) as u32;
        // Handle size/DPI changes; the internal size record follows the GetPixelSize reading.
        let sz = self.target.GetPixelSize();
        let need_rebuild = sz.width != w_px || sz.height != h_px || self.target_dpi != dpi;
        if need_rebuild {
            // Size-only changes prefer Resize reuse — a full rebuild reallocates the back
            // buffer and causes stutter;
            // Resize keeps the device, so already-created brushes stay valid.
            let size_only = self.target_dpi == dpi;
            let resized = size_only
                && self
                    .target
                    .Resize(&D2D_SIZE_U {
                        width: w_px,
                        height: h_px,
                    })
                    .is_ok();
            if !resized {
                // A full rebuild swaps the target: cached brushes are bound to the old
                // target that created them; leftover handles would push
                // the first post-rebuild frame's draw errors to EndDraw and waste a frame, so clear the cache first.
                self.brushes.clear();
                match create_hwnd_target(&self.factory, hwnd, rect_phys, dpi) {
                    Some((target, black)) => {
                        self.target = target;
                        self.black = black;
                        self.target_dpi = dpi;
                    }
                    // Keep the old target as-is and skip this frame,
                    // retrying the full rebuild on the next paint message;
                    // failure details are logged inside the helper.
                    None => return None,
                }
            }
        }
        Some(self.target.clone())
    }
}

/// Shared WM_PAINT skeleton: validate the client area → take out the renderer → on first
/// creation align the configured appearance and add the
/// fade-in → paint → put back unchanged on success, or request a full next-frame rebuild
/// on device loss. The appearance arrives as the raw
/// config string and is parsed only in the first-creation branch —
/// follow-system mode reads the registry on every parse, so parsing
/// per frame is pure waste.
/// Returns (renderer, keep, need_anim): need_anim means a first-open fade-in
/// was just armed and the caller owning the TIMER_ANIM identity must mount
/// its own clock — arming here would SetTimer a foreign HWND while the
/// caller's shadow record stays unaware, stalling its animations.
/// Writing back the painted flag and follow-up actions belong to the caller.
pub(crate) unsafe fn paint_common(
    hwnd: HWND,
    renderer: Option<Renderer>,
    appearance: Option<&str>,
    painted: bool,
    dpi: f32,
    paint: impl FnOnce(&mut Renderer, &RECT) -> bool,
) -> (Option<Renderer>, bool, bool) {
    use windows::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    let _ = ValidateRect(Some(hwnd), None);
    let mut rect = RECT::default();
    let _ = GetClientRect(hwnd, &mut rect);
    let fresh = renderer.is_none();
    let mut renderer = renderer.or_else(|| Renderer::new(hwnd, &rect, dpi));
    let mut keep = true;
    let mut need_anim = false;
    if let Some(r) = renderer.as_mut()
        && fresh
    {
        // Renderer::new falls back to reading the system appearance; when
        // the config forces light/dark opposite to the system, the first
        // frame would use the wrong theme; day-to-day switches are pushed by
        // apply_appearance, and per-frame rebuilds would just re-read
        // the registry over and over.
        r.theme = Theme::new(super::theme::resolved(appearance));
        // On first open the renderer is created only at paint, so
        // the fade-in set by open/show_at cannot land; device-loss
        // rebuilds also go through fresh, but the content is already on screen — excluded via painted.
        if !painted {
            r.anim.appear = Some(Tween::now(180));
            need_anim = true;
        }
    }
    if let Some(r) = renderer.as_mut() {
        keep = paint(r, &rect);
    }
    if !keep {
        // Without the request, a static panel would wait for the minute heartbeat for its
        // next frame, staying blank with hit testing dead in the meantime.
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    (renderer, keep, need_anim)
}

/// Platform badge text with the team annotation appended for team accounts:
/// the mapping and annotation policy live once — the settings card and the
/// account popup share it, differing only in the separator. Borrowed (zero
/// allocation) for personal accounts.
pub(super) fn platform_with_team(
    s: &crate::ui::i18n::Strings,
    platform: crate::api::Platform,
    team: bool,
    sep: &str,
) -> std::borrow::Cow<'static, str> {
    let base = if platform == crate::api::Platform::Cn {
        s.platform_cn
    } else {
        s.platform_intl
    };
    if team {
        std::borrow::Cow::Owned(format!("{base}{sep}{}", s.team_badge))
    } else {
        std::borrow::Cow::Borrowed(base)
    }
}

/// Badge horizontal padding (label width → box width), shared by the
/// popup row badge, its name-truncation budget, and the settings
/// account-card badges: an unowned copy at any of the three would drift
/// the same badge geometry across views.
pub(super) const BADGE_PAD_X: f32 = 14.0;

/// Quantize each color channel to 8 bits and pack it as the brush cache key —
/// matches display color depth, no visual loss
fn pack_color(r: f32, g: f32, b: f32, a: f32) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (q(r) << 24) | (q(g) << 16) | (q(b) << 8) | q(a)
}

/// Clamp invalid DPI to 1.0, preventing 0/NaN from slipping into the target creation parameters
fn sane_dpi(dpi: f32) -> f32 {
    if dpi.is_finite() && dpi >= 1.0 {
        dpi
    } else {
        1.0
    }
}

/// Create the software-rendering target and the black brush as a pair; either failing returns None,
/// shared by the first-creation and DPI full-rebuild paths, guaranteeing black is in
/// place whenever the target exists
unsafe fn create_hwnd_target(
    factory: &ID2D1Factory,
    hwnd: HWND,
    rect_phys: &RECT,
    dpi: f32,
) -> Option<(ID2D1HwndRenderTarget, ID2D1SolidColorBrush)> {
    let w_px = (rect_phys.right - rect_phys.left).max(1) as u32;
    let h_px = (rect_phys.bottom - rect_phys.top).max(1) as u32;
    let pf = D2D1_PIXEL_FORMAT {
        format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
        alphaMode: D2D1_ALPHA_MODE_IGNORE,
    };
    // Software rendering: the hardware path pulls up the d3d11 + GPU driver stack, and
    // the driver's internal commit
    // takes 60MB+, exceeding the memory budget; the window is small, so
    // software rasterization time is negligible.
    let props = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
        pixelFormat: pf,
        dpiX: dpi * 96.0,
        dpiY: dpi * 96.0,
        ..Default::default()
    };
    let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
        hwnd,
        pixelSize: D2D_SIZE_U {
            width: w_px,
            height: h_px,
        },
        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
    };
    let target = match factory.CreateHwndRenderTarget(&props, &hwnd_props) {
        Ok(t) => t,
        Err(e) => {
            crate::platform::log(&format!("[Quotify] CreateHwndRenderTarget 失败: {e}"));
            return None;
        }
    };
    let black = match target.CreateSolidColorBrush(
        &D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        },
        None,
    ) {
        Ok(b) => b,
        Err(e) => {
            crate::platform::log(&format!("[Quotify] black 刷子创建失败: {e}"));
            return None;
        }
    };
    Some((target, black))
}

impl Default for AnimState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the manual allowlist of is_input_hit: a new input-adjacent Hit
    /// variant that must keep focus (an apply-button pattern) compiles
    /// silently unregistered otherwise — no exhaustiveness check covers this
    /// list. When adding a variant, extend both the enum and this table.
    #[test]
    fn is_input_hit_allowlist_pinned() {
        let yes: &[Hit] = &[
            Hit::InputInterval,
            Hit::InputProxy,
            Hit::InputPeakStart,
            Hit::InputPeakEnd,
            Hit::InputName,
            Hit::InputKey,
            Hit::InputOrg,
            Hit::InputProject,
            Hit::CustomizeInterval,
            Hit::AddAccount,
            Hit::RevealKey,
            Hit::ApplyProxy,
            Hit::ApplyInterval,
            Hit::ApplyPeak,
            Hit::SaveAccount,
        ];
        let no: &[Hit] = &[
            Hit::Refresh,
            Hit::Settings,
            Hit::AccountSwitch,
            Hit::UsageInfo,
            Hit::Back,
            Hit::ClosePanel,
            Hit::IntervalPreset(300),
            Hit::Language(LanguageChoice::System),
            Hit::Appearance(AppearanceChoice::System),
            Hit::ToggleAutostart,
            Hit::ToggleThreshold,
            Hit::ToggleReset5h,
            Hit::ToggleResetWeekly,
            Hit::RemoveAccount(0),
            Hit::PickAccount(0),
            Hit::AccountType(ScopeChoice::Personal),
            Hit::Platform(crate::api::Platform::Cn),
            Hit::ExportConfig,
            Hit::ImportConfig,
            Hit::CheckUpdate,
            Hit::OpenDownload,
            Hit::LinkRepo,
            Hit::LinkIssues,
            Hit::CopyDiagnostics,
            Hit::NewsItem(0),
            Hit::AboutLogo,
        ];
        for h in yes {
            assert!(h.is_input_hit(), "should keep input state: {h:?}");
        }
        for h in no {
            assert!(!h.is_input_hit(), "should drop input state: {h:?}");
        }
    }
}
