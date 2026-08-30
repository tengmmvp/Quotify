//! 关于窗口视图

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{D2D1_ELLIPSE, D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget};
use windows_numerics::{Matrix3x2, Vector2};

use super::{Align, Hit, Renderer};
use crate::service::whatsnew::{NEWS_MAX, NewsItem};
use crate::ui::panel::anim::ease_out_cubic;
use crate::ui::panel::model::PanelModel;

/// 关于窗逻辑宽
pub const ABOUT_W: f32 = 500.0;

/// 仓库主页
pub const REPO_URL: &str = "https://github.com/tengmmvp/Quotify";
/// 仓库 Issue 区
pub const ISSUES_URL: &str = "https://github.com/tengmmvp/Quotify/issues";

/// 折叠条目块高：标题行 18 + 摘要行 15 + 底隙 7
const NEWS_FOLD_H: f32 = 40.0;
/// 条目间距
const NEWS_GAP: f32 = 6.0;
/// 展开条目正文行高
const NEWS_LINE_H: f32 = 16.0;
/// 动态区首条起点（相对窗顶）；位于描述与双链接等固定区之后
const NEWS_TOP: f32 = 204.0;
/// 时间轴列位：轴线与节点、日期、标题与正文
const NEWS_AXIS_X: f32 = 30.0;
/// 日期列起点
const NEWS_DATE_X: f32 = 40.0;
/// 标题与正文列起点
const NEWS_BODY_X: f32 = 78.0;

/// 关于窗逻辑高；无动态时为基础布局，有动态逐条累加。
/// 数值与 draw_about 的 y 推进链同源，布局改动须同步更新
pub fn about_height(news: Option<&[NewsItem]>, expanded: Option<usize>) -> i32 {
    let Some(news) = news.filter(|n| !n.is_empty()) else {
        return 200; // 无动态档：链接行底 176 + 底部余量 24
    };
    let mut h = NEWS_TOP;
    for (i, item) in news.iter().take(NEWS_MAX).enumerate() {
        h += item_h(item, expanded == Some(i)) + NEWS_GAP;
    }
    // 条目区尾隙 + 底部余量
    (h + 4.0 + 24.0).round() as i32
}

/// 单条动态的展示块高
fn item_h(item: &NewsItem, expanded: bool) -> f32 {
    if expanded {
        // 标题行 18 + 正文行 + 底隙 10
        18.0 + item.lines.len() as f32 * NEWS_LINE_H + 10.0
    } else {
        NEWS_FOLD_H
    }
}

impl Renderer {
    /// 关于窗一帧；骨架同面板 paint，返回 false 表示设备已丢失，
    /// 调用方须丢弃整个 Renderer。
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
            let Some(target) = self.ensure_target(hwnd, rect_phys, dpi) else {
                return true;
            };
            let w = (rect_phys.right - rect_phys.left) as f32 / dpi;
            let h = (rect_phys.bottom - rect_phys.top) as f32 / dpi;
            self.hits.clear();
            self.frame_measures.clear();
            target.BeginDraw();
            self.draw_about(&target, model, expanded, egg, egg_eaten, w, h);
            match target.EndDraw(None, None) {
                Ok(()) => true,
                Err(e) => {
                    crate::platform::log(&format!("[Quotify] 关于窗 EndDraw 失败: {e}"));
                    false
                }
            }
        }
    }

    /// 头部信息与双链接为固定区，最新动态区缀于末位
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

        let (dy, alpha) = match &self.anim.appear {
            Some(t) if self.anim_allowed => {
                let p = ease_out_cubic(t.progress());
                ((1.0 - p) * 6.0, p)
            }
            _ => (0.0, 1.0),
        };
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

        let has_news = model.news.is_some_and(|n| !n.is_empty());
        let (name_y, ver_y, desc_y, link_y) = if has_news {
            (78.0, 108.0, 128.0, 150.0)
        } else {
            (84.0, 114.0, 134.0, 158.0)
        };

        // ── 头部：logo + 应用名 + 版本 ──
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
        self.text_aligned(
            target,
            "Quotify",
            &name_rect,
            21.0,
            600,
            self.theme.text_primary,
            alpha,
            Align::Center,
            false,
        );
        let ver = s.version_label.replace("{v}", env!("CARGO_PKG_VERSION"));
        let eating = egg.filter(|_| !egg_eaten);
        // 全程距离 = 两行长度和的恒速推进；跨行交接处瞬移到下行左端。
        let (ver_eat, desc_eat) = match eating {
            Some(p) => {
                let ver_full = self.measure(&ver, 12.0, 400, false) + 12.0;
                let desc_full = self.measure(s.app_desc, 11.5, 400, false) + 12.0;
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
                    &ver,
                    dy + ver_y,
                    12.0,
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
                self.text_aligned(
                    target,
                    &ver,
                    &ver_rect,
                    12.0,
                    400,
                    self.theme.text_tertiary,
                    alpha,
                    Align::Center,
                    false,
                );
            }
        }

        // ── 项目描述 ──
        match eating {
            Some(_) if ver_eat.is_some() => {
                self.draw_desc(target, s, w, dy + desc_y, alpha);
            }
            Some(p) => {
                self.egg_eat_line(
                    target,
                    w,
                    s.app_desc,
                    dy + desc_y,
                    11.5,
                    self.theme.text_secondary,
                    desc_eat,
                    p,
                    alpha,
                );
            }
            None if !egg_eaten => {
                self.draw_desc(target, s, w, dy + desc_y, alpha);
            }
            None => {
                let quote = match model.lang {
                    crate::ui::i18n::Lang::Zh => "人不能两次踏进同一条河流。",
                    crate::ui::i18n::Lang::En => "No man ever steps in the same river twice.",
                };
                let mid_y = (ver_y + desc_y + 16.0) / 2.0;
                let quote_rect = D2D_RECT_F {
                    left: 0.0,
                    top: dy + mid_y,
                    right: w,
                    bottom: dy + mid_y + 16.0,
                };
                self.text_aligned(
                    target,
                    quote,
                    &quote_rect,
                    11.5,
                    400,
                    self.theme.text_tertiary,
                    alpha,
                    Align::Center,
                    false,
                );
            }
        }

        // ── 三链接并排：仓库 / 反馈 / 复制诊断 ──
        let repo = format!("[{}]", s.link_repo);
        let issues = format!("[{}]", s.link_issues);
        let diag = format!("[{}]", s.link_diag);
        let link_gap = 20.0;
        let rw = self.measure(&repo, 12.5, 500, false) + 8.0;
        let iw = self.measure(&issues, 12.5, 500, false) + 8.0;
        let dw = self.measure(&diag, 12.5, 500, false) + 8.0;
        let total = rw + link_gap + iw + link_gap + dw;
        let rx = (w - total) / 2.0;
        self.link(target, Hit::LinkRepo, &repo, rx, dy + link_y, rw, alpha);
        self.link(
            target,
            Hit::LinkIssues,
            &issues,
            rx + rw + link_gap,
            dy + link_y,
            iw,
            alpha,
        );
        self.link(
            target,
            Hit::CopyDiagnostics,
            &diag,
            rx + rw + link_gap + iw + link_gap,
            dy + link_y,
            dw,
            alpha,
        );

        // ── 最新动态区 ──
        if let Some(news) = model.news.filter(|n| !n.is_empty()) {
            self.dashed_divider(target, 24.0, dy + 178.0, w - 48.0, alpha);
            let sec_rect = D2D_RECT_F {
                left: 0.0,
                top: dy + 186.0,
                right: w,
                bottom: dy + 200.0,
            };
            self.text_aligned(
                target,
                s.whats_new_section,
                &sec_rect,
                11.0,
                600,
                self.theme.text_tertiary,
                alpha,
                Align::Center,
                false,
            );
            let items: Vec<(usize, &NewsItem)> = news.iter().take(NEWS_MAX).enumerate().collect();
            let mut node_cys = Vec::new();
            let mut yy = dy + NEWS_TOP;
            for &(i, item) in &items {
                node_cys.push(yy + 9.0);
                yy += item_h(item, expanded == Some(i)) + NEWS_GAP;
            }
            if let (Some(&first), Some(&last)) = (node_cys.first(), node_cys.last()) {
                let axis = self.brush(target, self.theme.text_tertiary, alpha * 0.5);
                self.line(target, NEWS_AXIS_X, first, NEWS_AXIS_X, last, &axis, 1.0);
            }
            let mut y = dy + NEWS_TOP;
            for &(i, item) in &items {
                self.news_item(target, model, item, i, expanded == Some(i), y, w, alpha);
                y += item_h(item, expanded == Some(i)) + NEWS_GAP;
            }
        }
    }

    /// 项目描述行
    unsafe fn draw_desc(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &crate::ui::i18n::Strings,
        w: f32,
        y: f32,
        alpha: f32,
    ) {
        let desc_rect = D2D_RECT_F {
            left: 12.0,
            top: y,
            right: w - 12.0,
            bottom: y + 16.0,
        };
        self.text_aligned(
            target,
            s.app_desc,
            &desc_rect,
            11.5,
            400,
            self.theme.text_secondary,
            alpha,
            Align::Center,
            false,
        );
    }

    /// 彩蛋横扫一行：文字色随行传入，与该行正常态一致避免跨行跳色
    #[allow(clippy::too_many_arguments)]
    unsafe fn egg_eat_line(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        w: f32,
        text: &str,
        line_y: f32,
        size: f32,
        color: [f32; 4],
        q: f32,
        p: f32,
        alpha: f32,
    ) {
        let tw = self.measure(text, size, 400, false);
        let full = tw + 12.0;
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
        self.text_aligned(
            target,
            text,
            &rect,
            size,
            400,
            color,
            alpha,
            Align::Center,
            false,
        );
        unsafe {
            target.PopAxisAlignedClip();
        }
        self.pacman_at(target, x, line_y + 8.0, 8.0, p, alpha);
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn pacman_at(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        cx: f32,
        cy: f32,
        r: f32,
        p: f32,
        alpha: f32,
    ) {
        if self.pacman_geo.is_none() {
            self.pacman_geo = self.build_pacman_geo();
        }
        let Some((upper, lower)) = self.pacman_geo.clone() else {
            return;
        };
        let phase = (p * 8.0).fract();
        let sm = |t: f32| {
            let c = t.clamp(0.0, 1.0);
            c * c * (3.0 - 2.0 * c)
        };
        let ratio = if phase < 0.65 {
            sm(phase / 0.65)
        } else {
            1.0 - sm((phase - 0.65) / 0.35)
        };
        let mouth = 0.02 + ratio * 0.94;
        let b = self.brush(target, self.theme.logo_tile, alpha);
        let k = r / super::widgets::PACMAN_R;
        for (geo, ang) in [(upper, -mouth), (lower, mouth)] {
            let (s, c) = ang.sin_cos();
            target.SetTransform(&Matrix3x2 {
                M11: k * c,
                M12: k * s,
                M21: -k * s,
                M22: k * c,
                M31: cx,
                M32: cy,
            });
            target.FillGeometry(&geo, &b, None);
            target.DrawGeometry(&geo, &b, 1.0, None);
        }
        target.SetTransform(&Matrix3x2::identity());
    }

    /// 一枚文字链接
    #[allow(clippy::too_many_arguments)]
    unsafe fn link(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        label: &str,
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
        self.text(target, label, x, y, wpx, 18.0, 12.5, 500, color, alpha);
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

    /// 单条动态
    #[allow(clippy::too_many_arguments)]
    unsafe fn news_item(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
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
        let unread = model.last_news_read.is_none_or(|r| item.date.as_str() > r);

        // 节点：未读实心强调色、已读空心三级色；右延短横线接日期列
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

        // 日期取 MM-DD；异常长度（非 YYYY-MM-DD）宽松回退整串，不 panic
        let date_disp = item
            .date
            .get(5..)
            .filter(|s| s.len() == 5)
            .unwrap_or(&item.date);
        self.text(
            target,
            date_disp,
            NEWS_DATE_X,
            mid - 7.0,
            36.0,
            14.0,
            10.0,
            400,
            self.theme.text_tertiary,
            alpha,
        );

        // 右端展开/收起箭头，NEW 徽标在其左
        let arrow_x = right - 4.0;
        let stroke = self.brush(target, self.theme.text_tertiary, alpha);
        let (ay, by) = if expanded {
            (mid + 2.5, mid - 1.5)
        } else {
            (mid - 2.5, mid + 1.5)
        };
        self.line(target, arrow_x - 3.0, ay, arrow_x, by, &stroke, 1.2);
        self.line(target, arrow_x, by, arrow_x + 3.0, ay, &stroke, 1.2);

        // 标题；右端为箭头与 NEW 留位，截断宽随 NEW 有无伸缩。
        // 宽度直接取 ellipsize 返回值，NEW 徽标跟随不复测
        let new_w = if unread { 36.0 } else { 0.0 }; // NEW 徽标 30 + 左隙 6
        let title_w = (right - 14.0 - new_w - NEWS_BODY_X).max(40.0);
        let (title, tw) = self.ellipsize(&item.title, 13.0, title_w, 600, false);
        self.text(
            target,
            &title,
            NEWS_BODY_X,
            y,
            title_w,
            18.0,
            13.0,
            600,
            self.theme.text_primary,
            alpha,
        );

        // NEW 徽标紧跟标题尾部；上限防越入右端箭头区
        if unread {
            let left = (NEWS_BODY_X + tw + 6.0).min(right - 44.0);
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
            self.text_aligned_vc(
                target,
                "NEW",
                &badge_rect,
                9.0,
                700,
                [1.0, 1.0, 1.0, 1.0],
                alpha,
                Align::Center,
                false,
            );
        }
        // 正文：折叠显首行截断，展开显全部行
        if expanded {
            let mut ly = y + 18.0;
            for line in &item.lines {
                let (t, _) = self.ellipsize(line, 12.0, right - NEWS_BODY_X, 400, false);
                self.text(
                    target,
                    &t,
                    NEWS_BODY_X,
                    ly,
                    right - NEWS_BODY_X,
                    NEWS_LINE_H,
                    12.0,
                    400,
                    self.theme.text_secondary,
                    alpha,
                );
                ly += NEWS_LINE_H;
            }
        } else if let Some(first) = item.lines.first() {
            let (t, _) = self.ellipsize(first, 12.0, right - NEWS_BODY_X, 400, false);
            self.text(
                target,
                &t,
                NEWS_BODY_X,
                y + 18.0,
                right - NEWS_BODY_X,
                15.0,
                12.0,
                400,
                self.theme.text_secondary,
                alpha,
            );
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

/// 高度链钉位回归：期望值由 draw_about 的 y 推进链推导而来，
/// 布局改动须同步更新
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
        // 无动态档：链接行底 176 + 底部余量 24；空切片同档
        assert_eq!(about_height(None, None), 200);
        assert_eq!(about_height(Some(&[]), None), 200);
        // 单条折叠：204 + (18+15+7) + 6 + 4 + 24
        let one = vec![item(2)];
        assert_eq!(about_height(Some(&one), None), 278);
        // 单条展开（两行正文）：204 + (18+2*16+10) + 6 + 4 + 24
        assert_eq!(about_height(Some(&one), Some(0)), 298);
        // 多条截断：5 条只陈列 NEWS_MAX=3 条折叠
        let many = vec![item(1); 5];
        assert_eq!(about_height(Some(&many), None), 370);
    }
}
