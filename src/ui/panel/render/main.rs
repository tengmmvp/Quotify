//! Main view

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget,
};
use windows_numerics::Matrix3x2;

use super::widgets::PACMAN_R;
use super::{
    Align, ERROR_SCROLL_BRACKET_WEIGHT, ERROR_SCROLL_PAUSE, ERROR_SCROLL_SIZE, ErrorScroll, Hit,
    Renderer, ScrollBlock, belt_width,
};
use crate::api::{ErrorCategory, ErrorDetail, ErrorInfo, FetchError, TOKEN_LEGS, TokenLeg};
use crate::ui::fmt;
use crate::ui::i18n::Strings;
use crate::ui::panel::layout;
use crate::ui::panel::model::PanelModel;
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

/// Legend badge geometry
const MCP_BADGE_SIZE: f32 = 11.0;
const MCP_SWATCH: f32 = 8.0;
const MCP_SWATCH_GAP: f32 = 3.0;
const MCP_BADGE_GAP: f32 = 6.0;

/// Legend badge: swatch color and the prebuilt label layout — the
/// over-width fallback layout is prebuilt alongside at cache build, so
/// every frame path draws blocks and never re-measures
struct McpBadge {
    color: [f32; 4],
    block: super::TextBlock,
}

/// Data-side artifacts of the MCP composition section, cached by
/// (snapshot time, inner width); a hit skips sorting, segmentation, and
/// all width measuring; drawing fills one rounded rect per cell
/// following the cached segments.
pub(super) struct McpCompCache {
    key: chrono::DateTime<chrono::Local>,
    inner_w: f32,
    /// Energy-bar fill sequence (gap-filled, colors already paired — data
    /// segments keep their color, gaps take the track color): the draw
    /// loop iterates straight through without rebuilding the range walk
    /// or re-searching colors per frame.
    ranges: Vec<(usize, usize, [f32; 4])>,
    badges: Vec<McpBadge>,
    shown: usize,
    /// Tools hidden from the legend: parse-side total_count minus the
    /// shown badges — covers both width-dropped badges and parse-side
    /// residency-truncated entries; the "+N" badge draws exactly when
    /// this is > 0.
    hidden: usize,
    /// Truncated first-badge label for the over-width fallback
    /// (shown == 0): the largest tool stays visible as ellipsized text
    fallback: super::TextBlock,
    plus: super::TextBlock,
}

/// Frame cache of poll-stable text in the main view: text is held as
/// prebuilt layout blocks, so zero layouts are created during the
/// marquee's 30fps full-window repaints; countdown rows change per
/// second and the footer per minute, each rebuilt independently. The
/// title cache is snapshot-independent and lives on the Renderer side.
pub(super) struct MainTexts {
    /// (snapshot time, language, bucket discriminants): any change
    /// rebuilds the whole cache. The discriminant sequence — not just a
    /// count — guards a future snapshot-reuse path that copies a
    /// timestamp with a different bucket set: same count with reordered
    /// or substituted buckets would otherwise index the cached rows into
    /// the wrong labels.
    key: (
        chrono::DateTime<chrono::Local>,
        crate::ui::i18n::Lang,
        Vec<u8>,
    ),
    /// Top bar subtitle (version · tier); None when absent
    meta: Option<super::TextBlock>,
    /// Metric row texts, in draw order (present bucket order)
    rows: Vec<RowTexts>,
    /// Balance row
    balance: Option<super::TextBlock>,
    /// Full footer string (same source as the static footer); the swap
    /// animation origin shares it via cached_footer
    pub(super) footer_text: String,
    /// Footer text block; the swap animation takes it via
    /// take_footer_block as the origin
    pub(super) footer: super::TextBlock,
    /// Cached second for countdown rows: each row's reset part is
    /// rebuilt only when the second advances
    resets_at: i64,
    /// Footer cache key: rebuilt only when the minute advances
    footer_at: i64,
}

/// Metric row texts: detail and percent are poll-stable; the countdown
/// advances per second
struct RowTexts {
    detail: Option<super::TextBlock>,
    percent: super::TextBlock,
    reset: Option<super::TextBlock>,
}

/// Bucket discriminant sequence of a snapshot: MainTexts' cache key
/// compares the ordered variant tags, not just a count — a same-count
/// substitution or reorder must rebuild rather than index rows into the
/// wrong labels.
fn bucket_tags(snap: &crate::api::UsageSnapshot) -> Vec<u8> {
    snap.present_buckets().map(bucket_tag).collect()
}

fn bucket_tag(b: crate::api::PresentBucket<'_>) -> u8 {
    match b {
        crate::api::PresentBucket::FiveHour(_) => 0,
        crate::api::PresentBucket::Weekly(_) => 1,
        crate::api::PresentBucket::Mcp(_) => 2,
    }
}

/// Ordered bucket-tag comparison without materializing: short-circuits
/// on the first mismatch, so cache-hit frames allocate nothing.
fn buckets_match(cached: &[u8], snap: &crate::api::UsageSnapshot) -> bool {
    let mut cached = cached.iter().copied();
    for b in snap.present_buckets() {
        if cached.next() != Some(bucket_tag(b)) {
            return false;
        }
    }
    cached.next().is_none()
}

/// Layout cache of the main view's constant labels: section titles,
/// bucket labels, Token card labels, and the peak badge copy; the content
/// only changes with language
pub(super) struct MainLabels {
    usage: super::TextBlock,
    tokens: super::TextBlock,
    balance: super::TextBlock,
    five_hour: super::TextBlock,
    weekly: super::TextBlock,
    mcp: super::TextBlock,
    today: super::TextBlock,
    week: super::TextBlock,
    month: super::TextBlock,
    peak_badge: super::TextBlock,
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
    /// Rebuild the main text cache: the poll-stable parts (subtitle, row
    /// details, percentages, balance) are fully prebuilt as layouts;
    /// countdown rows take their initial value for the current second.
    unsafe fn build_main_texts(
        &mut self,
        s: &Strings,
        snap: &crate::api::UsageSnapshot,
        lang: crate::ui::i18n::Lang,
        now_sec: i64,
        now_ms: i64,
    ) -> MainTexts {
        let v = snap.plan_version.label();
        let tier = snap.tier_label();
        let meta = match (v.is_empty(), tier.is_empty()) {
            (false, false) => Some(format!("{v} · {tier}")),
            (false, true) => Some(v.to_string()),
            (true, false) => Some(tier.to_string()),
            (true, true) => None,
        };
        let meta = meta.map(|m| super::TextBlock::new(self.make_layout(&m, 12.0, 400, false)));
        let used_of = |cur: f64, tot: f64| {
            s.used_of
                .replace("{cur}", &fmt::compact_number(cur))
                .replace("{tot}", &fmt::compact_number(tot))
        };
        let mut rows = Vec::new();
        for b in snap.present_buckets() {
            let (detail, percent) = match &b {
                crate::api::PresentBucket::FiveHour(b) | crate::api::PresentBucket::Weekly(b) => {
                    let detail = match (b.current, b.total) {
                        (Some(c), Some(t)) if t > 0.0 => Some(used_of(c, t)),
                        _ => None,
                    };
                    (detail, fmt::percent(b.used_percent))
                }
                crate::api::PresentBucket::Mcp(m) => (
                    (m.total > 0.0).then(|| used_of(m.current_value, m.total)),
                    fmt::percent(m.used_percent),
                ),
            };
            rows.push(RowTexts {
                detail: detail
                    .map(|d| super::TextBlock::new(self.make_layout(&d, 11.0, 400, false))),
                percent: super::TextBlock::new(self.make_layout(&percent, 14.0, 600, true)),
                reset: None,
            });
        }
        let balance = snap.balance.as_ref().map(|b| {
            let amount = if b.available.abs() >= 1e6 {
                fmt::compact_number(b.available)
            } else {
                format!("{:.2}", b.available)
            };
            format!("¥{amount}")
        });
        let balance = balance.map(|l| super::TextBlock::new(self.make_layout(&l, 12.0, 500, true)));
        let footer = fmt::updated_text(s, lang, snap.queried_at);
        let footer_block = self.make_footer_block(&footer);
        let mut texts = MainTexts {
            key: (snap.queried_at, lang, bucket_tags(snap)),
            meta,
            rows,
            balance,
            footer_text: footer,
            footer: footer_block,
            resets_at: now_sec,
            footer_at: footer_key(now_ms, snap.queried_at.timestamp_millis()),
        };
        self.refresh_resets(&mut texts.rows, s, snap, lang);
        texts
    }

    /// Countdown row rebuild: triggered when the second advances; only
    /// the reset layout of each row is rebuilt. Row order and row set
    /// share present_buckets, so build and rebuild stay aligned by
    /// construction.
    unsafe fn refresh_resets(
        &mut self,
        rows: &mut [RowTexts],
        s: &Strings,
        snap: &crate::api::UsageSnapshot,
        lang: crate::ui::i18n::Lang,
    ) {
        let line = |r: Option<chrono::DateTime<chrono::Utc>>| {
            r.map(|r| s.resets_line.replace("{t}", &fmt::countdown(r, lang)))
        };
        for (row, b) in rows.iter_mut().zip(snap.present_buckets()) {
            row.reset = line(b.resets_at())
                .map(|l| super::TextBlock::new(self.make_layout(&l, 11.0, 400, false)));
        }
    }

    /// Constant label layout set: the whole set is rebuilt on language
    /// change
    unsafe fn build_main_labels(&mut self, s: &Strings) -> MainLabels {
        let blk = |r: &Self, t: &str, size: f32, weight: u16| {
            super::TextBlock::new(r.make_layout(t, size, weight, false))
        };
        MainLabels {
            usage: blk(self, s.usage_section, 12.0, 600),
            tokens: blk(self, s.token_usage_section, 12.0, 600),
            balance: blk(self, s.balance_label, 12.0, 600),
            five_hour: blk(self, s.five_hour, 14.0, 400),
            weekly: blk(self, s.weekly, 14.0, 400),
            mcp: blk(self, s.mcp_tools, 14.0, 400),
            today: blk(self, s.today_tokens, 10.0, 400),
            week: blk(self, s.week_tokens, 10.0, 400),
            month: blk(self, s.month_tokens, 10.0, 400),
            peak_badge: blk(self, s.peak_badge, 12.0, 600),
        }
    }

    /// Main view: the top bar is always present; the body renders as
    /// three states: "no snapshot, error present, data present"
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn draw_main(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        w: f32,
        h: f32,
        content_h: f32,
        dy: f32,
        alpha: f32,
    ) {
        let s = model.strings;
        let pad = layout::CONTENT_PAD;
        let mut y = dy + layout::MAIN_TOP_PAD;
        let snap = model.snapshot;
        // Frame cache for main texts and constant labels (single exit,
        // put back at function end): zero layout creation during the
        // marquee's 30fps full-window repaints; countdown rows rebuild
        // independently per whole second.
        let mut texts = self.main_texts.take();
        let mut labels_opt = self.main_labels.take();
        if labels_opt.as_ref().is_none_or(|(l, _)| *l != model.lang) {
            labels_opt = Some((model.lang, self.build_main_labels(s)));
        }
        let labels = labels_opt
            .as_ref()
            .map(|(_, ls)| ls)
            .expect("labels 上方必已置");
        if let Some(snap) = snap {
            let now = chrono::Utc::now();
            let (now_sec, now_ms) = (now.timestamp(), now.timestamp_millis());
            if texts.as_ref().is_none_or(|t| {
                t.key.0 != snap.queried_at
                    || t.key.1 != model.lang
                    || !buckets_match(&t.key.2, snap)
            }) {
                texts = Some(self.build_main_texts(s, snap, model.lang, now_sec, now_ms));
            } else if let Some(t) = texts.as_mut() {
                if t.resets_at != now_sec {
                    t.resets_at = now_sec;
                    self.refresh_resets(&mut t.rows, s, snap, model.lang);
                }
                let fk = footer_key(now_ms, snap.queried_at.timestamp_millis());
                if t.footer_at != fk {
                    t.footer_at = fk;
                    let text = fmt::updated_text(s, model.lang, snap.queried_at);
                    t.footer = self.make_footer_block(&text);
                    t.footer_text = text;
                }
            }
        } else {
            // No snapshot means account switch or teardown: invalidate
            // the cache along with the previous account's subtitle; the
            // loading state and the error card must not retain the
            // previous account's content.
            texts = None;
        }
        // ── Top bar: account name + plan subtitle ──
        let meta = texts.as_ref().and_then(|t| t.meta.as_ref());
        y = self.topbar(target, model, meta, pad, w, y, alpha);

        // ── Body three states: empty/loading and the error card go to
        // empty_or_error_card; the data section is the _ arm ──
        match model.error {
            // No snapshot: configured = loading, unconfigured = onboarding
            // empty state.
            None if snap.is_none() => {
                self.empty_or_error_card(target, model, None, pad, w, y, dy, h, alpha)
            }
            // Error without data: two-line error card — the title carries
            // the category, the body carries the code and hint.
            Some(e) if snap.is_none() => {
                self.empty_or_error_card(target, model, Some(e), pad, w, y, dy, h, alpha)
            }
            // With data the data section is the body; when an error is
            // also present, stale data still renders normally and the
            // error is demoted to the footer.
            _ => {
                if let Some(snap) = snap {
                    let ty = self.section_header(target, &labels.usage, pad, y, w, alpha, false);
                    if crate::ui::peak::is_peak_now(model.peak_range) {
                        self.peak_badge(target, &labels.peak_badge, ty, w, alpha);
                    }
                    y = ty + layout::MAIN_MASTHEAD_ROW_H;

                    // Row texts come from the cache in draw order: row
                    // order and row set share present_buckets, so an
                    // out-of-bounds index exposes row-set drift between
                    // the two sides.
                    for (ri, b) in snap.present_buckets().enumerate() {
                        let row = &texts.as_ref().expect("数据分支 texts 必建").rows[ri];
                        // All text goes through MainTexts prebuilt
                        // layouts; row drawing is down to label dispatch;
                        // the MCP row's composition section is the sole
                        // bucket-specific addition.
                        let (label, used) = match &b {
                            crate::api::PresentBucket::FiveHour(b) => {
                                (&labels.five_hour, b.used_percent)
                            }
                            crate::api::PresentBucket::Weekly(b) => {
                                (&labels.weekly, b.used_percent)
                            }
                            crate::api::PresentBucket::Mcp(m) => (&labels.mcp, m.used_percent),
                        };
                        y = self.metric_row(target, label, row, used, y, w, alpha);
                        if let crate::api::PresentBucket::Mcp(m) = &b
                            // The composition section's display predicate
                            // has the single source has_mcp_details,
                            // shared with the height side.
                            && snap.has_mcp_details()
                        {
                            y = self.mcp_composition(target, m, snap.queried_at, y, w, alpha);
                        }
                    }

                    if let Some(ts) = &snap.token_stats {
                        y += layout::MAIN_SECTION_GAP;
                        let ty =
                            self.section_header(target, &labels.tokens, pad, y, w, alpha, true);
                        y = self.token_section(target, labels, ts, snap.queried_at, ty, alpha);
                    }

                    if let Some(t) = texts.as_ref().filter(|_| snap.balance.is_some()) {
                        y += layout::MAIN_SECTION_GAP;
                        let ty =
                            self.section_header(target, &labels.balance, pad, y, w, alpha, true);
                        if let Some(b) = t.balance.as_ref() {
                            self.draw_block_r(
                                target,
                                b,
                                w - pad,
                                ty,
                                self.theme.text_primary,
                                alpha,
                            );
                        }
                    }
                }
                // The footer is pinned to the bottom; when the viewport
                // is squeezed too short for the content, it pins to the
                // content bottom so it does not overlap the data section.
                let footer_y = dy + h.max(content_h) - layout::MAIN_FOOTER_H;
                // The error summary and the stale snapshot appear as a
                // pair: the prefix segment reports the data age; without
                // a snapshot no summary is shown.
                if let (Some(e), Some(snap)) = (model.error, snap) {
                    let avail = w - pad * 2.0;
                    let anim_on = self.animations_on();
                    // The cache key is a full-value tuple of the content
                    // (snapshot time first; identity across frames
                    // short-circuits): any change in the error payload,
                    // visible width, language, or animation toggle
                    // rebuilds it; on a hit there are zero measurements
                    // and zero allocations, with no intermediate Display
                    // strings. The hit test compares by reference; the
                    // owned clone happens only on the rebuild path — the
                    // marquee runs this at ~30fps.
                    let cached = self.anim.error_scroll.as_ref().is_some_and(|sc| {
                        sc.key.0 == snap.queried_at
                            && &sc.key.1 == e
                            && sc.key.2 == avail.to_bits()
                            && sc.key.3 == model.lang
                            && sc.key.4 == anim_on
                    });
                    if !cached {
                        let key = (
                            snap.queried_at,
                            e.clone(),
                            avail.to_bits(),
                            model.lang,
                            anim_on,
                        );
                        let head = s
                            .data_as_of
                            .replace("{t}", &snap.queried_at.format("%H:%M").to_string());
                        let tail = footer_error_tail(s, e);
                        let cat = category_label(s, e.category()).to_string();
                        // Layouts are prebuilt: scroll frames reuse them
                        // via direct drawing instead of rebuilding COM
                        // layouts per frame; widths are read from the
                        // layouts, and font size/weight share the same
                        // source as direct drawing so they are written in
                        // one place only. Block-boundary brackets are
                        // bold and content is regular: the three blocks
                        // share one pair of bracket layouts.
                        let size = ERROR_SCROLL_SIZE;
                        let bw = ERROR_SCROLL_BRACKET_WEIGHT;
                        let bracket = ScrollBlock::new(self.make_layout("[", size, bw, false));
                        let bracket_end = ScrollBlock::new(self.make_layout("] ", size, bw, false));
                        let mut blocks = [
                            ScrollBlock::new(self.make_layout(&head, size, 400, false)),
                            ScrollBlock::new(self.make_layout(&cat, size, 400, false)),
                            ScrollBlock::new(self.make_layout(&tail, size, 400, false)),
                        ];
                        let pair = bracket.w + bracket_end.w;
                        let over = belt_width(&blocks.each_ref().map(|b| b.w), pair) > avail;
                        let scrolling = over && anim_on;
                        if over && !anim_on {
                            // Over-width with animations disabled does
                            // not scroll: each block is ellipsized into
                            // a static string — the prefix and category
                            // keep the full string, the tail segment is
                            // truncated with an ellipsis; all go through
                            // the layout path, and widths reuse the
                            // values computed by truncation.
                            let rest0 = (avail - 3.0 * pair).max(0.0);
                            let (hd, hw) = self.ellipsize(&head, size, rest0, 400, false);
                            let rest = (rest0 - hw).max(0.0);
                            let (cd, cw) = self.ellipsize(&cat, size, rest, 400, false);
                            let rest = (rest - cw).max(0.0);
                            let (td, tw) = self.ellipsize(&tail, size, rest, 400, false);
                            // Hand-built instead of ScrollBlock::new: the
                            // width must be the truncation-budget value
                            // (the constructor would recompute it from the
                            // truncated layout and drift from the reserved
                            // space); h is unread — marquee consumers use
                            // .w only.
                            let trunc = |s: &str, w: f32| ScrollBlock {
                                layout: self.make_layout(s, size, 400, false),
                                w,
                                h: 0.0,
                            };
                            blocks = [trunc(&hd, hw), trunc(&cd, cw), trunc(&td, tw)];
                        }
                        self.anim.error_scroll = Some(ErrorScroll {
                            key,
                            blocks,
                            bracket,
                            bracket_end,
                            scrolling,
                            frozen: false,
                            offset: 0.0,
                            resume_at: std::time::Instant::now() + ERROR_SCROLL_PAUSE,
                            // last aligns with the resume point: the
                            // initial pause does not count as offset.
                            last: std::time::Instant::now() + ERROR_SCROLL_PAUSE,
                        });
                        // The wall clock is handled uniformly at WM_PAINT
                        // wrap-up: the outer layer of the render frame
                        // holds the App reference, and re-taking the
                        // reference would invalidate it.
                    }
                    // Summary clip region: left edge at pad, spanning the
                    // full width; the height is pinned to the
                    // section-height single source.
                    let clip = D2D_RECT_F {
                        left: pad,
                        top: footer_y,
                        right: pad + avail,
                        bottom: footer_y + layout::MAIN_FOOTER_H,
                    };
                    target.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                    // Take out the marquee state: drawing references the
                    // cached layouts and truncation artifacts directly
                    // throughout, with zero clones and zero allocations
                    // within the frame; single exit, put back at function
                    // end.
                    let mut sc_taken = self.anim.error_scroll.take();
                    if let Some(sc) = sc_taken.as_mut() {
                        // Direct-draw path (scrolling, static, and
                        // truncated share it): one brush of the same
                        // color and alpha is shared throughout.
                        let brush = self.brush(target, self.theme.text_secondary, alpha);
                        let mut draw_pair = |x: f32| {
                            let mut bx = x;
                            for b in sc.blocks.iter() {
                                // Empty blocks are skipped: they take
                                // part in neither advancement nor
                                // drawing, leaving no trailing blank in
                                // the period; blocks whose truncation
                                // budget ran out have zero width and are
                                // skipped the same way.
                                if b.w <= 0.0 {
                                    continue;
                                }
                                if let Some(l) = sc.bracket.layout.as_ref() {
                                    self.draw_layout(target, l, bx, footer_y + 6.0, &brush, false);
                                }
                                if let Some(l) = b.layout.as_ref() {
                                    self.draw_layout(
                                        target,
                                        l,
                                        bx + sc.bracket.w,
                                        footer_y + 6.0,
                                        &brush,
                                        false,
                                    );
                                }
                                let end_x = bx + sc.bracket.w + b.w;
                                if let Some(l) = sc.bracket_end.layout.as_ref() {
                                    self.draw_layout(
                                        target,
                                        l,
                                        end_x,
                                        footer_y + 6.0,
                                        &brush,
                                        false,
                                    );
                                }
                                bx = end_x + sc.bracket_end.w;
                            }
                        };
                        if sc.scrolling {
                            // Conveyor tiling: two copies of the text
                            // join at the period, exiting left and
                            // entering right in a seamless loop.
                            let period = sc.total_w();
                            let mut x = pad - sc.offset;
                            while x < pad + avail {
                                draw_pair(x);
                                x += period;
                            }
                        } else {
                            // Not over-width, or settled after
                            // truncation: widths are already cached;
                            // draw directly.
                            draw_pair(pad);
                        }
                    }
                    self.anim.error_scroll = sc_taken;
                    target.PopAxisAlignedClip();
                } else if self
                    .anim
                    .footer
                    .as_ref()
                    .is_some_and(|f| !f.tween.finished())
                {
                    // Pac-Man swap: take out the animation state; the
                    // draw section's &mut self calls conflict with the
                    // borrow; single exit, put back at the end of this
                    // section.
                    let mut fa_taken = self.anim.footer.take();
                    if let Some(fa) = fa_taken.as_mut() {
                        let (old_w, new_w) = (fa.old.w, fa.new.w);
                        let tween = fa.tween;
                        let old_home = w / 2.0 - old_w / 2.0;
                        let new_home = w / 2.0 - new_w / 2.0;
                        let raw = tween.progress();
                        let travel = w + 2.0 * PACMAN_R + 8.0;
                        let start = -PACMAN_R - 4.0;
                        let eat_end = old_home + old_w + PACMAN_R;
                        let frac_eat = ((eat_end - start) / travel).clamp(0.3, 0.85);
                        let p = if raw < 0.72 {
                            (raw / 0.72) * frac_eat
                        } else {
                            let t = (raw - 0.72) / 0.28;
                            frac_eat + t * t * (1.0 - frac_eat)
                        };
                        let cy = footer_y + 16.5;
                        let cx = start + p * travel;
                        let clip = D2D_RECT_F {
                            left: cx,
                            top: footer_y,
                            right: w,
                            bottom: footer_y + layout::MAIN_FOOTER_H,
                        };
                        unsafe {
                            target.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                        }
                        self.draw_block(
                            target,
                            &fa.old,
                            old_home,
                            footer_y + 8.0,
                            self.theme.text_tertiary,
                            alpha,
                        );
                        unsafe {
                            target.PopAxisAlignedClip();
                        }
                        self.pacman_at(
                            target,
                            cx,
                            cy,
                            super::widgets::PACMAN_R,
                            raw,
                            self.theme.text_tertiary,
                            alpha,
                        );
                        let bite = raw * 8.0;
                        let (phase, idx) = (bite.fract(), bite.floor() as u32);
                        let age = (phase - 0.8 + 1.0) % 1.0;
                        if p < frac_eat && age < 0.5 {
                            let t = age / 0.5;
                            let fall = t * t * 14.0;
                            // Alpha is quantized to 32 levels: levels
                            // are reused across rounds, so the brush
                            // cache does not grow with the animation.
                            let a = (alpha * (1.0 - t * t) * 32.0).round() / 32.0;
                            let cb = self.brush(target, self.theme.text_tertiary, a);
                            let born = if phase >= 0.8 {
                                idx
                            } else {
                                idx.saturating_sub(1)
                            };
                            let mid = ((born * 31) % 5) as f32 - 2.0;
                            for (base, dir) in
                                [(mid, 0.0f32), (mid - 3.0, -1.0f32), (mid + 3.5, 1.0f32)]
                            {
                                let dx = base + dir * t * 5.0;
                                let cr = D2D_RECT_F {
                                    left: cx + PACMAN_R * 0.45 + dx,
                                    top: cy + 3.0 + fall,
                                    right: cx + PACMAN_R * 0.45 + dx + 4.2,
                                    bottom: cy + 3.0 + fall + 4.2,
                                };
                                target.FillRectangle(&cr, &cb);
                            }
                        }
                        let nx = (cx - PACMAN_R - new_w - 10.0).min(new_home);
                        self.draw_block(
                            target,
                            &fa.new,
                            nx,
                            footer_y + 8.0,
                            self.theme.text_tertiary,
                            alpha,
                        );
                    }
                    self.anim.footer = fa_taken;
                } else if model.snapshot.is_some() {
                    // The footer goes through MainTexts' minute-granularity
                    // cache: marquee and animation frames cause zero
                    // rebuilds and zero layout creation.
                    if let Some(t) = texts.as_ref() {
                        self.draw_block_c(
                            target,
                            &t.footer,
                            w / 2.0,
                            footer_y + 8.0,
                            self.theme.text_tertiary,
                            alpha,
                        );
                    }
                }
            }
        }
        // Frame caches are put back unconditionally: single exit; the
        // snapshot-absent arm returns them as-is.
        self.main_texts = texts;
        self.main_labels = labels_opt;
    }

    /// Top bar: logo, account name and plan subtitle, switch chevron,
    /// and the refresh and settings buttons. Returns the top bar's
    /// bottom y; the title truncation cache key is (source string,
    /// available width), with zero builds during marquee repaints
    #[allow(clippy::too_many_arguments)]
    unsafe fn topbar(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        meta: Option<&super::TextBlock>,
        pad: f32,
        w: f32,
        y: f32,
        alpha: f32,
    ) -> f32 {
        let title = model.account.map(|a| a.name).unwrap_or("Quotify");
        let (logo_size, logo_y) = (38.0, y + 7.0);
        self.logo(target, pad, logo_y, logo_size, alpha);
        let tx = pad + logo_size + 10.0;
        let chevron_w = if model.accounts_count > 1 { 18.0 } else { 0.0 };
        let tw = w - tx - 88.0 - chevron_w;
        // The title truncation cache is snapshot-independent: the key is
        // (source string, available width) and the value is a prebuilt
        // layout; zero text builds and zero layout creation during
        // marquee repaints; single exit, put back at function end.
        let mut title_cache = self.title_cache.take();
        if title_cache
            .as_ref()
            .is_none_or(|(k, _)| k.0 != title || k.1 != tw)
        {
            let (d, _) = self.ellipsize(title, 16.0, tw, 500, false);
            let tb = super::TextBlock::new(self.make_layout(&d, 16.0, 500, false));
            title_cache = Some(((title.to_string(), tw), tb));
        }
        let title_block = &title_cache.as_ref().expect("title_cache 上方必已置").1;
        let block_h = if meta.is_some() { 39.0 } else { 22.0 };
        let block_top = y + 26.0 - block_h / 2.0;
        self.draw_block(
            target,
            title_block,
            tx,
            block_top,
            self.theme.text_primary,
            alpha,
        );
        if model.accounts_count > 1 {
            // The chevron sits right after the title tail: the position
            // uses the layout block's current width directly, with no
            // re-measure.
            let ax = tx + title_block.w + 6.0;
            self.chevron(
                target,
                ax,
                y + 26.0,
                self.theme.text_secondary,
                alpha,
                model.popup_open,
            );
            self.hits.push((
                Hit::AccountSwitch,
                D2D_RECT_F {
                    left: ax - 10.0,
                    top: y + 12.0,
                    right: ax + 16.0,
                    bottom: y + 40.0,
                },
            ));
        }
        if let Some(m) = meta {
            // The subtitle block comes from an unbounded prebuilt layout:
            // an unrecognized plan label can run free-form long, so clip to
            // the title's width budget — it must never overdraw the chevron
            // or the buttons. Real copy is far shorter and never clipped.
            let clip = D2D_RECT_F {
                left: tx,
                top: block_top + 22.0,
                right: tx + tw,
                bottom: block_top + 22.0 + m.h.max(14.0),
            };
            target.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.draw_block(
                target,
                m,
                tx + 1.0,
                block_top + 22.0,
                self.theme.text_secondary,
                alpha,
            );
            target.PopAxisAlignedClip();
        }
        let btn_r = 16.0;
        let refresh_cx = w - pad - btn_r - 30.0;
        let settings_cx = w - pad - btn_r;
        let btn_cy = y + 26.0;
        self.icon_button(target, Hit::Refresh, refresh_cx, btn_cy, self.anim.spin);
        self.sliders(target, Hit::Settings, settings_cx, btn_cy, btn_r);
        self.title_cache = title_cache;
        y + layout::MAIN_TOPBAR_H
    }

    /// Empty state and error card: the body when there is no snapshot.
    /// The configured-loading / unconfigured-onboarding empty states
    /// render centered; errors use the two-line card (title carries the
    /// category, body carries the code and hint).
    #[allow(clippy::too_many_arguments)]
    unsafe fn empty_or_error_card(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        error: Option<&crate::api::FetchError>,
        pad: f32,
        w: f32,
        y: f32,
        dy: f32,
        h: f32,
        alpha: f32,
    ) {
        // The strings reference comes through model — the same source the
        // rest of the body reads.
        let s = model.strings;
        match error {
            // No snapshot: configured = loading, unconfigured = onboarding
            // empty state.
            None => {
                let configured = model.account.is_some();
                let body_top = y + 6.0;
                let body_h = (dy + h - 16.0) - body_top;
                let cx = w / 2.0;
                let center_y = body_top + body_h / 2.0;
                let (t1, t2) = if configured {
                    (s.loading, "")
                } else {
                    (s.not_configured_title, s.not_configured_hint)
                };
                let has_sub = !t2.is_empty();
                let block_h = 48.0 + 18.0 + 28.0 + if has_sub { 44.0 } else { 0.0 };
                let top = center_y - block_h / 2.0;
                self.logo(target, cx - 24.0, top, 48.0, alpha);
                let title_rect = D2D_RECT_F {
                    left: pad,
                    top: top + 66.0,
                    right: w - pad,
                    bottom: top + 66.0 + 28.0,
                };
                self.text_aligned(
                    target,
                    t1,
                    &title_rect,
                    21.0,
                    400,
                    self.theme.text_primary,
                    alpha,
                    Align::Center,
                    false,
                );
                if has_sub {
                    let sub_rect = D2D_RECT_F {
                        left: pad + 12.0,
                        top: top + 96.0,
                        right: w - pad - 12.0,
                        bottom: top + 96.0 + 40.0,
                    };
                    self.text_wrapped(
                        target,
                        t2,
                        &sub_rect,
                        14.0,
                        400,
                        self.theme.text_secondary,
                        alpha,
                        Align::Center,
                        false,
                    );
                }
            }
            // Error without data: two-line error card — the title carries
            // the category, the body carries the code and hint.
            Some(e) => {
                let title = category_label(s, e.category());
                let body = self.cached_error_body(s, e, model.lang);
                let body_top = y + 10.0;
                let body_h = (dy + h - 16.0) - body_top;
                let has_body = !body.is_empty();
                let card_h = if has_body { 94.0 } else { 68.0 };
                let card_top = body_top + (body_h - card_h) / 2.0;
                let card = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: pad,
                        top: card_top,
                        right: w - pad,
                        bottom: card_top + card_h,
                    },
                    radiusX: CARD_RADIUS,
                    radiusY: CARD_RADIUS,
                };
                let fill = self.brush(
                    target,
                    [
                        self.theme.danger[0],
                        self.theme.danger[1],
                        self.theme.danger[2],
                        0.06,
                    ],
                    alpha,
                );
                target.FillRoundedRectangle(&card, &fill);
                let edge = self.brush(
                    target,
                    [
                        self.theme.danger[0],
                        self.theme.danger[1],
                        self.theme.danger[2],
                        0.35,
                    ],
                    alpha,
                );
                target.DrawRoundedRectangle(&card, &edge, 1.0, None);
                self.text(
                    target,
                    title,
                    pad + 12.0,
                    card_top + 14.0,
                    w - pad * 2.0 - 24.0,
                    22.0,
                    13.0,
                    700,
                    self.theme.text_primary,
                    alpha,
                );
                if has_body {
                    let body_rect = D2D_RECT_F {
                        left: pad + 12.0,
                        top: card_top + 38.0,
                        right: w - pad - 12.0,
                        bottom: card_top + 78.0,
                    };
                    // Word-wrap up to two lines: single-line copy still
                    // takes one line; for long text the end of the
                    // second line is clipped by the rect.
                    self.text_wrapped(
                        target,
                        &body,
                        &body_rect,
                        12.0,
                        400,
                        self.theme.text_secondary,
                        alpha,
                        Align::Left,
                        false,
                    );
                }
            }
        }
    }

    /// Data-section header: divider + accent bar + prebuilt-layout
    /// title; returns the title row's top y. Advancement within the
    /// section after it (usage goes through the masthead row height
    /// after stacking the peak badge, Token continues with the cards,
    /// balance is just a text row) is decided by the call site. The
    /// masthead uses a solid line hung 2px below the section start;
    /// data sections use a dashed line flush with the section start
    #[allow(clippy::too_many_arguments)]
    unsafe fn section_header(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &super::TextBlock,
        pad: f32,
        y: f32,
        w: f32,
        alpha: f32,
        dashed: bool,
    ) -> f32 {
        if dashed {
            self.dashed_divider(target, pad, y, w - pad * 2.0, alpha);
        } else {
            self.divider(target, pad, y + 2.0, w - pad * 2.0, alpha);
        }
        // Both section kinds place the title 12 from the section start:
        // for the masthead it is measured from the section start, for
        // data sections from the divider.
        let ty = y + if dashed {
            layout::MAIN_SECTION_HEAD
        } else {
            layout::MAIN_MASTHEAD_RULE_GAP
        };
        self.accent_label(target, label, pad, ty, alpha);
        ty
    }

    /// Accent bar + directly drawn prebuilt-layout title; the row geometry
    /// (inset, height) comes from widgets' ACCENT_INSET/ACCENT_ROW_H
    /// single source, shared with settings' section_label, so header
    /// tweaks cannot drift between the two views.
    unsafe fn accent_label(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &super::TextBlock,
        x: f32,
        y: f32,
        alpha: f32,
    ) {
        self.fill_accent_bar(target, x, y, alpha);
        if let Some(l) = label.layout.as_ref() {
            let brush = self.brush(target, self.theme.text_tertiary, alpha);
            let cy = y + (super::widgets::ACCENT_ROW_H - label.h) / 2.0;
            self.draw_layout(
                target,
                l,
                x + super::widgets::ACCENT_INSET,
                cy,
                &brush,
                false,
            );
        }
    }

    /// Left-aligned direct drawing of a prebuilt layout
    unsafe fn draw_block(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        b: &super::TextBlock,
        x: f32,
        y: f32,
        color: [f32; 4],
        alpha: f32,
    ) {
        self.draw_block_at(target, b, x, y, color, alpha, false);
    }

    /// Right-aligned direct drawing of a prebuilt layout; x is the right
    /// edge
    unsafe fn draw_block_r(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        b: &super::TextBlock,
        right: f32,
        y: f32,
        color: [f32; 4],
        alpha: f32,
    ) {
        self.draw_block_at(target, b, right - b.w, y, color, alpha, false);
    }

    /// Centered direct drawing of a prebuilt layout; x is the center
    unsafe fn draw_block_c(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        b: &super::TextBlock,
        cx: f32,
        y: f32,
        color: [f32; 4],
        alpha: f32,
    ) {
        self.draw_block_at(target, b, cx - b.w / 2.0, y, color, alpha, false);
    }

    /// Left-aligned snap-disabled direct drawing of a prebuilt layout:
    /// used by chain-advanced badges, whose spacing would be skewed by
    /// snapping offsets
    unsafe fn draw_block_nosnap(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        b: &super::TextBlock,
        x: f32,
        y: f32,
        color: [f32; 4],
        alpha: f32,
    ) {
        self.draw_block_at(target, b, x, y, color, alpha, true);
    }

    /// Metric row: label + percentage + progress bar, with the countdown
    /// on the left and the used detail on the right of the bottom line;
    /// all text goes through MainTexts prebuilt layouts and only drawing
    /// happens here. Returns the next row's y
    #[allow(clippy::too_many_arguments)]
    unsafe fn metric_row(
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
    /// badge has its name truncated. Data only changes with polls;
    /// segmentation, badges, and fitting results are cached by (snapshot
    /// time, inner width) and not recomputed on animation frames.
    /// Returns the next row's y
    unsafe fn mcp_composition(
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

        // Data-side artifacts are cached by (snapshot time, inner width):
        // a hit skips sorting, segmentation, and all width measuring;
        // width is part of the key because fitting results change with
        // panel width (across-DPI monitors). take() moves ownership out
        // so the draw section can freely call &mut self drawing methods;
        // this function has a single exit, put back at function end.
        let mut cache = self.mcp_cache.take();
        if cache
            .as_ref()
            .is_none_or(|c| c.key != queried_at || c.inner_w != inner_w)
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
            // The first badge is already over-width: the prebuilt
            // truncated fallback keeps the largest tool visible; drawing
            // is block-only, no per-frame measuring.
            let b = &badges[0];
            let sw = self.brush(target, b.color, alpha);
            target.FillRoundedRectangle(&swatch(x), &sw);
            self.draw_block_nosnap(
                target,
                &c.fallback,
                x + MCP_SWATCH + MCP_SWATCH_GAP,
                text_y(c.fallback.h),
                self.theme.text_secondary,
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
        // is width-driven: after the current badge fits, continuing
        // requires the remaining width to also hold the inter-badge gap
        // + "+N"; the trailing summary count is the number of remaining
        // tools, taken from the parse-side total_count so entries the
        // residency cap already dropped count as hidden too.
        // The raw strings live only for this build pass (the over-width
        // fallback layout is built right after fitting); the cache holds
        // blocks alone, so no frame path re-measures.
        let hidden_after = |shown: usize| m.total_count.saturating_sub(shown);
        let mut texts: Vec<String> = Vec::with_capacity(items.len());
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
                texts.push(text);
                McpBadge {
                    color: MCP_PALETTE[i.min(MCP_PALETTE.len() - 1)],
                    block,
                }
            })
            .collect();
        let badge_w = |b: &McpBadge| MCP_SWATCH + MCP_SWATCH_GAP + b.block.w;
        let mut shown = 0usize;
        let mut cur = 0.0f32;
        for (i, b) in badges.iter().enumerate() {
            let add = if i == 0 { 0.0 } else { MCP_BADGE_GAP } + badge_w(b);
            let tail = if hidden_after(i + 1) > 0 {
                MCP_BADGE_GAP
                    + self.measure(
                        &format!("+{}", hidden_after(i + 1)),
                        MCP_BADGE_SIZE,
                        400,
                        true,
                    )
            } else {
                0.0
            };
            if cur + add + tail > inner_w {
                break;
            }
            cur += add;
            shown = i + 1;
        }
        let hidden = hidden_after(shown);
        let plus = if hidden > 0 {
            let t = format!("+{hidden}");
            super::TextBlock::new(self.make_layout(&t, MCP_BADGE_SIZE, 400, true))
        } else {
            super::TextBlock::new(None)
        };
        let fallback = if shown == 0 && !texts.is_empty() {
            let avail = inner_w - MCP_SWATCH - MCP_SWATCH_GAP;
            let (t, _) = self.ellipsize(&texts[0], MCP_BADGE_SIZE, avail, 400, true);
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
            ranges,
            badges,
            shown,
            hidden,
            fallback,
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
            // means bold.
            let nw = self.measure(&num, TOKEN_NUM_SIZE, TOKEN_NUM_WEIGHT, true);
            let uw = if unit.is_empty() {
                0.0
            } else {
                self.measure_static(unit, TOKEN_UNIT_SIZE, 400, false)
            };
            // Absurdly large values can exceed the card width: truncate
            // into the width rather than crossing the stroke and
            // crowding the neighboring card.
            let num = if nw + uw + TOKEN_UNIT_GAP > avail {
                self.ellipsize(
                    &num,
                    TOKEN_NUM_SIZE,
                    avail - uw - TOKEN_UNIT_GAP,
                    TOKEN_NUM_WEIGHT,
                    true,
                )
                .0
            } else {
                num
            };
            TokenCell {
                num: super::TextBlock::new(self.make_layout(
                    &num,
                    TOKEN_NUM_SIZE,
                    TOKEN_NUM_WEIGHT,
                    true,
                )),
                unit: super::TextBlock::new(self.make_layout(unit, TOKEN_UNIT_SIZE, 400, false)),
            }
        });
        TokenCells {
            key: queried_at,
            cells,
        }
    }

    /// Three Token cards section: the typography cache hits by snapshot;
    /// rebuilds complete in one pass; single exit — the cache taken out
    /// must be put back at function end. Returns the section's bottom y
    unsafe fn token_section(
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

    /// "Peak" badge: bolt + text on the right of the title row; the label
    /// is a MainLabels block, so peak-hour frames create zero layouts;
    /// hover hit registration records UsageInfo
    unsafe fn peak_badge(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &super::TextBlock,
        title_y: f32,
        w: f32,
        alpha: f32,
    ) {
        let pad = layout::CONTENT_PAD;
        let bh = 14.0;
        let bw = bh * (7.0 / 13.0);
        let badge_w = bw + 4.0 + label.w;
        let bx = w - pad - badge_w;
        if self.bolt_geo.is_none() {
            self.bolt_geo = self.build_bolt_glyph();
        }
        if let Some(geo) = self.bolt_geo.clone() {
            let b = self.brush(target, self.theme.peak, alpha);
            let m = Matrix3x2 {
                M11: bh / 13.0,
                M12: 0.0,
                M21: 0.0,
                M22: bh / 13.0,
                M31: bx,
                M32: title_y + 1.0,
            };
            target.SetTransform(&m);
            target.FillGeometry(&geo, &b, None);
            target.SetTransform(&Matrix3x2::identity());
        }
        let tx = bx + bw + 4.0;
        self.draw_block_at(target, label, tx, title_y, self.theme.peak, alpha, false);
        let hit_w = badge_w;
        self.hits.push((
            Hit::UsageInfo,
            D2D_RECT_F {
                left: bx - 3.0,
                top: title_y - 3.0,
                right: bx + hit_w + 3.0,
                bottom: title_y + 18.0,
            },
        ));
        if self.hover == Some(Hit::UsageInfo) {
            self.pending_tip = Some((bx, title_y + 22.0, w));
        }
    }

    /// Peak/off-peak hover tip card
    pub(super) unsafe fn tip_card(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
        tip: &str,
    ) {
        let pad = layout::CONTENT_PAD;
        let cw = (w - pad * 2.0).min(300.0);
        let cx = x.min(pad + w - pad - cw).max(pad);
        let card = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: cx,
                top: y,
                right: cx + cw,
                bottom: y + 48.0,
            },
            radiusX: CARD_RADIUS,
            radiusY: CARD_RADIUS,
        };
        let [r, g, b, _] = self.theme.bg;
        let fill = self.brush(target, [r, g, b, 1.0], 1.0);
        target.FillRoundedRectangle(&card, &fill);
        let line = self.brush(target, self.theme.card_border, alpha);
        target.DrawRoundedRectangle(&card, &line, 1.0, None);
        self.text_wrapped(
            target,
            tip,
            &D2D_RECT_F {
                left: cx + 10.0,
                top: y + 8.0,
                right: cx + cw - 10.0,
                bottom: y + 40.0,
            },
            11.0,
            400,
            self.theme.text_secondary,
            alpha,
            Align::Left,
            false,
        );
    }

    /// Error card body (cached): the key is a full-value pair of error
    /// content and language, so marquee frames rebuild nothing; leaving
    /// out the language would keep stale copy after a language switch.
    /// An Rc refcount clone replaces a full deep copy of the string —
    /// hit registration still needs &mut self mid-drawing, so it cannot
    /// be borrowed out.
    fn cached_error_body(
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
fn category_label(s: &Strings, c: ErrorCategory) -> &'static str {
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
fn footer_key(now_ms: i64, at_ms: i64) -> i64 {
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
fn footer_error_tail(s: &Strings, e: &FetchError) -> String {
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
    use super::*;

    const C: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

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
}
