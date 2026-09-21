//! Metric region of the main view: metric rows, the MCP composition
//! section, and the Token cards

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget};

use super::Renderer;
use super::main::{MainLabels, RowTexts};
use crate::api::{TOKEN_LEGS, TokenLeg};
use crate::ui::fmt;
use crate::ui::panel::layout;
use crate::ui::panel::theme::CARD_RADIUS;

/// MCP tool palette
const MCP_PALETTE: [[f32; 4]; 4] = [
    [0.361, 0.616, 0.549, 1.0], // #5C9D8C gray-teal
    [0.431, 0.529, 0.659, 1.0], // #6E87A8 mist blue
    [0.584, 0.514, 0.624, 1.0], // #95839F gray-purple
    [0.678, 0.624, 0.561, 1.0], // #AD9F8F warm gray
];

/// Energy cell geometry
const MCP_CELLS: usize = 20;
const MCP_CELL_W: f32 = 12.0;
const MCP_CELL_GAP: f32 = 2.0;
const MCP_CELL_R: f32 = 2.0;

/// Token card typography spec: font sizes of number and unit, number
/// weight, unit gap, and in-card truncation inset — the measure and
/// draw sides share this source
const TOKEN_NUM_SIZE: f32 = 20.0;
const TOKEN_NUM_WEIGHT: u16 = 700;
const TOKEN_UNIT_SIZE: f32 = 11.0;
const TOKEN_UNIT_GAP: f32 = 1.0;
const TOKEN_CARD_INSET: f32 = 4.0;

/// Pending-fill range sequence for the energy bar (segment order is the
/// draw order): cell positions not covered between data segments get
/// track gap segments; colors are paired by the caller — data segments
/// use their segment color, gap segments use the track color.
fn mcp_job_ranges(segs: &[(usize, usize, [f32; 4])]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut cursor = 0usize;
    for (start, end, _) in segs {
        if *start > cursor {
            ranges.push((cursor, *start));
        }
        ranges.push((*start, *end));
        cursor = *end;
    }
    if cursor < MCP_CELLS {
        ranges.push((cursor, MCP_CELLS));
    }
    ranges
}

/// Legend width fitting (pure): walk the badge widths and return how many
/// fit whole. Keeping badge i requires the remaining width to also hold
/// the inter-badge gap plus the "+N" tail (hidden counts come from the
/// parse-side total, so residency-truncated tools count as hidden too);
/// an over-wide first badge yields 0 — the row degrades to the bare +N.
fn fit_badges(
    widths: impl Iterator<Item = f32>,
    inner_w: f32,
    total_count: usize,
    tail_unit: f32,
) -> usize {
    let mut shown = 0usize;
    let mut cur = 0.0f32;
    for (i, bw) in widths.enumerate() {
        let add = if i == 0 { 0.0 } else { MCP_BADGE_GAP } + bw;
        let tail = if total_count.saturating_sub(i + 1) > 0 {
            MCP_BADGE_GAP + tail_unit
        } else {
            0.0
        };
        if cur + add + tail > inner_w {
            break;
        }
        cur += add;
        shown = i + 1;
    }
    shown
}

/// Legend badge geometry
const MCP_BADGE_SIZE: f32 = 11.0;
const MCP_SWATCH: f32 = 8.0;
const MCP_SWATCH_GAP: f32 = 3.0;
const MCP_BADGE_GAP: f32 = 6.0;

/// Legend badge: swatch color and the prebuilt label layout; every
/// frame path draws blocks and never re-measures
struct McpBadge {
    color: [f32; 4],
    block: super::TextBlock,
}

/// Data-side artifacts of the MCP composition section, cached by
/// (snapshot time, inner width, theme discriminator); a hit skips
/// sorting, segmentation, and all width measuring; drawing fills one
/// rounded rect per cell following the cached segments.
pub(super) struct McpCompCache {
    key: chrono::DateTime<chrono::Local>,
    inner_w: f32,
    /// Theme discriminator (the track color baked into gap segments):
    /// an in-place theme swap invalidates the cache without waiting
    /// for the next poll
    theme: [f32; 4],
    /// Energy-bar fill sequence (gap-filled, colors already paired — data
    /// segments keep their color, gaps take the track color): the draw
    /// loop iterates straight through without rebuilding the range walk
    /// or re-searching colors per frame.
    ranges: Vec<(usize, usize, [f32; 4])>,
    badges: Vec<McpBadge>,
    shown: usize,
    /// Tools hidden from the legend: parse-side total_count minus the
    /// shown badges — covers both width-dropped badges and parse-side
    /// residency-truncated entries; shown == 0 counts every tool hidden
    /// (the compact form draws "+N" alone); the "+N" badge draws exactly
    /// when this is > 0.
    hidden: usize,
    plus: super::TextBlock,
}

impl McpCompCache {
    /// Cache-hit verdict: snapshot time, inner width, and theme
    /// discriminator must all match
    fn is_fresh(
        &self,
        queried_at: chrono::DateTime<chrono::Local>,
        inner_w: f32,
        theme: [f32; 4],
    ) -> bool {
        self.key == queried_at && self.inner_w == inner_w && self.theme == theme
    }
}

/// Typography artifacts of the Token cards, cached by snapshot time:
/// layout blocks change with polls and are not recomputed on animation
/// frames
pub(super) struct TokenCells {
    key: chrono::DateTime<chrono::Local>,
    cells: [TokenCell; TOKEN_LEGS.len()],
}

/// Single-card typography artifacts: prebuilt layouts for the number
/// and unit
struct TokenCell {
    num: super::TextBlock,
    unit: super::TextBlock,
}

impl Renderer {
    /// Metric row: label + percentage + progress bar, with the countdown
    /// on the left and the used detail on the right of the bottom line;
    /// all text goes through MainTexts prebuilt layouts and only drawing
    /// happens here. Returns the next row's y
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn metric_row(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &super::TextBlock,
        row: &RowTexts,
        used_percent: f64,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 {
        let pad = layout::CONTENT_PAD;
        // Values and the progress bar turn danger-colored; the threshold
        // shares the tray icon's red tier.
        let critical = used_percent >= crate::ui::icon::CRITICAL_PCT;
        let (c_label, c_value, c_caption, c_track) = (
            self.theme.text_primary,
            if critical {
                self.theme.danger
            } else {
                self.theme.text_primary
            },
            self.theme.text_tertiary,
            self.theme.track,
        );

        self.draw_block(target, label, pad, y + 2.0, c_label, alpha);
        self.draw_block_r(target, &row.percent, w - pad, y + 1.0, c_value, alpha);

        let bar_h = 5.0;
        let bar_y = y + 22.0;
        let track = D2D_RECT_F {
            left: pad,
            top: bar_y,
            right: w - pad,
            bottom: bar_y + bar_h,
        };
        let track_brush = self.brush(target, c_track, alpha);
        target.FillRectangle(&track, &track_brush);
        let frac = (used_percent / 100.0).clamp(0.0, 1.0) as f32;
        // Fractions below 0.4% draw no fill; when drawn, the fill is at
        // least 2px wide.
        if frac > 0.004 {
            let fg_w = ((w - pad * 2.0) * frac).max(2.0);
            let fg = D2D_RECT_F {
                left: pad,
                top: bar_y,
                right: pad + fg_w,
                bottom: bar_y + bar_h,
            };
            let fill = self.brush(target, c_value, alpha);
            target.FillRectangle(&fg, &fill);
        }

        // Bottom line split left/right: countdown takes the left 55%,
        // the used detail is right-aligned.
        if let Some(r) = row.reset.as_ref() {
            self.draw_block(target, r, pad, bar_y + bar_h + 5.0, c_caption, alpha);
        }
        if let Some(d) = row.detail.as_ref() {
            self.draw_block_r(target, d, w - pad, bar_y + bar_h + 5.0, c_caption, alpha);
        }
        y + layout::MAIN_METRIC_ROW_H
    }

    /// MCP tool composition section: inside a square-cornered frame, one
    /// rounded-cell energy bar [full string = total tool consumption]
    /// plus one row of three-part badges as the legend [swatch | name |
    /// count]. Cell segments and badge swatches take a low-saturation
    /// four-color palette ordered by descending usage, with the 5th+
    /// tools merged into the fourth color; when the legend does not fit,
    /// trailing whole badges are dropped for +N, and an over-width first
    /// badge degrades the row to the bare +N count (N = every tool).
    /// Data only changes with polls; segmentation, badges, and fitting
    /// results are cached by (snapshot time, inner width, theme
    /// discriminator) and not recomputed on animation frames.
    /// Returns the next row's y
    pub(super) unsafe fn mcp_composition(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        m: &crate::api::McpUsage,
        queried_at: chrono::DateTime<chrono::Local>,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 {
        let pad = layout::CONTENT_PAD;
        let top = y + layout::MAIN_MCP_COMP_TOP_GAP;
        let frame_h = layout::MAIN_MCP_COMP_H - layout::MAIN_MCP_COMP_TOP_GAP;
        let inner_x = pad + 1.0 + layout::MAIN_MCP_COMP_PAD_X;
        let inner_w = w - 2.0 * pad - 2.0 - 2.0 * layout::MAIN_MCP_COMP_PAD_X;
        let frame = D2D_RECT_F {
            left: pad + 0.5,
            top: top + 0.5,
            right: w - pad - 0.5,
            bottom: top + frame_h - 0.5,
        };
        let stroke = self.brush(target, self.theme.border, alpha);
        target.DrawRectangle(&frame, &stroke, 1.0, None);

        // Data-side artifacts are cached by (snapshot time, inner width,
        // theme discriminator): a hit skips sorting, segmentation, and
        // all width measuring; width is part of the key because fitting
        // results change with panel width (across-DPI monitors), and the
        // theme because gap segments bake in the track color — an
        // appearance switch invalidates without waiting for a poll.
        // take() moves ownership out so the draw section can freely call
        // &mut self drawing methods; this function has a single exit,
        // put back at function end.
        let theme = self.theme.track;
        let mut cache = self.mcp_cache.take();
        if cache
            .as_ref()
            .is_none_or(|c| !c.is_fresh(queried_at, inner_w, theme))
        {
            cache = Some(self.build_mcp_cache(m, queried_at, inner_w));
        }
        let c = cache.expect("mcp_cache 上方必已置");
        let badges = &c.badges;
        let shown = c.shown;

        let cell_h = layout::MAIN_MCP_CELL_H;
        let bar_y = top + 1.0 + layout::MAIN_MCP_COMP_PAD_Y;
        let bar_w = MCP_CELLS as f32 * MCP_CELL_W + (MCP_CELLS - 1) as f32 * MCP_CELL_GAP;
        let x0 = inner_x + (inner_w - bar_w) / 2.0;
        // Pending-fill segments come pre-paired from the cache: the draw
        // loop fills one rounded rect per cell with no per-frame range
        // walk or color lookup.
        for (rs, re, color) in &c.ranges {
            let b = self.brush(target, *color, alpha);
            for i in *rs..*re {
                let left = x0 + i as f32 * (MCP_CELL_W + MCP_CELL_GAP);
                let cell = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left,
                        top: bar_y,
                        right: left + MCP_CELL_W,
                        bottom: bar_y + cell_h,
                    },
                    radiusX: MCP_CELL_R,
                    radiusY: MCP_CELL_R,
                };
                target.FillRoundedRectangle(&cell, &b);
            }
        }

        let leg_y = bar_y + cell_h + layout::MAIN_MCP_LEGEND_ADV;
        let leg_h = layout::MAIN_MCP_LEGEND_H;
        let sw_top = leg_y + 4.0;
        let swatch = |x: f32| D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: sw_top,
                right: x + MCP_SWATCH,
                bottom: sw_top + MCP_SWATCH,
            },
            radiusX: MCP_CELL_R,
            radiusY: MCP_CELL_R,
        };
        let mut x = inner_x;
        // Badge text is vertically centered (the former text_raw
        // vcenter behavior); y is derived from the row height
        let text_y = |h: f32| leg_y + (leg_h - h) / 2.0;
        if shown == 0 && !badges.is_empty() {
            // First badge alone already over-width: the row degrades to
            // the bare "+N" count (N = every tool, all counted hidden) —
            // block-only drawing, no per-frame measuring.
            self.draw_block_nosnap(
                target,
                &c.plus,
                x,
                text_y(c.plus.h),
                self.theme.text_tertiary,
                alpha,
            );
        } else {
            for (i, b) in badges.iter().take(shown).enumerate() {
                if i > 0 {
                    x += MCP_BADGE_GAP;
                }
                let sw = self.brush(target, b.color, alpha);
                target.FillRoundedRectangle(&swatch(x), &sw);
                x += MCP_SWATCH + MCP_SWATCH_GAP;
                self.draw_block_nosnap(
                    target,
                    &b.block,
                    x,
                    text_y(b.block.h),
                    self.theme.text_secondary,
                    alpha,
                );
                x += b.block.w;
            }
            if c.hidden > 0 {
                x += MCP_BADGE_GAP;
                self.draw_block_nosnap(
                    target,
                    &c.plus,
                    x,
                    text_y(c.plus.h),
                    self.theme.text_tertiary,
                    alpha,
                );
            }
        }
        self.mcp_cache = Some(c);
        y + layout::MAIN_MCP_COMP_H
    }

    /// Rebuild the MCP composition cache: tool sorting, energy bar
    /// segmentation, and badge width-measured fitting
    unsafe fn build_mcp_cache(
        &mut self,
        m: &crate::api::McpUsage,
        queried_at: chrono::DateTime<chrono::Local>,
        inner_w: f32,
    ) -> McpCompCache {
        // Tools in descending order; segment boundaries are rounded
        // cumulative shares, seamless and non-overlapping between
        // segments, with the last segment filling the whole bar.
        let mut items: Vec<&crate::api::McpDetail> = m.details.iter().collect();
        items.sort_by(|a, b| {
            b.usage
                .partial_cmp(&a.usage)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        // Normalization denominator takes the parse-side whole-set
        // total: the residency cap drops the lowest-usage tail, and
        // normalizing against the kept top-N sum alone would overstate
        // the last visible segment's share. A zero total falls back to
        // the kept sum (zero as well) — no division either way.
        let sum: f64 = if m.total_usage > 0.0 {
            m.total_usage
        } else {
            items.iter().map(|d| d.usage).sum()
        };
        let mut segs: Vec<(usize, usize, [f32; 4])> = Vec::new();
        if sum > 0.0 {
            let mut cum = 0.0f64;
            for (i, d) in items.iter().enumerate() {
                cum += d.usage;
                let end = if i + 1 == items.len() {
                    MCP_CELLS
                } else {
                    (cum / sum * MCP_CELLS as f64)
                        .round()
                        .clamp(0.0, MCP_CELLS as f64) as usize
                };
                let start = segs.last().map_or(0, |s| s.1);
                if end > start {
                    segs.push((start, end, MCP_PALETTE[i.min(MCP_PALETTE.len() - 1)]));
                }
            }
        }
        // The legend is a run of badges [swatch | name · count]: swatch
        // colors match the energy bar segments, and capitalizing the
        // first letter of names only affects the display layer. Fitting
        // is width-driven (fit_badges); the cache holds blocks alone, so
        // no frame path re-measures.
        let hidden_after = |shown: usize| m.total_count.saturating_sub(shown);
        let badges: Vec<McpBadge> = items
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let mut cs = d.model_code.chars();
                let name = match cs.next() {
                    Some(c) => c.to_uppercase().collect::<String>() + cs.as_str(),
                    None => String::new(),
                };
                let text = format!("{name} {}", fmt::compact_number(d.usage));
                let block =
                    super::TextBlock::new(self.make_layout(&text, MCP_BADGE_SIZE, 400, true));
                McpBadge {
                    color: MCP_PALETTE[i.min(MCP_PALETTE.len() - 1)],
                    block,
                }
            })
            .collect();
        let badge_w = |b: &McpBadge| MCP_SWATCH + MCP_SWATCH_GAP + b.block.w;
        // The "+N" tail width is measured once before the loop at the
        // widest tier (the hidden count only shrinks with i): every
        // iteration reuses that conservative width, zero strings and
        // zero layouts inside the loop.
        let tail_unit = if m.total_count > 1 {
            self.measure(
                &format!("+{}", m.total_count - 1),
                MCP_BADGE_SIZE,
                400,
                true,
            )
        } else {
            0.0
        };
        let shown = fit_badges(
            badges.iter().map(badge_w),
            inner_w,
            m.total_count,
            tail_unit,
        );
        let hidden = hidden_after(shown);
        let plus = if hidden > 0 {
            let t = format!("+{hidden}");
            super::TextBlock::new(self.make_layout(&t, MCP_BADGE_SIZE, 400, true))
        } else {
            super::TextBlock::new(None)
        };
        // Gap-fill the bar once at build: data segments keep their color,
        // gaps take the track color — pairing here keeps the draw loop a
        // straight iteration.
        let track = self.theme.track;
        let ranges = mcp_job_ranges(&segs)
            .into_iter()
            .map(|(s, e)| {
                let color = segs
                    .iter()
                    .find(|(ps, pe, _)| (*ps, *pe) == (s, e))
                    .map_or(track, |(_, _, c)| *c);
                (s, e, color)
            })
            .collect();
        McpCompCache {
            key: queried_at,
            inner_w,
            theme: track,
            ranges,
            badges,
            shown,
            hidden,
            plus,
        }
    }

    /// Rebuild the Token card cache: formatting, measuring, and
    /// over-width truncation in one pass
    unsafe fn build_token_cells(
        &mut self,
        ts: &crate::api::TokenStats,
        queried_at: chrono::DateTime<chrono::Local>,
    ) -> TokenCells {
        let avail = layout::TOKEN_CARD_W - TOKEN_CARD_INSET;
        let cells = ts.legs.map(|v| {
            let (num, unit) = match v {
                Some(v) => fmt::compact_number_split(v),
                None => (fmt::MISSING.to_string(), ""),
            };
            // Consolas only has the 400/700 weight tiers; 700 explicitly
            // means bold. The number and unit layouts are built once and
            // measured through their blocks — layout metrics serve the
            // fit check, no second measure pass per string.
            let unit = self.make_text_block(unit, TOKEN_UNIT_SIZE, 400);
            let full = super::TextBlock::new(self.make_layout(
                &num,
                TOKEN_NUM_SIZE,
                TOKEN_NUM_WEIGHT,
                true,
            ));
            // Absurdly large values can exceed the card width: truncate
            // into the width rather than crossing the stroke and
            // crowding the neighboring card.
            let num = if full.w + unit.w + TOKEN_UNIT_GAP > avail {
                let (t, _) = self.ellipsize(
                    &num,
                    TOKEN_NUM_SIZE,
                    avail - unit.w - TOKEN_UNIT_GAP,
                    TOKEN_NUM_WEIGHT,
                    true,
                );
                super::TextBlock::new(self.make_layout(&t, TOKEN_NUM_SIZE, TOKEN_NUM_WEIGHT, true))
            } else {
                full
            };
            TokenCell { num, unit }
        });
        TokenCells {
            key: queried_at,
            cells,
        }
    }

    /// Three Token cards section: the typography cache hits by snapshot;
    /// rebuilds complete in one pass; single exit — the cache taken out
    /// must be put back at function end. Returns the section's bottom y
    pub(super) unsafe fn token_section(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        labels: &MainLabels,
        ts: &crate::api::TokenStats,
        queried_at: chrono::DateTime<chrono::Local>,
        y: f32,
        alpha: f32,
    ) -> f32 {
        let pad = layout::CONTENT_PAD;
        let cy = y + layout::MAIN_TOKEN_CARD_ADV;
        let mut cells = self.token_cells.take();
        if cells.as_ref().is_none_or(|c| c.key != queried_at) {
            cells = Some(self.build_token_cells(ts, queried_at));
        }
        let cells = cells.expect("token_cells 上方必已置");
        // Labels are dispatched exhaustively by tier: pairing stays
        // cohesive, and adding/removing tiers is compiler-checked.
        for (i, (leg, _, _)) in TOKEN_LEGS.iter().enumerate() {
            let label = match leg {
                TokenLeg::Today => &labels.today,
                TokenLeg::Week => &labels.week,
                TokenLeg::Month => &labels.month,
            };
            self.token_card(
                target,
                label,
                &cells.cells[i],
                pad + i as f32 * (layout::TOKEN_CARD_W + layout::TOKEN_CARD_GAP),
                cy,
                alpha,
            );
        }
        self.token_cells = Some(cells);
        cy + layout::TOKEN_CARD_H
    }

    /// Token card: transparent-fill thin-stroke container; number and
    /// unit are laid out separately (prebuilt layouts), with the label
    /// sitting at the bottom.
    unsafe fn token_card(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &super::TextBlock,
        cell: &TokenCell,
        x: f32,
        y: f32,
        alpha: f32,
    ) {
        let cw = layout::TOKEN_CARD_W;
        let rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + cw,
                bottom: y + layout::TOKEN_CARD_H,
            },
            radiusX: CARD_RADIUS,
            radiusY: CARD_RADIUS,
        };
        let edge = self.brush(target, self.theme.card_border, alpha);
        target.DrawRoundedRectangle(&rect, &edge, 1.0, None);
        // The number-unit combination is centered as a whole; the unit
        // hangs at the number's top-right corner as a superscript.
        let nx = x + (cw - cell.num.w - cell.unit.w - TOKEN_UNIT_GAP) / 2.0;
        self.draw_block(
            target,
            &cell.num,
            nx,
            y + layout::TOKEN_CARD_NUM_Y,
            self.theme.text_primary,
            alpha,
        );
        if cell.unit.w > 0.0 {
            self.draw_block(
                target,
                &cell.unit,
                nx + cell.num.w + TOKEN_UNIT_GAP,
                y + layout::TOKEN_CARD_NUM_Y,
                self.theme.text_secondary,
                alpha,
            );
        }
        self.draw_block_c(
            target,
            label,
            x + cw / 2.0,
            y + layout::TOKEN_CARD_LABEL_Y,
            self.theme.text_secondary,
            alpha,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    /// Pinned pairing of tiers with fallback day counts and log labels
    #[test]
    fn token_legs_pinned() {
        assert_eq!(TOKEN_LEGS[0], (TokenLeg::Today, 0, "today"));
        assert_eq!(TOKEN_LEGS[1], (TokenLeg::Week, 6, "week"));
        assert_eq!(TOKEN_LEGS[2], (TokenLeg::Month, 29, "month"));
    }

    /// Energy bar pending-fill ranges: data segments are kept as-is,
    /// uncovered cell positions get track segments, the head and tail
    /// are padded as needed, and segment order is the draw order
    #[test]
    fn mcp_ranges_cover_track_gaps() {
        // No data: the whole bar is track
        assert_eq!(mcp_job_ranges(&[]), vec![(0, MCP_CELLS)]);
        // A single segment fills everything: no track
        assert_eq!(mcp_job_ranges(&[(0, 20, C)]), vec![(0, 20)]);
        // Head gap
        assert_eq!(mcp_job_ranges(&[(5, 20, C)]), vec![(0, 5), (5, 20)]);
        // Mid gap; data segments join contiguously
        assert_eq!(
            mcp_job_ranges(&[(2, 6, C), (6, 10, C), (14, 20, C)]),
            vec![(0, 2), (2, 6), (6, 10), (10, 14), (14, 20)]
        );
    }

    /// Theme discriminator pins the cache: same snapshot time and width
    /// with a different theme must miss — appearance switches swap
    /// r.theme in place, and without the key the gap segments would keep
    /// the old track color until the next poll
    #[test]
    fn mcp_cache_theme_key() {
        let at = chrono::Local::now();
        let empty = || unsafe { crate::ui::panel::render::TextBlock::new(None) };
        let cache = McpCompCache {
            key: at,
            inner_w: 100.0,
            theme: [1.0, 1.0, 1.0, 1.0],
            ranges: Vec::new(),
            badges: Vec::new(),
            shown: 0,
            hidden: 0,
            plus: empty(),
        };
        assert!(cache.is_fresh(at, 100.0, [1.0, 1.0, 1.0, 1.0]));
        assert!(!cache.is_fresh(at, 100.0, [0.0, 0.0, 0.0, 1.0]));
    }

    /// Legend fitting pins: an over-wide first badge degrades to the
    /// compact form (shown = 0, hidden = every tool, "+N" draws);
    /// ordinary truncation keeps leading badges and reserves the +N tail
    #[test]
    fn mcp_legend_fit_pins() {
        // First badge alone over-width: compact form, every tool hidden
        let shown = fit_badges([500.0, 60.0].into_iter(), 100.0, 2, 20.0);
        assert_eq!((shown, 2usize.saturating_sub(shown)), (0, 2));
        // First fits, second + tail does not: truncates to the first
        assert_eq!(fit_badges([60.0, 60.0].into_iter(), 100.0, 2, 20.0), 1);
        // All fit, nothing hidden: no tail reservation on the last badge
        assert_eq!(fit_badges([60.0, 20.0].into_iter(), 200.0, 2, 20.0), 2);
    }
}
