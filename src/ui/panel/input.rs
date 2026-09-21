//! Keyboard/text input handling for the panel window: WM_KEYDOWN editing-key
//! dispatch, WM_CHAR character and clipboard edits, and WM_LBUTTONDOWN's
//! text-zone focus/multi-click logic — the wndproc arms keep only thin
//! dispatch calls; bodies live here verbatim.

use super::geom::LogPx;
use super::{
    DRAG_SLOP, InputField, MULTI_CLICK_MS, PanelInput, input_field_of_hit, invalidate_panel,
    manhattan,
};
use crate::app::{app_from_tray, sync_caret_after_geometry};
use crate::ui::panel::text_edit::{self, EditState};
use crate::ui::{x_of, y_of};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SetCapture, VK_DELETE, VK_END, VK_HOME, VK_LEFT, VK_RIGHT, VK_Y, VK_Z,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};

/// Run a mutating edit on the active buffer: clone out the text → operate
/// via EditState → write back. EditState and the buffers both live in
/// PanelInput; borrowing both fields directly is mutually exclusive, and
/// the clone decouples them. The edited marks latch only on a real change:
/// no-op edits (all-filtered paste, over-budget insert, empty-stack undo)
/// must not close the prefill gates while the buffer is untouched.
pub(crate) fn apply_edit(input: &mut PanelInput, f: impl FnOnce(&mut EditState, &mut String)) {
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

/// WM_KEYDOWN arm body: editing-key dispatch on the active input field
pub(crate) unsafe fn handle_keydown(hwnd: HWND, wparam: WPARAM) -> LRESULT {
    unsafe {
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
            use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
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
                invalidate_panel(hwnd);
            }
        }
        LRESULT(0)
    }
}

/// WM_CHAR arm body: character input, clipboard combinations, and confirm
pub(crate) unsafe fn handle_char(hwnd: HWND, wparam: WPARAM) -> LRESULT {
    unsafe {
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
                use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
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
                invalidate_panel(hwnd);
            }
        }
        LRESULT(0)
    }
}

/// WM_LBUTTONDOWN arm body: text-zone caret placement with multi-click
/// selection, or blank-press drag entry
pub(crate) unsafe fn handle_lbuttondown(hwnd: HWND, lparam: LPARAM) -> LRESULT {
    unsafe {
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
            // select a word. Multi-click judged by MULTI_CLICK_MS/DRAG_SLOP: double
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
                        if now.wrapping_sub(t) <= MULTI_CLICK_MS
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
                invalidate_panel(hwnd);
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
                // reconcile_fading_focus may clear_input (IME detach = pump):
                // the placement takes the re-fetched reference.
                if let Some(app) = crate::app::pump(app, hwnd, |app| {
                    crate::app::reconcile_fading_focus(app, hwnd);
                }) {
                    let n = app.config.accounts.len();
                    let logical_h = app.panel.view_height(n);
                    app.panel.place(hwnd, LogPx(logical_h), false);
                }
                // place's SetWindowPos can synchronously dispatch
                // WM_DPICHANGED (cross-monitor), whose arm re-derives
                // App: the drag latch writes go through the
                // re-fetched reference.
                if let Some(app) = app_from_tray(hwnd) {
                    invalidate_panel(hwnd);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(field: InputField) -> PanelInput {
        PanelInput {
            field: Some(field),
            ..PanelInput::default()
        }
    }

    /// Pins the prefill gates: a real edit latches the mark (prefill must
    /// not overwrite the draft), a no-op edit or another field's edit
    /// leaves it open (prefill follows external changes).
    #[test]
    fn prefill_gate_latches_on_real_edit_only() {
        let mut i = input(InputField::Interval);
        apply_edit(&mut i, |e, t| {
            e.insert(t, '9', InputField::Interval.max_bytes())
        });
        assert!(i.interval_edited, "真实编辑关门，prefill 不再覆写");
        assert!(!i.proxy_edited, "两个门闩各自独立");

        // No-op edits: empty-stack undo, backspace on an empty buffer,
        // over-budget insert — the gate stays open.
        let mut i = input(InputField::Interval);
        apply_edit(&mut i, EditState::undo);
        apply_edit(&mut i, |e, t| e.backspace(t, false));
        apply_edit(&mut i, |e, t| e.insert(t, '9', 0));
        assert!(!i.interval_edited, "无变化编辑不关门，prefill 跟随外部变化");

        // Clearing is an edit too: the mark, not buffer emptiness, decides.
        i.interval = "30".into();
        apply_edit(&mut i, |e, t| {
            e.select_all(t);
            e.delete(t, false);
        });
        assert!(
            i.interval.is_empty() && i.interval_edited,
            "清空也是编辑，门闩照闭"
        );

        let mut i = input(InputField::Proxy);
        apply_edit(&mut i, |e, t| {
            e.insert(t, 'x', InputField::Proxy.max_bytes())
        });
        assert!(
            i.proxy_edited && !i.interval_edited,
            "proxy 编辑只关 proxy 门闩"
        );

        let mut i = input(InputField::Name);
        apply_edit(&mut i, |e, t| {
            e.insert(t, 'n', InputField::Name.max_bytes())
        });
        assert!(
            !i.interval_edited && !i.proxy_edited,
            "其他字段编辑不动门闩"
        );
    }
}
