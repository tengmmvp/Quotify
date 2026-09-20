//! Error-summary marquee: scroll state, belt geometry, frame timing, and the tick

use super::{Renderer, TextBlock};

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
}

#[cfg(test)]
mod tests {
    use super::belt_width;

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
}
