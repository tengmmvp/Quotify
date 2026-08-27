//! 主视图

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget};
use windows_numerics::Matrix3x2;

use super::{Align, Hit, Renderer};
use crate::api::FetchError;
use crate::ui::fmt;
use crate::ui::i18n::Strings;
use crate::ui::panel::layout;
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::theme::RADIUS;

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
        let mut y = dy + 16.0;
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
        let title_disp = self.ellipsize(title, 16.0, tw, 500, false);
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
            let ax = tx + self.measure(&title_disp, 16.0, 500, false) + 6.0;
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
        y += 52.0;

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
                    self.divider(target, pad, y + 2.0, w - pad * 2.0, alpha);
                    y += 14.0;
                    let bar = self.brush(target, self.theme.text_primary, alpha * 0.9);
                    target.FillRectangle(
                        &D2D_RECT_F {
                            left: pad,
                            top: y + 1.0,
                            right: pad + 3.0,
                            bottom: y + 13.0,
                        },
                        &bar,
                    );
                    self.text(
                        target,
                        s.usage_section,
                        pad + 7.0,
                        y,
                        w - pad * 2.0 - 7.0,
                        17.0,
                        12.0,
                        600,
                        self.theme.text_tertiary,
                        alpha,
                    );
                    if crate::ui::peak::is_peak_now(model.peak_range) {
                        self.peak_badge(target, y, w, alpha, s);
                    }
                    y += 26.0;

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
                    }

                    if let Some(ts) = &snap.token_stats {
                        y += 6.0;
                        self.dashed_divider(target, pad, y, w - pad * 2.0, alpha);
                        y += 14.0;
                        let bar = self.brush(target, self.theme.text_primary, alpha * 0.9);
                        target.FillRectangle(
                            &D2D_RECT_F {
                                left: pad,
                                top: y + 1.0,
                                right: pad + 3.0,
                                bottom: y + 13.0,
                            },
                            &bar,
                        );
                        self.text(
                            target,
                            s.token_usage_section,
                            pad + 7.0,
                            y,
                            w - pad * 2.0 - 7.0,
                            17.0,
                            12.0,
                            600,
                            self.theme.text_tertiary,
                            alpha,
                        );
                        y += 22.0;
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
                        y += 6.0;
                        self.dashed_divider(target, pad, y, w - pad * 2.0, alpha);
                        y += 14.0;
                        let bar = self.brush(target, self.theme.text_primary, alpha * 0.9);
                        target.FillRectangle(
                            &D2D_RECT_F {
                                left: pad,
                                top: y + 1.0,
                                right: pad + 3.0,
                                bottom: y + 13.0,
                            },
                            &bar,
                        );
                        self.text(
                            target,
                            s.balance_label,
                            pad + 7.0,
                            y,
                            140.0,
                            17.0,
                            12.0,
                            600,
                            self.theme.text_tertiary,
                            alpha,
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
                            y,
                            140.0,
                            18.0,
                            12.0,
                            500,
                            self.theme.text_primary,
                            alpha,
                        );
                    }
                }
                // 页脚钉底；视口被压矮时装不下内容，钉内容底免叠数据区
                let footer_y = dy + h.max(content_h) - 36.0;
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
        y + 52.0
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
        let row_h = 19.0;
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
        let badge_w = bw + 4.0 + self.measure(s.peak_badge, 12.0, 600, false);
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
