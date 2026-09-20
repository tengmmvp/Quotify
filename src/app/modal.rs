//! Modal dialogs, the tray context menu, and config import/export — every
//! nested pump point is funneled into this file

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::SetTimer;
use windows::core::PCWSTR;

use super::config::Config;
use super::hits::{switch_poll_source, sync_customizing};
use super::notify::NOTIFY_TITLE;
use super::{IDM_ABOUT, IDM_EXIT, IDM_SETTINGS, normalize_config, relayout_panel};
use crate::platform::wide;
use crate::ui::i18n::Strings;
/// Size cap for imported config files
const IMPORT_MAX_BYTES: u64 = 1024 * 1024;
/// Count of live modal layers: the first to enter freezes the patrol and
/// the last to exit restores it; an inner Drop must not prematurely
/// restore a clock that an outer layer still keeps frozen.
static MODAL_DEPTH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Current modal depth — the single source for "a modal is present". The
/// patrol arm gates on this directly rather than a mirrored flag: a panel
/// (re)opened inside a modal pump is covered too, and no lockstep write can
/// be missed.
pub fn modal_depth() -> usize {
    MODAL_DEPTH.load(std::sync::atomic::Ordering::Acquire)
}

/// The panel handle the current modal stack disabled: set by whichever
/// layer found a panel at entry, cleared by whichever layer unwinds the
/// stack to zero. An entry snapshot (the old design) could not carry it
/// across layers — an outer None + inner Some nesting would have
/// restored nothing, leaving the panel disabled forever.
static DISABLED_PANEL: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Triple safeguard while a modal is up: patrol freeze (the patrol timer
/// gates on modal_depth, and a hung clock is killed here — the panel being
/// off-pointer behind the dialog box is normal, so a mistaken collapse must
/// be stopped), panel disable (an in-flight edit can no longer be swallowed
/// by a full overwrite), and no missed bookkeeping on early return. The
/// constructor's panel argument is None for no panel (the tray menu when
/// the panel is closed). Freeze and restore act on the live state at that
/// moment rather than a snapshot taken at entry: a panel reopened or
/// collapsed inside the modal pump is covered either way.
struct ModalGuard;

impl ModalGuard {
    fn new(panel: Option<HWND>) -> Self {
        use std::sync::atomic::Ordering;
        if MODAL_DEPTH.fetch_add(1, Ordering::AcqRel) == 0
            && let Some(panel) = panel
        {
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::KillTimer;
                let _ = KillTimer(Some(panel), crate::ui::panel::TIMER_OUTSIDE_CHECK);
            }
        }
        if let Some(panel) = panel {
            DISABLED_PANEL.store(panel.0 as isize, Ordering::Release);
            unsafe {
                use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
                let _ = EnableWindow(panel, false);
            }
        }
        Self
    }
}

impl Drop for ModalGuard {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        unsafe {
            // Restore is gated on depth reaching zero, same as freeze:
            // when an inner layer exits an outer modal is still up, and
            // restoring panel interactivity early would amount to never
            // having disabled it.
            if MODAL_DEPTH.fetch_sub(1, Ordering::AcqRel) == 1 {
                let stored = DISABLED_PANEL.swap(0, Ordering::AcqRel);
                if stored != 0 {
                    let panel = HWND(stored as *mut _);
                    use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
                    let _ = EnableWindow(panel, true);
                    // Patrol restore follows the live state: re-arm only if
                    // still pinned, and always start from the open-door grace
                    // period — regardless of whether it was regular or in
                    // grace before entry, or the panel was reopened midway,
                    // the grace only delays the first tick and can never
                    // trigger a mistaken collapse.
                    if let Some(app) = crate::ui::panel::app_from_tray(panel)
                        && app.panel.mode == crate::ui::panel::PanelMode::Pinned
                    {
                        let _ = SetTimer(
                            Some(panel),
                            crate::ui::panel::TIMER_OUTSIDE_CHECK,
                            crate::ui::panel::OUTSIDE_GRACE_MS,
                            None,
                        );
                    }
                }
            }
        }
    }
}

/// Tray context menu. The labels are taken as &'static without borrowing
/// App: TrackPopupMenu's nested pump dispatches other messages that
/// re-fetch App, so the reference held before the pump must not be used
/// afterwards. The menu is itself a modal pump: patrol is frozen via
/// ModalGuard — the cursor sits on the menu window, the panel being
/// off-pointer is normal, and without the freeze the patrol would
/// mistakenly collapse it while the user browses.
pub(crate) fn show_context_menu(owner: HWND, panel: Option<HWND>, s: &'static Strings, pos: POINT) {
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, MF_STRING, PostMessageW, SetForegroundWindow,
        TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
    };

    let _modal = ModalGuard::new(panel);
    unsafe {
        let settings = wide(s.settings);
        let about = wide(s.about);
        let exit = wide(s.exit);
        let menu = CreatePopupMenu().unwrap_or_default();
        if menu.is_invalid() {
            return;
        }
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            IDM_SETTINGS as usize,
            PCWSTR(settings.as_ptr()),
        );
        let _ = AppendMenuW(menu, MF_STRING, IDM_ABOUT as usize, PCWSTR(about.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING, IDM_EXIT as usize, PCWSTR(exit.as_ptr()));
        // [Required by the Shell_NotifyIcon docs] bring the window to the
        // foreground before showing the menu, otherwise clicking outside
        // the menu does not dismiss it.
        let _ = SetForegroundWindow(owner);
        let _ = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON,
            pos.x,
            pos.y,
            None,
            owner,
            None,
        );
        let _ = DestroyMenu(menu);
        // Let menu mode properly return activation, otherwise the next
        // menu popup may lose focus immediately and fail to stay open.
        let _ = PostMessageW(Some(owner), WM_NULL, Default::default(), Default::default());
    }
}

/// Warning-level OK/Cancel confirmation box; Cancel returns false.
/// Shared by export and import; takes only the owner and the labels
/// without borrowing App — once the nested pump returns, the caller's
/// reference is already invalidated.
fn confirm_box(owner: HWND, title: &str, body: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW};
    let title = wide(title);
    let body = wide(body);
    let r = unsafe {
        MessageBoxW(
            Some(owner),
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OKCANCEL | MB_ICONWARNING,
        )
    };
    r == IDOK
}

/// Direct-call dispatch for modal-class hits: routed into
/// handle_panel_hit these would hit reference invalidation from the
/// nested pump, so the dispatch layer intercepts them via is_modal and
/// forwards here. When adding a modal variant, register it in lockstep
/// with is_modal; accidental arrivals only leave a log line.
pub fn dispatch_modal(hit: crate::ui::panel::render::Hit, panel_hwnd: HWND) {
    match hit {
        crate::ui::panel::render::Hit::ExportConfig => export_config(panel_hwnd),
        crate::ui::panel::render::Hit::ImportConfig => import_config(panel_hwnd),
        _ => crate::platform::log("[Quotify] 非模态命中误入 dispatch_modal"),
    }
}
/// Exports the config as plain-text JSON. Two-phase reference handling:
/// the modal dialogs run nested message pumps, and other messages inside
/// the pump re-fetch App, invalidating this function's pre-pump
/// reference — take only &'static data before the modal, re-fetch the
/// reference after it to persist.
pub fn export_config(panel_hwnd: HWND) {
    let (title, body) = {
        let Some(app) = crate::ui::panel::app_from_tray(panel_hwnd) else {
            return;
        };
        (
            app.strings.export_confirm_title,
            app.strings.export_confirm_body,
        )
    };
    // Risk disclosure precedes the location pick: the warning about
    // sensitive content must come first.
    // On Cancel not even the save dialog opens: the export aborts
    // silently with no toast.
    // The panel is the owner: with the hidden tray window as owner, the
    // modal hierarchy is not frozen.
    let picked = {
        let _modal = ModalGuard::new(Some(panel_hwnd));
        confirm_box(panel_hwnd, title, body)
            .then(|| crate::platform::save_dialog(panel_hwnd, "quotify-config.json"))
            .flatten()
    };
    let Some(path) = picked else {
        return;
    };
    let Some(app) = crate::ui::panel::app_from_tray(panel_hwnd) else {
        return;
    };
    let body = match serde_json::to_string_pretty(&app.config)
        .map_err(|e| e.to_string())
        .and_then(|text| {
            // The exported file also lands on disk with plain-text keys:
            // tighten at creation, same standard as the config file.
            crate::platform::write_private(&path, &text).map_err(|e| e.to_string())
        }) {
        Ok(()) => app.strings.export_done.to_string(),
        Err(e) => {
            crate::platform::log(&format!("[Quotify] 导出失败: {e}"));
            app.strings.export_failed.to_string()
        }
    };
    crate::platform::notify::show(NOTIFY_TITLE, &body);
}

/// Imports a config JSON; two-phase like export_config: the modal phase
/// holds no App reference, and the apply phase re-fetches it.
pub fn import_config(panel_hwnd: HWND) {
    let picked = {
        let _modal = ModalGuard::new(Some(panel_hwnd));
        crate::platform::open_dialog(panel_hwnd)
    };
    let Some(path) = picked else {
        return;
    };
    // External files are untrusted; a bounded read guards memory against
    // oversized files (growth/replacement between the check and the read
    // is also bound by the cap, leaving no check-then-use window). Read
    // by bytes: a string read truncated mid-way through a multi-byte
    // sequence would falsely report a read failure — encoding problems
    // belong to the parse-failure bucket. The three failure classes land
    // in the log separately: over the cap, read failure, broken file;
    // the user-facing toast uniformly says import failed.
    let parsed = {
        use std::io::Read;
        let mut bytes = Vec::new();
        let read = std::fs::File::open(&path)
            .and_then(|f| f.take(IMPORT_MAX_BYTES + 1).read_to_end(&mut bytes));
        match read {
            Ok(n) if n as u64 > IMPORT_MAX_BYTES => {
                crate::platform::log(&format!(
                    "[Quotify] 配置导入放弃：文件超上限（>{IMPORT_MAX_BYTES} 字节）"
                ));
                None
            }
            // UTF-8 decoding and JSON parsing share one log wording: both
            // mean an invalid file, and single-sourcing the message keeps a
            // later re-wording from leaving one closure stale.
            Ok(_) => String::from_utf8(bytes)
                .map_err(|e| e.to_string())
                .and_then(|text| serde_json::from_str::<Config>(&text).map_err(|e| e.to_string()))
                .map_err(|e| {
                    crate::platform::log(&format!("[Quotify] 配置导入解析失败: {e}"));
                })
                .ok(),
            Err(e) => {
                crate::platform::log(&format!("[Quotify] 配置导入读取失败: {e}"));
                None
            }
        }
    };
    let Some(cfg) = parsed else {
        crate::platform::log("[Quotify] 配置导入失败：文件不可读或格式无效");
        let Some(app) = crate::ui::panel::app_from_tray(panel_hwnd) else {
            return;
        };
        crate::platform::notify::show(NOTIFY_TITLE, app.strings.import_failed);
        return;
    };
    // Import is a full overwrite: existing accounts get replaced and a
    // differing proxy gets switched too — confirm if either will happen,
    // since silently switching the proxy would hand usage queries to an
    // unknown proxy.
    // The current-state check and the labels are copied out in one take;
    // re-fetch after the confirmation box's modal pump returns.
    let confirm = {
        let Some(app) = crate::ui::panel::app_from_tray(panel_hwnd) else {
            return;
        };
        let replacing = !app.config.accounts.is_empty();
        let proxy_change = cfg.general.proxy != app.config.general.proxy;
        (replacing || proxy_change).then_some((
            app.strings.import_confirm_title,
            app.strings.import_confirm_body,
        ))
    };
    if let Some((title, body)) = confirm {
        let _modal = ModalGuard::new(Some(panel_hwnd));
        if !confirm_box(panel_hwnd, title, body) {
            return;
        }
    }
    let Some(app) = crate::ui::panel::app_from_tray(panel_hwnd) else {
        return;
    };
    // Keep the previous proxy on record to roll back when the imported
    // address is invalid; displaced accounts' key memory is cleaned by
    // retain_keys from the alive set alone.
    let prev_proxy = app.config.general.proxy.clone();
    app.config = cfg;
    // The imported config replaces both prefilled values wholesale: the
    // edited marks reset and the proxy buffer re-syncs to the imported
    // value — the settings page may be live on screen, and a stale buffer
    // there would let one Apply click silently revert the import on the
    // most security-sensitive field.
    app.panel.input.proxy_edited = false;
    app.panel.input.proxy = app.config.general.proxy.clone().unwrap_or_default();
    app.panel.input.interval_edited = false;
    // The external file's value ranges are untrusted; normalize the same
    // way as the startup load (the selected backfill is included here).
    normalize_config(&mut app.config);
    app.lang = crate::ui::i18n::resolve_lang(app.config.general.language.as_deref());
    app.strings = app.lang.strings();
    // The imported proxy must pass our own validation before being applied
    // (via try_apply_proxy, same rule as the settings page): once a form
    // that ureq accepts but validation rejects is persisted, every later
    // startup silently degrades to a direct connection. The rollback must
    // be stated explicitly — a success toast would make the user believe
    // the imported proxy took effect.
    let proxy_applied = super::hits::try_apply_proxy(app.config.general.proxy.clone());
    if !proxy_applied {
        app.config.general.proxy = prev_proxy.clone();
        // The buffer rolls back with the config: the settings page may be
        // live on screen, and a buffer still holding the rejected address
        // would let one Apply click silently reinstate it.
        app.panel.input.proxy = prev_proxy.unwrap_or_default();
    }
    // Memory for keys unique to the old set dies with those accounts: the
    // contract that re-adding the same key starts from a clean slate also
    // holds on the import path; the shared predicate goes through
    // retain_keys, same source as the removal path.
    {
        let alive: Vec<&str> = app
            .config
            .accounts
            .iter()
            .map(|a| a.api_key.as_str())
            .collect();
        crate::api::client::retain_keys(&alive);
    }
    if let Some(r) = app.panel.renderer.as_mut() {
        r.reset_hits();
    }
    switch_poll_source(app);
    // The shared release routine (not a hand-rolled subset): it also
    // resets the multi-click latch and repaints, keeping the import path
    // in lockstep with the other release paths.
    app.panel.end_edit_state(panel_hwnd);
    // The update check's in-flight gate is not cleared: clearing it would
    // let a second run start concurrently; a late old result landing
    // naturally is harmless.
    app.set_update_status(None);
    // The outcome labels are &'static: copy them before the relayout — its
    // SetWindowPos is a pump, and reading through this alias afterwards is
    // the one deviation this file forbids.
    let (done_msg, skipped_msg) = (
        app.strings.import_done,
        app.strings.import_done_proxy_skipped,
    );
    sync_customizing(app);
    relayout_panel(app, panel_hwnd);
    let msg = if proxy_applied { done_msg } else { skipped_msg };
    crate::platform::notify::show(NOTIFY_TITLE, msg);
}
