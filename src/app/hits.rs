//! Panel hit dispatch and settings application

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::InvalidateRect;

use super::App;
use super::modal::dispatch_modal;
use super::notify::NOTIFY_TITLE;
use super::{
    AccountData, UPDATE_TASK, apply_appearance, hide_panel, queue_tray_refresh, relayout_panel,
    sync_layout_effective,
};
use crate::service::poller::{MAX_POLL_SECS, MIN_POLL_SECS};
use crate::ui::panel::invalidate_panel;
use crate::ui::panel::layout::INTERVAL_PRESETS;
/// Shared entry into the settings page: switch the view, sync the layout,
/// prefill the proxy field, close the popup, relayout. Both the close and
/// the relayout are pump points (SW_HIDE re-entry; SetWindowPos can dispatch
/// WM_DPICHANGED): the relayout re-fetches its own reference, and nothing
/// uses this one after either.
pub(super) fn enter_settings(app: &mut App, panel_hwnd: HWND) {
    app.panel.view = crate::ui::panel::PanelView::Settings;
    sync_customizing(app);
    // The proxy field is prefilled with the currently effective value; the
    // user may edit or clear it — clearing and applying means direct
    // connection. A user-edited buffer (cleared or typed) is never
    // overwritten: the edited mark, not buffer emptiness, decides. The
    // prefill is clamped to the buffer cap (import can persist longer).
    if !app.panel.input.proxy_edited
        && let Some(p) = app.config.general.proxy.clone()
    {
        app.panel.input.proxy = crate::ui::panel::InputField::Proxy.clamp_prefill(&p);
    }
    // The relayout runs first: at close's SW_HIDE pump the view is already
    // Settings, so geometry must already be settings-shaped — a paint
    // dispatched inside that pump would otherwise draw the settings page
    // clipped to the main-view window for one frame.
    if let Some(app) = crate::app::pump(app, panel_hwnd, |app| {
        relayout_panel(app, panel_hwnd);
    }) {
        // The close is this reference's last use: its SW_HIDE pump ends
        // the arm.
        app.popup.close();
    }
}
/// Panel hit handling
/// The reference is taken as 'static: the ClosePanel arm moves it through
/// hide_panel to consume it across the pump, while every other arm is
/// re-fetched by the caller after the pump. origin is the window the hit
/// originated from — both the panel and the floating windows dispatch
/// here, so it is not guaranteed to be the panel handle.
pub fn handle_panel_hit(
    app: &'static mut App,
    hit: crate::ui::panel::render::Hit,
    origin_hwnd: HWND,
) {
    use crate::ui::panel::render::{Hit, ScopeChoice};
    match hit {
        Hit::Refresh => {
            // A manual retry opens a fresh logging round: a repeated failure
            // with the same message gets logged again.
            // The spin's driving clock is re-armed uniformly inside the
            // helper: the appear finale tears down TIMER_ANIM, and callers
            // need not know about this trap.
            retry_with_feedback(app, origin_hwnd);
        }
        Hit::Settings => enter_settings(app, origin_hwnd),
        Hit::AccountSwitch => {
            if app.popup.is_open() {
                // close's SW_HIDE re-enters the message pump synchronously;
                // this reference must not be used after the pump — the arm
                // ends right here and returns.
                app.popup.close();
            } else if let Some(p) = app.panel.hwnd {
                app.popup.open(app.hwnd(), p, app.config.accounts.len());
            }
        }
        // The hover badge has no click semantics.
        Hit::UsageInfo => {}

        // ── Navigation ──
        Hit::Back => {
            app.panel.key_revealed = false;
            app.panel.customizing.set_pending(false);
            app.panel.clear_input(origin_hwnd);
            // Back from the form view returns to the settings page; back
            // from the settings page is what returns to the main view.
            app.panel.view = if app.panel.view == crate::ui::panel::PanelView::AddForm {
                crate::ui::panel::PanelView::Settings
            } else {
                crate::ui::panel::PanelView::Main
            };
            relayout_panel(app, origin_hwnd);
        }
        Hit::ClosePanel => {
            hide_panel(app, origin_hwnd);
        }

        // ── Settings · polling interval ──
        Hit::IntervalPreset(secs) => {
            // Clicking the same value skips persisting (unchanged value,
            // no disk write); the error state is the exception — clicking
            // the current preset retries with the same feedback as the
            // refresh button, otherwise the red footer would wait for the
            // next scheduled poll.
            let changed = app.config.general.poll_interval_secs != secs;
            if changed {
                app.config.general.poll_interval_secs = secs;
                // Applied: the draft in the buffer is superseded by the new
                // config value, so the prefill gate reopens (same rule as
                // apply_interval's success path).
                app.panel.input.interval_edited = false;
                crate::app::config::save(&app.config);
                app.sync_poll_context();
            }
            app.panel.customizing.set_pending(false);
            app.panel.clear_input(origin_hwnd);
            // The layout sync's SetWindowPos is a pump: the retry goes
            // last and takes the re-fetched reference. A changed value
            // always refetches; an unchanged click retries only in the
            // error state (no dead clicks).
            if let Some(app) = crate::app::pump(app, origin_hwnd, |app| {
                sync_layout_effective(app, origin_hwnd)
            }) {
                if changed {
                    retry_with_feedback(app, origin_hwnd);
                } else {
                    retry_if_error(app, origin_hwnd);
                }
            }
        }
        Hit::CustomizeInterval => {
            app.panel.customizing.set_pending(true);
            if !app.panel.input.interval_edited {
                prefill_interval(app);
            }
            // The layout sync's SetWindowPos is a pump: the focus handoff
            // takes the re-fetched reference.
            if let Some(app) = crate::app::pump(app, origin_hwnd, |app| {
                sync_layout_effective(app, origin_hwnd)
            }) {
                app.panel
                    .focus_input(origin_hwnd, crate::ui::panel::InputField::Interval);
            }
        }
        Hit::ApplyInterval => apply_interval(app, origin_hwnd),
        Hit::InputInterval => {
            app.panel
                .focus_input(origin_hwnd, crate::ui::panel::InputField::Interval);
        }

        // ── Settings · general ──
        Hit::Language(choice) => {
            app.config.general.language = choice.to_config();
            app.lang = crate::ui::i18n::resolve_lang(app.config.general.language.as_deref());
            app.strings = app.lang.strings();
            crate::app::config::save(&app.config);
            invalidate_ui(app);
            // The language changed, so the tooltip must be rebuilt.
            queue_tray_refresh(app.hwnd());
        }
        Hit::Appearance(choice) => {
            app.config.general.appearance = choice.to_config();
            crate::app::config::save(&app.config);
            if apply_appearance(app) {
                invalidate_ui(app);
            }
        }
        Hit::ToggleAutostart => {
            let next = !crate::platform::autostart::is_enabled();
            match crate::platform::autostart::set_enabled(next) {
                Ok(()) => app.autostart_enabled = next,
                Err(e) => crate::platform::log(&format!("开机自启设置失败: {e}")),
            }
        }

        // ── Settings · network proxy ──
        Hit::InputProxy => {
            app.panel
                .focus_input(origin_hwnd, crate::ui::panel::InputField::Proxy);
        }
        Hit::ApplyProxy => apply_proxy(app, origin_hwnd),

        // ── Settings · usage notifications ──
        Hit::ToggleThreshold => {
            app.config.general.notify_threshold_enabled =
                !app.config.general.notify_threshold_enabled;
            crate::app::config::save(&app.config);
        }
        Hit::ToggleReset5h => {
            app.config.general.notify_reset_5h_enabled =
                !app.config.general.notify_reset_5h_enabled;
            crate::app::config::save(&app.config);
        }
        Hit::ToggleResetWeekly => {
            app.config.general.notify_reset_weekly_enabled =
                !app.config.general.notify_reset_weekly_enabled;
            crate::app::config::save(&app.config);
        }

        // ── Settings · peak window ──
        Hit::InputPeakStart => {
            app.panel
                .focus_input(origin_hwnd, crate::ui::panel::InputField::PeakStart);
        }
        Hit::InputPeakEnd => {
            app.panel
                .focus_input(origin_hwnd, crate::ui::panel::InputField::PeakEnd);
        }
        Hit::ApplyPeak => apply_peak(app, origin_hwnd),

        // ── Settings · accounts ──
        Hit::AddAccount => {
            app.panel.view = crate::ui::panel::PanelView::AddForm;
            app.panel.pending_platform = crate::api::Platform::Cn;
            app.panel.team.set_pending(false);
            app.panel.key_revealed = false;
            for f in [
                crate::ui::panel::InputField::Name,
                crate::ui::panel::InputField::Key,
                crate::ui::panel::InputField::Org,
                crate::ui::panel::InputField::Project,
            ] {
                app.panel.input.clear_field(f);
            }
            // relayout's SetWindowPos can dispatch WM_DPICHANGED, which
            // re-fetches the App alias: the focus handoff takes the
            // re-fetched reference.
            if let Some(app) = crate::app::pump(app, origin_hwnd, |app| {
                relayout_panel(app, origin_hwnd);
            }) {
                app.panel
                    .focus_input(origin_hwnd, crate::ui::panel::InputField::Name);
            }
        }
        Hit::RemoveAccount(i) => {
            if i < app.config.accounts.len() {
                // The source-switch decision compares target-id snapshots
                // before and after: the semantics of selected falling back
                // to the first account when missing is defined by
                // selected_account, so deleting the first account matches
                // too.
                let before = poll_target_id(app);
                app.config.accounts.remove(i);
                // The credential-form memory dies with the account, leaving
                // no ownerless fingerprint behind; the shared-key exception
                // goes through retain_keys, same source as the import path.
                let alive: Vec<&str> = app
                    .config
                    .accounts
                    .iter()
                    .map(|a| a.api_key.as_str())
                    .collect();
                crate::api::client::retain_keys(&alive);
                // Backfill the dangling selected (same single source as
                // normalize).
                app.config.backfill_selected();
                commit_account_change(app, before);
                relayout_panel(app, origin_hwnd);
            }
        }
        Hit::PickAccount(i) => {
            if i < app.config.accounts.len() {
                // select_account's error path runs start_anim, a pump; the
                // close's SW_HIDE is the next one; the relayout takes the
                // final re-fetched reference. After picking, the cursor
                // still sits at the popup's old spot (outside the panel),
                // so the off-pointer timer restarts.
                if let Some(app) = crate::app::pump(app, origin_hwnd, |app| {
                    select_account(app, i, origin_hwnd);
                }) && let Some(app) = crate::app::pump(app, origin_hwnd, |app| {
                    app.panel.view = crate::ui::panel::PanelView::Main;
                    app.panel.outside_since = None;
                    app.popup.close();
                }) {
                    relayout_panel(app, origin_hwnd);
                }
            }
        }
        Hit::AccountType(scope) => {
            app.panel
                .team
                .set_pending(matches!(scope, ScopeChoice::Team));
            if app.panel.team.pending {
                // Team edition exists on the CN site only: switching the
                // type to team also snaps the platform back to CN.
                app.panel.pending_platform = crate::api::Platform::Cn;
            }
            reconcile_fading_focus(app, origin_hwnd);
            sync_layout_effective(app, origin_hwnd);
        }
        Hit::InputName => {
            app.panel
                .focus_input(origin_hwnd, crate::ui::panel::InputField::Name);
        }
        Hit::InputKey => {
            app.panel
                .focus_input(origin_hwnd, crate::ui::panel::InputField::Key);
        }
        // Toggles key masking; the click does not steal input focus, so
        // the edit state is preserved. Toggling changes the displayed
        // string's width, so the caret and the IME composition window must
        // be repositioned for the new width.
        Hit::RevealKey => {
            app.panel.key_revealed = !app.panel.key_revealed;
            if let Some(p) = app.panel.hwnd {
                app.panel.update_caret(p, app.panel.renderer.as_ref());
                invalidate_panel(p);
            }
        }
        Hit::InputOrg => {
            app.panel
                .focus_input(origin_hwnd, crate::ui::panel::InputField::Org);
        }
        Hit::InputProject => {
            app.panel
                .focus_input(origin_hwnd, crate::ui::panel::InputField::Project);
        }
        Hit::SaveAccount => save_pending_account(app, origin_hwnd),
        Hit::Platform(platform) => {
            app.panel.pending_platform = platform;
            if platform == crate::api::Platform::Intl {
                // Team edition exists on the CN site only: switching to the
                // international site also snaps the type back to personal.
                app.panel.team.set_pending(false);
            }
            reconcile_fading_focus(app, origin_hwnd);
            sync_layout_effective(app, origin_hwnd);
        }

        // ── Settings · config management and about ──
        // The two modal arms are dispatched at the window-procedure layer
        // to guard against post-pump reference invalidation; reaching this
        // function means a wiring bug.
        Hit::ExportConfig | Hit::ImportConfig => {
            crate::platform::log("模态命中误入 handle_panel_hit");
        }
        Hit::CheckUpdate => {
            if !app.update_checking {
                // Keep any existing result on record: roll back if the
                // thread fails to spawn, so version info already in hand
                // is not wiped out by one failure.
                let prev_status = app.update_status.take();
                app.update_checking = true;
                if !UPDATE_TASK.spawn(app.hwnd(), crate::service::update::check_latest) {
                    app.update_checking = false;
                    app.set_update_status(prev_status);
                }
            }
        }
        Hit::OpenDownload => {
            if let Some(Ok(info)) = app.update_status.as_ref() {
                crate::platform::open_url(&info.url);
            }
        }

        // ── About window ──
        Hit::LinkRepo => {
            crate::platform::open_url(crate::ui::panel::render::about::REPO_URL);
        }
        Hit::LinkIssues => {
            crate::platform::open_url(crate::ui::panel::render::about::ISSUES_URL);
        }
        Hit::CopyDiagnostics => {
            // Both the clipboard write and the toast are pump points: take
            // the labels as &'static and build the text up front, then never
            // touch this reference after the pump. Both outcomes speak: a
            // silent click looks dead under clipboard contention.
            let (text, done_msg, fail_msg) = (
                diagnostics_text(app),
                app.strings.notify_diag_copied,
                app.strings.notify_copy_failed,
            );
            let msg = if crate::platform::clipboard::write_text(origin_hwnd, &text) {
                done_msg
            } else {
                fail_msg
            };
            crate::platform::notify::show(NOTIFY_TITLE, msg);
        }
        Hit::NewsItem(i) => {
            // Expand/collapse is mutually exclusive; expanding marks the
            // item read, advancing the read marker to that item's date and
            // persisting it.
            let Some(item) = app.news.as_ref().and_then(|n| n.get(i)) else {
                return;
            };
            let need_read = crate::service::whatsnew::is_unread(
                item.date.as_str(),
                app.config.general.last_news_read.as_deref(),
            );
            if need_read {
                app.config.general.last_news_read = Some(item.date.clone());
                crate::app::config::save(&app.config);
            }
            app.about.news_expanded = if app.about.news_expanded == Some(i) {
                None
            } else {
                Some(i)
            };
            refit_about(app);
        }
        Hit::AboutLogo => {
            // One-way easter egg: once eaten, further clicks do nothing;
            // reset when the About window closes.
            let about = &mut app.about;
            if about.egg.is_none() && !about.egg_eaten {
                about.egg_clicks = about.egg_clicks.saturating_add(1);
                if about.egg_clicks >= 5 {
                    about.egg_clicks = 0;
                    let anim = about
                        .wnd
                        .renderer
                        .as_ref()
                        .is_some_and(|r| r.animations_on());
                    if anim {
                        about.egg = Some(crate::ui::panel::anim::Tween::now(1500));
                        // The animation clock may already have been torn
                        // down by the appear finale; re-arm it for the
                        // easter egg's duration through the float module's
                        // single owner (id + period).
                        crate::ui::float_wnd::start_float_anim(origin_hwnd);
                    } else {
                        about.egg_eaten = true;
                    }
                }
            }
        }
    }
    invalidate_panel(origin_hwnd);
}
/// Strips the user-credentials segment from a proxy string so the
/// diagnostics output carries no userinfo
fn redact_proxy(p: &str) -> String {
    let Some(scheme_end) = p.find("://") else {
        return p.to_string();
    };
    let rest = &p[scheme_end + 3..];
    // The split shares proxy_split_userinfo with proxy_valid: a divergence
    // in the two parsers' edge semantics would leak the password tail into
    // the diagnostics text.
    let (userinfo, host) = crate::api::client::proxy_split_userinfo(rest);
    match userinfo {
        // The head already contains the scheme's trailing "://"; just
        // concatenate.
        Some(_) => format!("{}{}", &p[..scheme_end + 3], host),
        None => p.to_string(),
    }
}

/// Assembles the About window's "copy diagnostics" text
/// One line each: version, OS, config path, proxy, last error, then a
/// recent-logs tail.
fn diagnostics_text(app: &App) -> String {
    let os = windows_registry::LOCAL_MACHINE
        .open("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
        .map(|k| {
            let display = k.get_string("DisplayVersion").unwrap_or_default();
            let build = k.get_string("CurrentBuildNumber").unwrap_or_default();
            if build.is_empty() {
                "unknown".to_string()
            } else if display.is_empty() {
                build
            } else {
                format!("{display} (build {build})")
            }
        })
        .unwrap_or_else(|_| "unknown".into());
    let mut text = [
        format!("Quotify v{}", env!("CARGO_PKG_VERSION")),
        format!("OS: Windows {os}"),
        // Mask the profile name: the account name is the one identifying
        // part of the config path, and the diagnostics text gets pasted
        // wholesale into public issues.
        format!(
            "Config: {}",
            crate::platform::redact_user_segment(
                &crate::app::config::config_path().display().to_string()
            )
        ),
        match &app.config.general.proxy {
            // Strip the userinfo: proxy strings often carry credentials
            // reused from passwords, and the diagnostics text gets pasted
            // wholesale into public issues — keep only scheme://host:port.
            Some(p) => format!("Proxy: {}", redact_proxy(p)),
            None => "Proxy: direct".into(),
        },
        match &app.data.last_error {
            Some(e) => format!("Last error: {e}"),
            None => "Last error: none".into(),
        },
    ]
    .join("\n");
    // Recent-logs tail: the section appears only when the ring holds
    // lines — a bare heading conveys nothing. The heading is the only
    // localized part, matching the copy-diagnostics UI language.
    let logs = crate::platform::recent_logs();
    if !logs.is_empty() {
        text.push('\n');
        text.push_str(app.strings.diag_recent_logs);
        for line in logs {
            text.push('\n');
            text.push_str(&line);
        }
    }
    text
}

/// After an account switch the old snapshot's reset baselines are void;
/// reset the notification state so the new account's first snapshot does
/// not false-alarm
fn reset_notify_state(app: &mut App) {
    app.threshold_armed_5h = true;
    app.last_reset_5h = None;
    app.threshold_armed_weekly = true;
    app.last_reset_weekly = None;
}

/// The polling target's account id: every add/remove/edit decision about
/// "did the target change" compares this single-source snapshot before
/// and after, instead of hand-set flags per arm
fn poll_target_id(app: &App) -> Option<String> {
    app.config.selected_account().map(|(_, a)| a.id.clone())
}

/// Finalization after an account-set mutation: reset hits, then either
/// switch the poll source or just persist, depending on whether the
/// target changed. Shared by the removal and add paths; import is a
/// whole-set replacement semantic that switches unconditionally and does
/// not go through here.
fn commit_account_change(app: &mut App, before: Option<String>) {
    if let Some(r) = app.panel.renderer.as_mut() {
        r.reset_hits();
    }
    if poll_target_id(app) != before {
        switch_poll_source(app);
    } else {
        crate::app::config::save(&app.config);
    }
}

/// A visible retry: the same feedback as the refresh button (the button
/// spins). If a re-click/re-pick in the error state produced no visual
/// change, a repeated failure would leave the panel pixel-identical to
/// before the click — the click would seem dead.
fn retry_with_feedback(app: &mut App, hwnd: HWND) {
    app.refresh_now();
    if let Some(r) = app.panel.renderer.as_mut() {
        r.start_spin();
        unsafe { crate::ui::panel::start_anim(hwnd) };
    }
}

/// Retry gate for re-click/re-pick arms that change nothing in the
/// healthy state: the error state is the exception and retries with
/// feedback — any new trigger point where an unchanged click can land on
/// an error panel goes through here, keeping the no-dead-click rule in
/// one place.
fn retry_if_error(app: &mut App, hwnd: HWND) {
    if app.data.last_error.is_some() {
        retry_with_feedback(app, hwnd);
    }
}

/// The source-switch sequence shared by account switch, add/remove, and
/// import: persist, clear the snapshot and notification baselines,
/// rearrange polling and fetch immediately (via refresh_now), refresh the
/// tray icon. The renderer hit reset and the panel relayout are needed by
/// only some paths, so they stay at each call site.
pub(super) fn switch_poll_source(app: &mut App) {
    crate::app::config::save(&app.config);
    app.data = AccountData {
        snapshot: None,
        last_error: None,
    };
    reset_notify_state(app);
    app.sync_poll_context();
    app.poll_gen
        .fetch_add(1, std::sync::atomic::Ordering::Release);
    queue_tray_refresh(app.hwnd());
    app.refresh_now();
}
pub(crate) fn select_account(app: &mut App, i: usize, panel_hwnd: HWND) {
    if let Some(id) = app.config.accounts.get(i).map(|a| a.id.clone()) {
        // Clicking the already-selected account is the natural gesture to
        // close the popup: no source switch, no re-fetch; the error state
        // is the exception — re-picking retries with the same feedback as
        // the refresh button.
        if app.config.selected.as_deref() == Some(id.as_str()) {
            retry_if_error(app, panel_hwnd);
            return;
        }
        app.config.selected = Some(id);
        switch_poll_source(app);
    }
}

/// Clears the edit state when focus lands on an input row that has
/// collapsed away, preventing an orphan caret and stray writes; called
/// from both the immediate switch and the transition catch-up
pub(crate) fn reconcile_fading_focus(app: &mut App, panel_hwnd: HWND) {
    use crate::ui::panel::InputField;
    let fading_team = !app.panel.team.pending
        && matches!(
            app.panel.input.field,
            Some(InputField::Org) | Some(InputField::Project)
        );
    let fading_custom =
        !app.panel.customizing.pending && app.panel.input.field == Some(InputField::Interval);
    if fading_team || fading_custom {
        app.panel.clear_input(panel_hwnd);
    }
}

/// Saves the add-account form
fn save_pending_account(app: &mut App, panel_hwnd: HWND) {
    // The add path shares the import cap (MAX_ACCOUNTS): without this gate
    // accounts could accumulate past the limit in the UI, and the app's own
    // export would then be rejected at re-import with a misleading
    // "invalid file" toast.
    if app.config.accounts.len() >= crate::app::config::MAX_ACCOUNTS {
        crate::platform::log(&format!(
            "添加账号放弃：已达上限（{}/{}）",
            app.config.accounts.len(),
            crate::app::config::MAX_ACCOUNTS
        ));
        // The toast is a pump point: the text is built up front and this
        // arm ends right after it — the reference is dead past the pump.
        crate::platform::notify::show(NOTIFY_TITLE, app.strings.notify_account_limit);
        return;
    }
    let name = app.panel.input.name.trim().to_string();
    let key = app.panel.input.key.trim().to_string();
    if key.is_empty() {
        return;
    }
    if app.panel.team.pending {
        let missing = if app.panel.input.org.trim().is_empty() {
            Some(crate::ui::panel::InputField::Org)
        } else if app.panel.input.project.trim().is_empty() {
            Some(crate::ui::panel::InputField::Project)
        } else {
            None
        };
        if let Some(field) = missing {
            let tip = app.strings.notify_team_required;
            // The toast is a pump point: the focus handoff takes the
            // re-fetched reference.
            if let Some(app) = crate::app::pump(app, panel_hwnd, |_| {
                crate::platform::notify::show(NOTIFY_TITLE, tip);
            }) {
                app.panel.focus_input(panel_hwnd, field);
            }
            return;
        }
    }
    let name = if name.is_empty() {
        app.strings.default_account_name.to_string()
    } else {
        name
    };

    let acc = crate::app::config::Account {
        id: app.config.new_account_id(),
        name,
        platform: app.panel.pending_platform,
        team: app.panel.team.pending,
        // Org/project persist for the team edition only: form residue from
        // switching back to personal must not enter the account, and a
        // personal account carrying team fields would send polling down
        // the team query path by mistake.
        org_id: if app.panel.team.pending {
            app.panel.input.org.trim().to_string()
        } else {
            String::new()
        },
        project_id: if app.panel.team.pending {
            app.panel.input.project.trim().to_string()
        } else {
            String::new()
        },
        api_key: key,
    };
    let before = poll_target_id(app);
    app.config.accounts.push(acc.clone());
    // Selection backfill goes through the shared single source: a first
    // account selects itself, an already-valid selection stays (adding a
    // non-selected account does not switch the source, so no
    // loading-state flash); the config persists as usual.
    app.config.backfill_selected();
    commit_account_change(app, before);
    app.panel.view = crate::ui::panel::PanelView::Settings;
    app.panel.key_revealed = false;
    // The key buffer is cleared on save: plain text must not linger in
    // the input buffer until the form is next opened.
    app.panel
        .input
        .clear_field(crate::ui::panel::InputField::Key);
    app.panel.clear_input(panel_hwnd);
    relayout_panel(app, panel_hwnd);
}

/// Custom interval text (minutes) → seconds: 0/non-numeric/overflow are
/// rejected; out-of-range values are clamped into the legal range
fn parse_interval_secs(s: &str) -> Option<u64> {
    s.trim()
        .parse::<u64>()
        .ok()
        .filter(|m| *m > 0)
        .and_then(|m| m.checked_mul(60))
        .map(|s| s.clamp(MIN_POLL_SECS, MAX_POLL_SECS))
}

/// Applies the custom polling interval (minutes)
fn apply_interval(app: &mut App, panel_hwnd: HWND) {
    let Some(secs) = parse_interval_secs(&app.panel.input.interval) else {
        // Both the log and the toast carry the rejected value; a silent
        // rejection is a dead click on screen.
        let raw = app.panel.input.interval.trim().to_string();
        crate::platform::log(&format!("自定义间隔无效: {raw}"));
        // The toast is a pump point: the text is built up front and this
        // arm ends right after it — the reference is dead past the pump.
        crate::platform::notify::show(
            NOTIFY_TITLE,
            &app.strings.notify_interval_invalid.replace("{v}", &raw),
        );
        return;
    };
    app.config.general.poll_interval_secs = secs;
    // Applied: the buffer is back in sync with the config, so the prefill
    // gate reopens for later config-side changes (import).
    app.panel.input.interval_edited = false;
    crate::app::config::save(&app.config);
    app.sync_poll_context();
    app.refresh_now();
    sync_customizing(app);
    app.panel.clear_input(panel_hwnd);
    sync_layout_effective(app, panel_hwnd);
}

/// Applies the peak window: boxes left unedited keep the current config
/// values; both values must be valid and different to be written
fn apply_peak(app: &mut App, panel_hwnd: HWND) {
    let start = if app.panel.input.peak_start.trim().is_empty() {
        app.config.general.peak_start.clone()
    } else {
        app.panel.input.peak_start.trim().to_string()
    };
    let end = if app.panel.input.peak_end.trim().is_empty() {
        app.config.general.peak_end.clone()
    } else {
        app.panel.input.peak_end.trim().to_string()
    };
    if let (Some(s), Some(e)) = (
        crate::ui::peak::parse_hhmm(&start),
        crate::ui::peak::parse_hhmm(&end),
    ) && s != e
    {
        app.config.general.peak_start = start;
        app.config.general.peak_end = end;
        crate::app::config::save(&app.config);
        app.panel.clear_input(panel_hwnd);
    } else {
        // The log carries the rejected pair, same style as the interval
        // and proxy rejection logs.
        crate::platform::log(&format!(
            "高峰区间格式无效，应为 HH:MM 且两端不相等: {start} ~ {end}"
        ));
    }
    relayout_panel(app, panel_hwnd);
}
/// Invalidates and repaints the panel together with any open popup and
/// the About window; shared by the appearance and language switch paths
pub(super) fn invalidate_ui(app: &App) {
    let hwnds = [
        app.panel.hwnd,
        app.popup.wnd.hwnd.filter(|_| app.popup.is_open()),
        app.about.wnd.hwnd.filter(|_| app.about.is_open()),
    ];
    for h in hwnds.into_iter().flatten() {
        invalidate_panel(h);
    }
}

/// Rearranges the About window's height for the current news and expand
/// state and repaints; does nothing while closed. The centered rect
/// shares the single source AboutWindow::centered_rect with the open
/// path.
pub(crate) fn refit_about(app: &mut App) {
    let Some(a) = app.about.wnd.hwnd.filter(|_| app.about.is_open()) else {
        return;
    };
    let h =
        crate::ui::panel::render::about::about_height(app.news.as_deref(), app.about.news_expanded);
    // The reposition centers within the window's own monitor (centered_rect
    // clamps to it), so no WM_DPICHANGED re-derivation is expected; the
    // alias tripwire pins that assumption in debug builds.
    let snap = crate::app::alias_probe(app);
    unsafe {
        let monitor = windows::Win32::Graphics::Gdi::MonitorFromWindow(
            a,
            windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
        );
        let (x, y, w, hgt) = app.about.centered_rect(monitor, h);
        let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
            a,
            None,
            x,
            y,
            w,
            hgt.0,
            windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOCOPYBITS,
        );
        let _ = InvalidateRect(Some(a), None, true);
    }
    crate::app::alias_assert_live(snap, app);
}
/// Validates and applies the proxy; returns false on any failure (the
/// rejection reason is already logged). Shared by the settings-page apply
/// and import paths — divergent acceptance rules would let import persist
/// an address the Apply button would reject.
pub(super) fn try_apply_proxy(next: Option<String>) -> bool {
    if let Some(p) = next.as_deref()
        && let Err(e) = crate::api::client::proxy_valid(p)
    {
        crate::platform::log(&format!("代理地址无效，未应用: {e}"));
        return false;
    }
    if let Err(e) = crate::api::client::set_proxy(next) {
        crate::platform::log(&format!("代理设置失败，未应用: {e}"));
        return false;
    }
    true
}

/// Applies the network proxy; rebuilds the connection immediately and
/// triggers one fetch to verify
fn apply_proxy(app: &mut App, panel_hwnd: HWND) {
    let raw = app.panel.input.proxy.trim().to_string();
    let next = if raw.is_empty() { None } else { Some(raw) };
    // Validate first (via try_apply_proxy, same rule): an invalid address
    // must not be persisted — once a bad value lands on disk, every later
    // startup silently degrades to a direct connection; on failure the
    // input field keeps its state for editing.
    if !try_apply_proxy(next.clone()) {
        crate::platform::notify::show(NOTIFY_TITLE, app.strings.notify_proxy_invalid);
        return;
    }
    app.config.general.proxy = next;
    // Applied: the buffer is back in sync with the config, so the prefill
    // gate reopens for later config-side changes (import).
    app.panel.input.proxy_edited = false;
    crate::app::config::save(&app.config);
    app.panel.clear_input(panel_hwnd);
    // The fetch after a proxy change is a fresh attempt round: a repeated
    // failure with the same message gets logged again.
    app.refresh_now();
    relayout_panel(app, panel_hwnd);
}
/// Hit dispatch routing: modal-class hits are intercepted for the direct
/// two-phase call and return true; the rest re-fetch the reference,
/// dispatch, and return false. Both the panel's and the floating
/// windows' dispatch layers share this source, so the modal-intercept
/// synchronization has exactly one consumer left.
pub(crate) fn dispatch_hit(hit: crate::ui::panel::render::Hit, owner: HWND) -> bool {
    if hit.is_modal() {
        dispatch_modal(hit, owner);
        return true;
    }
    if let Some(app) = crate::app::app_from_tray(owner) {
        handle_panel_hit(app, hit, owner);
    }
    false
}
/// Non-preset intervals expand the custom row and prefill it; presets
/// collapse it
pub(super) fn sync_customizing(app: &mut App) {
    let cur = app.config.general.poll_interval_secs;
    let is_preset = INTERVAL_PRESETS.contains(&cur);
    app.panel.customizing.set_pending(!is_preset);
    // A user-edited buffer keeps its unapplied edit; the edited mark, not
    // buffer emptiness, decides (a cleared buffer is an edit too).
    if !is_preset && !app.panel.input.interval_edited {
        prefill_interval(app);
    }
}

/// Prefills the custom interval (minutes), rounded up
fn prefill_interval(app: &mut App) {
    let cur = app.config.general.poll_interval_secs;
    let mins = cur.div_ceil(60);
    app.panel.input.interval = mins.to_string();
}

/// Enter-confirm behavior inside the panel: name → move to key; key →
/// save; interval → apply
pub(crate) fn confirm_panel_input(app: &mut App, panel_hwnd: HWND) {
    use crate::ui::panel::InputField;
    match app.panel.input.field {
        Some(InputField::Name) => {
            if !app.panel.input.name.trim().is_empty() {
                app.panel.focus_input(panel_hwnd, InputField::Key);
            }
        }
        Some(InputField::Key) => {
            // The key is required; the name may be left empty.
            if !app.panel.input.key.trim().is_empty() {
                if app.panel.team.pending {
                    app.panel.focus_input(panel_hwnd, InputField::Org);
                } else {
                    save_pending_account(app, panel_hwnd);
                }
            }
        }
        Some(InputField::Org) => {
            if !app.panel.input.org.trim().is_empty() {
                app.panel.focus_input(panel_hwnd, InputField::Project);
            }
        }
        Some(InputField::Project) => {
            if !app.panel.input.key.trim().is_empty() {
                save_pending_account(app, panel_hwnd);
            }
        }
        Some(InputField::Interval) => apply_interval(app, panel_hwnd),
        Some(InputField::Proxy) => apply_proxy(app, panel_hwnd),
        Some(InputField::PeakStart) => {
            app.panel.focus_input(panel_hwnd, InputField::PeakEnd);
        }
        Some(InputField::PeakEnd) => apply_peak(app, panel_hwnd),
        None => {}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// With @ in the password, split at the last @: the password tail must
    /// not leak into the diagnostics text; no-credentials and standard
    /// forms pass through unchanged; a stray @ is stripped too
    #[test]
    fn redact_proxy_at_in_password() {
        assert_eq!(
            redact_proxy("http://user:p@ss@host:1080"),
            "http://host:1080"
        );
        assert_eq!(redact_proxy("http://host:1080"), "http://host:1080");
        assert_eq!(redact_proxy("socks5://u:p@h:1"), "socks5://h:1");
        assert_eq!(redact_proxy("http://@proxy:1080"), "http://proxy:1080");
    }

    #[test]
    fn interval_minutes_parse_to_secs() {
        assert_eq!(parse_interval_secs("5"), Some(300));
        // Trimmed before parsing
        assert_eq!(parse_interval_secs("  10 "), Some(600));
    }

    #[test]
    fn interval_rejects_zero_and_non_numeric() {
        assert_eq!(parse_interval_secs("0"), None);
        assert_eq!(parse_interval_secs(""), None);
        assert_eq!(parse_interval_secs("abc"), None);
        // The interval granularity is whole minutes; decimals are rejected
        assert_eq!(parse_interval_secs("1.5"), None);
    }

    #[test]
    fn interval_overflow_rejected() {
        assert_eq!(parse_interval_secs(&u64::MAX.to_string()), None);
    }

    #[test]
    fn interval_above_cap_clamped() {
        // 1441 minutes = 86460 seconds, over the one-day cap
        assert_eq!(parse_interval_secs("1441"), Some(MAX_POLL_SECS));
    }
}
