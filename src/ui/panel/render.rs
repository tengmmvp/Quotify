//! 面板内容渲染：Direct2D + DirectWrite。
//!
//! 渲染器持有 DC Render Target，按主题 token 绘制主视图与设置视图；
//! 命中区（按钮/开关/选项的矩形）在每帧布局时记录，供鼠标命中检测。

use std::collections::HashMap;

use windows::core::PCWSTR;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_BEZIER_SEGMENT, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_END_CLOSED, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows_numerics::{Matrix3x2, Vector2};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, D2D1_DRAW_TEXT_OPTIONS, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_ROUNDED_RECT,
    ID2D1HwndRenderTarget, ID2D1Factory, ID2D1PathGeometry, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, DWRITE_FACTORY_TYPE_SHARED, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Foundation::HWND;

use super::anim::{Tween, animations_allowed, ease_in_out_cubic, ease_out_cubic};
use super::theme::{BAR_HEIGHT, RADIUS, Theme};
use super::{PanelView};
use crate::app::App;
use crate::ui::fmt;

/// 可点击元素标识（命中检测）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Refresh,
    Settings,
    AccountSwitch,
    Retry,
    Back,
    IntervalPreset(u64),
    Language(&'static str),
    /// 添加账号时的平台选择（"cn" / "intl"）
    Platform(&'static str),
    /// 添加账号：保存
    SaveAccount,
    ToggleThreshold,
    ToggleReset5h,
    ToggleResetWeekly,
    ToggleAutostart,
    AddAccount,
    RemoveAccount(usize),
    SelectAccount(usize),
    CheckUpdate,
}

/// 进度条等数值的动画插值状态。
pub struct AnimState {
    pub appear: Option<Tween>,
    pub bars: [f32; 3],
    /// 刷新按钮剩余旋转弧度（>0 表示在转）
    pub spin: f32,
}

impl AnimState {
    pub fn new() -> Self {
        Self { appear: None, bars: [0.0; 3], spin: 0.0 }
    }
}

impl Renderer {
    /// 刷新按钮动画是否仍在进行。
    pub fn spin_remaining(&mut self) -> bool {
        if self.anim.spin > 0.0 {
            self.anim.spin = (self.anim.spin - 0.15).max(0.0);
            true
        } else {
            false
        }
    }

    /// 触发一次刷新按钮旋转。
    pub fn start_spin(&mut self) {
        self.anim.spin = std::f32::consts::TAU;
    }
}

pub struct Renderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: Option<ID2D1HwndRenderTarget>,
    /// 兜底黑刷（运行期 brush 分配失败时使用）
    black: Option<ID2D1SolidColorBrush>,
    formats: HashMap<(u32, u16, bool), IDWriteTextFormat>,
    pub theme: Theme,
    /// 本帧记录的命中区
    pub hits: Vec<(Hit, D2D_RECT_F)>,
    /// 鼠标悬停中的命中项
    pub hover: Option<Hit>,
    pub anim: AnimState,
    font_fallback: bool,
    /// 当前 target 的 DPI（变化时才重设——每帧 SetDpi 会触发内部重建）
    target_dpi: f32,
    /// logo 的 Z 字形路径几何（30×30 viewBox，懒构建缓存）
    logo_geo: Option<ID2D1PathGeometry>,
}

impl Renderer {
    pub fn new() -> Option<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
            .ok()?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(
                DWRITE_FACTORY_TYPE_SHARED,
            )
            .ok()?;
            Some(Self {
                factory,
                dwrite,
                target: None,
                black: None,
                formats: HashMap::new(),
                theme: Theme::new(Theme::system_appearance()),
                hits: Vec::new(),
                hover: None,
                anim: AnimState::new(),
                font_fallback: false,
                target_dpi: 0.0,
                logo_geo: None,
            })
        }
    }

    fn format(&mut self, size: f32, weight: u16, mono: bool) -> Option<IDWriteTextFormat> {
        // f32 不实现 Eq/Hash，用 size 的位模式做 key
        let key = (size.to_bits(), weight, mono);
        if let Some(f) = self.formats.get(&key) {
            return Some(f.clone());
        }
        unsafe {
            let make = |family: &str| -> Option<IDWriteTextFormat> {
                self.dwrite
                    .CreateTextFormat(
                        PCWSTR(wide(family).as_ptr()),
                        None,
                        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT(weight as i32),
                        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STYLE_NORMAL,
                        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STRETCH_NORMAL,
                        size,
                        PCWSTR(wide("").as_ptr()),
                    )
                    .ok()
            };
            // 等宽（Consolas）用于数值 / 元数据——「技术之声」
            let fmt = if mono {
                make("Consolas")?
            } else {
                make("Segoe UI Variable Text").or_else(|| {
                    self.font_fallback = true;
                    make("Segoe UI")
                })?
            };
            let _ = fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
            self.formats.insert(key, fmt.clone());
            Some(fmt)
        }
    }

    /// 绘制一帧。硬件加速（HwndRenderTarget / DXGI 交换链）——之前用
    /// DC RenderTarget 是纯软件光栅，一帧 150ms 导致全局卡顿。
    /// `rect_phys` 为客户区（物理像素），渲染全程使用逻辑像素（DIP）。
    pub fn paint(&mut self, hwnd: HWND, rect_phys: &RECT, app: &App, view: PanelView, dpi: f32) {
        unsafe {
            let dpi = if dpi.is_finite() && dpi >= 1.0 { dpi } else { 1.0 };
            let w_px = (rect_phys.right - rect_phys.left).max(1) as u32;
            let h_px = (rect_phys.bottom - rect_phys.top).max(1) as u32;
            // 尺寸变化时重建（Resize 失败的兜底）
            let need_rebuild = match self.target.as_ref() {
                Some(t) => {
                    let sz = t.GetPixelSize();
                    sz.width != w_px || sz.height != h_px || self.target_dpi != dpi
                }
                None => true,
            };
            if need_rebuild {
                if let Some(t) = self.target.take() {
                    let _ = t;
                }
                let pf = D2D1_PIXEL_FORMAT {
                    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                };
                let props = D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                    pixelFormat: pf,
                    ..Default::default()
                };
                let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                    hwnd: hwnd.into(),
                    pixelSize: D2D_SIZE_U { width: w_px, height: h_px },
                    presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                };
                match self.factory.CreateHwndRenderTarget(&props, &hwnd_props) {
                    Ok(target) => {
                        let black = target
                            .CreateSolidColorBrush(
                                &windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F {
                                    r: 0.0, g: 0.0, b: 0.0, a: 1.0,
                                },
                                None,
                            )
                            .ok();
                        self.black = black;
                        self.target = Some(target);
                        self.target_dpi = dpi;
                    }
                    Err(e) => {
                        eprintln!("[quotify] CreateHwndRenderTarget 失败: {e}");
                        return;
                    }
                }
            }
            let Some(target) = self.target.clone() else { return };
            // DPI 只在创建/变化时设置（每帧 SetDpi 会触发内部状态重建）
            if self.target_dpi != dpi {
                target.SetDpi(dpi * 96.0, dpi * 96.0);
                self.target_dpi = dpi;
            }
            let rect_logical = RECT {
                left: 0,
                top: 0,
                right: ((rect_phys.right - rect_phys.left) as f32 / dpi).round() as i32,
                bottom: ((rect_phys.bottom - rect_phys.top) as f32 / dpi).round() as i32,
            };
            self.hits.clear();
            target.BeginDraw();
            self.draw(&target, app, view, &rect_logical);
            match target.EndDraw(None, None) {
                Ok(()) => {}
                Err(e) => eprintln!("[quotify] EndDraw 失败: {e}"),
            }
        }
    }

    unsafe fn draw(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        app: &App,
        view: PanelView,
        rect: &RECT,
    ) { unsafe {
        let w = (rect.right - rect.left) as f32;
        let h = (rect.bottom - rect.top) as f32;

        // 弹出动画：整帧上浮 + 内容透明度
        let (dy, alpha) = match &self.anim.appear {
            Some(t) if animations_allowed() => {
                let p = ease_out_cubic(t.progress());
                ((1.0 - p) * 10.0, p)
            }
            _ => (0.0, 1.0),
        };

        let bg = self.theme.bg;
        let black = self.black.clone();
        let solid = |c: [f32; 4]| -> ID2D1SolidColorBrush {
            target
                .CreateSolidColorBrush(&windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F {
                    r: c[0], g: c[1], b: c[2], a: c[3] * alpha,
                }, None)
                .unwrap_or_else(|_| black.clone().expect("fallback brush"))
        };
        let _bg_brush = solid(bg);
        // 背景（含动画位移）
        let bg_rect = D2D_RECT_F { left: 0.0, top: dy, right: w, bottom: h + dy };
        target.FillRectangle(&bg_rect, &_bg_brush);

        match view {
            PanelView::Main => self.draw_main(target, app, w, h, dy, alpha),
            PanelView::Settings => self.draw_settings(target, app, w, dy, alpha),
        }
    }}

    /// 主视图。
    unsafe fn draw_main(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        app: &App,
        w: f32,
        h: f32,
        dy: f32,
        alpha: f32,
    ) { unsafe {
        let s = app.strings;
        let pad = 20.0;
        let mut y = dy + 16.0;
        let snap = app.data.snapshot.as_ref();

        // ── 顶栏：无数据时以 logo 作为标识；有数据按原方案显示账号名 ──
        let account = app.config.selected_account();
        let title = account.map(|a| a.name.as_str()).unwrap_or("Quotify");
        if snap.is_none() {
            self.logo(target, pad, y + 1.0, 22.0, alpha);
            self.text(target, title, pad + 30.0, y + 2.0, w - pad * 2.0 - 90.0, 22.0, 15.0, 500, self.theme.text_primary, alpha);
        } else {
            self.text(target, title, pad, y + 2.0, w - pad * 2.0 - 90.0, 22.0, 15.0, 500, self.theme.text_primary, alpha);
        }
        let btn_r = 16.0;
        let refresh_cx = w - pad - btn_r - 30.0;
        let settings_cx = w - pad - btn_r;
        self.icon_button(target, Hit::Refresh, refresh_cx, y + 12.0, btn_r, self.anim.spin);
        self.sliders(target, Hit::Settings, settings_cx, y + 12.0, btn_r);
        if app.config.accounts.len() > 1 {
            self.hits.push((
                Hit::AccountSwitch,
                D2D_RECT_F { left: pad, top: y, right: w - 110.0, bottom: y + 26.0 },
            ));
        }
        y += 38.0;

        // ── 数据态 ──
        match (snap, &app.data.last_error) {
            (None, None) => {
                // 空态：居中的全局提示（Apple 空态规范——图形 + 标题 + 副标题）
                let configured = app.config.selected_account().is_some();
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
                // 标题 weight 400——克制的「耳语标题」是本风格的签名
                let title_rect = D2D_RECT_F { left: pad, top: top + 66.0, right: w - pad, bottom: top + 66.0 + 28.0 };
                self.text_aligned(target, t1, &title_rect, 21.0, 400, self.theme.text_primary, alpha, 1, false);
                if has_sub {
                    let sub_rect = D2D_RECT_F { left: pad + 12.0, top: top + 96.0, right: w - pad - 12.0, bottom: top + 96.0 + 40.0 };
                    self.text_aligned(target, t2, &sub_rect, 14.0, 400, self.theme.text_secondary, alpha, 1, false);
                }
            }
            _ => {
                if let Some(snap) = snap {
                    // 套餐徽标行：V3 · Max
                    let mut badge = String::new();
                    let v = snap.plan_version.label();
                    if !v.is_empty() {
                        badge.push_str(v);
                    }
                    let t = snap.tier.label();
                    if !t.is_empty() {
                        if !badge.is_empty() {
                            badge.push_str(" · ");
                        }
                        badge.push_str(t);
                    } else if let Some(l) = &snap.plan_label {
                        if !badge.is_empty() {
                            badge.push_str(" · ");
                        }
                        badge.push_str(l);
                    }
                    if badge.is_empty() {
                        badge.push_str("GLM");
                    }
                    self.badge(target, &badge, pad, y, alpha);
                    y += 30.0;

                    // 5h / 周 两条进度（MCP 另行处理）
                    let rows: [(Option<&crate::api::QuotaBucket>, &str, f32); 2] = [
                        (snap.five_hour.as_ref(), s.five_hour, self.anim.bars[0]),
                        (snap.weekly.as_ref(), s.weekly, self.anim.bars[1]),
                    ];
                    let mut row_y = y;
                    for (bucket, label, anim_v) in rows.iter() {
                        if let Some(b) = bucket {
                            row_y = self.quota_row(target, label, b, *anim_v, row_y, w, alpha, app.lang);
                        }
                    }
                    if let Some(mcp) = &snap.mcp {
                        let b = crate::api::QuotaBucket {
                            used_percent: mcp.used_percent,
                            resets_at: None,
                            total: if mcp.total > 0.0 { Some(mcp.total) } else { None },
                            current: if mcp.current_value > 0.0 { Some(mcp.current_value) } else { None },
                        };
                        row_y = self.quota_row(target, s.mcp_tools, &b, self.anim.bars[2], row_y, w, alpha, app.lang);
                    }
                    y = row_y + 4.0;

                    // 迷你柱状图 + Top 模型
                    if let Some(mu) = &snap.model_usage {
                        if !mu.series.is_empty() {
                            y = self.sparkline(target, &mu.series, pad, y, w - pad * 2.0, alpha);
                        }
                        if !mu.by_model.is_empty() {
                            let mut line = String::new();
                            for (name, tokens) in mu.by_model.iter().take(3) {
                                if !line.is_empty() {
                                    line.push_str("  ·  ");
                                }
                                line.push_str(&format!("{name} {}", fmt::compact_number(*tokens as f64)));
                            }
                            self.text(target, &line, pad, y, w - pad * 2.0, 18.0, 12.0, 400, self.theme.text_tertiary, alpha);
                            y += 20.0;
                        }
                    }

                    // 余额（国内版，等宽数值）
                    if let Some(b) = &snap.balance {
                        let line = format!("¥{:.2}", b.available);
                        self.text_mono_r(target, &line, w - pad - 140.0, y, 140.0, 18.0, 12.0, 500, self.theme.text_secondary, alpha);
                        y += 20.0;
                    }
                }
                // 失败态标注（有旧数据时）
                if let Some(e) = &app.data.last_error {
                    let line = match app.data.snapshot.as_ref() {
                        Some(snap) => format!(
                            "{} · {} {e}",
                            s.data_as_of.replace("{t}", &fmt::as_of_time(snap.queried_at)),
                            s.fetch_failed,
                        ),
                        None => e.to_string(),
                    };
                    self.text(target, &line, pad, y + 6.0, w - pad * 2.0 - 70.0, 34.0, 12.0, 400, self.theme.text_secondary, alpha);
                    // 重试按钮（主样式 Ink）
                    self.pill_button(target, Hit::Retry, w - pad - 62.0, y, 62.0, 28.0, s.retry, alpha, true);
                }
            }
        }
    }}

    unsafe fn quota_row(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &str,
        b: &crate::api::QuotaBucket,
        anim_v: f32,
        y: f32,
        w: f32,
        alpha: f32,
        lang: crate::ui::i18n::Lang,
    ) -> f32 { unsafe {
        let pad = 20.0;
        // 拷贝所需颜色（后续调用 &mut self 的 text/brush，不能持有 theme 引用）
        let (c_primary, c_tertiary, c_track, tier_color) = (
            self.theme.text_primary,
            self.theme.text_tertiary,
            self.theme.track,
            self.theme.tier_color(b.used_percent),
        );

        // 标签行：名称（eyebrow 风 13/500）+ 右侧等宽数值
        let right = if let (Some(cur), Some(tot)) = (b.current, b.total) {
            format!("{} / {}", fmt::compact_number(cur), fmt::compact_number(tot))
        } else {
            fmt::percent(b.used_percent)
        };
        self.text(target, label, pad, y + 2.0, w - pad * 2.0 - 150.0, 18.0, 13.0, 500, c_tertiary, alpha);
        self.text_mono_r(target, &right, w - pad - 140.0, y, 140.0, 18.0, 13.0, 500, c_primary, alpha);

        // 倒计时（等宽 11，元数据质感）
        if let Some(r) = b.resets_at {
            let cd_text = format!("{} {}", fmt::countdown(r, lang), lang.strings().resets_in);
            self.text_mono_r(target, &cd_text, w - pad - 140.0, y + 17.0, 140.0, 14.0, 11.0, 400, c_tertiary, alpha);
        }

        // 进度条（4px 圆角，Forest/Amber/Crimson 档位色）
        let track = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: pad, top: y + 24.0, right: w - pad, bottom: y + 24.0 + BAR_HEIGHT },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        let track_brush = self.brush(target, c_track, alpha);
        target.FillRoundedRectangle(&track, &track_brush);

        let frac = (anim_v / 100.0).clamp(0.0, 1.0);
        if frac > 0.003 {
            let fg_w = (w - pad * 2.0) * frac;
            let color = tier_color;
            let fg = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: pad, top: y + 24.0, right: pad + fg_w, bottom: y + 24.0 + BAR_HEIGHT },
                radiusX: RADIUS,
                radiusY: RADIUS,
            };
            let fill = self.brush(target, color, alpha);
            target.FillRoundedRectangle(&fg, &fill);
        }
        y + 24.0 + BAR_HEIGHT + 18.0
    }}

    unsafe fn sparkline(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        series: &[(String, i64)],
        x: f32,
        y: f32,
        width: f32,
        alpha: f32,
    ) -> f32 { unsafe {
        let h = 28.0;
        let max = series.iter().map(|(_, v)| *v).max().unwrap_or(1).max(1) as f32;
        let n = series.len().max(1) as f32;
        let gap = 1.5;
        let bw = ((width - gap * (n - 1.0)) / n).clamp(1.0, 6.0);
        // 模型用量柱：Forest 低调绿（Ember 留给文字强调）
        let brush = self.brush(target, self.theme.ok, alpha * 0.75);
        for (i, (_, v)) in series.iter().enumerate() {
            let bh = (*v as f32 / max * h).max(1.5);
            let bx = x + i as f32 * (bw + gap);
            let r = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: bx, top: y + h - bh, right: bx + bw, bottom: y + h },
                radiusX: 1.0,
                radiusY: 1.0,
            };
            target.FillRoundedRectangle(&r, &brush);
        }
        y + h + 8.0
    }}

    /// 设置视图。
    unsafe fn draw_settings(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        app: &App,
        w: f32,
        dy: f32,
        alpha: f32,
    ) { unsafe {
        let s = app.strings;
        let pad = 20.0;
        let cw = w - pad * 2.0; // 内容区宽度
        let mut y = dy + 12.0;

        // ── 导航栏：设置页左上返回；添加页由底部「取消」承担返回（箭头含义易误解）
        if !app.panel.adding_account {
            self.back_arrow(target, Hit::Back, pad, y + 6.0);
        }
        let nav_title = if app.panel.adding_account { s.add_account } else { s.settings };
        let title_rect = D2D_RECT_F { left: pad, top: y, right: w - pad, bottom: y + 26.0 };
        self.text_aligned(target, nav_title, &title_rect, 16.0, 400, self.theme.text_primary, alpha, 1, false);
        y += 30.0;

        // ── 账号（第一项：数据来源；添加页区块标题用「版本」）──
        let section = if app.panel.adding_account { s.platform_section } else { s.accounts_section };
        y = self.section_label(target, section, pad, y, w, alpha, false);
        if app.panel.adding_account {
            // 添加流程：先选平台（账号类型），再逐行输入名称与 key
            let plats: [(Hit, &str); 2] = [
                (Hit::Platform("cn"), s.platform_cn),
                (Hit::Platform("intl"), s.platform_intl),
            ];
            let cur_plat = app.panel.pending_platform;
            y = self.segmented_raw(
                target,
                &plats,
                |h| matches!(h, Hit::Platform(v) if cur_plat.platform_tag() == *v),
                pad,
                y,
                cw,
                alpha,
            );
            y += 8.0;
            // 名称（视觉输入框 + EDIT 子窗口内缩 2px）
            self.text(target, s.account_name, pad, y, cw, 16.0, 12.0, 400, self.theme.text_secondary, alpha);
            y += 18.0;
            self.input_field(target, pad, y, cw);
            y += 26.0 + 6.0;
            // API key
            self.text(target, s.api_key_label, pad, y, cw, 16.0, 12.0, 400, self.theme.text_secondary, alpha);
            y += 18.0;
            self.input_field(target, pad, y, cw);
            y += 26.0 + 12.0;
            // 保存/取消成组水平居中
            let pair_w = 88.0 * 2.0 + 12.0;
            let bx = pad + (cw - pair_w) / 2.0;
            self.pill_button(target, Hit::SaveAccount, bx, y, 88.0, 30.0, s.save, alpha, true);
            self.pill_button(target, Hit::Back, bx + 100.0, y, 88.0, 30.0, s.cancel, alpha, false);
            let _ = ease_in_out_cubic(0.5);
            return;
        }
        for (i, acc) in app.config.accounts.iter().enumerate() {
            let selected = app.config.selected.as_deref() == Some(acc.id.as_str());
            let label = format!(
                "{} · {}",
                acc.name,
                if acc.platform == crate::api::client::Platform::Cn { s.platform_cn } else { s.platform_intl }
            );
            self.account_row(target, Hit::SelectAccount(i), Hit::RemoveAccount(i), &label, selected, pad, y, w, alpha);
            y += 34.0;
        }
        // 添加账号独占一行：占满内容区，视觉对称
        self.pill_button(target, Hit::AddAccount, pad, y + 2.0, cw, 30.0, s.add_account, alpha, false);
        y += 36.0;

        // ── 轮询间隔 ──
        y = self.section_label(target, s.poll_interval, pad, y, w, alpha, true);
        let presets: [(Hit, &str); 4] = [
            (Hit::IntervalPreset(60), s.interval_1m),
            (Hit::IntervalPreset(300), s.interval_5m),
            (Hit::IntervalPreset(900), s.interval_15m),
            (Hit::IntervalPreset(1800), s.interval_30m),
        ];
        let cur = app.config.general.poll_interval_secs;
        y = self.segmented_raw(target, &presets, |h| matches!(h, Hit::IntervalPreset(v) if *v == cur), pad, y, cw, alpha);
        y += 10.0;

        // ── 语言 ──
        y = self.section_label(target, s.language, pad, y, w, alpha, true);
        let langs: [(Hit, &str); 3] = [
            (Hit::Language(""), s.follow_system),
            (Hit::Language("zh"), "中文"),
            (Hit::Language("en"), "English"),
        ];
        let cur_lang = app.config.general.language.as_deref().unwrap_or("");
        y = self.segmented_raw(target, &langs, |h| matches!(h, Hit::Language(v) if *v == cur_lang), pad, y, cw, alpha);
        y += 10.0;

        // ── 通知 ──
        y = self.section_label(target, s.notifications, pad, y, w, alpha, true);
        let g = &app.config.general;
        y = self.toggle_row(target, Hit::ToggleThreshold, s.notify_threshold, s.notify_threshold_desc, g.notify_threshold_enabled, pad, y, cw, alpha);
        y = self.toggle_row(target, Hit::ToggleReset5h, s.notify_reset_5h_opt, s.notify_reset_5h_desc, g.notify_reset_5h_enabled, pad, y, cw, alpha);
        y = self.toggle_row(target, Hit::ToggleResetWeekly, s.notify_reset_weekly_opt, s.notify_reset_weekly_desc, g.notify_reset_weekly_enabled, pad, y, cw, alpha);
        y += 6.0;

        // ── 通用：开机自启 ──
        y = self.section_label(target, s.settings_general, pad, y, w, alpha, true);
        y = self.toggle_row(target, Hit::ToggleAutostart, s.autostart, "", crate::platform::autostart::is_enabled(), pad, y, cw, alpha);
        y += 6.0;

        // ── 关于：检查更新 + 版本（底部）──
        let update_label = match &app.update_status {
            Some(Ok(info)) if crate::service::update::is_newer(&info.tag, env!("CARGO_PKG_VERSION")) => {
                format!("{} · {}", s.check_update, info.tag)
            }
            Some(Ok(_)) => s.up_to_date.into(),
            Some(Err(_)) => s.update_check_failed.into(),
            None => s.check_update.into(),
        };
        y = self.section_label(target, "", pad, y, w, alpha, true);
        // 左「当前版本」12px，右描边小按钮——字号一致、视觉平衡
        let ver_line = s.version_label.replace("{v}", env!("CARGO_PKG_VERSION"));
        self.text(target, &ver_line, pad, y + 7.0, w - pad * 2.0 - 124.0, 16.0, 12.0, 400, self.theme.text_tertiary, alpha);
        self.outline_button(target, Hit::CheckUpdate, w - pad - 104.0, y + 1.0, 104.0, 28.0, &update_label, alpha);
        let _ = ease_in_out_cubic(0.5);
    }}

    // ── 绘制小部件 ──

    unsafe fn section_label(&mut self, target: &ID2D1HwndRenderTarget, label: &str, x: f32, y: f32, _w: f32, alpha: f32, rule: bool) -> f32 { unsafe {
        let mut ny = y;
        if rule {
            // hairline 分隔（Stone 1px）——纸感分层先于阴影
            self.divider(target, x, ny + 5.0, _w - x * 2.0, alpha);
            ny += 10.0;
        }
        if !label.is_empty() {
            self.text(target, label, x, ny, _w, 16.0, 12.0, 500, self.theme.text_tertiary, alpha);
        }
        ny + 20.0
    }}

    /// 输入框视觉：Linen 底 + hairline 描边（EDIT 子窗口在其内 2px）。
    unsafe fn input_field(&mut self, target: &ID2D1HwndRenderTarget, x: f32, y: f32, w: f32) { unsafe {
        let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + 26.0 };
        let fill = self.brush(target, self.theme.track, 1.0);
        target.FillRectangle(&rect, &fill);
        let edge = self.brush(target, self.theme.border, 1.0);
        target.DrawRectangle(&rect, &edge, 1.0, None);
    }}

    /// hairline 分隔线（Stone，暖色 1px）。
    unsafe fn divider(&mut self, target: &ID2D1HwndRenderTarget, x: f32, y: f32, width: f32, alpha: f32) { unsafe {
        let b = self.brush(target, self.theme.border, alpha * 0.7);
        self.line(target, x, y, x + width, y, &b, 1.0);
    }}

    unsafe fn toggle_row(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        title: &str,
        desc: &str,
        on: bool,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 { unsafe {
        // 标题 14/400（body-sm），描述 12/400；开关右侧垂直居中
        self.text(target, title, x, y + 1.0, w - 56.0, 18.0, 14.0, 400, self.theme.text_primary, alpha);
        let mut ty = y + 19.0;
        if !desc.is_empty() {
            self.text(target, desc, x, ty, w - 56.0, 14.0, 12.0, 400, self.theme.text_tertiary, alpha);
            ty += 14.0;
        }
        // 开关（38×22，on 为 Forest）——「小一点点」
        let (tw, th) = (38.0, 22.0);
        let tx = x + w - tw;
        let cy = (y + (ty - y - th) / 2.0).max(y);
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: tx, top: cy, right: tx + tw, bottom: cy + th },
            radiusX: th / 2.0,
            radiusY: th / 2.0,
        };
        let color = if on { self.theme.ok } else { self.theme.border };
        let b = self.brush(target, color, alpha);
        target.FillRoundedRectangle(&r, &b);
        // 圆钮
        let knob = th - 4.0;
        let kx = if on { tx + tw - knob - 2.0 } else { tx + 2.0 };
        let kb = self.brush(target, [1.0, 1.0, 1.0, 1.0], alpha);
        let ellipse = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
            point: Vector2 { X: kx + knob / 2.0, Y: cy + th / 2.0 },
            radiusX: knob / 2.0,
            radiusY: knob / 2.0,
        };
        target.FillEllipse(&ellipse, &kb);
        self.hits.push((hit, D2D_RECT_F { left: x, top: y, right: x + w, bottom: ty + 2.0 }));
        ty + 9.0
    }}

    unsafe fn segmented_raw(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        items: &[(Hit, &str)],
        selected: impl Fn(&Hit) -> bool,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 { unsafe {
        let h = 30.0;
        let n = items.len().max(1) as f32;
        let seg_w = w / n;
        // 透明轨道 + hairline 描边（Stone 1px，4px 圆角）
        let track = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        let tb = self.brush(target, self.theme.border, alpha * 0.9);
        target.DrawRoundedRectangle(&track, &tb, 1.0, None);
        for (i, (hit, label)) in items.iter().enumerate() {
            let sel = selected(hit);
            if sel {
                // 选中段：Ink 填充（「深墨块压在纸上」）
                let seg = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: x + i as f32 * seg_w + 2.0,
                        top: y + 2.0,
                        right: x + (i as f32 + 1.0) * seg_w - 2.0,
                        bottom: y + h - 2.0,
                    },
                    radiusX: 2.5,
                    radiusY: 2.5,
                };
                let sb = self.brush(target, self.theme.action, alpha);
                target.FillRoundedRectangle(&seg, &sb);
            }
            let color = if sel { self.theme.action_text } else { self.theme.text_secondary };
            let tx = x + i as f32 * seg_w;
            let rect = D2D_RECT_F { left: tx, top: y + 5.0, right: tx + seg_w, bottom: y + h - 2.0 };
            // 选项文字在段内居中（视觉平衡）
            self.text_aligned(target, label, &rect, 12.0, 400, color, alpha, 1, false);
            self.hits.push((*hit, D2D_RECT_F { left: tx, top: y, right: tx + seg_w, bottom: y + h }));
        }
        y + h + 10.0
    }}

    unsafe fn account_row(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        remove: Hit,
        label: &str,
        selected: bool,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) { unsafe {
        // 选中态：Linen 底 + Forest 左侧 2px 标记（selected navigation 手势）
        if selected {
            let r = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: x - 6.0, top: y - 2.0, right: w - 8.0, bottom: y + 26.0 },
                radiusX: RADIUS,
                radiusY: RADIUS,
            };
            let b = self.brush(target, self.theme.track, alpha);
            target.FillRoundedRectangle(&r, &b);
            let mark = self.brush(target, self.theme.ok, alpha);
            self.line(target, x - 2.0, y + 3.0, x - 2.0, y + 21.0, &mark, 2.0);
        }
        self.text(target, label, x, y + 5.0, w - 60.0, 20.0, 14.0, if selected { 500 } else { 400 }, self.theme.text_primary, alpha);
        // 删除 ×
        self.x_button(target, remove, w - x - 24.0, y + 8.0);
        self.hits.push((hit, D2D_RECT_F { left: x - 6.0, top: y - 2.0, right: w - 34.0, bottom: y + 28.0 }));
    }}

    /// 按钮：primary = Ink 填充（画布上的深墨块）；次级 = Linen 填充。
    /// 4px 圆角、weight 400——按钮不加粗，靠明度对比立住。
    #[allow(clippy::too_many_arguments)]
    unsafe fn pill_button(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        label: &str,
        alpha: f32,
        primary: bool,
    ) { unsafe {
        let hovered = self.hover == Some(hit);
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        let (base, fill_alpha, fg) = if primary {
            // 主按钮 hover 轻微透纸（alpha 呼吸，不变色）
            (self.theme.action, if hovered { alpha * 0.86 } else { alpha }, self.theme.action_text)
        } else {
            // 次级：Linen，hover 沉一档到 Stone
            (
                if hovered { self.theme.border } else { self.theme.track },
                alpha,
                self.theme.text_primary,
            )
        };
        let b = self.brush(target, base, fill_alpha);
        target.FillRoundedRectangle(&r, &b);
        let rect = D2D_RECT_F { left: x, top: y + 5.0, right: x + w, bottom: y + h - 4.0 };
        // 按钮文字居中对齐
        self.text_aligned(target, label, &rect, 13.0, 400, fg, alpha, 1, false);
        self.hits.push((hit, D2D_RECT_F { left: x - 4.0, top: y - 4.0, right: x + w + 4.0, bottom: y + h + 4.0 }));
    }}

    /// 描边小按钮：透明底 + hairline 边框，与同行文字（如版本号）视觉平衡。
    #[allow(clippy::too_many_arguments)]
    unsafe fn outline_button(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        label: &str,
        alpha: f32,
    ) { unsafe {
        let hovered = self.hover == Some(hit);
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        if hovered {
            let b = self.brush(target, self.theme.track, alpha);
            target.FillRoundedRectangle(&r, &b);
        }
        let edge = self.brush(target, self.theme.border, alpha * 0.9);
        target.DrawRoundedRectangle(&r, &edge, 1.0, None);
        let fg = if hovered { self.theme.accent } else { self.theme.text_secondary };
        let rect = D2D_RECT_F { left: x, top: y + 5.0, right: x + w, bottom: y + h - 4.0 };
        self.text_aligned(target, label, &rect, 12.0, 400, fg, alpha, 1, false);
        self.hits.push((hit, D2D_RECT_F { left: x - 4.0, top: y - 4.0, right: x + w + 4.0, bottom: y + h + 4.0 }));
    }}

    /// 套餐徽标：Linen 底 + Ember 等宽文字（tag 风）。
    unsafe fn badge(&mut self, target: &ID2D1HwndRenderTarget, label: &str, x: f32, y: f32, alpha: f32) { unsafe {
        // 等宽 11px 的字宽近似，中文按 1.7 倍计
        let units: f32 = label
            .chars()
            .map(|c| if c.is_ascii() { 1.0 } else { 1.7 })
            .sum();
        let w = units * 6.8 + 20.0;
        let h = 22.0;
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        let b = self.brush(target, self.theme.track, alpha);
        target.FillRoundedRectangle(&r, &b);
        let rect = D2D_RECT_F { left: x, top: y + 3.0, right: x + w, bottom: y + h - 3.0 };
        self.text_rect_opts(target, label, &rect, 11.0, 500, self.theme.accent, alpha, false, true);
    }}

    unsafe fn icon_button(&mut self, target: &ID2D1HwndRenderTarget, hit: Hit, cx: f32, cy: f32, r: f32, spin: f32) { unsafe {
        let hovered = self.hover == Some(hit);
        // 圆形底
        let ellipse = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
            point: Vector2 { X: cx, Y: cy },
            radiusX: r,
            radiusY: r,
        };
        let base = if hovered { self.theme.track } else { [0.0, 0.0, 0.0, 0.0] };
        if base[3] > 0.0 {
            let b = self.brush(target, base, 1.0);
            target.FillEllipse(&ellipse, &b);
        }
        // 刷新图标（refresh-cw）：两段弧 + 箭头头，一眼可辨；spin 为旋转角
        let rot = |px: f32, py: f32| -> (f32, f32) {
            let (sin, cos) = spin.sin_cos();
            (cx + px * cos - py * sin, cy + px * sin + py * cos)
        };
        let stroke = self.brush(target, self.theme.text_secondary, 1.0);
        let rr = r * 0.47; // 与滑杆图标的视觉尺寸平衡
        // 弧段与箭头顶点先在局部坐标生成，再统一应用旋转
        let mut segs: Vec<(f32, f32, f32, f32)> = Vec::new();
        for (a0, a1) in [(-150f32, -20f32), (30f32, 160f32)] {
            let (r0, r1) = (a0.to_radians(), a1.to_radians());
            let steps = 12;
            for i in 0..steps {
                let t0 = r0 + (r1 - r0) * i as f32 / steps as f32;
                let t1 = r0 + (r1 - r0) * (i + 1) as f32 / steps as f32;
                segs.push((rr * t0.cos(), rr * t0.sin(), rr * t1.cos(), rr * t1.sin()));
            }
            // 箭头头：末端切向 F（θ 增方向），两翼自 F 反向张开 30°
            let (fx, fy) = (-r1.sin(), r1.cos());
            let (px, py) = (rr * r1.cos(), rr * r1.sin());
            let al = 4.0;
            let (fs, fc) = 150f32.to_radians().sin_cos();
            segs.push((px, py, px + (fx * fc - fy * fs) * al, py + (fx * fs + fy * fc) * al));
            segs.push((px, py, px + (fx * fc + fy * fs) * al, py + (-fx * fs + fy * fc) * al));
        }
        for (x0, y0, x1, y1) in segs {
            let (ax, ay) = rot(x0, y0);
            let (bx, by) = rot(x1, y1);
            self.line(target, ax, ay, bx, by, &stroke, 1.6);
        }
        self.hits.push((hit, D2D_RECT_F { left: cx - r - 4.0, top: cy - r - 4.0, right: cx + r + 4.0, bottom: cy + r + 4.0 }));
    }}

    /// 设置入口：滑杆图标（三条横线各骑一个圆点，错落分布）。
    /// 细线形态在 16px 下依然清晰，齿轮在这个尺寸会糊成一团。
    unsafe fn sliders(&mut self, target: &ID2D1HwndRenderTarget, hit: Hit, cx: f32, cy: f32, r: f32) { unsafe {
        let hovered = self.hover == Some(hit);
        let base = if hovered { self.theme.track } else { [0.0, 0.0, 0.0, 0.0] };
        if base[3] > 0.0 {
            let ellipse = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                point: Vector2 { X: cx, Y: cy },
                radiusX: r,
                radiusY: r,
            };
            let b = self.brush(target, base, 1.0);
            target.FillEllipse(&ellipse, &b);
        }
        let stroke = self.brush(target, self.theme.text_secondary, 1.0);
        let hole_c = if hovered {
            self.theme.track
        } else {
            let bg = self.theme.bg;
            [bg[0], bg[1], bg[2], 1.0]
        };
        let hole = self.brush(target, hole_c, 1.0);
        let half = r * 0.62; // 线半长（放大到与刷新图标的视觉量级一致）
        let dot_r = r * 0.16;
        // 三行：y 偏移与圆点 x 位置错落
        let rows = [(-0.40f32, -0.20f32), (0.0, 0.22), (0.40, -0.12)];
        for (dy, dx) in rows {
            let ly = cy + dy * r;
            self.line(target, cx - half, ly, cx + half, ly, &stroke, 1.5);
            let (px, py) = (cx + dx * r, ly);
            // 圆点：底色挖空 + 线色描边（骑在线上的空心圆）
            let he = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                point: Vector2 { X: px, Y: py },
                radiusX: dot_r,
                radiusY: dot_r,
            };
            target.FillEllipse(&he, &hole);
            target.DrawEllipse(&he, &stroke, 1.5, None);
        }
        self.hits.push((hit, D2D_RECT_F { left: cx - r - 4.0, top: cy - r - 4.0, right: cx + r + 4.0, bottom: cy + r + 4.0 }));
    }}

    unsafe fn back_arrow(&mut self, target: &ID2D1HwndRenderTarget, hit: Hit, x: f32, y: f32) { unsafe {
        // 中性灰细线箭头（Ember 只留给文字强调，不做图标）
        let stroke = self.brush(target, self.theme.text_secondary, 1.0);
        let (cx, cy) = (x + 8.0, y + 6.0);
        self.line(target, cx + 5.0, cy - 6.0, cx - 4.0, cy, &stroke, 1.8);
        self.line(target, cx - 4.0, cy, cx + 5.0, cy + 6.0, &stroke, 1.8);
        self.hits.push((hit, D2D_RECT_F { left: x - 6.0, top: y - 6.0, right: x + 24.0, bottom: y + 20.0 }));
    }}

    unsafe fn x_button(&mut self, target: &ID2D1HwndRenderTarget, hit: Hit, x: f32, y: f32) { unsafe {
        let stroke = self.brush(target, self.theme.text_tertiary, 1.0);
        self.line(target, x, y, x + 10.0, y + 10.0, &stroke, 1.4);
        self.line(target, x + 10.0, y, x, y + 10.0, &stroke, 1.4);
        self.hits.push((hit, D2D_RECT_F { left: x - 6.0, top: y - 6.0, right: x + 16.0, bottom: y + 16.0 }));
    }}

    unsafe fn line(
        &self,
        target: &ID2D1HwndRenderTarget,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        brush: &ID2D1SolidColorBrush,
        width: f32,
    ) { unsafe {
        target.DrawLine(
            Vector2 { X: x0, Y: y0 },
            Vector2 { X: x1, Y: y1 },
            brush,
            width,
            None,
        );
    }}

    unsafe fn brush(&self, target: &ID2D1HwndRenderTarget, c: [f32; 4], alpha: f32) -> ID2D1SolidColorBrush { unsafe {
        target
            .CreateSolidColorBrush(
                &windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F {
                    r: c[0], g: c[1], b: c[2], a: (c[3] * alpha).clamp(0.0, 1.0),
                },
                None,
            )
            .unwrap_or_else(|_| self.black.clone().expect("fallback brush"))
    }}

    unsafe fn text(
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
    ) { unsafe {
        let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h };
        self.text_rect(target, s, &rect, size, weight, color, alpha);
    }}

    /// 等宽右对齐（数值 / 元数据）。
    #[allow(clippy::too_many_arguments)]
    unsafe fn text_mono_r(
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
    ) { unsafe {
        let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h };
        self.text_rect_opts(target, s, &rect, size, weight, color, alpha, true, true);
    }}

    unsafe fn text_rect(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        rect: &D2D_RECT_F,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
    ) { unsafe {
        self.text_rect_opts(target, s, rect, size, weight, color, alpha, false, false);
    }}

    unsafe fn text_rect_opts(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        rect: &D2D_RECT_F,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
        right: bool,
        mono: bool,
    ) { unsafe {
        self.text_aligned(target, s, rect, size, weight, color, alpha, if right { 2 } else { 0 }, mono);
    }}

    /// 对齐方式：0=左（leading）、1=居中、2=右（trailing）。mono 选择等宽字体。
    unsafe fn text_aligned(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        rect: &D2D_RECT_F,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
        align: u8,
        mono: bool,
    ) { unsafe {
        let Some(fmt) = self.format(size, weight, mono) else { return };
        let align_set = match align {
            1 => DWRITE_TEXT_ALIGNMENT_CENTER,
            2 => DWRITE_TEXT_ALIGNMENT_TRAILING,
            _ => DWRITE_TEXT_ALIGNMENT_LEADING,
        };
        let _ = fmt.SetTextAlignment(align_set);
        let brush = self.brush(target, color, alpha);
        let w16: Vec<u16> = s.encode_utf16().collect();
        if !w16.is_empty() {
            let _ = target.DrawText(
                &w16,
                &fmt,
                rect as *const D2D_RECT_F,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS(0),
                windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        let _ = fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
    }}

    /// 应用 logo（assets/logo.svg 的矢量重绘）：圆角磁贴 + 白色 Z 字形。
    /// 几何按 30×30 viewBox 构建一次，绘制时以矩阵缩放平移，任意 DPI 无损。
    unsafe fn logo(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        x: f32,
        y: f32,
        size: f32,
        alpha: f32,
    ) { unsafe {
        // 磁贴底（圆角约 4/30）
        let tile = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + size, bottom: y + size },
            radiusX: size * (4.0 / 30.0),
            radiusY: size * (4.0 / 30.0),
        };
        let tb = self.brush(target, self.theme.logo_tile, alpha);
        target.FillRoundedRectangle(&tile, &tb);

        // Z 字形几何（懒构建）
        if self.logo_geo.is_none() {
            self.logo_geo = self.build_logo_glyph();
        }
        let Some(geo) = self.logo_geo.clone() else { return };
        let zb = self.brush(target, [1.0, 1.0, 1.0, 1.0], alpha);
        let m = Matrix3x2 {
            M11: size / 30.0, M12: 0.0,
            M21: 0.0, M22: size / 30.0,
            M31: x, M32: y,
        };
        target.SetTransform(&m);
        target.FillGeometry(&geo, &zb, None);
        target.SetTransform(&Matrix3x2::identity());
    }}

    /// 构建白色 Z 字形路径（坐标取自 logo.svg 的三段图形单位）。
    fn build_logo_glyph(&self) -> Option<ID2D1PathGeometry> {
        unsafe {
            let geo = self.factory.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            // 上横杠（右端斜切 + 圆角过渡）
            sink.BeginFigure(Vector2 { X: 15.47, Y: 7.10 }, D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(Vector2 { X: 14.17, Y: 8.95 });
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: Vector2 { X: 13.97, Y: 9.24 },
                point2: Vector2 { X: 13.63, Y: 9.42 },
                point3: Vector2 { X: 13.27, Y: 9.42 },
            });
            sink.AddLine(Vector2 { X: 6.17, Y: 9.42 });
            sink.AddLine(Vector2 { X: 6.17, Y: 7.09 });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            // 对角斜杠
            sink.BeginFigure(Vector2 { X: 24.30, Y: 7.10 }, D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(Vector2 { X: 13.14, Y: 22.91 });
            sink.AddLine(Vector2 { X: 5.70, Y: 22.91 });
            sink.AddLine(Vector2 { X: 16.86, Y: 7.10 });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            // 下横杠（左端斜切 + 圆角过渡）
            sink.BeginFigure(Vector2 { X: 14.53, Y: 22.91 }, D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(Vector2 { X: 15.84, Y: 21.05 });
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: Vector2 { X: 16.04, Y: 20.76 },
                point2: Vector2 { X: 16.38, Y: 20.58 },
                point3: Vector2 { X: 16.74, Y: 20.58 },
            });
            sink.AddLine(Vector2 { X: 23.83, Y: 20.58 });
            sink.AddLine(Vector2 { X: 23.83, Y: 22.91 });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close().ok()?;
            Some(geo)
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Default for AnimState {
    fn default() -> Self {
        Self::new()
    }
}
