//! Main view

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget,
};
use windows_numerics::Matrix3x2;

use super::marquee::{category_label, footer_error_tail, footer_key};
use super::widgets::PACMAN_R;
use super::{
    Align, ERROR_SCROLL_BRACKET_WEIGHT, ERROR_SCROLL_PAUSE, ERROR_SCROLL_SIZE, ErrorScroll, Hit,
    Renderer, ScrollBlock, belt_width,
};
use crate::ui::fmt;
use crate::ui::i18n::Strings;
use crate::ui::panel::layout;
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::theme::CARD_RADIUS;

/// Frame cache of poll-stable text in the main view: text is held as
/// prebuilt layout blocks, so zero layouts are created during the
/// marquee's 30fps full-window repaints; countdown rows change per
/// second and the footer per minute, each rebuilt independently. The
/// title cache is snapshot-independent and lives on the Renderer side.
pub(super) struct MainTexts {
    /// (snapshot time, language): any change rebuilds the whole cache —
    /// queried_at is a full-precision timestamp stamped on every parse
    /// and the cache dies with the snapshot, so one timestamp cannot
    /// carry two different bucket sets.
    key: (chrono::DateTime<chrono::Local>, crate::ui::i18n::Lang),
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
pub(super) struct RowTexts {
    pub(super) detail: Option<super::TextBlock>,
    pub(super) percent: super::TextBlock,
    pub(super) reset: Option<super::TextBlock>,
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
    pub(super) today: super::TextBlock,
    pub(super) week: super::TextBlock,
    pub(super) month: super::TextBlock,
    peak_badge: super::TextBlock,
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
        let balance = snap.balance.as_ref().map(|b| fmt::money(b.available));
        let balance = balance.map(|l| super::TextBlock::new(self.make_layout(&l, 12.0, 500, true)));
        let footer = fmt::updated_text(s, lang, snap.queried_at);
        let footer_block = self.make_footer_block(&footer);
        let mut texts = MainTexts {
            key: (snap.queried_at, lang),
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
        MainLabels {
            usage: self.make_text_block(s.usage_section, 12.0, 600),
            tokens: self.make_text_block(s.token_usage_section, 12.0, 600),
            balance: self.make_text_block(s.balance_label, 12.0, 600),
            five_hour: self.make_text_block(s.five_hour, 14.0, 400),
            weekly: self.make_text_block(s.weekly, 14.0, 400),
            mcp: self.make_text_block(s.mcp_tools, 14.0, 400),
            today: self.make_text_block(s.today_tokens, 10.0, 400),
            week: self.make_text_block(s.week_tokens, 10.0, 400),
            month: self.make_text_block(s.month_tokens, 10.0, 400),
            peak_badge: self.make_text_block(s.peak_badge, 12.0, 600),
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
            if texts
                .as_ref()
                .is_none_or(|t| t.key.0 != snap.queried_at || t.key.1 != model.lang)
            {
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
    pub(super) unsafe fn draw_block(
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
    pub(super) unsafe fn draw_block_r(
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
    pub(super) unsafe fn draw_block_c(
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
    pub(super) unsafe fn draw_block_nosnap(
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
}
