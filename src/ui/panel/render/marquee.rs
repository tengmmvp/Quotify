//! Error marquee: scroll state, belt geometry, frame timing, the tick, and
//! the error copy family shared by the card and the footer

use super::{Renderer, TextBlock};
use crate::api::{ErrorCategory, ErrorDetail, ErrorInfo, FetchError};
use crate::ui::i18n::Strings;

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
    pub(super) key: ScrollKey,
    /// Three content blocks (prefix / category / tail): layout and width are held as
    /// a pair, consumers do no re-layout;
    /// under reduced motion the over-wide case stores the truncated string's layout
    pub(super) blocks: [ScrollBlock; 3],
    /// Left and right brackets delimiting the blocks: bold weight, shared by the three blocks
    pub(super) bracket: ScrollBlock,
    pub(super) bracket_end: ScrollBlock,
    /// Whether the summary is over-wide and scrolls: the non-scrolling state is
    /// cache-only, tick does not keep it alive
    pub(super) scrolling: bool,
    /// Frozen while unfocused: the timer is already removed and reactivation wakes it;
    /// both the re-mount decision and advancement bypass it
    pub(super) frozen: bool,
    /// Current offset, 0..total_w
    pub(super) offset: f32,
    /// Pause deadline; a past instant while scrolling
    pub(super) resume_at: std::time::Instant,
    /// Last frame's time; the frame rate varies with animation priority,
    /// advance by the actual elapsed time
    pub(super) last: std::time::Instant,
}

/// Marquee content blocks reuse the TextBlock structure
pub(crate) type ScrollBlock = TextBlock;

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

impl Renderer {
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

    /// Error card body (cached): the key is a full-value pair of error
    /// content and language, so marquee frames rebuild nothing; leaving
    /// out the language would keep stale copy after a language switch.
    /// An Rc refcount clone replaces a full deep copy of the string —
    /// hit registration still needs &mut self mid-drawing, so it cannot
    /// be borrowed out.
    pub(super) fn cached_error_body(
        &mut self,
        s: &Strings,
        e: &FetchError,
        lang: crate::ui::i18n::Lang,
    ) -> std::rc::Rc<str> {
        // Reference comparison on the hit path; the owned clone only on a
        // rebuild — the marquee calls this every frame while scrolling.
        if let Some((k, b)) = self.err_card.as_ref()
            && k.0 == *e
            && k.1 == lang
        {
            return std::rc::Rc::clone(b);
        }
        let key = (e.clone(), lang);
        let b: std::rc::Rc<str> = error_card_body(s, e).into();
        self.err_card = Some((key, std::rc::Rc::clone(&b)));
        b
    }
}

/// Category title copy
pub(super) fn category_label(s: &Strings, c: ErrorCategory) -> &'static str {
    match c {
        ErrorCategory::Unauthenticated => s.err_cat_auth,
        ErrorCategory::Request => s.err_cat_bad_request,
        ErrorCategory::Throttled => s.err_cat_rate_limited,
        ErrorCategory::Server => s.err_cat_server,
        ErrorCategory::Network => s.err_cat_network,
        ErrorCategory::Empty => s.err_cat_no_data,
    }
}

/// Footer cache key: a thin delegation to fmt::elapsed_minutes_since —
/// the same clamped minute bucket the text tiers read, so key
/// transitions land exactly at whole elapsed minutes (the zero clamp
/// single-sources there; a wall-clock step back must not bucket to -1).
/// Taking a whole-second timestamp would drop the snapshot's sub-second
/// part; the key transition would lead the real one by up to a second,
/// and crossing a tier would cache the stale copy under the new key,
/// displaying it a full minute late
pub(super) fn footer_key(now_ms: i64, at_ms: i64) -> i64 {
    crate::ui::fmt::elapsed_minutes_since(now_ms, at_ms)
}

/// Display material for FetchError: the official error info and the
/// fallback copy for categories with no official hint; categories whose
/// info is None emit the raw copy directly, with no fallback chain. The
/// error card and the footer tail segment share this source; the two
/// views' fallback pairing never diverges.
fn error_parts<'a>(e: &'a FetchError, s: &'a Strings) -> (Option<&'a ErrorInfo>, &'a str) {
    match e {
        FetchError::Unauthenticated(info) => (Some(info), s.err_auth_hint),
        FetchError::Empty => (None, ""),
        FetchError::Failure(info) => (Some(info), ""),
        FetchError::Network(d) => (None, d),
        FetchError::Throttled { info, .. } => (Some(info), ""),
    }
}

/// Error display material shared by the card body and the footer tail:
/// a (code, message) pair each surface renders its own way, or plain
/// text (code-only template, raw message, fallback copy). One ladder —
/// the two views' material selection cannot diverge.
enum ErrorMaterial<'a> {
    CodeMsg(i64, std::borrow::Cow<'a, str>),
    Text(std::borrow::Cow<'a, str>),
}

/// Material selection has the single source `ErrorInfo::detail`
fn error_material<'a>(e: &'a FetchError, s: &'a Strings) -> ErrorMaterial<'a> {
    match error_parts(e, s) {
        (Some(info), fallback) => match info.detail() {
            ErrorDetail::CodeAndMsg { code, msg } => {
                ErrorMaterial::CodeMsg(code, std::borrow::Cow::Borrowed(msg))
            }
            ErrorDetail::CodeOnly { code } => ErrorMaterial::Text(code_only_body(s, code).into()),
            ErrorDetail::Msg(msg) => ErrorMaterial::Text(msg),
            ErrorDetail::Fallback => ErrorMaterial::Text(fallback.into()),
        },
        (None, raw) => ErrorMaterial::Text(raw.into()),
    }
}

/// Error card body: only the card template assembly happens here.
fn error_card_body(s: &Strings, e: &FetchError) -> String {
    // The empty state is onboarding body text and does not go through
    // the fallback chain.
    if matches!(e, FetchError::Empty) {
        return s.err_empty_hint.to_string();
    }
    match error_material(e, s) {
        ErrorMaterial::CodeMsg(code, msg) => s
            .err_code_msg
            .replace("{code}", &code.to_string())
            .replace("{msg}", &msg),
        ErrorMaterial::Text(t) => t.into_owned(),
    }
}

/// Suffix segment of the footer summary; an empty string means no tail
/// segment.
pub(super) fn footer_error_tail(s: &Strings, e: &FetchError) -> String {
    match error_material(e, s) {
        ErrorMaterial::CodeMsg(_, msg) => msg.into_owned(),
        ErrorMaterial::Text(t) => t.into_owned(),
    }
}

/// Code-only error body: shared by the error card and the footer tail — the
/// two views are mutually exclusive states of the same error and must not
/// render it differently.
fn code_only_body(s: &Strings, code: i64) -> String {
    s.err_code_only.replace("{code}", &code.to_string())
}

#[cfg(test)]
mod tests {
    use super::{belt_width, footer_key};

    /// Belt geometry pins: empty blocks contribute zero (no bracket pair
    /// either), positive blocks accumulate width + pair each, and the
    /// period carries no trailing blank.
    #[test]
    fn belt_width_pins() {
        assert_eq!(belt_width(&[], 4.0), 0.0);
        // Zero-width blocks (truncation budget exhausted) are skipped whole
        assert_eq!(belt_width(&[0.0, 0.0, 0.0], 4.0), 0.0);
        assert_eq!(belt_width(&[10.0], 4.0), 14.0);
        assert_eq!(belt_width(&[10.0, 0.0, 20.0], 4.0), 38.0);
    }

    /// Footer key and copy tiers share the same boundary: transitions
    /// land exactly at whole elapsed minutes; the snapshot's sub-second
    /// part never makes the key early or late
    #[test]
    fn footer_key_matches_text_tiers() {
        let at = 1_000_000i64 * 60_000 + 700;
        assert_eq!(footer_key(at + 59_999, at), 0);
        assert_eq!(footer_key(at + 60_000, at), 1);
        assert_eq!(footer_key(at + 119_999, at), 1);
        assert_eq!(footer_key(at + 120_000, at), 2);
        // Wall clock stepped back (NTP correction): delta clamps to zero,
        // never bucketing to -1 — same clamp as fmt::updated_text
        assert_eq!(footer_key(at - 30_000, at), 0);
    }
}
