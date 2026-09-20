//! About window view

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{D2D1_ELLIPSE, D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget};
use windows_numerics::Vector2;

use super::{Hit, Renderer, TextBlock};
use crate::service::whatsnew::{NEWS_MAX, NewsItem};
use crate::ui::panel::LogPx;
use crate::ui::panel::model::PanelModel;

/// About window logical width
pub const ABOUT_W: f32 = 500.0;

/// Repository home page
pub const REPO_URL: &str = "https://github.com/tengmmvp/Quotify";
/// Repository issue tracker
pub const ISSUES_URL: &str = "https://github.com/tengmmvp/Quotify/issues";

/// Folded item block height: title row 18 + summary row 15 + bottom gap 7
const NEWS_FOLD_H: f32 = 40.0;
/// Item spacing
const NEWS_GAP: f32 = 6.0;
/// Expanded item body line height
const NEWS_LINE_H: f32 = 16.0;
/// News section's first-item origin (relative to the window top); sits
/// after the fixed areas such as the description and the three links
const NEWS_TOP: f32 = 204.0;
/// Timeline column positions: axis line and nodes, dates, titles and
/// bodies
const NEWS_AXIS_X: f32 = 30.0;
/// Date column origin
const NEWS_DATE_X: f32 = 40.0;
/// Title and body column origin
const NEWS_BODY_X: f32 = 78.0;

/// Rebuild key of the about text cache: language, logical width (by
/// bits — f32 semantic equality is bitwise), the news set, the expanded
/// item, and the read marker (it sizes the title truncation budget via
/// the NEW badge). The version string is compile-time constant, so it
/// needs no component. The hit path compares field-wise against
/// borrowed frame values; the owned clone happens only on rebuild.
type AboutKey = (
    crate::ui::i18n::Lang,
    u32,
    Option<Vec<NewsItem>>,
    Option<usize>,
    Option<String>,
);

/// One news item's text artifacts: date and title blocks plus the body
/// blocks matching the key's expanded state (folded = first line
/// truncated, expanded = all lines)
struct AboutItem {
    date: TextBlock,
    title: TextBlock,
    body: Vec<TextBlock>,
}

/// Frame cache of the about view's text: constant strings (name,
/// version, description, quote, links, section header, NEW badge) are
/// held as prebuilt layouts and news artifacts are ellipsized once per
/// key, so the easter-egg animation and hover repaints create no layout
pub(super) struct AboutTexts {
    key: AboutKey,
    name: TextBlock,
    ver: TextBlock,
    desc: TextBlock,
    quote: TextBlock,
    repo: TextBlock,
    issues: TextBlock,
    diag: TextBlock,
    section: TextBlock,
    new_badge: TextBlock,
    items: Vec<AboutItem>,
}

/// About window logical height; the base layout without news, with
/// news items accumulating one by one. The values share the same source
/// as draw_about's y advancement chain; layout changes must update both
pub fn about_height(news: Option<&[NewsItem]>, expanded: Option<usize>) -> LogPx {
    let Some(news) = news.filter(|n| !n.is_empty()) else {
        return LogPx(200); // No-news tier: link row bottom 176 + bottom margin 24
    };
    let mut h = NEWS_TOP;
    for (i, item) in news.iter().take(NEWS_MAX).enumerate() {
        h += item_h(item, expanded == Some(i)) + NEWS_GAP;
    }
    // Item section tail gap + bottom margin
    LogPx((h + 4.0 + 24.0).round() as i32)
}

/// Display block height of a single news item
fn item_h(item: &NewsItem, expanded: bool) -> f32 {
    if expanded {
        // Title row 18 + body lines + bottom gap 10
        18.0 + item.lines.len() as f32 * NEWS_LINE_H + 10.0
    } else {
        NEWS_FOLD_H
    }
}

impl Renderer {
    /// One about-window frame; skeleton same as the panel paint;
    /// returning false means the device is lost and the caller must
    /// drop the whole Renderer.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_about(
        &mut self,
        hwnd: windows::Win32::Foundation::HWND,
        rect_phys: &RECT,
        model: &PanelModel,
        expanded: Option<usize>,
        egg: Option<f32>,
        egg_eaten: bool,
        dpi: f32,
    ) -> bool {
        unsafe {
            self.paint_frame(hwnd, rect_phys, dpi, "关于窗", |s, t, w, h| {
                s.draw_about(t, model, expanded, egg, egg_eaten, w, h)
            })
        }
    }

    /// Header info and the three links form the fixed areas; the latest
    /// news section is appended at the end
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_about(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        expanded: Option<usize>,
        egg: Option<f32>,
        egg_eaten: bool,
        w: f32,
        h: f32,
    ) {
        let s = model.strings;

        let (dy, alpha) = self.appear_offset();
        let bg = self.brush(target, self.theme.bg, 1.0);
        target.FillRectangle(
            &D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: w,
                bottom: h,
            },
            &bg,
        );

        // Frame cache for the about texts (single exit, put back at
        // function end): content changes only with the key, so the
        // easter-egg animation and hover repaints create no layout.
        // The hit check compares field-wise against borrowed frame
        // values; the owned clone happens only on the rebuild path.
        let news = model.news.filter(|n| !n.is_empty());
        let mut texts = self.about_texts.take();
        if texts.as_ref().is_none_or(|t| {
            t.key.0 != model.lang
                || t.key.1 != w.to_bits()
                || t.key.2.as_deref() != news
                || t.key.3 != expanded
                || t.key.4.as_deref() != model.last_news_read
        }) {
            texts = Some(self.build_about_texts(
                s,
                model.lang,
                news,
                expanded,
                model.last_news_read,
                w,
            ));
        }
        let t = texts.as_ref().expect("about cache armed above");

        let (name_y, ver_y, desc_y, link_y) = if news.is_some() {
            (78.0, 108.0, 128.0, 150.0)
        } else {
            (84.0, 114.0, 134.0, 158.0)
        };

        // ── Header: logo + app name + version ──
        self.logo(target, w / 2.0 - 22.0, dy + 24.0, 44.0, alpha);
        self.hits.push((
            Hit::AboutLogo,
            D2D_RECT_F {
                left: w / 2.0 - 26.0,
                top: dy + 20.0,
                right: w / 2.0 + 26.0,
                bottom: dy + 72.0,
            },
        ));
        let name_rect = D2D_RECT_F {
            left: 0.0,
            top: dy + name_y,
            right: w,
            bottom: dy + name_y + 26.0,
        };
        self.block_c(target, &t.name, &name_rect, self.theme.text_primary, alpha);
        let eating = egg.filter(|_| !egg_eaten);
        // Total distance = constant-speed travel over the sum of both
        // lines' lengths; at the hand-off between lines it teleports to
        // the next line's left end. The widths come from the cached
        // blocks, equal to the old per-frame measures by construction.
        let (ver_eat, desc_eat) = match eating {
            Some(p) => {
                let ver_full = t.ver.w + 12.0;
                let desc_full = t.desc.w + 12.0;
                let d = p * (ver_full + desc_full);
                if d < ver_full {
                    (Some(d / ver_full), 0.0)
                } else {
                    (None, (d - ver_full) / desc_full)
                }
            }
            None => (None, 0.0),
        };
        match ver_eat {
            Some(q) => {
                self.egg_eat_line(
                    target,
                    w,
                    &t.ver,
                    dy + ver_y,
                    self.theme.text_tertiary,
                    q,
                    egg.unwrap_or(0.0),
                    alpha,
                );
            }
            None if eating.is_some() || egg_eaten => {}
            None => {
                let ver_rect = D2D_RECT_F {
                    left: 0.0,
                    top: dy + ver_y,
                    right: w,
                    bottom: dy + ver_y + 16.0,
                };
                self.block_c(target, &t.ver, &ver_rect, self.theme.text_tertiary, alpha);
            }
        }

        // ── Project description ──
        let desc_rect = D2D_RECT_F {
            left: 12.0,
            top: dy + desc_y,
            right: w - 12.0,
            bottom: dy + desc_y + 16.0,
        };
        match eating {
            Some(_) if ver_eat.is_some() => {
                self.block_c(
                    target,
                    &t.desc,
                    &desc_rect,
                    self.theme.text_secondary,
                    alpha,
                );
            }
            Some(p) => {
                self.egg_eat_line(
                    target,
                    w,
                    &t.desc,
                    dy + desc_y,
                    self.theme.text_secondary,
                    desc_eat,
                    p,
                    alpha,
                );
            }
            None if !egg_eaten => {
                self.block_c(
                    target,
                    &t.desc,
                    &desc_rect,
                    self.theme.text_secondary,
                    alpha,
                );
            }
            None => {
                let mid_y = (ver_y + desc_y + 16.0) / 2.0;
                let quote_rect = D2D_RECT_F {
                    left: 0.0,
                    top: dy + mid_y,
                    right: w,
                    bottom: dy + mid_y + 16.0,
                };
                self.block_c(
                    target,
                    &t.quote,
                    &quote_rect,
                    self.theme.text_tertiary,
                    alpha,
                );
            }
        }

        // ── Three links in a row: repo / feedback / copy diagnostics ──
        let link_gap = 20.0;
        let rw = t.repo.w + 8.0;
        let iw = t.issues.w + 8.0;
        let dw = t.diag.w + 8.0;
        let total = rw + link_gap + iw + link_gap + dw;
        let rx = (w - total) / 2.0;
        self.link(target, Hit::LinkRepo, &t.repo, rx, dy + link_y, rw, alpha);
        self.link(
            target,
            Hit::LinkIssues,
            &t.issues,
            rx + rw + link_gap,
            dy + link_y,
            iw,
            alpha,
        );
        self.link(
            target,
            Hit::CopyDiagnostics,
            &t.diag,
            rx + rw + link_gap + iw + link_gap,
            dy + link_y,
            dw,
            alpha,
        );

        // ── Latest news section ──
        if let Some(news) = news {
            self.dashed_divider(target, 24.0, dy + 178.0, w - 48.0, alpha);
            let sec_rect = D2D_RECT_F {
                left: 0.0,
                top: dy + 186.0,
                right: w,
                bottom: dy + 200.0,
            };
            self.block_c(
                target,
                &t.section,
                &sec_rect,
                self.theme.text_tertiary,
                alpha,
            );
            // The axis needs only the first/last node centers: a running
            // y walk, no per-frame Vecs (the follow-up draw walk repeats
            // the same advance).
            let mut axis_ends: Option<(f32, f32)> = None;
            let mut yy = dy + NEWS_TOP;
            for (i, item) in news.iter().take(NEWS_MAX).enumerate() {
                let cy = yy + 9.0;
                axis_ends = Some(match axis_ends {
                    Some((first, _)) => (first, cy),
                    None => (cy, cy),
                });
                yy += item_h(item, expanded == Some(i)) + NEWS_GAP;
            }
            if let Some((first, last)) = axis_ends {
                let axis = self.brush(target, self.theme.text_tertiary, alpha * 0.5);
                self.line(target, NEWS_AXIS_X, first, NEWS_AXIS_X, last, &axis, 1.0);
            }
            let mut y = dy + NEWS_TOP;
            for (i, item) in news.iter().take(NEWS_MAX).enumerate() {
                self.news_item(target, model, t, item, i, expanded == Some(i), y, w, alpha);
                y += item_h(item, expanded == Some(i)) + NEWS_GAP;
            }
        }
        // Frame cache put back: single exit
        self.about_texts = texts;
    }

    /// Rebuild the about text cache: constant labels become prebuilt
    /// layouts, news artifacts are ellipsized once per the key's
    /// expanded and read state
    unsafe fn build_about_texts(
        &mut self,
        s: &crate::ui::i18n::Strings,
        lang: crate::ui::i18n::Lang,
        news: Option<&[NewsItem]>,
        expanded: Option<usize>,
        read: Option<&str>,
        w: f32,
    ) -> AboutTexts {
        let blk = |r: &Self, t: &str, size: f32, weight: u16| {
            TextBlock::new(r.make_layout(t, size, weight, false))
        };
        let ver = s.version_label.replace("{v}", env!("CARGO_PKG_VERSION"));
        let right = w - 24.0;
        let mut items = Vec::new();
        for (i, item) in news.unwrap_or(&[]).iter().take(NEWS_MAX).enumerate() {
            // Unread turns on the NEW badge and widens its reservation,
            // shrinking the title budget; is_unread is shared with the
            // draw side (node fill and badge presence) and the click-side
            // read-marker advance
            let unread = crate::service::whatsnew::is_unread(item.date.as_str(), read);
            let new_w = if unread { 36.0 } else { 0.0 }; // NEW badge 30 + left gap 6
            let title_w = (right - 14.0 - new_w - NEWS_BODY_X).max(40.0);
            let (disp, tw) = self.ellipsize(&item.title, 13.0, title_w, 600, false);
            // w keeps the ellipsize budget width: the NEW badge follows
            // the title tail by it; recomputing from the truncated
            // layout's metrics could drift (the marquee's trunc blocks
            // in main.rs share this rationale)
            let mut title = TextBlock::new(self.make_layout(&disp, 13.0, 600, false));
            title.w = tw;
            // Dates render as MM-DD; odd lengths (not YYYY-MM-DD)
            // leniently fall back to the whole string instead of
            // panicking.
            let date_disp = item
                .date
                .get(5..)
                .filter(|d| d.len() == 5)
                .unwrap_or(&item.date);
            // Folded caches the first line truncated, expanded all
            // lines; the key's expanded item keeps the stored set
            // aligned with the drawn state
            let lines: Vec<&String> = if expanded == Some(i) {
                item.lines.iter().collect()
            } else {
                item.lines.first().into_iter().collect()
            };
            let mut body = Vec::new();
            for line in lines {
                let (disp, _) = self.ellipsize(line, 12.0, right - NEWS_BODY_X, 400, false);
                body.push(TextBlock::new(self.make_layout(&disp, 12.0, 400, false)));
            }
            items.push(AboutItem {
                date: blk(self, date_disp, 10.0, 400),
                title,
                body,
            });
        }
        AboutTexts {
            key: (
                lang,
                w.to_bits(),
                news.map(|n| n.to_vec()),
                expanded,
                read.map(str::to_string),
            ),
            name: blk(self, "Quotify", 21.0, 600),
            ver: blk(self, &ver, 12.0, 400),
            desc: blk(self, s.app_desc, 11.5, 400),
            quote: blk(self, s.egg_quote, 11.5, 400),
            repo: blk(self, &format!("[{}]", s.link_repo), 12.5, 500),
            issues: blk(self, &format!("[{}]", s.link_issues), 12.5, 500),
            diag: blk(self, &format!("[{}]", s.link_diag), 12.5, 500),
            section: blk(self, s.whats_new_section, 11.0, 600),
            new_badge: blk(self, "NEW", 9.0, 700),
            items,
        }
    }

    /// Easter-egg sweep across one prebuilt line: the text color is
    /// passed in per line, matching that line's normal state to avoid
    /// color jumps between lines
    #[allow(clippy::too_many_arguments)]
    unsafe fn egg_eat_line(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        w: f32,
        block: &TextBlock,
        line_y: f32,
        color: [f32; 4],
        q: f32,
        p: f32,
        alpha: f32,
    ) {
        let full = block.w + 12.0;
        let x = w / 2.0 - full / 2.0 + q * full;
        let rect = D2D_RECT_F {
            left: 0.0,
            top: line_y,
            right: w,
            bottom: line_y + 16.0,
        };
        unsafe {
            target.PushAxisAlignedClip(
                &D2D_RECT_F {
                    left: x + 9.0,
                    ..rect
                },
                windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
        }
        // The sweep clip is tighter than the text rect on every edge,
        // so the nested clip inside block_c never widens it
        self.block_c(target, block, &rect, color, alpha);
        unsafe {
            target.PopAxisAlignedClip();
        }
        self.pacman_at(target, x, line_y + 8.0, 8.0, p, self.theme.logo_tile, alpha);
    }

    /// One text link; the label block comes from the about cache, hover
    /// only re-dispatches color
    #[allow(clippy::too_many_arguments)]
    unsafe fn link(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        label: &TextBlock,
        x: f32,
        y: f32,
        wpx: f32,
        alpha: f32,
    ) {
        let color = if self.hover == Some(hit) {
            self.theme.accent
        } else {
            self.theme.text_secondary
        };
        let rect = D2D_RECT_F {
            left: x,
            top: y,
            right: x + wpx,
            bottom: y + 18.0,
        };
        self.block_l(target, label, &rect, color, alpha);
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: x - 4.0,
                top: y,
                right: x + wpx + 4.0,
                bottom: y + 18.0,
            },
        ));
    }

    /// One news item; text artifacts come from the about cache, drawing
    /// is down to geometry and color dispatch
    #[allow(clippy::too_many_arguments)]
    unsafe fn news_item(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        texts: &AboutTexts,
        item: &NewsItem,
        i: usize,
        expanded: bool,
        y: f32,
        w: f32,
        alpha: f32,
    ) {
        let pad = 24.0;
        let right = w - pad;
        let mid = y + 9.0;
        let unread = crate::service::whatsnew::is_unread(item.date.as_str(), model.last_news_read);

        // Node: unread is filled with the accent color, read is hollow
        // with the tertiary color; a short horizontal extends right to
        // join the date column.
        let node = D2D1_ELLIPSE {
            point: Vector2 {
                X: NEWS_AXIS_X,
                Y: mid,
            },
            radiusX: 3.5,
            radiusY: 3.5,
        };
        let tick = self.brush(target, self.theme.text_tertiary, alpha * 0.6);
        self.line(target, NEWS_AXIS_X + 3.5, mid, NEWS_DATE_X, mid, &tick, 1.0);
        if unread {
            let b = self.brush(target, self.theme.accent, alpha);
            target.FillEllipse(&node, &b);
        } else {
            let b = self.brush(target, self.theme.text_tertiary, alpha);
            target.DrawEllipse(&node, &b, 1.0, None);
        }

        // Date block; the MM-DD display was decided at cache build. The
        // clip keeps the whole-string fallback inside the 36px column,
        // same as the old DrawText rect.
        let date_rect = D2D_RECT_F {
            left: NEWS_DATE_X,
            top: mid - 7.0,
            right: NEWS_DATE_X + 36.0,
            bottom: mid - 7.0 + 14.0,
        };
        self.block_l(
            target,
            &texts.items[i].date,
            &date_rect,
            self.theme.text_tertiary,
            alpha,
        );

        // The right end holds the expand/collapse arrow, with the NEW
        // badge to its left.
        let arrow_x = right - 4.0;
        let stroke = self.brush(target, self.theme.text_tertiary, alpha);
        let (ay, by) = if expanded {
            (mid + 2.5, mid - 1.5)
        } else {
            (mid - 2.5, mid + 1.5)
        };
        self.line(target, arrow_x - 3.0, ay, arrow_x, by, &stroke, 1.2);
        self.line(target, arrow_x, by, arrow_x + 3.0, ay, &stroke, 1.2);

        // Title; the truncation width flexed with NEW's presence at
        // cache build. The block's w is the ellipsize budget, so the
        // NEW badge follows the title tail without re-measuring; the
        // clip edge at the column end never engages because the
        // ellipsized width fits the budget by construction.
        let title_rect = D2D_RECT_F {
            left: NEWS_BODY_X,
            top: y,
            right,
            bottom: y + 18.0,
        };
        self.block_l(
            target,
            &texts.items[i].title,
            &title_rect,
            self.theme.text_primary,
            alpha,
        );

        // The NEW badge follows the title tail; the cap keeps it out of
        // the right-end arrow area.
        if unread {
            let left = (NEWS_BODY_X + texts.items[i].title.w + 6.0).min(right - 44.0);
            let badge_rect = D2D_RECT_F {
                left,
                top: mid - 6.0,
                right: left + 30.0,
                bottom: mid + 6.0,
            };
            let badge = D2D1_ROUNDED_RECT {
                rect: badge_rect,
                radiusX: 4.0,
                radiusY: 4.0,
            };
            let b = self.brush(target, self.theme.accent, alpha);
            target.FillRoundedRectangle(&badge, &b);
            self.block_vc(
                target,
                &texts.new_badge,
                &badge_rect,
                [1.0, 1.0, 1.0, 1.0],
                alpha,
            );
        }
        // Body: folded shows the first line truncated, expanded shows
        // all lines; the stored blocks already match the state.
        if expanded {
            let mut ly = y + 18.0;
            for b in &texts.items[i].body {
                let line_rect = D2D_RECT_F {
                    left: NEWS_BODY_X,
                    top: ly,
                    right,
                    bottom: ly + NEWS_LINE_H,
                };
                self.block_l(target, b, &line_rect, self.theme.text_secondary, alpha);
                ly += NEWS_LINE_H;
            }
        } else if let Some(b) = texts.items[i].body.first() {
            let line_rect = D2D_RECT_F {
                left: NEWS_BODY_X,
                top: y + 18.0,
                right,
                bottom: y + 18.0 + 15.0,
            };
            self.block_l(target, b, &line_rect, self.theme.text_secondary, alpha);
        }
        let block_h = item_h(item, expanded);
        self.hits.push((
            Hit::NewsItem(i),
            D2D_RECT_F {
                left: pad - 6.0,
                top: y,
                right: right + 6.0,
                bottom: y + block_h,
            },
        ));
    }
}

/// Height-chain pinning regression: expected values are derived from
/// draw_about's y advancement chain; layout changes must update both
#[cfg(test)]
mod tests {
    use super::*;

    fn item(lines: usize) -> NewsItem {
        NewsItem {
            date: "2026-08-28".into(),
            title: "t".into(),
            lines: vec!["l".to_string(); lines],
        }
    }

    #[test]
    fn about_height_pinned() {
        // No-news tier: link row bottom 176 + bottom margin 24; an
        // empty slice takes the same tier
        assert_eq!(about_height(None, None), LogPx(200));
        assert_eq!(about_height(Some(&[]), None), LogPx(200));
        // Single folded item: 204 + (18+15+7) + 6 + 4 + 24
        let one = vec![item(2)];
        assert_eq!(about_height(Some(&one), None), LogPx(278));
        // Single expanded item (two body lines): 204 + (18+2*16+10) + 6 + 4 + 24
        assert_eq!(about_height(Some(&one), Some(0)), LogPx(298));
        // Multi-item truncation: of 5 items only NEWS_MAX=3 folded ones
        // are displayed
        let many = vec![item(1); 5];
        assert_eq!(about_height(Some(&many), None), LogPx(370));
    }
}
