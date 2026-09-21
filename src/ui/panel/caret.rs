//! Input caret and IME: visible window layout, blink phase, composition window positioning
#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Foundation::HWND;

use super::InputField;
use super::Panel;
use super::layout;
use super::render::Renderer;

/// Char list + prefix width table of a display string: the expensive
/// artifacts of one input pass, shared by caret positioning, hit
/// conversion, the focused render slice, and the unfocused tail slice —
/// no site rebuilds its own TextLayout
pub(crate) struct WidthArtifacts {
    pub(crate) chars: Vec<char>,
    /// cumulative width of the first i chars; table length = char count + 1
    pub(crate) widths: Vec<f32>,
}

/// Layout product for a single input pass: visible window + caret x over
/// the shared width artifacts. Within the same mouse event, caret
/// positioning, hit conversion, and rendering slices share one copy.
pub(crate) struct CaretLayout {
    /// char index of the visible window start
    pub(crate) vis_start: usize,
    /// caret x within the window
    pub(crate) cx: f32,
    pub(crate) widths: std::rc::Rc<WidthArtifacts>,
}

/// Width-artifact cache entry: key is the field's display string — the
/// table depends on the string alone, so caret moves and drag-selection
/// reuse it instead of rebuilding; the window and caret x are recomputed
/// per call as pure arithmetic.
pub(super) type CaretCacheEntry = (String, std::rc::Rc<WidthArtifacts>);

/// Slot count of the per-field width cache: the render side queries every
/// visible field each frame, so one shared slot would thrash between
/// fields and rebuild a full DWrite layout per field per frame. Kept in
/// lockstep with InputField::slot by the pinned test in fields.rs.
pub(super) const CARET_SLOTS: usize = 8;

/// Blink half-period: on and off each span this; the phase decision and
/// the focus-time clock arm share it
const BLINK_HALF_MS: u32 = 500;

/// Text-zone hit edge tolerance: the outermost strip inside the box edge is
/// box chrome, not caret territory (the right side's strip is field_geo's tail)
const FIELD_EDGE_TOL: f32 = 4.0;

impl CaretLayout {
    /// Width of chars[a..b]; indices are clamped into the table, out-of-range never panics
    pub(crate) fn seg(&self, a: usize, b: usize) -> f32 {
        let n = self.widths.widths.len() - 1;
        self.widths.widths[b.min(n)] - self.widths.widths[a.min(n)]
    }
}

/// A field's content for the width table and the draw side: plain fields
/// carry the buffer, the masked Key carries its parts so no frame
/// materializes the dot string just to compare against a cache key.
#[derive(Clone, Copy)]
pub(crate) enum FieldContent<'a> {
    Plain(&'a str),
    Masked { key: &'a str, active: bool },
}

impl<'a> FieldContent<'a> {
    /// The active field's content: the masked Key when unrevealed, the
    /// buffer otherwise — one rule with the caret width table.
    fn of_active(panel: &'a Panel) -> Self {
        if panel.input.field == Some(InputField::Key) && !panel.key_revealed {
            FieldContent::Masked {
                key: &panel.input.key,
                active: true,
            }
        } else {
            FieldContent::Plain(panel.input.active_str())
        }
    }

    /// Emptiness decides the placeholder branch; the masked form's char
    /// count equals the key's, so both arms agree with mask_key.
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            FieldContent::Plain(s) => s.is_empty(),
            FieldContent::Masked { key, .. } => key.is_empty(),
        }
    }

    /// Structural equality against a cached display string without
    /// materializing: the masked forms are fully determined by the key.
    pub(crate) fn matches(&self, cached: &str) -> bool {
        match self {
            FieldContent::Plain(s) => cached == *s,
            FieldContent::Masked { key, active } => masked_key_matches(cached, key, *active),
        }
    }

    /// Materialize the display string — the cache-miss path only.
    pub(crate) fn display(&self) -> String {
        match self {
            FieldContent::Plain(s) => (*s).to_string(),
            FieldContent::Masked { key, active } => super::mask_key(key, *active),
        }
    }
}

/// Structural equality against a masked Key display without materializing
/// it: n dots when active (or under 12 chars), head…tail otherwise — the
/// same shapes mask_key builds, read from the same constants so a
/// masking-rule change cannot leave this predicate recognizing a stale
/// shape.
pub(crate) fn masked_key_matches(cached: &str, key: &str, active: bool) -> bool {
    let n = key.chars().count();
    if active || n < 12 {
        cached.chars().count() == n && cached.chars().all(|ch| ch == super::MASK_GLYPH)
    } else {
        let mut cached = cached.chars();
        cached.clone().take(4).eq(key.chars().take(4))
            && cached.nth(4) == Some('…')
            && cached.eq(key.chars().skip(n - 4))
    }
}

/// Focus-time blink-clock arm: from an idle clock, one arm at the flip
/// point — the interaction zeroing puts the flip a full half-period out;
/// a running clock (owned by an animation) or no caret means no tuning.
fn blink_arm_period(clock_running: bool, caret_present: bool) -> Option<u32> {
    (!clock_running && caret_present).then_some(BLINK_HALF_MS)
}

impl Panel {
    /// Focus an input field: enter the edit state, the IME composition window
    /// follows the caret. Switching fields resets the edit state — an undo
    /// stack carried across fields would pollute the new buffer; re-focusing
    /// the same field keeps the caret position, so click positioning is not
    /// bounced back to the end.
    pub fn focus_input(&mut self, hwnd: HWND, field: InputField) {
        let switched = self.input.field != Some(field);
        if switched {
            // Commit the in-flight composition into the OLD field before
            // the switch: standard edit controls commit on focus change,
            // while destroying the old IME context below would silently
            // drop it. The commit arrives as WM_CHAR while the old field is
            // still the active buffer.
            unsafe {
                self.commit_ime_composition(hwnd);
            }
            // Switching triggers whole-window invalidation: clears the old
            // field's active border and selection pixels; the caret on first
            // focus also appears via this repaint.
            super::invalidate_panel(hwnd);
            self.input.edit.reset();
            // A stale high surrogate left over across fields would mis-pair
            // with a later low surrogate into a wrong supplementary-plane char.
            self.input.surrogate = None;
            self.vis_start.set(0);
            // The multi-click latch belongs to the old field: clicks across
            // a field switch must not chain into a phantom double/triple.
            self.text_clicks = None;
        }
        self.input.field = Some(field);
        if switched {
            let buf = self.input.active_str().to_string();
            self.input.edit.caret_to_end(&buf);
        }
        self.note_caret_interaction();
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hwnd));
            // Blink clock follows the on_anim_tick discipline: from an idle
            // clock, arm once at the caret's flip point; a running animation
            // keeps its period (the next tick re-derives it), and no focus
            // means no arm — zero spinning while idle.
            let focused = windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == hwnd;
            if let Some(period) = blink_arm_period(self.anim_period != 0, focused) {
                self.retune_anim(hwnd, period);
            }
            // Compute the layout only once: caret x and IME composition window
            // positioning share the same product; when the renderer is absent
            // (before the first frame), fall back to 0 and correct on the next
            // update_caret.
            let cx = if let Some(r) = self.renderer.as_ref() {
                let cx = self.caret_layout(r).cx;
                self.sync_ime_pos(hwnd, cx);
                cx
            } else {
                0.0
            };
            if switched {
                self.attach_ime(hwnd, cx);
            }
        }
    }

    /// Commit an in-flight IME composition by reading the composition
    /// string and appending it to the active buffer directly — the
    /// standard edit-control behavior on focus change (field switch and
    /// kill-focus alike, via clear_input), instead of losing the
    /// composition when the context is destroyed.
    /// Deliberately not ImmNotifyIME(CPS_COMPLETE): its delivery is either
    /// synchronous (a re-entrant WM_CHAR re-derives the App alias under
    /// this live &mut Panel) or queued (the composed text lands in the NEW
    /// field after the switch); a plain context read has no notification
    /// and no re-entrancy. No-op without a live composition.
    pub(super) unsafe fn commit_ime_composition(&mut self, hwnd: HWND) {
        use windows::Win32::UI::Input::Ime::{
            GCS_COMPSTR, ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext,
        };
        unsafe {
            let himc = ImmGetContext(hwnd);
            if himc.is_invalid() {
                return;
            }
            let bytes = ImmGetCompositionStringW(himc, GCS_COMPSTR, None, 0);
            if bytes > 0 {
                let mut w16 = vec![0u16; bytes as usize / 2];
                // Pass the buffer's own byte capacity, not the reported
                // count: an odd count rounded down by the allocation would
                // let the API copy one byte past the end.
                ImmGetCompositionStringW(
                    himc,
                    GCS_COMPSTR,
                    Some(w16.as_mut_ptr().cast()),
                    (w16.len() * 2) as u32,
                );
                let text = String::from_utf16_lossy(&w16);
                let max = InputField::max_bytes_for(self.input.field);
                // Paste semantics fit a composition commit: replaces the
                // active selection, filters control characters, enforces
                // the byte budget.
                super::apply_edit(&mut self.input, |e, t| e.paste(t, &text, max));
            }
            let _ = ImmReleaseContext(hwnd, himc);
        }
    }

    /// Attach the IME context and position the composition window at the caret;
    /// cx comes from the caller's layout product
    unsafe fn attach_ime(&mut self, hwnd: HWND, cx: f32) {
        unsafe {
            use windows::Win32::UI::Input::Ime::{
                ImmAssociateContext, ImmCreateContext, ImmDestroyContext,
            };
            // On creation failure, do not re-associate: a null context would
            // cut off IME input, and setting owned would make the restore
            // logic skip. Keep the default context and retry on next focus.
            let ctx = ImmCreateContext();
            if ctx.is_invalid() {
                return;
            }
            let old = ImmAssociateContext(hwnd, ctx);
            // The old context returned by re-association: a self-created one must be
            // destroyed to avoid leaks; the default context swapped out the first time
            // belongs to the system and must not be destroyed.
            if self.ime_owned && !old.is_invalid() {
                let _ = ImmDestroyContext(old);
            }
            self.ime_owned = true;
            self.sync_ime_pos(hwnd, cx);
        }
    }

    /// Move the composition window along with the caret; called after every
    /// caret move so a Chinese IME composition string hugs the caret.
    /// cx is passed in from the caller with the same layout, never recomputed here
    unsafe fn sync_ime_pos(&self, hwnd: HWND, cx: f32) {
        unsafe {
            use windows::Win32::Foundation::POINT;
            use windows::Win32::UI::Input::Ime::{
                CFS_POINT, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext,
                ImmSetCompositionWindow,
            };
            let Some(field) = self.input.field else {
                return;
            };
            let bx = layout::field_geo(field).0;
            let by = self.caret_line_y(field);
            let x = bx + layout::FIELD_TEXT_INSET + cx;
            // The composition window anchors near the bottom of the field box;
            // the logical→physical conversion goes through the same source
            // as every other conversion.
            let pt = POINT {
                x: self.pxf(x),
                y: self.pxf(by + 12.0),
            };
            let ctx = ImmGetContext(hwnd);
            if !ctx.is_invalid() {
                let cf = COMPOSITIONFORM {
                    dwStyle: CFS_POINT,
                    ptCurrentPos: pt,
                    rcArea: windows::Win32::Foundation::RECT::default(),
                };
                let _ = ImmSetCompositionWindow(ctx, &cf);
                let _ = ImmReleaseContext(hwnd, ctx);
            }
        }
    }

    /// Caret visible layout: the shared width artifacts feed the window
    /// decision; caret positioning, hit conversion, and rendering slices
    /// all query the same table, so positions always agree. Returns an
    /// Rc: hit paths only clone the reference, no per-call deep copy.
    pub(crate) fn caret_layout(&self, renderer: &Renderer) -> std::rc::Rc<CaretLayout> {
        let Some(field) = self.input.field else {
            return std::rc::Rc::new(CaretLayout {
                vis_start: 0,
                cx: 0.0,
                widths: std::rc::Rc::new(WidthArtifacts {
                    chars: Vec::new(),
                    widths: vec![0.0],
                }),
            });
        };
        let caret = self.input.edit.caret;
        let sticky = self.vis_start.get();
        // The hit test avoids materializing the display: FieldContent
        // matches the cached string structurally, so blink and drag frames
        // never rebuild the dot string just to confirm a cache hit.
        let content = FieldContent::of_active(self);
        let widths = self.widths_of(renderer, field, &content);
        std::rc::Rc::new(self.window_of(field, &widths, caret, sticky))
    }

    /// Width artifacts of a field's display, cache-hit without
    /// materializing: the masked Key matches structurally (see
    /// FieldContent::matches); only a real miss builds the display
    /// string. The caret path and the render side's unfocused slice
    /// share this one entry.
    pub(crate) fn widths_of(
        &self,
        renderer: &Renderer,
        field: InputField,
        content: &FieldContent<'_>,
    ) -> std::rc::Rc<WidthArtifacts> {
        {
            let cache = self.caret_cache.borrow();
            if let Some((b, w)) = cache[field.slot()].as_ref()
                && content.matches(b)
            {
                return std::rc::Rc::clone(w);
            }
        }
        let display = content.display();
        self.display_widths(renderer, field, &display)
    }

    /// Width artifacts of a field's display string, cached by (field,
    /// display): hit paths clone only the Rc — caret moves,
    /// drag-selection, and the unfocused render slice reuse one table
    /// instead of rebuilding a full DWrite layout per frame. The display
    /// string is compared as-is; callers holding a FieldContent go
    /// through widths_of, which short-circuits before materializing.
    pub(crate) fn display_widths(
        &self,
        renderer: &Renderer,
        field: InputField,
        display: &str,
    ) -> std::rc::Rc<WidthArtifacts> {
        {
            let cache = self.caret_cache.borrow();
            if let Some((b, w)) = cache[field.slot()].as_ref()
                && b == display
            {
                return std::rc::Rc::clone(w);
            }
        }
        let chars: Vec<char> = display.chars().collect();
        let (font_sz, font_wt, font_mono) = layout::INPUT_FONT;
        let widths = unsafe { renderer.prefix_widths(&chars, font_sz, font_wt, font_mono) };
        let rc = std::rc::Rc::new(WidthArtifacts { chars, widths });
        self.caret_cache.borrow_mut()[field.slot()] =
            Some((display.to_string(), std::rc::Rc::clone(&rc)));
        rc
    }

    /// Visible-window decision over the shared width table: pure
    /// arithmetic, no layout work — called per caret move and hit test.
    /// When content fits, start at 0; when overflowing, keep the previous
    /// start for stickiness so the window does not jump back at the
    /// clicked spot — only a caret leaving the window triggers the
    /// binary-search adjustment. In Key masked mode dots map one-to-one
    /// to chars, so the index space is unchanged.
    fn window_of(
        &self,
        field: InputField,
        art: &std::rc::Rc<WidthArtifacts>,
        caret: usize,
        sticky: usize,
    ) -> CaretLayout {
        let widths = &art.widths;
        let n = art.chars.len();
        let caret = caret.min(n);
        let avail = layout::field_avail(field);
        if widths[n] <= avail {
            self.vis_start.set(0);
            return CaretLayout {
                vis_start: 0,
                cx: widths[caret],
                widths: std::rc::Rc::clone(art),
            };
        }
        let sticky = sticky.min(caret);
        if widths[caret] - widths[sticky] <= avail {
            self.vis_start.set(sticky);
            return CaretLayout {
                vis_start: sticky,
                cx: widths[caret] - widths[sticky],
                widths: std::rc::Rc::clone(art),
            };
        }
        // Shift the sticky start rightward only when it can no longer fit the
        // caret; width is monotonically non-increasing in the start, so
        // binary-search the smallest satisfying one.
        let mut lo = sticky;
        let mut hi = caret;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if widths[caret] - widths[mid] <= avail {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        self.vis_start.set(lo);
        CaretLayout {
            vis_start: lo,
            cx: widths[caret] - widths[lo],
            widths: std::rc::Rc::clone(art),
        }
    }

    /// Anchor y of the caret line (logical pixels, including the in-box
    /// vertical offset and scroll translation);
    /// shared by IME composition window positioning, text-zone hit
    /// testing, and the blink frame's narrow repaint band.
    pub(crate) fn caret_line_y(&self, field: InputField) -> f32 {
        let y = match field {
            InputField::Interval => {
                layout::interval_input_y(self.caret_has_account, self.auth_error)
            }
            InputField::Proxy => layout::proxy_input_y(
                self.caret_has_account,
                self.auth_error,
                self.customizing.effective,
            ),
            InputField::PeakStart | InputField::PeakEnd => layout::peak_input_y(
                self.caret_has_account,
                self.auth_error,
                self.customizing.effective,
            ),
            InputField::Name => layout::ADD_NAME_Y,
            InputField::Key => layout::ADD_KEY_Y,
            InputField::Org => layout::ADD_ORG_Y,
            InputField::Project => layout::ADD_PROJECT_Y,
        };
        // After content scrolls up, the anchor moves up in sync so
        // the IME composition window hugs the visible position.
        y + layout::CARET_Y_OFFSET - self.scroll_dy
    }

    /// x within the text zone (logical pixels) → char index: same visible
    /// window as caret_layout, resolves to the nearest character boundary
    /// (used by click positioning and drag selection); all prefix widths
    /// query the passed-in width table, zero layout creation
    pub(crate) fn caret_hit_test(&self, cl: &CaretLayout, field: InputField, x: f32) -> usize {
        let (bx, _w, _tail) = layout::field_geo(field);
        let base = bx + layout::FIELD_TEXT_INSET;
        // Prefix widths are monotonically non-decreasing in the index [the
        // same property caret_layout's binary search relies on]: the nearest
        // boundary can only be the last index whose width <= x or the next
        // slot, so binary search suffices.
        let target = x - base;
        let n = cl.widths.chars.len();
        let vis_start = cl.vis_start;
        let mut lo = vis_start;
        let mut hi = n;
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if cl.seg(vis_start, mid) <= target {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        // Ties resolve to the smaller index; within an equal-width block of
        // zero-width chars take the last index — the caret x is identical and
        // only backspace/word-delete semantics differ, visually equivalent.
        if lo < n
            && (cl.seg(vis_start, lo + 1) - target).abs() < (target - cl.seg(vis_start, lo)).abs()
        {
            lo + 1
        } else {
            lo
        }
    }

    /// Hit test for the active field's text zone: caret-positioning clicks only count
    /// within the field box
    pub(crate) fn hit_text_zone(&self, x: f32, y: f32) -> bool {
        if let Some(f) = self.input.field {
            let (bx, w, tail) = layout::field_geo(f);
            let top = self.caret_line_y(f) - layout::CARET_Y_OFFSET;
            x >= bx + FIELD_EDGE_TOL && x <= bx + w - tail && y >= top && y <= top + layout::INPUT_H
        } else {
            false
        }
    }

    /// Move the IME composition window using an already-computed caret x,
    /// for call sites within the same event that already hold the layout
    /// product.
    pub(crate) fn place_caret(&self, hwnd: HWND, cx: f32) {
        unsafe {
            self.sync_ime_pos(hwnd, cx);
        }
    }

    /// Caret blink phase: measured from the last interaction moment, on/off
    /// each half period via modulo; an interaction resets to zero so the
    /// caret shows immediately. Returns current visibility and milliseconds
    /// until the next flip.
    pub(crate) fn caret_blink(&self) -> (bool, u32) {
        Self::blink_phase(self.last_input_at.get().elapsed().as_millis())
    }

    /// Phase decision decoupled from the time source, for pinned tests.
    fn blink_phase(elapsed_ms: u128) -> (bool, u32) {
        let total = 2 * BLINK_HALF_MS as u128;
        let t = elapsed_ms % total;
        if t < BLINK_HALF_MS as u128 {
            (true, BLINK_HALF_MS - t as u32)
        } else {
            (false, (total - t) as u32)
        }
    }

    /// Clicks, typing, caret moves, and focus all reset the blink phase uniformly.
    pub(crate) fn note_caret_interaction(&self) {
        self.last_input_at.set(std::time::Instant::now());
        // A zeroed phase is necessarily visible; consistent with the next
        // frame's drawing, so flip detection never misfires an extra frame.
        self.blink_drawn.set(true);
    }

    /// Compute the caret position from field content and move the IME
    /// composition window; shares the same layout product with input_field
    /// drawing; also resets the blink phase — every call site is a point
    /// where caret or text changed.
    /// When the renderer is absent (before the first frame), skip
    /// positioning and make up for it on the next call.
    pub fn update_caret(&self, hwnd: HWND, renderer: Option<&Renderer>) {
        if self.input.field.is_none() {
            return;
        }
        self.note_caret_interaction();
        let Some(r) = renderer else {
            return;
        };
        let cx = self.caret_layout(r).cx;
        self.place_caret(hwnd, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Caret click positioning: nearest boundary under equal widths,
    /// out-of-range clamping, zero-width ties take the last index, absolute
    /// index returned under visible-window offset
    #[test]
    fn caret_hit_positions() {
        let p = Panel::new();
        let field = InputField::Name;
        let base = layout::field_geo(field).0 + layout::FIELD_TEXT_INSET;
        let cl = |widths: &[f32], vis_start: usize| CaretLayout {
            vis_start,
            cx: 0.0,
            widths: std::rc::Rc::new(WidthArtifacts {
                chars: vec!['x'; widths.len() - 1],
                widths: widths.to_vec(),
            }),
        };
        // Equal-width table [0,5,10,15,20]: a click between two slots takes the nearer
        // one; equidistant takes the smaller index
        let w = cl(&[0.0, 5.0, 10.0, 15.0, 20.0], 0);
        assert_eq!(p.caret_hit_test(&w, field, base + 2.0), 0);
        assert_eq!(p.caret_hit_test(&w, field, base + 7.6), 2);
        assert_eq!(p.caret_hit_test(&w, field, base + 7.4), 1);
        // Extreme left/right inside the box clamps to head/tail
        assert_eq!(p.caret_hit_test(&w, field, base - 100.0), 0);
        assert_eq!(p.caret_hit_test(&w, field, base + 1000.0), 4);
        // Zero-width chars (indices 1→2 share a width): clicking at that width takes the last index
        let zw = cl(&[0.0, 5.0, 5.0, 10.0], 0);
        assert_eq!(p.caret_hit_test(&zw, field, base + 5.0), 2);
        // Visible-window start offset: returns the absolute index, not the window-relative one
        let vs = cl(&[0.0, 5.0, 10.0, 15.0, 20.0], 1);
        assert_eq!(p.caret_hit_test(&vs, field, base + 5.0), 2);
    }

    /// Pins window_of: full fit forces start 0, overflow keeps a sticky that still fits the caret, a mismatched sticky re-aims by binary search.
    #[test]
    fn window_of_visible_window() {
        let p = Panel::new();
        let field = InputField::Name;
        // Name tier: CONTENT_W(340 - 2*20) - FIELD_TEXT_INSET(6) - tail(4) = 290
        assert_eq!(layout::field_avail(field), 290.0);
        let art = |n: usize, cw: f32| {
            std::rc::Rc::new(WidthArtifacts {
                chars: vec!['a'; n],
                widths: (0..=n).map(|i| i as f32 * cw).collect(),
            })
        };

        // (a) 5*10=50 fits: start forced to 0 despite sticky 2, cx is the
        // absolute prefix width; a caret past the end clamps to n
        p.vis_start.set(2);
        let a5 = art(5, 10.0);
        let cl = p.window_of(field, &a5, 3, 2);
        assert_eq!((cl.vis_start, cl.cx), (0, 30.0));
        assert_eq!(p.vis_start.get(), 0);
        assert!(std::rc::Rc::ptr_eq(&cl.widths, &a5));
        let cl = p.window_of(field, &a5, 99, 0);
        assert_eq!((cl.vis_start, cl.cx), (0, 50.0));

        // (b) 40*10=400 overflows, but the sticky 20..caret 25 segment (50px)
        // fits: kept as-is
        let a40 = art(40, 10.0);
        let cl = p.window_of(field, &a40, 25, 20);
        assert_eq!((cl.vis_start, cl.cx), (20, 50.0));
        assert_eq!(p.vis_start.get(), 20);
        // sticky past the caret clamps down to the caret itself
        let cl = p.window_of(field, &a40, 5, 9);
        assert_eq!((cl.vis_start, cl.cx), (5, 0.0));

        // (c) caret 39 from sticky 0 (390px) overflows the window too: the
        // smallest fitting start is 10 — 390-100=290 fills avail exactly
        let cl = p.window_of(field, &a40, 39, 0);
        assert_eq!((cl.vis_start, cl.cx), (10, 290.0));
        assert_eq!(p.vis_start.get(), 10);

        // (c') unequal widths: total 340 needs widths[start] >= 50 → start 4
        let unequal = std::rc::Rc::new(WidthArtifacts {
            chars: vec!['a'; 8],
            widths: vec![0.0, 5.0, 15.0, 30.0, 200.0, 310.0, 320.0, 330.0, 340.0],
        });
        let cl = p.window_of(field, &unequal, 8, 0);
        assert_eq!((cl.vis_start, cl.cx), (4, 140.0));
    }

    /// Caret blink phase: interaction zeroing shows immediately, on/off each
    /// half second, remaining ms to flip decreasing, the cycle wraps back to
    /// on.
    #[test]
    fn caret_blink_phase() {
        assert_eq!(Panel::blink_phase(0), (true, 500));
        assert_eq!(Panel::blink_phase(499), (true, 1));
        assert_eq!(Panel::blink_phase(500), (false, 500));
        assert_eq!(Panel::blink_phase(999), (false, 1));
        assert_eq!(Panel::blink_phase(1000), (true, 500));
        assert_eq!(Panel::blink_phase(1499), (true, 1));
        assert_eq!(Panel::blink_phase(1500), (false, 500));
    }

    /// Focus-time clock arm: idle + caret present arms once at the flip
    /// point (the full on-half after the interaction zeroing, not the frame
    /// rate); a running clock or an absent caret leaves the clock untouched.
    #[test]
    fn blink_arm_period_pinned() {
        assert_eq!(blink_arm_period(false, true), Some(BLINK_HALF_MS));
        assert_eq!(blink_arm_period(true, true), None);
        assert_eq!(blink_arm_period(false, false), None);
        assert_eq!(blink_arm_period(true, false), None);
    }

    /// Structural matching against the masked Key display: exact-count
    /// dots for the active form, head…tail for the long inactive form,
    /// and near-miss shapes rejected (cache keys never alias).
    #[test]
    fn masked_key_structural_match() {
        let key = "abcdefghij0123456789"; // 20 chars
        // Active: one dot per char
        let dots = crate::ui::panel::MASK_GLYPH.to_string().repeat(20);
        assert!(masked_key_matches(&dots, key, true));
        assert!(!masked_key_matches(
            &crate::ui::panel::MASK_GLYPH.to_string().repeat(19),
            key,
            true
        ));
        // Inactive 12+: first4 + … + last4
        assert!(masked_key_matches("abcd…6789", key, false));
        assert!(!masked_key_matches("abcd…6780", key, false), "尾段不符");
        assert!(!masked_key_matches("ab…6789", key, false), "头段不符");
        // Short keys dot per char in both states
        let short = "abc";
        assert!(masked_key_matches("•••", short, false));
        assert!(!masked_key_matches("a…c", short, false));
    }
}
