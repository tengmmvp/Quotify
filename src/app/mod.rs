//! Application layer: assembles the modules, maintains global state, and drives the message loop.

pub mod config;
mod hits;
pub(crate) mod modal;
mod notify;

pub(crate) use hits::{confirm_panel_input, dispatch_hit, reconcile_fading_focus, refit_about};
use hits::{enter_settings, invalidate_ui};

use chrono::{DateTime, Utc};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_HWNDPARENT,
    GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, MSG, PostMessageW, PostQuitMessage, SetTimer,
    SetWindowLongPtrW, TranslateMessage, WINDOW_EX_STYLE, WM_COMMAND, WM_DESTROY,
    WM_POWERBROADCAST, WM_TIMER, WS_POPUP,
};
use windows::core::PCWSTR;

use crate::api::{FetchError, UsageSnapshot};
use crate::app::config::Config;
use crate::platform::instance::TRAY_WND_CLASS;
use crate::platform::msg::{
    WM_APP_NEWS_RESULT, WM_APP_POLL_RESULT, WM_APP_TRAY, WM_APP_UPDATE_RESULT, WM_APP_WAKE_INSTANCE,
};
use crate::platform::wide;
use crate::service::poller::{
    DEFAULT_INTERVAL_SECS, MAX_POLL_SECS, MIN_POLL_SECS, PollGeneration, PollInterval, PollMessage,
    PollOutcome, PollTarget, Poller,
};
use crate::ui::i18n::{Lang, Strings};
use crate::ui::icon;
use crate::ui::panel::LogPx;
use crate::ui::panel::Panel;
use crate::ui::panel::invalidate_panel;
use crate::ui::tray::{self, TrayIcon};
use notify::{NOTIFY_TITLE, check_reset, check_threshold};

/// Tray menu command IDs; definition order matches the menu item order,
/// and modal.rs builds the menu from the same source
pub(crate) const IDM_SETTINGS: u16 = 1001;
pub(crate) const IDM_ABOUT: u16 = 1002;
pub(crate) const IDM_EXIT: u16 = 1003;

/// Tray registration retry timer: on first-registration failure retries
/// every 2s up to 15 times, removed on success or when the cap is hit;
/// the explorer-restart fallback goes through TaskbarCreated separately.
const TIMER_TRAY_RETRY: usize = 1;
const TRAY_RETRY_MS: u32 = 2000;
const TRAY_RETRY_MAX: u32 = 15;

/// Background task for the update check: channel bound to the wake code
static UPDATE_TASK: crate::platform::post::BackgroundTask<
    Result<crate::service::update::ReleaseInfo, String>,
> = crate::platform::post::BackgroundTask::new(WM_APP_UPDATE_RESULT);

/// Background task for fetching repo news: channel bound to the wake code
static NEWS_TASK: crate::platform::post::BackgroundTask<
    Result<Vec<crate::service::whatsnew::NewsItem>, String>,
> = crate::platform::post::BackgroundTask::new(WM_APP_NEWS_RESULT);

/// Keeps the last snapshot for panel display on failure.
pub struct AccountData {
    pub(crate) snapshot: Option<UsageSnapshot>,
    pub(crate) last_error: Option<FetchError>,
}

/// Tray aggregate: the registration carrier, first-registration retry
/// counter, installed icon handle, submission generation, and the
/// tooltip/icon content keys — the fields that move together through
/// every tray writer (update, rearm, retry timer, exit). The repeated
/// `tray.as_ref().is_some_and(|t| t.registered)` gate lives here once as
/// registered().
struct TrayState {
    tray: Option<TrayIcon>,
    /// Retry counter of the first-registration retry timer
    retries: u32,
    /// The currently installed icon handle; destroyed on replacement/exit
    icon: Option<windows::Win32::UI::WindowsAndMessaging::HICON>,
    /// Content fingerprint of the tooltip; text building is skipped when unchanged
    last_tip: Option<String>,
    /// Content fingerprint of the icon (pixel size, usage percent, data
    /// presence, failure); the icon is rebuilt only when it changes
    last_icon_key: Option<(i32, i64, bool, bool)>,
    /// Tray state generation: each successful writer (this round's commit,
    /// a nested refresh, a rearm re-add) advances it once; compared after
    /// the nested pump, and a change means yield — a single criterion
    /// replacing the stacked guesswork of handle identity plus content-key
    /// snapshots
    generation: u64,
}

impl TrayState {
    fn new() -> Self {
        Self {
            tray: None,
            retries: 0,
            icon: None,
            last_tip: None,
            last_icon_key: None,
            generation: 0,
        }
    }

    /// Whether the icon is registered with the shell — the gate ahead of
    /// every NIM_MODIFY submission: while unregistered, building the
    /// tooltip or icon would just be discarded along with the failure.
    fn registered(&self) -> bool {
        self.tray.as_ref().is_some_and(|t| t.registered)
    }

    /// Submit a tooltip update through the registered tray icon
    fn set_tooltip(&self, tip: &str) -> bool {
        self.tray.as_ref().is_some_and(|t| t.set_tooltip(tip))
    }

    /// Submit an icon update through the registered tray icon
    fn update_icon(&self, hicon: windows::Win32::UI::WindowsAndMessaging::HICON) -> bool {
        self.tray.as_ref().is_some_and(|t| t.update_icon(hicon))
    }

    /// Icon rect as placed by the shell (cross-process query, pump point)
    fn rect(&self) -> Option<RECT> {
        self.tray.as_ref().and_then(|t| t.rect())
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    /// Advance the generation once per successful writer
    fn bump_gen(&mut self) {
        self.generation += 1;
    }

    /// Startup installation: the initial icon handle and its TrayIcon
    /// carrier enter together
    fn install(&mut self, tray: TrayIcon, icon: windows::Win32::UI::WindowsAndMessaging::HICON) {
        self.tray = Some(tray);
        self.icon = Some(icon);
    }

    /// Commit a just-submitted icon: destroy the retired handle and
    /// advance the content key — only called after update_icon succeeded
    fn install_icon(
        &mut self,
        new: Option<windows::Win32::UI::WindowsAndMessaging::HICON>,
        key: (i32, i64, bool, bool),
    ) {
        if let Some(old) = self.icon.take() {
            icon::destroy_owned(old);
        }
        self.icon = new;
        self.last_icon_key = Some(key);
    }

    /// Post-re-add write-back: mark registered, drop both content keys so
    /// the posted full refresh resubmits them, and advance the generation
    /// (a re-add is an external overwrite; any in-flight refresh yields)
    fn rearmed(&mut self) {
        if let Some(t) = self.tray.as_mut() {
            t.registered = true;
        }
        self.last_tip = None;
        self.last_icon_key = None;
        self.bump_gen();
    }

    /// Exit cleanup: drop the registration (NIM_DELETE) and destroy the
    /// owned icon handle
    fn shutdown(&mut self) {
        if let Some(t) = self.tray.take() {
            drop(t);
        }
        if let Some(ic) = self.icon.take() {
            icon::destroy_owned(ic);
        }
    }
}

/// Assembly-layer state machine
pub struct App {
    pub(crate) config: Config,
    pub(crate) lang: Lang,
    pub(crate) strings: &'static Strings,
    pub(crate) data: AccountData,
    tray_state: TrayState,
    poller: Option<Poller>,
    poll_target: PollTarget,
    poll_interval: PollInterval,
    poll_gen: PollGeneration,
    pub(crate) panel: Panel,
    pub(crate) popup: crate::ui::popup::AccountPopup,
    pub(crate) about: crate::ui::about::AboutWindow,
    hwnd: Option<HWND>,
    pub(crate) update_status: Option<Result<crate::service::update::ReleaseInfo, String>>,
    /// is_newer verdict of update_status, computed once at assignment:
    /// the render model read it per frame and re-parsed the version
    /// strings on every paint.
    pub(crate) update_available: bool,
    update_checking: bool,
    pub(crate) news: Option<Vec<crate::service::whatsnew::NewsItem>>,
    news_fetched: bool,
    pub(crate) autostart_enabled: bool,
    /// Last applied resolved appearance: apply_appearance's whole effect
    /// is a pure function of the resolved value, so an unchanged value
    /// skips the menu-theme call and every renderer's Theme rebuild —
    /// relayout invokes it on each poll drain and hit dispatch. Resolving
    /// still reads the registry in follow-system mode (the read produces
    /// the compared value)
    applied_appearance: Option<crate::ui::panel::theme::Appearance>,
    threshold_armed_5h: bool,
    last_reset_5h: Option<DateTime<Utc>>,
    threshold_armed_weekly: bool,
    last_reset_weekly: Option<DateTime<Utc>>,
    last_logged_error: Option<String>,
    /// Alias generation: bumped on every app_from_tray re-derive. A
    /// snapshot compared after a synchronous pump tells whether the
    /// reference was re-derived inside it (debug alias tripwire — see
    /// alias_assert_live).
    alias_gen: std::cell::Cell<u64>,
}

impl App {
    fn new(mut config: Config) -> Self {
        normalize_config(&mut config);
        let lang = crate::ui::i18n::resolve_lang(config.general.language.as_deref());
        Self {
            config,
            lang,
            strings: lang.strings(),
            data: AccountData {
                snapshot: None,
                last_error: None,
            },
            tray_state: TrayState::new(),
            poller: None,
            poll_target: std::sync::Arc::new(std::sync::Mutex::new(None)),
            poll_interval: std::sync::Arc::new(std::sync::Mutex::new(DEFAULT_INTERVAL_SECS)),
            poll_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            panel: Panel::new(),
            popup: crate::ui::popup::AccountPopup::new(),
            about: crate::ui::about::AboutWindow::new(),
            hwnd: None,
            update_status: None,
            update_available: false,
            update_checking: false,
            news: None,
            news_fetched: false,
            autostart_enabled: crate::platform::autostart::is_enabled(),
            applied_appearance: None,
            threshold_armed_5h: false,
            last_reset_5h: None,
            threshold_armed_weekly: false,
            last_reset_weekly: None,
            last_logged_error: None,
            alias_gen: std::cell::Cell::new(0),
        }
    }

    pub(crate) fn hwnd(&self) -> HWND {
        self.hwnd.unwrap_or_default()
    }

    /// Whether any surface is visible: the pinned panel, the account popup,
    /// or the About window. The poll arm's trim gate and the float-window
    /// sweep's last-window check share this single source — a fourth
    /// surface registered in only one copy would either trim under a
    /// visible UI or never return to the idle working set.
    pub(crate) fn ui_visible(&self) -> bool {
        self.panel.mode != crate::ui::panel::PanelMode::Hidden
            || self.popup.is_open()
            || self.about.is_open()
    }

    /// Single assignment point for update_status: derives the is_newer
    /// verdict alongside, so the mirror consumed by the settings-page
    /// cache key can never drift from its source.
    pub(crate) fn set_update_status(
        &mut self,
        r: Option<Result<crate::service::update::ReleaseInfo, String>>,
    ) {
        self.update_available = match &r {
            Some(Ok(info)) => {
                crate::service::update::is_newer(&info.tag, env!("CARGO_PKG_VERSION"))
            }
            _ => false,
        };
        self.update_status = r;
    }

    /// Opens a fresh attempt round: clears the failure-log cursor and
    /// fetches immediately — a repeated failure with the same message gets
    /// logged again. Shared by manual retry and the proxy-change/source-
    /// switch arms
    pub(crate) fn refresh_now(&mut self) {
        self.last_logged_error = None;
        if let Some(p) = &self.poller {
            p.refresh_now();
        }
    }

    fn sync_poll_context(&self) {
        let target = self
            .config
            .selected_account()
            .filter(|(_, a)| !a.api_key.trim().is_empty())
            .map(|(_, a)| crate::api::AccountSpec {
                platform: a.platform,
                team: a.team,
                org_id: a.org_id.clone(),
                project_id: a.project_id.clone(),
                api_key: a.api_key.clone(),
            });
        *borrow(&self.poll_target) = target;
        *borrow(&self.poll_interval) = self
            .config
            .general
            .poll_interval_secs
            .clamp(MIN_POLL_SECS, MAX_POLL_SECS);
    }

    /// Refreshes the tray tooltip and icon
    /// NIM_MODIFY is a cross-thread synchronous call that may re-enter the
    /// message pump and invalidate the reference: after each submission,
    /// re-fetch the reference, recompute the decision, and only then
    /// continue. Content keys advance only after a successful submission;
    /// failures are retried by the next refresh.
    pub(crate) fn update_tray_icon(&mut self) {
        let tray_hwnd = self.hwnd();
        // While unregistered, NIM_MODIFY is guaranteed to fail: skip the
        // call, do not advance the key, and let the full resubmit after
        // re-registration self-heal. Checked before building the tooltip —
        // the text build allocates per bucket just to be discarded.
        let tray_up = self.tray_state.registered();
        // Tooltip first: compare against the last submitted text and skip
        // the call when unchanged. Text-level comparison naturally tracks
        // label additions/edits, whereas a fingerprint key missing a
        // segment would freeze the old text.
        if let Some(tip) = tray_up.then(|| tooltip_text(self))
            && self.tray_state.last_tip.as_deref() != Some(tip.as_str())
        {
            let gen_before = self.tray_state.generation();
            let tip_set = self.tray_state.set_tooltip(&tip);
            // A nested writer winning costs at most one redundant resubmit
            // (harmless); recording an undelivered recomputed value would
            // make the next round mistakenly skip the resubmit (harmful).
            if tip_set {
                submit_tray_gen(tray_hwnd, gen_before, |app| {
                    app.tray_state.last_tip = Some(tip);
                });
            }
        }
        // set_tooltip's NIM_MODIFY is a pump point: re-fetch the reference
        // and recompute the decision, so new state arriving inside the pump
        // is not overwritten by pre-pump old values.
        let Some(app) = app_from(tray_hwnd) else {
            return;
        };
        // The icon is likewise only submitted to a registered tray: while
        // unregistered, building the icon would just be destroyed along
        // with the failure — pure wasted GDI work.
        if !app.tray_state.registered() {
            return;
        }
        let failed = app.data.last_error.is_some() && app.data.snapshot.is_none();
        // Single source for ring-progress bucket priority: api::ring_percent;
        // any criteria divergence from the panel row set / tooltip is
        // reconciled here.
        let used = app
            .data
            .snapshot
            .as_ref()
            .map(UsageSnapshot::ring_percent)
            .unwrap_or(0.0);
        // The key includes the pixel size: after a display-scale change the
        // icon is rebuilt at the new size, not reused from the old bitmap.
        let px = icon::tray_px();
        let key = (px, used.round() as i64, app.data.snapshot.is_some(), failed);
        if app.tray_state.last_icon_key == Some(key) {
            return;
        }
        let new = match &app.data.snapshot {
            Some(_) => icon::ring_icon(px, used, failed),
            None if failed => icon::ring_icon(px, 0.0, true),
            None => icon::resource_icon(px).or_else(|| icon::logo_icon(px)),
        };
        let Some(new) = new else {
            return;
        };
        // Pre-pump generation: a nested refresh inside the pump installing
        // its own icon, or a rearm re-adding the in-use handle, both
        // advance it; after the pump, yielding is decided on this one
        // criterion alone. A nested writer winning means committing would
        // destroy the in-use handle and writing back the key would seal off
        // the full refresh rearm posted.
        let gen_before = app.tray_state.generation();
        // Destroy the old handle and advance the key only on successful
        // submission; failures are retried by the next refresh.
        let set = app.tray_state.update_icon(new);
        if !set {
            icon::destroy_owned(new);
            return;
        }
        let mut new = Some(new);
        if !submit_tray_gen(tray_hwnd, gen_before, |app| {
            app.tray_state.install_icon(new.take(), key);
        }) && let Some(n) = new.take()
        {
            icon::destroy_owned(n);
        }
    }

    /// Storage and animation phase; returns notifications to send. The
    /// trailing pump points (relayout / working-set trim / notification
    /// sending) are executed by the drain arm after re-fetching the
    /// reference — no reference inside this method crosses a pump.
    fn handle_poll_result(&mut self, msg: PollMessage) -> Vec<String> {
        // A late result from an old generation is dropped outright: after
        // switching the poll source, the old account's data must not
        // masquerade as the current account's.
        if msg.generation != self.poll_gen.load(std::sync::atomic::Ordering::Acquire) {
            return Vec::new();
        }
        // Notifications are only collected, never sent here: the WinRT
        // toast re-enters the message pump, and the reference is void
        // afterwards.
        let mut notices = Vec::new();
        // Hoisted above the footer-animation block: start_anim re-fetches
        // the App alias, so reads through `self` afterwards are stale.
        let tray = self.hwnd();
        match msg.outcome {
            PollOutcome::Success(mut snap) => {
                // Play the change-up animation only when the snapshot
                // timestamp advanced; not when the footer currently shows an
                // error summary: the pac-man's start point is a fixed-width
                // static label, and splicing it onto scrolling text would
                // jump — recovery from an error switches straight to the
                // normal label.
                let prev_at = self.data.snapshot.as_ref().map(|s| s.queried_at);
                let anim_prev = prev_at.filter(|t| *t != snap.queried_at);
                let anim_ok = self.data.last_error.is_none()
                    && anim_prev.is_some()
                    && self.panel.mode == crate::ui::panel::PanelMode::Pinned
                    && self.panel.view == crate::ui::panel::PanelView::Main;
                let anim_texts = match (anim_prev, anim_ok) {
                    (Some(prev), true) => {
                        let s = self.strings;
                        Some((prev, s.updated_just_now.to_string()))
                    }
                    _ => None,
                };
                // Notifications are collected first and sent later: the
                // WinRT toast re-enters the message pump, and sending here
                // would make the snapshot storage that follows go through
                // an invalidated reference.
                notices = self.collect_notifications(&snap);
                // Sidecar backfill: the balance and Token tiers reuse the
                // previous snapshot when this round failed.
                let prev = self.data.snapshot.take();
                crate::api::merge_sidecar(&mut snap, prev.as_ref());
                self.data.snapshot = Some(*snap);
                self.data.last_error = None;
                self.last_logged_error = None;
                if let Some((prev, new_text)) = anim_texts {
                    let ph = self.panel.hwnd;
                    if let (Some(p), Some(r)) = (ph, self.panel.renderer.as_mut()) {
                        // The animation toggle is decided from the
                        // renderer's cache — the same source as
                        // appear/spin.
                        if r.animations_on() {
                            // The start point takes the on-screen frozen
                            // string (the frame buffer's current value),
                            // or when absent is recomputed from the old
                            // snapshot time: byte-for-byte equal to the
                            // previous frame.
                            let old_text =
                                r.cached_footer().map(str::to_string).unwrap_or_else(|| {
                                    crate::ui::fmt::updated_text(self.strings, self.lang, prev)
                                });
                            if old_text != new_text {
                                // Pre-build one layout each for old and new
                                // so animation frames create zero layouts;
                                // the start block is exactly the on-screen
                                // frozen block, typeset via
                                // make_footer_block — same source as the
                                // static footer.
                                let old = r
                                    .take_footer_block()
                                    .unwrap_or_else(|| unsafe { r.make_footer_block(&old_text) });
                                let new = unsafe { r.make_footer_block(&new_text) };
                                r.start_footer_anim(old, new);
                                unsafe { crate::ui::panel::start_anim(p) };
                            }
                        }
                    }
                }
            }
            PollOutcome::Failure(e) => {
                // A failure whose message matches the previous one is
                // logged only once, so fixed-interval retries do not flood
                // the log.
                let msg = e.to_string();
                let repeated = self
                    .last_logged_error
                    .as_ref()
                    .is_some_and(|prev| *prev == msg);
                if !repeated {
                    crate::platform::log(&format!("轮询失败: {msg}"));
                }
                self.last_logged_error = Some(msg);
                self.data.last_error = Some(*e);
            }
        }
        // The tray refresh is posted asynchronously: the NIM_MODIFY pump
        // point is confined to the wndproc arm, and no reference here
        // crosses a pump. The handle is hoisted above the footer-animation
        // block: start_anim re-fetches the App alias, and this trailing
        // read through `self` would go through the invalidated one.
        queue_tray_refresh(tray);
        notices
    }

    /// Collects threshold/reset notifications: updates the armed baselines
    /// and produces message texts without sending.
    fn collect_notifications(&mut self, snap: &UsageSnapshot) -> Vec<String> {
        let g = &self.config.general;
        let mut out: Vec<String> = Vec::new();
        out.extend(check_reset(
            snap.five_hour.as_ref(),
            &mut self.last_reset_5h,
            &mut self.threshold_armed_5h,
            g.notify_reset_5h_enabled,
            self.strings.notify_reset_5h,
        ));
        out.extend(check_reset(
            snap.weekly.as_ref(),
            &mut self.last_reset_weekly,
            &mut self.threshold_armed_weekly,
            g.notify_reset_weekly_enabled,
            self.strings.notify_reset_weekly,
        ));

        if g.notify_threshold_enabled {
            let th = g.notify_threshold_percent as f64;
            out.extend(check_threshold(
                snap.five_hour.as_ref(),
                &mut self.threshold_armed_5h,
                th,
                self.strings.five_hour,
                self.strings.notify_threshold_title,
            ));
            out.extend(check_threshold(
                snap.weekly.as_ref(),
                &mut self.threshold_armed_weekly,
                th,
                self.strings.weekly,
                self.strings.notify_threshold_title,
            ));
        }
        out
    }

    /// The sole entry point for left-click panel toggling: collapse when
    /// pinned, expand when hidden. The expand sequence sets height → pops
    /// the panel → applies the appearance; the branch resets to the main
    /// view before showing, so a view switch must come after it — wrong
    /// order leaves the door opened on the wrong page. Callers that want
    /// to expand only (never collapse) must first confirm the state is
    /// Hidden themselves. The collapse path needs no height recomputation
    pub(crate) fn toggle_panel(&'static mut self, tray: HWND, anchor: RECT) {
        let n = self.config.accounts.len();
        match self.panel.mode {
            // Hiding re-enters the message pump; the reference is not
            // touched afterwards.
            crate::ui::panel::PanelMode::Pinned => {
                if let Some(p) = self.panel.hwnd {
                    hide_panel(self, p);
                }
                return;
            }
            crate::ui::panel::PanelMode::Hidden => {
                sync_panel_derived(self);
                self.panel.show_from_hidden(tray, anchor, n);
            }
        }
        // The expand sequence embeds a message pump; re-fetch the reference
        // after it to finish up.
        // Already inside the window's final minute: switch to the second
        // tick immediately instead of waiting for the first tick.
        if let Some(app) = app_from(tray) {
            apply_appearance(app);
            if let Some(h) = app.panel.hwnd {
                crate::ui::panel::retune_minute(app, h);
            }
        }
    }
}

/// A poisoned lock still hands back its inner data to keep using: the
/// lock is only held for short critical sections, so poisoning means the
/// lock-holding thread died — the data itself remains intact.
use crate::locks::borrow;

/// Shared post-pump commit for the tray's NIM_MODIFY submissions: the
/// generation is captured pre-pump by the caller, re-fetched and compared
/// here; `commit` runs only on an unchanged generation and advances it —
/// a nested writer inside the pump (refresh or rearm) has already
/// advanced the state, and pre-pump values would clobber it. Returns
/// whether the commit ran; callers holding artifacts (an uninstalled
/// icon handle) discard them on false.
fn submit_tray_gen(tray_hwnd: HWND, gen_before: u64, commit: impl FnOnce(&mut App)) -> bool {
    if let Some(app) = app_from(tray_hwnd)
        && app.tray_state.generation() == gen_before
    {
        // Re-borrow for the commit so the generation advance below still
        // has the reference.
        commit(&mut *app);
        app.tray_state.bump_gen();
        true
    } else {
        false
    }
}

/// Asynchronous entry point for tray content refresh
/// The NIM_MODIFY pump point is confined to the wndproc arm; the caller's
/// reference does not cross a pump
pub(crate) fn queue_tray_refresh(tray_hwnd: HWND) {
    unsafe {
        let _ = PostMessageW(
            Some(tray_hwnd),
            crate::platform::msg::WM_APP_TRAY_REFRESH,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// Self-healing sequence for tray re-registration: shared source for both
/// the TaskbarCreated route and the retry timer. NIM_ADD and NIM_MODIFY
/// are both pump points, each re-fetching the reference on the way in and
/// out; on success, write back the registered flag, clear the content
/// keys, and post a full refresh — the usage ring takes over immediately
/// without waiting for the next polling reply. Returns false on failure;
/// the retry cadence belongs to the caller.
fn rearm_tray(hwnd: HWND) -> bool {
    // Take everything in one pass before the pump point (readd's NIM_ADD):
    // icon/tip/tray all derive from the same immutable borrow, so the pump
    // point spans no mutable alias.
    let prepared = app_from(hwnd).map(|app| {
        let tip = tooltip_text(app);
        let tray: Option<&'static crate::ui::tray::TrayIcon> = app.tray_state.tray.as_ref();
        (app.tray_state.icon, tip, tray)
    });
    let Some((icon, tip, tray)) = prepared else {
        return false;
    };
    let ok = match (icon, tray) {
        (Some(icon), Some(tray)) => tray.readd(icon, &tip),
        _ => false,
    };
    // A single re-fetch after the pump: the tray write-back and the App
    // field cleanup share one reference — no second alias stacked on top.
    // rearmed folds the write-back: registered flag, cleared content keys,
    // advanced generation (a re-add is an external overwrite; any
    // in-flight refresh yields).
    if ok && let Some(app) = app_from(hwnd) {
        app.tray_state.rearmed();
        queue_tray_refresh(hwnd);
    }
    ok
}

/// Shared exit point for the tray menu (right-click and keyboard routes
/// share it): dismiss the account popup before opening the menu — the
/// menu window belongs to this process too, so foreground detection is
/// unaffected by the patrol, and two UI pieces coexisting only adds
/// confusion. The hidden pump inside close consumes the reference within
/// this function, and nothing continues with it after the pump; pos is
/// located by each of the two routes (cursor vs icon-rect center).
fn open_tray_menu(
    hwnd: HWND,
    panel: Option<windows::Win32::Foundation::HWND>,
    s: &'static crate::ui::i18n::Strings,
    pos: windows::Win32::Foundation::POINT,
) {
    if let Some(app) = app_from(hwnd) {
        let _ = close_popup(app, hwnd);
    }
    // The menu's owner is always this tray window itself (USERDATA carries
    // App from the moment the window exists), so hwnd doubles as the owner.
    crate::app::modal::show_context_menu(hwnd, panel, s, pos);
}

/// Hidden tray window procedure
extern "system" fn tray_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        crate::platform::msg::WM_APP_TRAY_REFRESH => {
            if let Some(app) = app_from(hwnd) {
                app.update_tray_icon();
            }
            LRESULT(0)
        }
        WM_APP_TRAY => {
            let app = app_from(hwnd);
            let code = tray::parse_callback(lparam);
            match code {
                // Hover shows the tooltip summary; left click opens the
                // panel.
                // tray_rect is a cross-process synchronous call and thus a
                // pump point: copy the decision first, re-fetch after the
                // pump, then touch the panel.
                windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP => {
                    let rect = app.map(|app| open_anchor(hwnd, app));
                    if let Some(rect) = rect
                        && let Some(app) = app_from(hwnd)
                    {
                        app.toggle_panel(hwnd, rect);
                    }
                }
                windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONUP => {
                    let ctx = app.map(|a| (a.panel.hwnd, a.strings));
                    if let Some((panel, s)) = ctx {
                        open_tray_menu(hwnd, panel, s, tray::context_menu_pos());
                    }
                }
                // Keyboard activation arrives as WM_CONTEXTMENU without
                // coordinates (right click has its own RBUTTONUP arm); the
                // cursor may be anywhere, so the menu is positioned at the
                // icon rect's center.
                windows::Win32::UI::WindowsAndMessaging::WM_CONTEXTMENU => {
                    if let Some(app) = app {
                        let panel = app.panel.hwnd;
                        let s = app.strings;
                        let pos = tray_rect(app)
                            .map(|r| windows::Win32::Foundation::POINT {
                                x: (r.left + r.right) / 2,
                                y: (r.top + r.bottom) / 2,
                            })
                            .unwrap_or_else(tray::context_menu_pos);
                        open_tray_menu(hwnd, panel, s, pos);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        msg if msg == *tray::TASKBAR_CREATED => {
            // Explorer restart: the tray icon died with the taskbar
            // process; re-register as soon as the broadcast arrives.
            // readd and tray_rect are both pump points: do the read-only
            // decision up front, re-fetch after the pump.
            let pinned = app_from(hwnd)
                .is_some_and(|app| app.panel.mode == crate::ui::panel::PanelMode::Pinned);
            rearm_tray(hwnd);
            // The icon may have moved after the taskbar rebuild; refresh
            // the anchor while the panel is still open, so near_tray
            // keep-alive and the next relayout do not misjudge from the
            // stale rect.
            if pinned
                && let Some(rect) = app_from(hwnd).and_then(|app| tray_rect(app))
                && let Some(app) = app_from(hwnd)
            {
                app.panel.anchor = Some(rect);
            }
            // On a successful catch-up, kill any retry timer still running;
            // on failure, arm the retry timer to take over so the icon is
            // not absent until the next explorer restart.
            if let Some(app) = app_from(hwnd) {
                if app.tray_state.registered() {
                    unsafe {
                        use windows::Win32::UI::WindowsAndMessaging::KillTimer;
                        let _ = KillTimer(Some(hwnd), TIMER_TRAY_RETRY);
                    }
                } else {
                    app.tray_state.retries = 0;
                    unsafe {
                        SetTimer(Some(hwnd), TIMER_TRAY_RETRY, TRAY_RETRY_MS, None);
                    }
                }
            }
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_ACTIVATE => {
            // Foreground arrives at the tray window (the context menu
            // flow): also wake the panel's frozen marquee — the activation
            // message goes only to this window, so the panel never receives
            // it itself.
            if (wparam.0 & 0xFFFF) != 0 {
                crate::ui::panel::wake_frozen_marquee(hwnd);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_POWERBROADCAST => {
            // On resume, fetch immediately: the wait timer and the wall
            // clock drifted apart during sleep. Match AUTOMATIC only —
            // every resume broadcasts it, while a user-initiated resume
            // also adds SUSPEND, and catching both would double-trigger.
            use windows::Win32::UI::WindowsAndMessaging::PBT_APMRESUMEAUTOMATIC;
            if wparam.0 == PBT_APMRESUMEAUTOMATIC as usize
                && let Some(app) = app_from(hwnd)
            {
                // Through App::refresh_now, same as every other immediate
                // pull: it also clears the failure-log cursor, so an
                // identical post-resume failure still leaves a log trace.
                app.refresh_now();
            }
            LRESULT(1)
        }
        WM_TIMER => {
            // Short-haul retry for a failed first tray registration;
            // removed on success or when the cap is hit, with
            // TaskbarCreated as the fallback. Unknown timer ids fall
            // through to default handling so future callback timers are
            // not swallowed.
            if wparam.0 == TIMER_TRAY_RETRY {
                let registered = app_from(hwnd).map(|app| app.tray_state.registered());
                if let Some(registered) = registered {
                    let mut done = registered;
                    if !registered {
                        done = rearm_tray(hwnd);
                        // The counter only records retries actually
                        // performed: rearm's NIM_ADD pump may re-enter this
                        // arm, and counting before the pump would record
                        // one retry as several and exhaust the cap early.
                        if !done && let Some(app) = app_from(hwnd) {
                            app.tray_state.retries += 1;
                        }
                    }
                    if done
                        || app_from(hwnd)
                            .is_some_and(|app| app.tray_state.retries >= TRAY_RETRY_MAX)
                    {
                        unsafe {
                            use windows::Win32::UI::WindowsAndMessaging::KillTimer;
                            let _ = KillTimer(Some(hwnd), TIMER_TRAY_RETRY);
                        }
                    }
                }
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }
        WM_APP_POLL_RESULT => {
            // Wake code with no payload: results are queued in the channel
            // and taken blindly — a forged message misses at worst.
            // The tail phases (relayout / trim / notification sending) are
            // all pump points: re-fetch the reference each round; the
            // previous round's alias must not carry across a pump.
            // App is re-fetched before taking a result: a transiently
            // missing App leaves the queue intact for the next wake
            // instead of dropping the taken message.
            while let Some(app) = app_from(hwnd)
                && let Some(msg) = crate::service::poller::POLL_SLOT.take()
            {
                let notices = app.handle_poll_result(msg);
                // Notification sending runs first in the round's tail: the
                // WinRT toast re-enters the message pump, and every later
                // pump (relayout, trim) would let a queued account switch
                // run mid-round — delivering this account's toast while
                // the app already displays the next one. Sending before
                // those pumps keeps the toast attributed to the account
                // the baselines were armed against.
                for body in &notices {
                    crate::platform::notify::show(NOTIFY_TITLE, body);
                }
                if let Some(p) = app_from(hwnd).and_then(|app| app.panel.hwnd)
                    && let Some(app) = app_from(hwnd)
                {
                    relayout_panel(app, p);
                }
                // relayout's SetWindowPos is itself a pump (a cross-monitor
                // move dispatches WM_DPICHANGED, which re-derives the App
                // alias): re-fetch before reading visibility.
                let Some(app) = app_from(hwnd) else {
                    break;
                };
                // Skip the trim while the UI is visible: freshly used pages
                // would be paged out immediately and faulted back in next
                // round — a pure loss on the round trip; returning pages
                // when idle is left to the collapse path as fallback.
                if !app.ui_visible() {
                    crate::platform::trim_working_set();
                }
            }
            LRESULT(0)
        }
        WM_APP_UPDATE_RESULT => {
            // relayout is a pump point: re-fetch the reference each round;
            // aliases do not carry across pumps. App is re-fetched before
            // taking a result: a transiently missing App leaves the queue
            // intact for the next wake instead of dropping the taken one.
            while let Some(app) = app_from(hwnd)
                && let Some(r) = UPDATE_TASK.drain().next()
            {
                app.update_checking = false;
                // Failure reasons go to the log: the UI only shows a fixed
                // label, and the diagnostic chain for weak networks /
                // rate limiting must not be severed here.
                if let Err(e) = &r {
                    crate::platform::log(&format!("检查更新失败: {e}"));
                }
                app.set_update_status(Some(r));
                if let Some(p) = app.panel.hwnd {
                    relayout_panel(app, p);
                    invalidate_panel(p);
                }
            }
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE => {
            // With no explicit appearance set, a system theme switch
            // repaints the panel immediately. lParam stays un-dereferenced:
            // it points into the sender's address space, and a forged
            // cross-process message can carry a garbage pointer — instead
            // every settings change re-resolves the system theme, and the
            // no-change case is absorbed by apply_appearance's idempotent
            // guard (settings changes are rare user actions).
            if let Some(app) = app_from(hwnd)
                && crate::ui::panel::theme::explicit_appearance(
                    app.config.general.appearance.as_deref(),
                )
                .is_none()
                && apply_appearance(app)
            {
                invalidate_ui(app);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_APP_NEWS_RESULT => {
            // refit_about's SetWindowPos is a pump point: re-fetch the
            // reference each round.
            while let Some(app) = app_from(hwnd)
                && let Some(result) = NEWS_TASK.drain().next()
            {
                match result {
                    Ok(news) => {
                        app.news = Some(news);
                        // On a slow network the About window may already be
                        // open at the base height: rearrange the window
                        // height once the news arrives.
                        refit_about(app);
                    }
                    Err(e) => {
                        crate::platform::log(&format!("动态拉取失败: {e}"));
                        // Reset the gate so the next About-window open can
                        // retry.
                        app.news_fetched = false;
                    }
                }
            }
            LRESULT(0)
        }
        WM_APP_WAKE_INSTANCE => {
            // The second instance's wake semantics are "pop out": do
            // nothing when already shown; open the door only when hidden.
            // tray_rect is a pump point: copy the decision first, re-fetch
            // after the pump, then touch the panel.
            let rect = app_from(hwnd)
                .filter(|app| app.panel.mode == crate::ui::panel::PanelMode::Hidden)
                .map(|app| open_anchor(hwnd, app));
            if let Some(rect) = rect
                && let Some(app) = app_from(hwnd)
            {
                app.toggle_panel(hwnd, rect);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd = (wparam.0 & 0xFFFF) as u16;
            match cmd {
                IDM_SETTINGS => {
                    // Do not re-enter when already on the settings page:
                    // re-entry would close an open account popup and run a
                    // redundant relayout — the prefill itself is safe (the
                    // edited marks hold), but the no-op on an already-shown
                    // page is the quieter behavior. tray_rect is a pump
                    // point: copy the decision first, re-fetch after the
                    // pump.
                    // When the panel is visible, rect is None: toggle then
                    // only switches the view; carrying a rect would take the
                    // open-door branch and collapse the panel. The open
                    // anchor shares open_anchor with left-click and wake, so
                    // no entry has a dead end.
                    let decision = app_from(hwnd)
                        .filter(|app| {
                            !(app.panel.mode != crate::ui::panel::PanelMode::Hidden
                                && app.panel.view == crate::ui::panel::PanelView::Settings)
                        })
                        .map(|app| {
                            (app.panel.mode == crate::ui::panel::PanelMode::Hidden)
                                .then(|| open_anchor(hwnd, app))
                        });
                    if let Some(rect) = decision {
                        if let Some(rect) = rect
                            && let Some(app) = app_from(hwnd)
                        {
                            app.toggle_panel(hwnd, rect);
                        }
                        // Enter settings only when the panel is visible:
                        // if opening the door failed, the panel stays
                        // hidden and the view change would be discarded by
                        // the next open's reset.
                        if let Some(app) = app_from(hwnd)
                            .filter(|app| app.panel.mode != crate::ui::panel::PanelMode::Hidden)
                            && let Some(p) = app.panel.hwnd
                        {
                            enter_settings(app, p);
                        }
                    }
                }
                IDM_ABOUT => {
                    if let Some(app) = app_from(hwnd) {
                        // Fetch only when the About window is first opened;
                        // not at startup — on a weak network the raw domain's
                        // slow resolution would queue inside the process,
                        // stalling the startup poll and delaying the panel's
                        // first paint.
                        if app.news.is_none() && !app.news_fetched {
                            app.news_fetched = true;
                            if !NEWS_TASK.spawn(hwnd, crate::service::whatsnew::fetch_latest) {
                                app.news_fetched = false;
                            }
                        }
                        // Close the popup before opening the About window:
                        // SW_HIDE re-enters the message pump synchronously,
                        // and the reference is moved into the helper in
                        // exchange for the post-pump new one; the two
                        // floating windows are never on stage together.
                        // The height is recomputed after the pump from the
                        // live state: news arriving inside the pump is only
                        // refitted for an already-open window, and a stale
                        // pre-pump height would clip the news section
                        // outside the window.
                        if let Some(app) = close_popup(app, hwnd) {
                            let h = crate::ui::panel::render::about::about_height(
                                app.news.as_deref(),
                                app.about.news_expanded,
                            );
                            app.about.open(hwnd, h);
                        }
                    }
                }
                IDM_EXIT => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_ENDSESSION => {
            // Session end (logoff/shutdown) goes through destroy cleanup:
            // the tray icon's NIM_DELETE hangs off the destroy path, and
            // skipping it leaves a ghost icon. wparam 0 means the session
            // was canceled.
            if wparam.0 != 0 {
                // The poll thread is abandoned before the destroy path:
                // joining it (the message loop's post-exit Drop) blocks on
                // an in-flight request's agent timeout — up to ~15s of
                // stalled session teardown and a force-kill risk that
                // defeats the NIM_DELETE cleanup this arm exists for. The
                // process dies with the session either way.
                if let Some(app) = app_from(hwnd)
                    && let Some(p) = app.poller.take()
                {
                    p.detach();
                }
                let _ = unsafe { DestroyWindow(hwnd) };
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn tray_rect(app: &App) -> Option<RECT> {
    app.tray_state.rect()
}

/// Opening anchor for toggle_panel: the tray icon rect, falling back to the
/// work area's bottom-right when unobtainable (Explorer misbehaving, or the
/// re-registration retry window). Shared by left-click, wake, and the
/// settings menu so no open path has a dead end.
fn open_anchor(hwnd: HWND, app: &App) -> RECT {
    tray_rect(app).unwrap_or_else(|| fallback_anchor(hwnd))
}

/// Open-door anchor for when the icon rect is unobtainable: a small
/// square at the bottom-right corner of the tray window's monitor work
/// area — the panel pops out anchored as usual, just without the icon's
/// exact position
fn fallback_anchor(hwnd: HWND) -> RECT {
    let mi = unsafe { crate::ui::panel::monitor_info_of(hwnd) };
    RECT {
        left: mi.rcWork.right - 44,
        top: mi.rcWork.bottom - 44,
        right: mi.rcWork.right - 20,
        bottom: mi.rcWork.bottom - 20,
    }
}

/// Tray tooltip text: app name + a usage summary per bucket; app name
/// alone when there is no data. The bucket set shares present_buckets
/// with the panel row set, with labels paired per bucket variant —
/// with a bucket missing, the positional zip would pair later buckets
/// with earlier labels.
fn tooltip_text(app: &App) -> String {
    let Some(snap) = app.data.snapshot.as_ref() else {
        return "Quotify".to_string();
    };
    use crate::api::PresentBucket;
    let label = |b: &PresentBucket<'_>| match b {
        PresentBucket::FiveHour(_) => app.strings.tooltip_5h,
        PresentBucket::Weekly(_) => app.strings.tooltip_weekly,
        PresentBucket::Mcp(_) => app.strings.tooltip_mcp,
    };
    let segs: Vec<String> = snap
        .present_buckets()
        .map(|b| {
            format!(
                "{} {}",
                label(&b),
                crate::ui::fmt::percent(b.used_percent())
            )
        })
        .collect();
    if segs.is_empty() {
        "Quotify".to_string()
    } else {
        format!("Quotify | {}", segs.join(" | "))
    }
}

/// Single entry to get App from any window hwnd of this program: the tray
/// window's USERDATA is App and hits directly; the panel and floating
/// windows go through their parent. There were once two lookup functions,
/// and the tray arm mistakenly using the parent-chain version silently
/// returned None — the single entry eliminates that wrong-pick possibility.
/// App is boxed and lives until process exit, so the GWLP_USERDATA pointer
/// is valid throughout and can be reinterpreted as `&'static mut`; alias
/// safety relies on the single-threaded message loop.
///
/// Pump-point registry — synchronous calls that re-enter the message loop
/// and thereby invalidate the caller's App alias; after any of these, the
/// alias must be re-derived here (or the call routed through pump()):
/// SetWindowPos (cross-monitor dispatches WM_DPICHANGED) · ShowWindow /
/// SW_HIDE · SetForegroundWindow · SetFocus · SetCapture / ReleaseCapture
/// · Shell_NotifyIcon (NIM_ADD/MODIFY/DELETE) · Shell_NotifyIconGetRect ·
/// TrackPopupMenu / MessageBoxW / file dialogs · WinRT toast · clipboard
/// OpenClipboard / delayed-render reads · ImmAssociateContext /
/// ImmDestroyContext · EnableWindow. Extend this list when a new pump
/// call is introduced.
pub(crate) fn app_from_tray(hwnd: HWND) -> Option<&'static mut App> {
    unsafe {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if p != 0 {
            let app = &mut *(p as *mut App);
            bump_alias_gen(app);
            return Some(app);
        }
        let parent = GetWindowLongPtrW(hwnd, GWLP_HWNDPARENT);
        if parent == 0 {
            return None;
        }
        let p = GetWindowLongPtrW(HWND(parent as *mut _), GWLP_USERDATA);
        (p != 0).then(|| {
            let app = &mut *(p as *mut App);
            bump_alias_gen(app);
            app
        })
    }
}

/// Advance the alias generation on each re-derive (debug tripwire state).
fn bump_alias_gen(app: &mut App) {
    app.alias_gen.set(app.alias_gen.get().wrapping_add(1));
}

/// Snapshot the alias generation before a synchronous pump; pair with
/// alias_assert_live after it. Debug-only UB tripwire: a mismatch means
/// the alias was re-derived inside the pump and the caller is holding a
/// dead reference — exactly the class the pump discipline guards against,
/// turned from silent UB into an assertion. The trailing read is itself
/// through the possibly-dead alias, which is the point: one controlled
/// read to catch every later one.
pub(crate) fn alias_probe(app: &App) -> u64 {
    app.alias_gen.get()
}

/// Assert the alias survived the pump since the probe snapshot.
pub(crate) fn alias_assert_live(snapshot: u64, app: &App) {
    debug_assert_eq!(
        app.alias_gen.get(),
        snapshot,
        "App 引用在同步泵内被重派生——泵后必须重取（app_from_tray/pump）"
    );
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
/// For bare pump points that keep using the old alias afterwards (on the
/// justified assumption the pump dispatches no re-fetching message),
/// bracket the call with alias_probe/alias_assert_live instead — the
/// debug tripwire turns a violated assumption into an assertion. Default
/// to this wrapper (re-fetch semantics for free); the probe/assert pair
/// is the fallback for continuations that must stay on the same alias.
pub(crate) fn pump(
    app: &mut App,
    hwnd: HWND,
    f: impl FnOnce(&mut App),
) -> Option<&'static mut App> {
    f(app);
    app_from_tray(hwnd)
}

/// Short alias keeping this file's call sites terse
use self::app_from_tray as app_from;

/// Normalizes config value ranges, shared by the startup load and import;
/// the threshold is clamped back to 1–100, the interval into the legal
/// range, and selected is backfilled via backfill_selected — with no
/// selection there is neither a polling target nor selection UI (a single
/// account renders no switch arrows), and the main view would fall into
/// the onboarding empty state by mistake
fn normalize_config(c: &mut Config) {
    c.general.notify_threshold_percent = c.general.notify_threshold_percent.clamp(1, 100);
    c.general.poll_interval_secs = c
        .general
        .poll_interval_secs
        .clamp(MIN_POLL_SECS, MAX_POLL_SECS);
    // Team edition exists only on the CN site: an imported team+Intl
    // account would send team query params and selection headers to the
    // international base URL and get empty limits every round — the exact
    // mismatch the rule prevents. Force the account back to CN; the two UI
    // toggle arms keep this combination out interactively.
    for a in &mut c.accounts {
        if a.team && a.platform == crate::api::Platform::Intl {
            a.platform = crate::api::Platform::Cn;
        }
    }
    // A hand-edited read marker may carry a non-zero-padded date, whose
    // lexical comparison would suppress every NEW badge: fold onto the
    // canonical form; a non-date value carries no marker semantics and
    // clears (back to the all-unread default).
    c.general.last_news_read = c
        .general
        .last_news_read
        .as_deref()
        .and_then(crate::service::whatsnew::normalize_date);
    c.backfill_selected();
}

/// Applies the effective appearance; returns whether the resolved value
/// changed (callers gate their repaint on it — the WM_SETTINGCHANGE arm
/// fires on every unrelated settings broadcast and must not repaint on a
/// no-change). The whole effect below is a pure function of the resolved
/// value, so an unchanged value skips it: the menu-theme call and every
/// live renderer's Theme rebuild. In follow-system mode a transient
/// registry read failure resolves to None and keeps the last applied
/// value — flipping to light here would repaint every surface wrong on
/// an unrelated broadcast.
fn apply_appearance(app: &mut App) -> bool {
    let Some(appearance) =
        crate::ui::panel::theme::resolved_checked(app.config.general.appearance.as_deref())
    else {
        return false;
    };
    if app.applied_appearance == Some(appearance) {
        return false;
    }
    // The tray menu is drawn by the system, so the mode must be set
    // separately at the process level for subsequently popped menus to
    // carry the theme.
    crate::platform::menu_theme::apply(matches!(
        appearance,
        crate::ui::panel::theme::Appearance::Dark
    ));
    if let Some(r) = app.panel.renderer.as_mut() {
        r.theme = crate::ui::panel::theme::Theme::new(appearance);
    }
    for r in [&mut app.popup.wnd.renderer, &mut app.about.wnd.renderer]
        .into_iter()
        .flatten()
    {
        r.theme = crate::ui::panel::theme::Theme::new(appearance);
    }
    app.applied_appearance = Some(appearance);
    true
}

/// Catch-up entry point for render-derived fields: main-view height,
/// account error marker, caret context
fn sync_panel_derived(app: &mut App) {
    app.panel.sync_derived(
        app.data.snapshot.as_ref(),
        app.data.last_error.as_ref(),
        !app.config.accounts.is_empty(),
    );
}

/// Effective-layout switch: expansion takes effect immediately with a
/// gradual reveal; shrink keeps the origin layout while the window
/// collapses and clips it, catching up when the transition ends.
fn sync_layout_effective(app: &mut App, panel_hwnd: HWND) {
    app.panel.height_anim = None;
    let target_h = app.panel.view_height_for(
        app.panel.team.pending,
        app.panel.customizing.pending,
        app.config.accounts.len(),
    );
    if app.panel.begin_shrink_anim(panel_hwnd, LogPx(target_h)) {
        invalidate_panel(panel_hwnd);
        return;
    }
    relayout_panel(app, panel_hwnd);
}

/// Realigns an open popup with the panel geometry; driven uniformly by
/// the panel's WM_WINDOWPOSCHANGED
pub(crate) fn reposition_popup(app: &mut App, panel_hwnd: HWND) {
    app.popup
        .reposition(app.hwnd(), panel_hwnd, app.config.accounts.len());
}

/// Unified entry point that collapses the panel and closes the popup
/// along with it; collapses that miss this path are covered only by the
/// patrol fallback. The two steps each contain a hidden pump point: the
/// popup step is composed via close_popup, the reference moved in and
/// consumed in exchange for the post-pump new one — and the SW_HIDE at
/// the end of begin_hide is another round of nested pump, so any
/// reference taken afterwards must never be passed outward; continuations
/// re-fetch via app_from themselves.
pub(crate) fn hide_panel(app: &'static mut App, panel_hwnd: HWND) {
    let tray = app.hwnd();
    let mut app = close_popup(app, tray);
    if let Some(app) = &mut app {
        app.panel.begin_hide(panel_hwnd);
    }
}

/// Closes the account popup and hands back the post-pump new reference:
/// close's SW_HIDE re-enters the message pump synchronously, and the
/// caller's App reference is moved in and consumed here — using the old
/// reference after the pump is a compile error. With no popup open,
/// close is idempotent, equivalent to a plain re-fetch. Returns None if
/// the re-fetch fails; continuations take the empty path and exit early.
pub(crate) fn close_popup(app: &'static mut App, hwnd: HWND) -> Option<&'static mut App> {
    app.popup.close();
    app_from(hwnd)
}

/// Syncs layout state and repositions the panel
pub(crate) fn relayout_panel(app: &mut App, panel_hwnd: HWND) {
    // The effective layout catches up on relayout by default; the
    // exception is a same-view shrink transition — the origin layout
    // belongs to the animation alone there, while a cross-view transition
    // catches up as usual once terminated.
    let frozen = app
        .panel
        .height_anim
        .is_some_and(|a| a.view == app.panel.view);
    if !frozen {
        app.panel.height_anim = None;
        app.panel.catch_up_layout();
        reconcile_fading_focus(app, panel_hwnd);
    }
    sync_panel_derived(app);
    apply_appearance(app);
    if app.panel.mode == crate::ui::panel::PanelMode::Hidden {
        return;
    }
    if app.panel.anchor.is_none() {
        return;
    }
    app.panel.place(
        panel_hwnd,
        LogPx(app.panel.view_height(app.config.accounts.len())),
        false,
    );
    invalidate_panel(panel_hwnd);
    // place's SetWindowPos is a pump (a cross-monitor move dispatches
    // WM_DPICHANGED, which re-derives the alias): the shared tail
    // re-fetches and re-syncs the caret — layout shifts move the caret
    // row, and without the re-sync an in-progress IME composition window
    // floats at the old y until the next keystroke.
    sync_caret_after_geometry(panel_hwnd);
}

/// Application entry point
pub fn run() -> i32 {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        crate::platform::notify::ensure_aumid();

        let config = config::load();
        // The configured proxy takes effect before the first request, via
        // try_apply_proxy under the same rule as the settings page / import
        // — divergent acceptance rules would give the same address two
        // faces: "works at startup, rejected by Apply". On validation
        // failure no proxy is set and direct connection must be restored
        // explicitly: the proxy is controllable only inside the app, and
        // leaving it empty would let an environment-variable proxy take
        // effect silently.
        if !hits::try_apply_proxy(config.general.proxy.clone()) {
            let _ = crate::api::client::set_proxy(None);
        }
        let app = Box::new(App::new(config));
        let mut app = app;
        // Apply the appearance at startup so the first context menu before
        // the panel ever opens also carries the theme.
        // No renderers exist at this point, so there are no side effects.
        apply_appearance(&mut app);

        let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok();
        let hinst = hinst.unwrap_or_default();

        // Use the shared registration like the panel / floating windows:
        // class policy is maintained at a single point.
        if !crate::platform::register_wnd_class(TRAY_WND_CLASS, Some(tray_wndproc)) {
            return 1;
        }
        let class_name = wide(TRAY_WND_CLASS);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap_or_default();
        // Window creation failure (desktop heap exhaustion, etc.) must
        // abort startup: an empty hwnd entering the message loop would
        // block forever as an invisible process, and with the mutex held
        // every later launch would silently exit.
        if hwnd.is_invalid() {
            crate::platform::log("托盘窗口创建失败，进程退出");
            return 1;
        }

        app.hwnd = Some(hwnd);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *app as *mut App as isize);
        // The tray window exists: release any duplicate waiting on the
        // startup handshake (see platform::instance).
        crate::platform::instance::signal_tray_ready();

        let px = icon::tray_px();
        let initial = icon::resource_icon(px).or_else(|| icon::logo_icon(px));
        if initial.is_none() {
            crate::platform::log(&format!("初始图标加载失败 (px={px})"));
        }
        let initial = initial.unwrap_or_default();
        app.tray_state
            .install(TrayIcon::new(hwnd, initial), initial);
        // Autostart may race an unready taskbar; on failure arm the
        // short-haul retry until the icon is in place.
        if !app.tray_state.registered() {
            app.tray_state.retries = 0;
            SetTimer(Some(hwnd), TIMER_TRAY_RETRY, TRAY_RETRY_MS, None);
        }

        app.sync_poll_context();
        app.poller = Poller::spawn(
            hwnd,
            app.poll_target.clone(),
            app.poll_interval.clone(),
            app.poll_gen.clone(),
        );
        if let Some(p) = &app.poller {
            p.refresh_now();
        }

        // Diagnostics backdoor, effective in debug only:
        // QUOTIFY_DIAG=adding auto-opens the settings → add-account page.
        #[cfg(debug_assertions)]
        if std::env::var("QUOTIFY_DIAG").as_deref() == Ok("adding") {
            let rect = tray_rect(&app).unwrap_or(RECT {
                left: 1300,
                top: 880,
                right: 1340,
                bottom: 920,
            });
            // Moving into toggle_panel must use the re-fetched reference;
            // the outer reference is reserved for exit cleanup.
            if let Some(app) = app_from(hwnd) {
                app.toggle_panel(hwnd, rect);
            }
            if let Some(app) = app_from(hwnd) {
                app.panel.view = crate::ui::panel::PanelView::AddForm;
                app.panel.pending_platform = crate::api::Platform::Cn;
                if let Some(p) = app.panel.hwnd {
                    relayout_panel(app, p);
                    if let Some(app) = app_from(hwnd) {
                        app.panel.focus_input(p, crate::ui::panel::InputField::Name);
                    }
                }
            }
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        app.tray_state.shutdown();
        drop(app.poller.take());
        if com {
            CoUninitialize();
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_clamps_out_of_range() {
        let mut c = Config::default();
        c.general.notify_threshold_percent = 0;
        c.general.poll_interval_secs = 0;
        normalize_config(&mut c);
        assert_eq!(c.general.notify_threshold_percent, 1);
        assert_eq!(c.general.poll_interval_secs, MIN_POLL_SECS);

        c.general.notify_threshold_percent = 200;
        c.general.poll_interval_secs = u64::MAX;
        normalize_config(&mut c);
        assert_eq!(c.general.notify_threshold_percent, 100);
        assert_eq!(c.general.poll_interval_secs, MAX_POLL_SECS);
    }

    #[test]
    fn normalize_keeps_valid_values() {
        let mut c = Config::default();
        c.general.notify_threshold_percent = 80;
        c.general.poll_interval_secs = 300;
        normalize_config(&mut c);
        assert_eq!(c.general.notify_threshold_percent, 80);
        assert_eq!(c.general.poll_interval_secs, 300);
    }

    /// selected missing or dangling backfills the first account; an empty
    /// account set normalizes to empty
    #[test]
    fn normalize_backfills_selected() {
        let mut c = Config::default();
        normalize_config(&mut c);
        assert_eq!(c.selected, None);
        c.accounts = vec![crate::app::config::Account {
            id: "a1".into(),
            name: "n".into(),
            platform: crate::api::Platform::Cn,
            team: false,
            org_id: String::new(),
            project_id: String::new(),
            api_key: "k".into(),
        }];
        normalize_config(&mut c);
        assert_eq!(c.selected.as_deref(), Some("a1"));
        // A dangling pointer backfills too
        c.selected = Some("gone".into());
        normalize_config(&mut c);
        assert_eq!(c.selected.as_deref(), Some("a1"));
        // A valid selection is left untouched
        c.accounts.push(crate::app::config::Account {
            id: "a2".into(),
            name: "n2".into(),
            platform: crate::api::Platform::Cn,
            team: false,
            org_id: String::new(),
            project_id: String::new(),
            api_key: "k".into(),
        });
        c.selected = Some("a2".into());
        normalize_config(&mut c);
        assert_eq!(c.selected.as_deref(), Some("a2"));
    }

    #[test]
    fn appearance_resolved_from_setting() {
        use crate::ui::panel::theme::{Appearance, resolved};
        assert_eq!(resolved(Some("light")), Appearance::Light);
        // Case-insensitive
        assert_eq!(resolved(Some("DARK")), Appearance::Dark);
        // Unknown values behave like unset: follow the system
        assert_eq!(
            resolved(Some("blue")),
            resolved(None),
            "未知值与未配置同为跟随系统"
        );
    }
}
