//! 关于窗口视图

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{D2D1_ELLIPSE, D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget};
use windows_numerics::Vector2;

use super::{Align, Hit, Renderer};
use crate::service::whatsnew::NewsItem;
use crate::ui::panel::anim::ease_out_cubic;
use crate::ui::panel::model::PanelModel;

/// 关于窗逻辑宽
pub const ABOUT_W: f32 = 500.0;

/// 仓库主页
pub const REPO_URL: &str = "https://github.com/tengmmvp/Quotify";
/// 仓库 Issue 区
pub const ISSUES_URL: &str = "https://github.com/tengmmvp/Quotify/issues";

/// 动态区最多陈列条数
const NEWS_MAX: usize = 3;

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
    /// 调用方须丢弃整个 Renderer
    pub fn paint_about(
        &mut self,
        hwnd: windows::Win32::Foundation::HWND,
        rect_phys: &RECT,
        model: &PanelModel,
        expanded: Option<usize>,
        dpi: f32,
    ) -> bool {
        unsafe {
            let Some(target) = self.ensure_target(hwnd, rect_phys, dpi) else {
                return true;
            };
            let w = (rect_phys.right - rect_phys.left) as f32 / dpi;
            let h = (rect_phys.bottom - rect_phys.top) as f32 / dpi;
            self.hits.clear();
            target.BeginDraw();
            self.draw_about(&target, model, expanded, w, h);
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
    unsafe fn draw_about(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        expanded: Option<usize>,
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

        // ── 项目描述 ──
        let desc_rect = D2D_RECT_F {
            left: 12.0,
            top: dy + desc_y,
            right: w - 12.0,
            bottom: dy + desc_y + 16.0,
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

        // ── 双链接并排 ──
        let repo = format!("[{}]", s.link_repo);
        let issues = format!("[{}]", s.link_issues);
        let link_gap = 20.0;
        let rw = self.measure(&repo, 12.5, 500, false) + 8.0;
        let iw = self.measure(&issues, 12.5, 500, false) + 8.0;
        let total = rw + link_gap + iw;
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

        // 日期取 MM-DD
        self.text(
            target,
            &item.date[5..],
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

        // 标题；右端为箭头与 NEW 留位，截断宽随 NEW 有无伸缩
        let new_w = if unread { 36.0 } else { 0.0 }; // NEW 徽标 30 + 左隙 6
        let title_w = (right - 14.0 - new_w - NEWS_BODY_X).max(40.0);
        let title = self.ellipsize(&item.title, 13.0, title_w, 600, false);
        let tw = self.measure(&title, 13.0, 600, false);
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
                let t = self.ellipsize(line, 12.0, right - NEWS_BODY_X, 400, false);
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
            let t = self.ellipsize(first, 12.0, right - NEWS_BODY_X, 400, false);
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
