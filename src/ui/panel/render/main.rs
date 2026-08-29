//! 主视图

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget, ID2D1PathGeometry,
};
use windows_numerics::{Matrix3x2, Vector2};

use super::widgets::PACMAN_R;
use super::{Align, Hit, Renderer};
use crate::api::FetchError;
use crate::ui::fmt;
use crate::ui::i18n::Strings;
use crate::ui::panel::layout;
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::theme::RADIUS;

/// MCP 工具色板
const MCP_PALETTE: [[f32; 4]; 4] = [
    [0.361, 0.616, 0.549, 1.0], // #5C9D8C 灰青
    [0.431, 0.529, 0.659, 1.0], // #6E87A8 雾蓝
    [0.584, 0.514, 0.624, 1.0], // #95839F 灰紫
    [0.678, 0.624, 0.561, 1.0], // #AD9F8F 暖灰
];

/// 能量格几何
const MCP_CELLS: usize = 20;
const MCP_CELL_W: f32 = 12.0;
const MCP_CELL_GAP: f32 = 2.0;
const MCP_CELL_SKEW: f32 = 4.0;

/// 图例徽标几何
const MCP_BADGE_SIZE: f32 = 11.0;
const MCP_SWATCH: f32 = 8.0;
const MCP_SWATCH_GAP: f32 = 3.0;
const MCP_BADGE_GAP: f32 = 6.0;

/// 图例徽标
struct McpBadge {
    color: [f32; 4],
    text: String,
    text_w: f32,
}

/// MCP 构成区按 (快照时间, 内宽) 缓存的数据侧产物，命中时跳过排序、
/// 分段与全部测宽。段几何与轨道空格色依赖逐帧位置/主题，绘制时现算
pub(super) struct McpCompCache {
    key: chrono::DateTime<chrono::Local>,
    inner_w: f32,
    segs: Vec<(usize, usize, [f32; 4])>,
    badges: Vec<McpBadge>,
    shown: usize,
    plus_w: f32,
}

impl Renderer {
    /// 主视图：顶栏常驻；主体按「无快照、有错误、有数据」三态渲染
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

        // ── 顶栏：账号名 + 套餐副标题 ──
        let title = model.account.map(|a| a.name).unwrap_or("Quotify");
        let meta = snap.and_then(|s| {
            let v = s.plan_version.label();
            let tier = {
                let t = s.tier.label();
                if t.is_empty() {
                    s.plan_label.clone().unwrap_or_default()
                } else {
                    t.to_string()
                }
            };
            match (v.is_empty(), tier.is_empty()) {
                (false, false) => Some(format!("{v} · {tier}")),
                (false, true) => Some(v.to_string()),
                (true, false) => Some(tier),
                (true, true) => None,
            }
        });
        let (logo_size, logo_y) = (38.0, y + 7.0);
        self.logo(target, pad, logo_y, logo_size, alpha);
        let tx = pad + logo_size + 10.0;
        let chevron_w = if model.accounts_count > 1 { 18.0 } else { 0.0 };
        let tw = w - tx - 88.0 - chevron_w;
        let block_h = if meta.is_some() { 39.0 } else { 22.0 };
        let block_top = y + 26.0 - block_h / 2.0;
        let (title_disp, title_disp_w) = self.ellipsize(title, 16.0, tw, 500, false);
        self.text(
            target,
            &title_disp,
            tx,
            block_top,
            tw,
            22.0,
            16.0,
            500,
            self.theme.text_primary,
            alpha,
        );
        if model.accounts_count > 1 {
            // chevron 紧跟标题尾：宽度直接用 ellipsize 的返回值，不再复测
            let ax = tx + title_disp_w + 6.0;
            self.chevron(target, ax, y + 26.0, self.theme.text_secondary, alpha);
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
        if let Some(m) = &meta {
            self.text(
                target,
                m,
                tx + 1.0,
                block_top + 22.0,
                tw,
                17.0,
                12.0,
                400,
                self.theme.text_secondary,
                alpha,
            );
        }
        let btn_r = 16.0;
        let refresh_cx = w - pad - btn_r - 30.0;
        let settings_cx = w - pad - btn_r;
        let btn_cy = y + 26.0;
        self.icon_button(
            target,
            Hit::Refresh,
            refresh_cx,
            btn_cy,
            btn_r,
            self.anim.spin,
        );
        self.sliders(target, Hit::Settings, settings_cx, btn_cy, btn_r);
        y += layout::MAIN_TOPBAR_H;

        // ── 主体三态：空/加载、错误卡、数据区 ──
        match (snap, model.error) {
            // 无快照：已配置=加载中，未配置=引导空态
            (None, None) => {
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
            // 无数据有错误：错误卡居中 + 重试
            (None, Some(e)) => {
                let msg = error_text(s, e);
                let body_top = y + 10.0;
                let body_h = (dy + h - 16.0) - body_top;
                let cx = w / 2.0;
                let card_h = 96.0;
                let card_top = body_top + (body_h - card_h) / 2.0;
                let card = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: pad,
                        top: card_top,
                        right: w - pad,
                        bottom: card_top + card_h,
                    },
                    radiusX: RADIUS,
                    radiusY: RADIUS,
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
                let title_rect = D2D_RECT_F {
                    left: pad + 12.0,
                    top: card_top + 18.0,
                    right: w - pad - 12.0,
                    bottom: card_top + 40.0,
                };
                let (msg, _) = self.ellipsize(
                    &msg,
                    13.0,
                    (title_rect.right - title_rect.left).max(60.0),
                    500,
                    false,
                );
                self.text_aligned(
                    target,
                    &msg,
                    &title_rect,
                    13.0,
                    500,
                    self.theme.danger,
                    alpha,
                    Align::Center,
                    false,
                );
                self.outline_button(
                    target,
                    Hit::Retry,
                    cx - 44.0,
                    card_top + 50.0,
                    88.0,
                    28.0,
                    s.retry,
                    alpha,
                );
            }
            // 有数据即以数据区为主体；同时带错误时旧数据照常展示，错误降级页脚
            _ => {
                if let Some(snap) = snap {
                    let ty = self.section_header(
                        target,
                        s.usage_section,
                        pad,
                        y,
                        w,
                        w - pad * 2.0 - 7.0,
                        alpha,
                        false,
                    );
                    if crate::ui::peak::is_peak_now(model.peak_range) {
                        self.peak_badge(target, ty, w, alpha, s);
                    }
                    y = ty + layout::MAIN_MASTHEAD_ROW_H;

                    let detail_of = |cur: Option<f64>, tot: Option<f64>| -> Option<String> {
                        match (cur, tot) {
                            (Some(c), Some(t)) if t > 0.0 => Some(
                                s.used_of
                                    .replace("{cur}", &fmt::compact_number(c))
                                    .replace("{tot}", &fmt::compact_number(t)),
                            ),
                            _ => None,
                        }
                    };
                    if let Some(b) = snap.five_hour.as_ref() {
                        y = self.metric_row(
                            target,
                            s.five_hour,
                            b.used_percent,
                            detail_of(b.current, b.total),
                            b.resets_at,
                            y,
                            w,
                            alpha,
                            model.lang,
                        );
                    }
                    if let Some(b) = snap.weekly.as_ref() {
                        y = self.metric_row(
                            target,
                            s.weekly,
                            b.used_percent,
                            detail_of(b.current, b.total),
                            b.resets_at,
                            y,
                            w,
                            alpha,
                            model.lang,
                        );
                    }
                    if let Some(m) = snap.mcp.as_ref() {
                        let detail = if m.total > 0.0 {
                            Some(
                                s.used_of
                                    .replace("{cur}", &fmt::compact_number(m.current_value))
                                    .replace("{tot}", &fmt::compact_number(m.total)),
                            )
                        } else {
                            None
                        };
                        y = self.metric_row(
                            target,
                            s.mcp_tools,
                            m.used_percent,
                            detail,
                            m.resets_at,
                            y,
                            w,
                            alpha,
                            model.lang,
                        );
                        // 明细非空才陈列构成区，与 sync_main_height 判定同源
                        if !m.details.is_empty() {
                            y = self.mcp_composition(target, m, snap.queried_at, y, w, alpha);
                        }
                    }

                    if let Some(ts) = &snap.token_stats {
                        y += layout::MAIN_SECTION_GAP;
                        let ty = self.section_header(
                            target,
                            s.token_usage_section,
                            pad,
                            y,
                            w,
                            w - pad * 2.0 - 7.0,
                            alpha,
                            true,
                        );
                        y = ty + layout::MAIN_TOKEN_ROWS_ADV;
                        y = self.leader_row(
                            target,
                            s.today_tokens,
                            &fmt::compact_number(ts.today),
                            y,
                            w,
                            alpha,
                        );
                        y = self.leader_row(
                            target,
                            s.week_tokens,
                            &fmt::compact_number(ts.week),
                            y,
                            w,
                            alpha,
                        );
                    }

                    if let Some(b) = &snap.balance {
                        y += layout::MAIN_SECTION_GAP;
                        let ty = self.section_header(
                            target,
                            s.balance_label,
                            pad,
                            y,
                            w,
                            140.0,
                            alpha,
                            true,
                        );
                        let amount = if b.available.abs() >= 1e6 {
                            fmt::compact_number(b.available)
                        } else {
                            format!("{:.2}", b.available)
                        };
                        let line = format!("¥{amount}");
                        self.text_mono_r(
                            target,
                            &line,
                            w - pad - 140.0,
                            ty,
                            140.0,
                            layout::MAIN_BALANCE_ROW_H,
                            12.0,
                            500,
                            self.theme.text_primary,
                            alpha,
                        );
                    }
                }
                // 页脚钉底；视口被压矮时装不下内容，钉内容底免叠数据区
                let footer_y = dy + h.max(content_h) - layout::MAIN_FOOTER_H;
                if let Some(e) = model.error {
                    let msg = error_text(s, e);
                    let line = match model.snapshot {
                        Some(snap) => format!(
                            "{} · {} {msg}",
                            s.data_as_of
                                .replace("{t}", &fmt::as_of_time(snap.queried_at)),
                            s.fetch_failed,
                        ),
                        None => msg,
                    };
                    let (line, _) = self.ellipsize(&line, 12.0, w - pad * 2.0 - 74.0, 400, false);
                    self.text(
                        target,
                        &line,
                        pad,
                        footer_y + 6.0,
                        w - pad * 2.0 - 74.0,
                        30.0,
                        12.0,
                        400,
                        self.theme.text_secondary,
                        alpha,
                    );
                    self.outline_button(
                        target,
                        Hit::Retry,
                        w - pad - 62.0,
                        footer_y + 4.0,
                        62.0,
                        26.0,
                        s.retry,
                        alpha,
                    );
                } else if let Some(fa) = self.anim.footer.as_ref().filter(|f| !f.tween.finished()) {
                    // 吃豆人换装
                    let (old_text, new_text, tween) =
                        (fa.old_text.clone(), fa.new_text.clone(), fa.tween);
                    let old_w = self.measure(&old_text, 12.0, 400, false);
                    let new_w = self.measure(&new_text, 12.0, 400, false);
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
                        bottom: footer_y + 34.0,
                    };
                    unsafe {
                        target.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                    }
                    self.text_aligned(
                        target,
                        &old_text,
                        &D2D_RECT_F {
                            left: old_home,
                            top: footer_y + 8.0,
                            right: old_home + old_w,
                            bottom: footer_y + 25.0,
                        },
                        12.0,
                        400,
                        self.theme.text_tertiary,
                        alpha,
                        Align::Left,
                        false,
                    );
                    unsafe {
                        target.PopAxisAlignedClip();
                    }
                    self.pacman(target, cx, cy, raw, alpha);
                    let bite = raw * 8.0;
                    let (phase, idx) = (bite.fract(), bite.floor() as u32);
                    let age = (phase - 0.8 + 1.0) % 1.0;
                    if p < frac_eat && age < 0.5 {
                        let t = age / 0.5;
                        let fall = t * t * 14.0;
                        let cb =
                            self.brush(target, self.theme.text_tertiary, alpha * (1.0 - t * t));
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
                    self.text_aligned(
                        target,
                        &new_text,
                        &D2D_RECT_F {
                            left: nx,
                            top: footer_y + 8.0,
                            right: nx + new_w,
                            bottom: footer_y + 25.0,
                        },
                        12.0,
                        400,
                        self.theme.text_tertiary,
                        alpha,
                        Align::Left,
                        false,
                    );
                } else if let Some(snap) = model.snapshot {
                    let fresh = (chrono::Local::now() - snap.queried_at).num_seconds() < 60;
                    let text = if fresh {
                        s.updated_just_now.to_string()
                    } else {
                        s.updated_ago
                            .replace("{t}", &fmt::ago(snap.queried_at, model.lang))
                    };
                    self.text_aligned(
                        target,
                        &text,
                        &D2D_RECT_F {
                            left: pad,
                            top: footer_y + 8.0,
                            right: w - pad,
                            bottom: footer_y + 25.0,
                        },
                        12.0,
                        400,
                        self.theme.text_tertiary,
                        alpha,
                        Align::Center,
                        false,
                    );
                }
            }
        }
    }

    /// 数据段区块头：分隔线 + 强调条 + 标题；返回标题行顶 y，段内后续
    /// 推进（usage 叠高峰徽标后走刊头行高、Token 接票据行、余额即文本
    /// 行）由调用点自定。刊头用实线且低 2px 挂段起点，数据段虚线贴段起点
    #[allow(clippy::too_many_arguments)]
    unsafe fn section_header(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &str,
        pad: f32,
        y: f32,
        w: f32,
        title_w: f32,
        alpha: f32,
        dashed: bool,
    ) -> f32 {
        if dashed {
            self.dashed_divider(target, pad, y, w - pad * 2.0, alpha);
        } else {
            self.divider(target, pad, y + 2.0, w - pad * 2.0, alpha);
        }
        // 两种段的标题距段起点同为 14：刊头是段起点起算，数据段是分隔线起算
        let ty = y + if dashed {
            layout::MAIN_SECTION_HEAD
        } else {
            layout::MAIN_MASTHEAD_RULE_GAP
        };
        let bar = self.brush(target, self.theme.text_primary, alpha * 0.9);
        target.FillRectangle(
            &D2D_RECT_F {
                left: pad,
                top: ty + 1.0,
                right: pad + 3.0,
                bottom: ty + 13.0,
            },
            &bar,
        );
        self.text(
            target,
            label,
            pad + 7.0,
            ty,
            title_w,
            17.0,
            12.0,
            600,
            self.theme.text_tertiary,
            alpha,
        );
        ty
    }

    /// 指标行：标签 + 百分比 + 进度条，底行左倒计时右已用明细；返回下一行 y
    #[allow(clippy::too_many_arguments)]
    unsafe fn metric_row(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &str,
        used_percent: f64,
        detail: Option<String>,
        resets_at: Option<chrono::DateTime<chrono::Utc>>,
        y: f32,
        w: f32,
        alpha: f32,
        lang: crate::ui::i18n::Lang,
    ) -> f32 {
        let pad = layout::CONTENT_PAD;
        let strings = lang.strings();
        // ≥90% 数值与进度条入危险色，阈值与托盘图标红档一致
        let critical = used_percent >= 90.0;
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

        self.text(
            target,
            label,
            pad,
            y + 2.0,
            w - pad * 2.0 - 110.0,
            19.0,
            14.0,
            400,
            c_label,
            alpha,
        );
        let pct = fmt::percent(used_percent);
        self.text_mono_r(
            target,
            &pct,
            w - pad - 100.0,
            y + 1.0,
            100.0,
            19.0,
            14.0,
            600,
            c_value,
            alpha,
        );

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
        // 占比不足 0.4% 不画填充；画出时保底 2px 宽
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

        // 底行左右分栏：左 55% 倒计时，右侧对齐已用明细
        if let Some(r) = resets_at {
            let line = strings.resets_line.replace("{t}", &fmt::countdown(r, lang));
            self.text(
                target,
                &line,
                pad,
                bar_y + bar_h + 5.0,
                (w - pad * 2.0) * 0.55,
                16.0,
                11.0,
                400,
                c_caption,
                alpha,
            );
        }
        if let Some(d) = detail {
            self.text_rect_opts(
                target,
                &d,
                &D2D_RECT_F {
                    left: pad + (w - pad * 2.0) * 0.55,
                    top: bar_y + bar_h + 5.0,
                    right: w - pad,
                    bottom: bar_y + bar_h + 21.0,
                },
                11.0,
                400,
                c_caption,
                alpha,
                Align::Right,
                false,
            );
        }
        y + layout::MAIN_METRIC_ROW_H
    }

    /// MCP 工具构成区：直角外框内一条右斜平行四边形能量格条[满串 = 工具
    /// 消耗合计]加一行三格徽标图例[色块|名称|次数]。格段与徽标色块按用量
    /// 降序取低饱和四色板，第 5+ 工具并入第四色；图例装不下整枚徽标截尾
    /// +N、首枚超宽截名称。数据只随轮询变化，分段/徽标/装填结果按
    /// (快照时间, 内宽) 缓存，动画帧不重算。返回下一行 y
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

        // 数据侧产物按 (快照时间, 内宽) 缓存：命中时跳过排序、分段与全部
        // 测宽；宽度入键因装填结果随面板宽变化（跨 DPI 显示器）。take
        // 取走所有权，绘制段才能自由调用 &mut self 的绘制方法；本函数
        // 单一出口，函数尾放回
        let mut cache = self.mcp_cache.take();
        if cache
            .as_ref()
            .is_none_or(|c| c.key != queried_at || c.inner_w != inner_w)
        {
            cache = Some(self.build_mcp_cache(m, queried_at, inner_w));
        }
        let c = cache.unwrap();
        let segs = &c.segs;
        let badges = &c.badges;
        let shown = c.shown;

        let cell_h = layout::MAIN_MCP_CELL_H;
        let bar_y = top + 1.0 + layout::MAIN_MCP_COMP_PAD_Y;
        let bar_w = MCP_CELLS as f32 * MCP_CELL_W + (MCP_CELLS - 1) as f32 * MCP_CELL_GAP;
        let x0 = inner_x + (inner_w - bar_w) / 2.0;
        // 待填段队列：明细合计可小于总消耗，未覆盖格位画轨道空格
        let mut jobs: Vec<(usize, usize, [f32; 4])> = Vec::new();
        let mut cursor = 0usize;
        for (start, end, color) in segs {
            if *start > cursor {
                jobs.push((cursor, *start, self.theme.track));
            }
            jobs.push((*start, *end, *color));
            cursor = *end;
        }
        if cursor < MCP_CELLS {
            jobs.push((cursor, MCP_CELLS, self.theme.track));
        }
        for (from, to, color) in jobs {
            if let Some(geo) = self.build_mcp_cells(from, to, x0, bar_y) {
                let b = self.brush(target, color, alpha);
                target.FillGeometry(&geo, &b, None);
            }
        }

        let leg_y = bar_y + cell_h + layout::MAIN_MCP_LEGEND_ADV;
        let leg_h = layout::MAIN_MCP_LEGEND_H;
        let sw_top = leg_y + 3.5;
        let swatch_rect = |x: f32| D2D_RECT_F {
            left: x,
            top: sw_top,
            right: x + MCP_SWATCH,
            bottom: sw_top + MCP_SWATCH,
        };
        let mut x = inner_x;
        if shown == 0 && !badges.is_empty() {
            // 首枚即超宽：截文本兜底，最大工具保底可见
            let b = &badges[0];
            let sw = self.brush(target, b.color, alpha);
            target.FillRectangle(&swatch_rect(x), &sw);
            let avail = inner_w - MCP_SWATCH - MCP_SWATCH_GAP;
            let (t, _) = self.ellipsize(&b.text, MCP_BADGE_SIZE, avail, 400, true);
            self.text_nosnap(
                target,
                &t,
                x + MCP_SWATCH + MCP_SWATCH_GAP,
                leg_y,
                avail,
                leg_h,
                MCP_BADGE_SIZE,
                400,
                self.theme.text_secondary,
                alpha,
            );
        } else {
            for (i, b) in badges.iter().take(shown).enumerate() {
                if i > 0 {
                    x += MCP_BADGE_GAP;
                }
                let sw = self.brush(target, b.color, alpha);
                target.FillRectangle(&swatch_rect(x), &sw);
                x += MCP_SWATCH + MCP_SWATCH_GAP;
                self.text_nosnap(
                    target,
                    &b.text,
                    x,
                    leg_y,
                    b.text_w + 2.0,
                    leg_h,
                    MCP_BADGE_SIZE,
                    400,
                    self.theme.text_secondary,
                    alpha,
                );
                x += b.text_w;
            }
            if shown < badges.len() {
                let t = format!("+{}", badges.len() - shown);
                let tw = c.plus_w;
                x += MCP_BADGE_GAP;
                self.text_nosnap(
                    target,
                    &t,
                    x,
                    leg_y,
                    tw + 2.0,
                    leg_h,
                    MCP_BADGE_SIZE,
                    400,
                    self.theme.text_tertiary,
                    alpha,
                );
            }
        }
        self.mcp_cache = Some(c);
        y + layout::MAIN_MCP_COMP_H
    }

    /// 重建 MCP 构成区缓存：工具排序、能量条分段与徽标测宽装填
    unsafe fn build_mcp_cache(
        &mut self,
        m: &crate::api::McpUsage,
        queried_at: chrono::DateTime<chrono::Local>,
        inner_w: f32,
    ) -> McpCompCache {
        // 工具降序；段界累计取整，段间无缝不重叠，末段吃满整串
        let mut items: Vec<&crate::api::McpDetail> = m.details.iter().collect();
        items.sort_by(|a, b| {
            b.usage
                .partial_cmp(&a.usage)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let sum: f64 = items.iter().map(|d| d.usage).sum();
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
        // 图例为徽标串[色块|名称·次数]：色块与能量条分段同色，名称首字母
        // 大写仅动显示层。装填按宽度驱动：装下当前枚后若仍有剩余，须再
        // 容得下徽标间空隙 + 「+N」才继续装，尾部概括数即剩余工具数
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
                let text_w = self.measure(&text, MCP_BADGE_SIZE, 400, true);
                McpBadge {
                    color: MCP_PALETTE[i.min(MCP_PALETTE.len() - 1)],
                    text,
                    text_w,
                }
            })
            .collect();
        let badge_w = |b: &McpBadge| MCP_SWATCH + MCP_SWATCH_GAP + b.text_w;
        let mut shown = 0usize;
        let mut cur = 0.0f32;
        for (i, b) in badges.iter().enumerate() {
            let add = if i == 0 { 0.0 } else { MCP_BADGE_GAP } + badge_w(b);
            let tail = if badges.len() - i > 1 {
                MCP_BADGE_GAP
                    + self.measure(
                        &format!("+{}", badges.len() - i - 1),
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
        let plus_w = if shown < badges.len() {
            self.measure(
                &format!("+{}", badges.len() - shown),
                MCP_BADGE_SIZE,
                400,
                true,
            )
        } else {
            0.0
        };
        McpCompCache {
            key: queried_at,
            inner_w,
            segs,
            badges,
            shown,
            plus_w,
        }
    }

    /// 禁用像素吸附的文本绘制：吸附偏移随位置独立抖动，会拉花链式
    /// 推进的徽标间距；仅徽标图例使用。mono + 垂直居中，走 text_raw
    #[allow(clippy::too_many_arguments)]
    unsafe fn text_nosnap(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
    ) {
        let rect = D2D_RECT_F {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        };
        self.text_raw(
            target,
            s,
            &rect,
            size,
            weight,
            color,
            alpha,
            Align::Left,
            true,
            false,
            true,
            true,
        );
    }

    /// 一段能量格的合成路径
    fn build_mcp_cells(
        &self,
        from: usize,
        to: usize,
        x0: f32,
        bar_y: f32,
    ) -> Option<ID2D1PathGeometry> {
        unsafe {
            let geo = self.factory.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            for i in from..to {
                let s = x0 + i as f32 * (MCP_CELL_W + MCP_CELL_GAP);
                let h = layout::MAIN_MCP_CELL_H;
                sink.BeginFigure(
                    Vector2 {
                        X: s + MCP_CELL_SKEW,
                        Y: bar_y,
                    },
                    D2D1_FIGURE_BEGIN_FILLED,
                );
                sink.AddLine(Vector2 {
                    X: s + MCP_CELL_SKEW + MCP_CELL_W,
                    Y: bar_y,
                });
                sink.AddLine(Vector2 {
                    X: s + MCP_CELL_W,
                    Y: bar_y + h,
                });
                sink.AddLine(Vector2 { X: s, Y: bar_y + h });
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            }
            sink.Close().ok()?;
            Some(geo)
        }
    }

    /// 票据合计行：左 label、右数值，中间引导点自动填满；返回下一行 y
    unsafe fn leader_row(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &str,
        value: &str,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 {
        let pad = layout::CONTENT_PAD;
        let row_h = layout::MAIN_LEADER_ROW_H;
        self.text(
            target,
            label,
            pad,
            y + 1.0,
            (w - pad * 2.0) * 0.4,
            row_h,
            12.0,
            400,
            self.theme.text_secondary,
            alpha,
        );
        let vw = self.measure(value, 12.0, 500, true) + 6.0;
        self.text_mono_r(
            target,
            value,
            w - pad - vw,
            y,
            vw,
            row_h,
            12.0,
            500,
            self.theme.text_primary,
            alpha,
        );
        // 引导点铺在 label 右端到数值左端之间的行视觉中心上
        let label_w = self.measure(label, 12.0, 400, false);
        let cy = y + 10.0;
        let dot = self.brush(target, self.theme.text_tertiary, alpha * 0.55);
        let mut x = pad + label_w + 8.0;
        let end = w - pad - vw - 8.0;
        while x <= end {
            let e = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                point: windows_numerics::Vector2 { X: x, Y: cy },
                radiusX: 0.75,
                radiusY: 0.75,
            };
            target.FillEllipse(&e, &dot);
            x += 5.0;
        }
        y + row_h
    }

    /// 「高峰」徽标：闪电 + 文字居标题行右侧，悬停命中登记 UsageInfo
    unsafe fn peak_badge(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        title_y: f32,
        w: f32,
        alpha: f32,
        s: &Strings,
    ) {
        let pad = layout::CONTENT_PAD;
        let bh = 14.0;
        let bw = bh * (7.0 / 13.0);
        // 徽标文字是 i18n 常量文本，走帧内去重测宽
        let badge_w = bw + 4.0 + self.measure_static(s.peak_badge, 12.0, 600, false);
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
        self.text(
            target,
            s.peak_badge,
            tx,
            title_y,
            w - tx,
            17.0,
            12.0,
            600,
            self.theme.peak,
            alpha,
        );
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

    /// 峰谷悬停说明卡片
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
            radiusX: 6.0,
            radiusY: 6.0,
        };
        let [r, g, b, _] = self.theme.bg;
        let fill = self.brush(target, [r, g, b, 1.0], 1.0);
        target.FillRoundedRectangle(&card, &fill);
        let line = self.brush(target, self.theme.border, alpha);
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

/// 错误类型 → 用户文案；Api/Network 的细节经 with_detail 拼进前缀
fn error_text(s: &Strings, e: &FetchError) -> String {
    match e {
        FetchError::Auth => s.err_auth.to_string(),
        FetchError::EmptyLimits => s.err_empty.to_string(),
        FetchError::Api(detail) => with_detail(s.err_api, detail),
        FetchError::Network(detail) => with_detail(s.err_network, detail),
    }
}

/// 细节非空拼「前缀: 细节」，空则只回前缀
fn with_detail(prefix: &str, detail: &str) -> String {
    if detail.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}: {detail}")
    }
}
