//! Account switcher popup view

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget};

use super::{Hit, Renderer};
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::theme::CARD_RADIUS;

/// Popup logical width
pub const POPUP_W: f32 = 220.0;
/// Account row height
pub const ROW_H: f32 = 36.0;
/// Height cap as a share of the work-area height: longer lists scroll
/// instead of running off the screen
const HEIGHT_CAP: f32 = 0.8;
/// Popup horizontal padding: the row insets, the badge anchor, and the hit
/// rects share it (row build and row draw must not drift apart)
const PAD: f32 = 10.0;

/// Uncapped content extent (logical px): title area + account rows +
/// top/bottom padding; the scroll range is this minus the viewport height
pub(crate) fn popup_content_height(accounts: usize) -> f32 {
    36.0 + accounts as f32 * ROW_H + 12.0
}

/// Popup logical height: the content extent capped at HEIGHT_CAP of the
/// work-area height (logical px)
pub fn popup_height(accounts: usize, work_h: f32) -> i32 {
    popup_content_height(accounts)
        .min(work_h * HEIGHT_CAP)
        .round() as i32
}

/// Row badge geometry chain shared by the row build and the row draw:
/// badge width → badge x. One source keeps the drawn badge box on the
/// budget the name's truncation was measured against.
fn row_badge_x(w: f32, badge_w: f32) -> (f32, f32) {
    let pw = badge_w + super::BADGE_PAD_X;
    (pw, w - PAD - pw)
}

/// One account row's cache entry: the leading identity fields double as the
/// per-frame account-set fingerprint; the blocks hold the row's prebuilt
/// text artifacts
struct PopupRow {
    name: String,
    platform: crate::api::Platform,
    team: bool,
    /// Ellipsized account name, laid out at the row's drawn weight (600
    /// selected, 500 otherwise); truncation itself always measures at 600 —
    /// preserving this legacy weight mismatch keeps cached rows identical to
    /// the former per-frame output
    name_blk: super::TextBlock,
    /// Platform badge label (10.5/400); its width fixes both the badge box
    /// and the name's truncation budget
    badge_blk: super::TextBlock,
}

/// Frame cache of the account popup: the heading and every row's name and
/// badge layouts are prebuilt, so fade-in and hover frames create zero
/// layouts and allocate zero strings. The content changes only with the key
/// below, so the key comparison alone also invalidates a stale cache left
/// over from a previous open
pub(super) struct PopupTexts {
    lang: crate::ui::i18n::Lang,
    w: f32,
    /// Selected row index: the name's drawn weight (600 vs 500) follows it,
    /// so a selection change must rebuild the name blocks
    selected: Option<usize>,
    /// Heading ("switch account"); content depends on the language only
    title: super::TextBlock,
    rows: Vec<PopupRow>,
}

impl PopupTexts {
    /// Full key check without allocation: width, language, selected index,
    /// row count, and each row's (name, platform, team)
    fn matches(
        &self,
        w: f32,
        lang: crate::ui::i18n::Lang,
        selected: Option<usize>,
        accounts: &[crate::app::config::Account],
    ) -> bool {
        self.w == w
            && self.lang == lang
            && self.selected == selected
            && self.rows.len() == accounts.len()
            && self
                .rows
                .iter()
                .zip(accounts)
                .all(|(r, a)| r.name == a.name && r.platform == a.platform && r.team == a.team)
    }
}

impl Renderer {
    /// One popup frame; skeleton same as the panel paint: ensure_target +
    /// BeginDraw/EndDraw; returning false means the device is lost and
    /// the caller must drop the whole Renderer
    pub fn paint_popup(
        &mut self,
        hwnd: windows::Win32::Foundation::HWND,
        rect_phys: &RECT,
        model: &PanelModel,
        scroll_dy: f32,
        dpi: f32,
    ) -> bool {
        unsafe {
            self.paint_frame(hwnd, rect_phys, dpi, "弹窗", |s, t, w, h| {
                s.draw_account_popup(t, model, w, h, scroll_dy)
            })
        }
    }

    /// Account list: subdued title + rows; the current item gets a left
    /// vertical bar, and hover fills the whole row lightly. The whole
    /// content translates with the scroll offset (logical px, same
    /// discipline as the panel)
    unsafe fn draw_account_popup(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        w: f32,
        h: f32,
        scroll_dy: f32,
    ) {
        let s = model.strings;

        // Pop animation: content floats up with a fade-in; the
        // background must be drawn opaquely at full size, or the first
        // frame exposes the swap chain.
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

        let selected = model.account.map(|a| a.index);

        // Row cache (single exit, put back at function end): the fade-in
        // and hover frames reuse the prebuilt name/badge layouts; a
        // change in language, width, selection, or the account set
        // rebuilds — the key comparison alone also invalidates the
        // leftovers of a previously closed popup.
        let mut texts = self.popup_texts.take();
        if texts
            .as_ref()
            .is_none_or(|t| !t.matches(w, model.lang, selected, model.accounts))
        {
            texts = Some(self.build_popup_texts(s, model.lang, selected, model.accounts, w));
        }
        let cached = texts.as_ref().expect("popup 缓存上方必已置");

        // Heading: horizontally centered in the popup, top of its band
        self.draw_block_at(
            target,
            &cached.title,
            (w - cached.title.w) / 2.0,
            dy + 13.0 - scroll_dy,
            self.theme.text_tertiary,
            alpha,
            false,
        );

        let mut y = dy + 36.0 - scroll_dy;
        for (i, row) in cached.rows.iter().enumerate() {
            // Off-viewport rows draw nothing and register no hits; rows
            // only go downward, so the first fully below the viewport ends
            // the loop
            if y >= h {
                break;
            }
            if y + ROW_H <= 0.0 {
                y += ROW_H;
                continue;
            }
            if self.hover == Some(Hit::PickAccount(i)) {
                let row = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: PAD - 4.0,
                        top: y + 2.0,
                        right: w - PAD + 4.0,
                        bottom: y + ROW_H - 2.0,
                    },
                    radiusX: CARD_RADIUS,
                    radiusY: CARD_RADIUS,
                };
                let fill = self.brush(target, self.theme.track, alpha);
                target.FillRoundedRectangle(&row, &fill);
            }
            let cur = selected == Some(i);
            if cur {
                // Height matches the account name's font size
                let bar = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: PAD - 4.0,
                        top: y + 11.25,
                        right: PAD - 1.0,
                        bottom: y + 24.75,
                    },
                    radiusX: 1.5,
                    radiusY: 1.5,
                };
                let b = self.brush(target, self.theme.accent, alpha);
                target.FillRoundedRectangle(&bar, &b);
            }
            // Right-end badge: platform, with the team badge appended for
            // team accounts; the popup is narrow and cannot fit a
            // key-prefix badge. The label mapping goes through
            // platform_with_team and the box geometry through badge_blk,
            // both shared with the settings card — the two views' badges
            // cannot drift apart.
            let (pw, px) = row_badge_x(w, row.badge_blk.w);
            self.badge_blk(
                target,
                &row.badge_blk,
                px,
                y + 12.5,
                pw,
                self.theme.border,
                self.theme.text_secondary,
                alpha,
            );
            self.draw_block_at(
                target,
                &row.name_blk,
                PAD + 6.0,
                y + 9.0,
                self.theme.text_primary,
                alpha,
                false,
            );
            self.hits.push((
                Hit::PickAccount(i),
                D2D_RECT_F {
                    left: PAD - 4.0,
                    top: y,
                    right: w - PAD + 4.0,
                    bottom: y + ROW_H,
                },
            ));
            y += ROW_H;
        }
        // Frame cache put back unconditionally: single exit
        self.popup_texts = texts;
    }

    /// Rebuild the popup text cache: the heading, each row's platform badge
    /// label (via the platform_with_team mapping shared with the settings
    /// card), and each row's ellipsized name at its drawn weight. The badge
    /// width also settles the name's truncation budget, so one build fixes
    /// the whole row's text geometry
    unsafe fn build_popup_texts(
        &mut self,
        s: &crate::ui::i18n::Strings,
        lang: crate::ui::i18n::Lang,
        selected: Option<usize>,
        accounts: &[crate::app::config::Account],
        w: f32,
    ) -> PopupTexts {
        let title = super::TextBlock::new(self.make_layout(s.switch_account, 12.0, 400, false));
        let rows = accounts
            .iter()
            .enumerate()
            .map(|(i, acc)| {
                let badge_blk = super::TextBlock::new(self.make_layout(
                    &super::platform_with_team(s, acc.platform, acc.team, " · "),
                    10.5,
                    400,
                    false,
                ));
                // Same geometry chain as the former per-frame path: badge
                // width → badge x → name right boundary → name budget
                let (_, px) = row_badge_x(w, badge_blk.w);
                let name_w = (px - 8.0 - (PAD + 6.0)).max(56.0);
                let (name, _) = self.ellipsize(&acc.name, 13.5, name_w, 600, false);
                let name_blk = super::TextBlock::new(self.make_layout(
                    &name,
                    13.5,
                    if selected == Some(i) { 600 } else { 500 },
                    false,
                ));
                PopupRow {
                    name: acc.name.clone(),
                    platform: acc.platform,
                    team: acc.team,
                    name_blk,
                    badge_blk,
                }
            })
            .collect();
        PopupTexts {
            lang,
            w,
            selected,
            title,
            rows,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_height_caps_at_work_area_share() {
        // Small sets take the raw formula
        assert_eq!(popup_height(0, 240.0), 48);
        assert_eq!(popup_height(3, 240.0), 156);
        // Formula exactly at the cap: unchanged
        assert_eq!(popup_height(4, 240.0), 192);
        // Past the cap: pinned at 80% of the work-area height
        assert_eq!(popup_height(5, 240.0), 192);
        assert_eq!(popup_height(100, 1000.0), 800);
    }

    #[test]
    fn popup_content_height_stays_uncapped() {
        assert_eq!(popup_content_height(0), 48.0);
        assert_eq!(popup_content_height(100), 3648.0);
    }
}
