//! 面板内容渲染

#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::HashMap;

use windows::core::PCWSTR;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_END_CLOSED, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows_numerics::{Matrix3x2, Vector2};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, D2D1_DRAW_TEXT_OPTIONS, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_ROUNDED_RECT, D2D1_CAP_STYLE_FLAT,
    D2D1_DASH_STYLE_DASH, D2D1_STROKE_STYLE_PROPERTIES,
    ID2D1HwndRenderTarget, ID2D1Factory, ID2D1PathGeometry, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, DWRITE_FACTORY_TYPE_SHARED, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Foundation::HWND;

use super::anim::{Tween, animations_allowed, ease_out_cubic};
use super::layout;
use super::model::PanelModel;
use super::theme::{RADIUS, Theme};
use super::{Panel, PanelView};
use crate::api::FetchError;
use crate::api::client::Platform;
use crate::ui::fmt;
use crate::ui::i18n::Strings;

/// 外观模式选择：System = 跟随系统
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppearanceChoice {
    System,
    Light,
    Dark,
}

/// 界面语言选择：System = 跟随系统
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageChoice {
    System,
    Zh,
    En,
}

/// 添加账号的类型选择：个人版 / 团队版，团队仅国内站
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeChoice {
    Personal,
    Team,
}

/// 可点击元素标识，用于命中检测
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Refresh,
    Settings,
    AccountSwitch,
    Retry,
    Back,
    IntervalPreset(u64),
    CustomizeInterval,
    ApplyInterval,
    InputName,
    InputKey,
    InputInterval,
    InputOrg,
    InputProject,
    Language(LanguageChoice),
    Appearance(AppearanceChoice),
    Platform(Platform),
    AccountType(ScopeChoice),
    SaveAccount,
    ToggleThreshold,
    ToggleReset5h,
    ToggleResetWeekly,
    ToggleAutostart,
    AddAccount,
    RemoveAccount(usize),
    CheckUpdate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

/// 进度条等数值的动画插值状态
pub struct AnimState {
    pub appear: Option<Tween>,
    pub spin: f32,
}

impl AnimState {
    pub fn new() -> Self {
        Self { appear: None, spin: 0.0 }
    }
}

impl Renderer {
    /// 刷新按钮动画是否仍在进行
    pub fn spin_remaining(&mut self) -> bool {
        if self.anim.spin > 0.0 {
            self.anim.spin = (self.anim.spin - 0.15).max(0.0);
            true
        } else {
            false
        }
    }

    /// 触发一次刷新按钮旋转
    pub fn start_spin(&mut self) {
        self.anim.spin = std::f32::consts::TAU;
    }

    /// 鼠标位置（逻辑像素）命中的可点击元素；未命中返回 None
    pub fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .find(|(_, rc)| x >= rc.left && x <= rc.right && y >= rc.top && y <= rc.bottom)
            .map(|(h, _)| *h)
    }
}

/// D2D/DWrite 渲染器
pub struct Renderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: Option<ID2D1HwndRenderTarget>,
    black: Option<ID2D1SolidColorBrush>,
    formats: HashMap<(u32, u16, bool), IDWriteTextFormat>,
    pub theme: Theme,
    pub hits: Vec<(Hit, D2D_RECT_F)>,
    pub hover: Option<Hit>,
    pub anim: AnimState,
    font_fallback: bool,
    target_dpi: f32,
    anim_allowed: bool,
    logo_geo: Option<ID2D1PathGeometry>,
    dash_style: Option<ID2D1StrokeStyle>,
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
                anim_allowed: animations_allowed(),
                logo_geo: None,
                dash_style: None,
            })
        }
    }

    fn format(&mut self, size: f32, weight: u16, mono: bool) -> Option<IDWriteTextFormat> {
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

    /// 绘制一帧；`rect_phys` 为物理像素，绘制全程用逻辑像素 DIP。
    /// 须用 HwndRenderTarget——DC RenderTarget 纯软件光栅，不可改用
    pub fn paint(
        &mut self,
        hwnd: HWND,
        rect_phys: &RECT,
        panel: &Panel,
        model: &PanelModel,
        view: PanelView,
        dpi: f32,
    ) {
        unsafe {
            let dpi = if dpi.is_finite() && dpi >= 1.0 { dpi } else { 1.0 };
            let w_px = (rect_phys.right - rect_phys.left).max(1) as u32;
            let h_px = (rect_phys.bottom - rect_phys.top).max(1) as u32;
            // 尺寸/DPI 变化时处理；内部尺寸记录以 GetPixelSize 读数为准
            let need_rebuild = match self.target.as_ref() {
                Some(t) => {
                    let sz = t.GetPixelSize();
                    sz.width != w_px || sz.height != h_px || self.target_dpi != dpi
                }
                None => true,
            };
            if need_rebuild {
                // 仅尺寸变化优先 Resize 复用——整建重分配后台缓冲引发顿挫
                let size_only = self.target.is_some() && self.target_dpi == dpi;
                let resized = size_only
                    && self
                        .target
                        .as_ref()
                        .is_some_and(|t| t.Resize(&D2D_SIZE_U { width: w_px, height: h_px }).is_ok());
                if !resized {
                    self.target = None;
                    let pf = D2D1_PIXEL_FORMAT {
                        format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_IGNORE,
                    };
                    // DPI 经创建 props 显式给定
                    let props = D2D1_RENDER_TARGET_PROPERTIES {
                        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                        pixelFormat: pf,
                        dpiX: dpi * 96.0,
                        dpiY: dpi * 96.0,
                        ..Default::default()
                    };
                    let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                        hwnd,
                        pixelSize: D2D_SIZE_U { width: w_px, height: h_px },
                        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                    };
                    match self.factory.CreateHwndRenderTarget(&props, &hwnd_props) {
                        Ok(target) => {
                            let black = target
                                .CreateSolidColorBrush(
                                    &D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 },
                                    None,
                                )
                                .ok();
                            self.black = black;
                            self.target = Some(target);
                            self.target_dpi = dpi;
                        }
                        Err(e) => {
                            crate::platform::log(&format!("[Quotify] CreateHwndRenderTarget 失败: {e}"));
                            return;
                        }
                    }
                }
            }
            let Some(target) = self.target.clone() else { return };
            let rect_logical = RECT {
                left: 0,
                top: 0,
                right: ((rect_phys.right - rect_phys.left) as f32 / dpi).round() as i32,
                bottom: ((rect_phys.bottom - rect_phys.top) as f32 / dpi).round() as i32,
            };
            self.hits.clear();
            target.BeginDraw();
            self.draw(&target, panel, model, view, &rect_logical);
            match target.EndDraw(None, None) {
                Ok(()) => {}
                Err(e) => {
                    // 设备丢失：丢弃 target 与设备绑定资源，下帧整建——不清理则面板永久空白
                    crate::platform::log(&format!("[Quotify] EndDraw 失败: {e}"));
                    self.target = None;
                    self.black = None;
                    self.logo_geo = None;
                    self.dash_style = None;
                }
            }
        }
    }

    unsafe fn draw(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        panel: &Panel,
        model: &PanelModel,
        view: PanelView,
        rect: &RECT,
    ) {
        let w = (rect.right - rect.left) as f32;
        let h = (rect.bottom - rect.top) as f32;

        // 弹出动画：内容上浮渐入；背景必须不透明全幅绘制，否则首帧露交换链、错位抖动
        let (dy, alpha) = match &self.anim.appear {
            Some(t) if self.anim_allowed => {
                let p = ease_out_cubic(t.progress());
                ((1.0 - p) * 6.0, p)
            }
            _ => (0.0, 1.0),
        };

        let bg = self.theme.bg;
        let bg_brush = self.brush(target, bg, 1.0);
        let bg_rect = D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h };
        target.FillRectangle(&bg_rect, &bg_brush);

        match view {
            PanelView::Main => self.draw_main(target, model, w, h, dy, alpha),
            PanelView::Settings => self.draw_settings(target, panel, model, w, dy, alpha),
        }
    }

    /// 主视图
    unsafe fn draw_main(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        w: f32,
        h: f32,
        dy: f32,
        alpha: f32,
    ) {
        let s = model.strings;
        let pad = 20.0;
        let mut y = dy + 16.0;
        let snap = model.snapshot;

        // ── 顶栏：账号名 + 套餐副标题 ──
        let title = model.account.map(|a| a.name).unwrap_or("Quotify");
        // 副标题只留代际与档位，形如「V3 · Max」——产品名由图标承担
        let meta = snap.and_then(|s| {
            let v = s.plan_version.label();
            let tier = {
                let t = s.tier.label();
                if t.is_empty() { s.plan_label.clone().unwrap_or_default() } else { t.to_string() }
            };
            match (v.is_empty(), tier.is_empty()) {
                (false, false) => Some(format!("{v} · {tier}")),
                (false, true) => Some(v.to_string()),
                (true, false) => Some(tier),
                (true, true) => None,
            }
        });
        let (logo_size, logo_y) = if meta.is_some() { (32.0, y + 5.0) } else { (22.0, y + 1.0) };
        self.logo(target, pad, logo_y, logo_size, alpha);
        let tx = pad + logo_size + 10.0;
        let tw = w - tx - 88.0;
        let title_disp = ellipsize_px(title, 16.0, tw);
        self.text(target, &title_disp, tx, y + 2.0, tw, 22.0, 16.0, 500, self.theme.text_primary, alpha);
        if let Some(m) = &meta {
            self.text(target, m, tx + 1.0, y + 24.0, tw, 17.0, 12.0, 400, self.theme.text_secondary, alpha);
        }
        let btn_r = 16.0;
        let refresh_cx = w - pad - btn_r - 30.0;
        let settings_cx = w - pad - btn_r;
        let btn_cy = if snap.is_some() { y + 19.0 } else { y + 12.0 };
        self.icon_button(target, Hit::Refresh, refresh_cx, btn_cy, btn_r, self.anim.spin);
        self.sliders(target, Hit::Settings, settings_cx, btn_cy, btn_r);
        if model.accounts_count > 1 {
            self.hits.push((
                Hit::AccountSwitch,
                D2D_RECT_F { left: pad, top: y, right: w - 110.0, bottom: y + if snap.is_some() { 42.0 } else { 26.0 } },
            ));
        }
        // 与 sync_main_height 同用 has_meta 谓词，否则内容会与 footer 重叠
        y += if snap.is_some_and(|s| s.has_meta()) { 52.0 } else { 38.0 };

        // ── 数据态 ──
        match (snap, model.error) {
            (None, None) => {
                // 空态：居中全局提示，遵循 Apple 空态规范——图形 + 标题 + 副标题
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
                let title_rect = D2D_RECT_F { left: pad, top: top + 66.0, right: w - pad, bottom: top + 66.0 + 28.0 };
                self.text_aligned(target, t1, &title_rect, 21.0, 400, self.theme.text_primary, alpha, Align::Center, false);
                if has_sub {
                    let sub_rect = D2D_RECT_F { left: pad + 12.0, top: top + 96.0, right: w - pad - 12.0, bottom: top + 96.0 + 40.0 };
                    self.text_aligned(target, t2, &sub_rect, 14.0, 400, self.theme.text_secondary, alpha, Align::Center, false);
                }
            }
            (None, Some(e)) => {
                // 错误卡：danger 染底 + hairline 边，居中信息 + 重试
                let msg = error_text(s, e);
                let body_top = y + 10.0;
                let body_h = (dy + h - 16.0) - body_top;
                let cx = w / 2.0;
                let card_h = 96.0;
                let card_top = body_top + (body_h - card_h) / 2.0;
                let card = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F { left: pad, top: card_top, right: w - pad, bottom: card_top + card_h },
                    radiusX: RADIUS,
                    radiusY: RADIUS,
                };
                let fill = self.brush(
                    target,
                    [self.theme.danger[0], self.theme.danger[1], self.theme.danger[2], 0.06],
                    alpha,
                );
                target.FillRoundedRectangle(&card, &fill);
                let edge = self.brush(
                    target,
                    [self.theme.danger[0], self.theme.danger[1], self.theme.danger[2], 0.35],
                    alpha,
                );
                target.DrawRoundedRectangle(&card, &edge, 1.0, None);
                let title_rect = D2D_RECT_F { left: pad + 12.0, top: card_top + 18.0, right: w - pad - 12.0, bottom: card_top + 40.0 };
                self.text_aligned(target, &msg, &title_rect, 13.0, 500, self.theme.danger, alpha, Align::Center, false);
                self.outline_button(target, Hit::Retry, cx - 44.0, card_top + 50.0, 88.0, 28.0, s.retry, alpha);
            }
            _ => {
                if let Some(snap) = snap {
                    // 区块刊头：hairline + 墨色强调块 + 标题，与设置页同款
                    self.divider(target, pad, y + 2.0, w - pad * 2.0, alpha);
                    y += 14.0;
                    let bar = self.brush(target, self.theme.text_primary, alpha * 0.9);
                    target.FillRectangle(
                        &D2D_RECT_F { left: pad, top: y + 1.0, right: pad + 3.0, bottom: y + 13.0 },
                        &bar,
                    );
                    self.text(target, s.usage_section, pad + 7.0, y, w - pad * 2.0 - 7.0, 17.0, 12.0, 600, self.theme.text_tertiary, alpha);
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
                        y = self.metric_row(target, s.five_hour, b.used_percent, detail_of(b.current, b.total), b.resets_at, y, w, alpha, model.lang);
                    }
                    if let Some(b) = snap.weekly.as_ref() {
                        y = self.metric_row(target, s.weekly, b.used_percent, detail_of(b.current, b.total), b.resets_at, y, w, alpha, model.lang);
                    }
                    if let Some(m) = snap.mcp.as_ref() {
                        let detail = if m.total > 0.0 {
                            Some(s.used_of
                                .replace("{cur}", &fmt::compact_number(m.current_value))
                                .replace("{tot}", &fmt::compact_number(m.total)))
                        } else {
                            None
                        };
                        y = self.metric_row(target, s.mcp_tools, m.used_percent, detail, m.resets_at, y, w, alpha, model.lang);
                    }

                    // 余额为国内版功能：撕线之下的「总额」行——刊头带强调块，右侧等宽数值
                    if let Some(b) = &snap.balance {
                        y += 6.0;
                        self.dashed_divider(target, pad, y, w - pad * 2.0, alpha);
                        y += 14.0;
                        let bar = self.brush(target, self.theme.text_primary, alpha * 0.9);
                        target.FillRectangle(
                            &D2D_RECT_F { left: pad, top: y + 1.0, right: pad + 3.0, bottom: y + 13.0 },
                            &bar,
                        );
                        self.text(target, s.balance_label, pad + 7.0, y, 140.0, 17.0, 12.0, 600, self.theme.text_tertiary, alpha);
                        let line = format!("¥{:.2}", b.available);
                        self.text_mono_r(target, &line, w - pad - 140.0, y, 140.0, 18.0, 12.0, 500, self.theme.text_primary, alpha);
                    }
                }
                let footer_y = dy + h - 36.0;
                if let Some(e) = model.error {
                    let msg = error_text(s, e);
                    let line = match model.snapshot {
                        Some(snap) => format!(
                            "{} · {} {msg}",
                            s.data_as_of.replace("{t}", &fmt::as_of_time(snap.queried_at)),
                            s.fetch_failed,
                        ),
                        None => msg,
                    };
                    self.text(target, &line, pad, footer_y + 6.0, w - pad * 2.0 - 74.0, 30.0, 12.0, 400, self.theme.text_secondary, alpha);
                    self.outline_button(target, Hit::Retry, w - pad - 62.0, footer_y + 4.0, 62.0, 26.0, s.retry, alpha);
                } else if let Some(snap) = model.snapshot {
                    let fresh = (chrono::Local::now() - snap.queried_at).num_seconds() < 60;
                    let text = if fresh {
                        s.updated_just_now.to_string()
                    } else {
                        s.updated_ago.replace("{t}", &fmt::ago(snap.queried_at, model.lang))
                    };
                    self.text_aligned(
                        target,
                        &text,
                        &D2D_RECT_F { left: pad, top: footer_y + 8.0, right: w - pad, bottom: footer_y + 25.0 },
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
        let pad = 20.0;
        let strings = lang.strings();
        let critical = used_percent >= 90.0;
        // 先拷贝所需颜色：后续 text/brush 调用要 &mut self，不能持有 theme 引用
        let (c_label, c_value, c_caption, c_track) = (
            self.theme.text_primary,
            if critical { self.theme.danger } else { self.theme.text_primary },
            self.theme.text_tertiary,
            self.theme.track,
        );

        self.text(target, label, pad, y + 2.0, w - pad * 2.0 - 110.0, 19.0, 14.0, 400, c_label, alpha);
        let pct = fmt::percent(used_percent);
        self.text_mono_r(target, &pct, w - pad - 100.0, y + 1.0, 100.0, 19.0, 14.0, 600, c_value, alpha);

        // 5px 直角细条，与面板锐利纸感一致；critical 转警示色
        let bar_h = 5.0;
        let bar_y = y + 22.0;
        let track = D2D_RECT_F { left: pad, top: bar_y, right: w - pad, bottom: bar_y + bar_h };
        let track_brush = self.brush(target, c_track, alpha);
        target.FillRectangle(&track, &track_brush);
        let frac = (used_percent / 100.0).clamp(0.0, 1.0) as f32;
        if frac > 0.004 {
            // 最小可见长度 2px，避免 0.x% 时出现针尖
            let fg_w = ((w - pad * 2.0) * frac).max(2.0);
            let fg = D2D_RECT_F { left: pad, top: bar_y, right: pad + fg_w, bottom: bar_y + bar_h };
            let fill = self.brush(target, c_value, alpha);
            target.FillRectangle(&fg, &fill);
        }

        if let Some(r) = resets_at {
            let line = strings.resets_line.replace("{t}", &fmt::countdown(r, lang));
            self.text(target, &line, pad, bar_y + bar_h + 5.0, (w - pad * 2.0) * 0.55, 16.0, 11.0, 400, c_caption, alpha);
        }
        if let Some(d) = detail {
            self.text_rect_opts(
                target,
                &d,
                &D2D_RECT_F { left: pad + (w - pad * 2.0) * 0.55, top: bar_y + bar_h + 5.0, right: w - pad, bottom: bar_y + bar_h + 21.0 },
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

    /// 设置视图
    unsafe fn draw_settings(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        panel: &Panel,
        model: &PanelModel,
        w: f32,
        dy: f32,
        alpha: f32,
    ) {
        let s = model.strings;
        let pad = 20.0;
        let cw = w - pad * 2.0; // 内容区宽度
        let mut y = dy + 12.0;

        // ── 导航栏：设置页左上返回；添加页返回由底部「取消」承担，箭头含义易误解 ──
        if !panel.adding_account {
            self.back_arrow(target, Hit::Back, pad, y + 6.0);
        }
        let nav_title = if panel.adding_account { s.add_account } else { s.settings };
        let title_rect = D2D_RECT_F { left: pad, top: y, right: w - pad, bottom: y + 26.0 };
        self.text_aligned(target, nav_title, &title_rect, 16.0, 400, self.theme.text_primary, alpha, Align::Center, false);
        y += 30.0;

        // ── 账号：设置页首项即数据来源；添加页此处标题用「版本」──
        let section = if panel.adding_account { s.platform_section } else { s.accounts_section };
        y = self.section_label(target, section, pad, y, w, alpha, false);
        if panel.adding_account {
            // 添加流程：平台 → 类型 → 名称/key，团队版追加组织/项目 ID
            y = self.sub_label(target, s.account_platform, pad, y, cw, alpha);
            let plats: [(Hit, &str); 2] = [
                (Hit::Platform(Platform::Cn), s.platform_cn),
                (Hit::Platform(Platform::Intl), s.platform_intl),
            ];
            let cur_plat = panel.pending_platform;
            y = self.segmented_raw(
                target,
                &plats,
                |h| matches!(h, Hit::Platform(v) if cur_plat == *v),
                pad,
                y,
                cw,
                alpha,
            );
            y = self.sub_label(target, s.account_type_label, pad, y, cw, alpha);
            let team = panel.pending_team;
            let types: [(Hit, &str); 2] = [
                (Hit::AccountType(ScopeChoice::Personal), s.type_personal),
                (Hit::AccountType(ScopeChoice::Team), s.type_team),
            ];
            y = self.segmented_raw(
                target,
                &types,
                |h| matches!(h, Hit::AccountType(v) if team == matches!(*v, ScopeChoice::Team)),
                pad,
                y,
                cw,
                alpha,
            );
            // y 钉在 layout 常量：绘制、光标、高度公式三方同源
            let input = &panel.input;
            self.sub_label(target, s.account_name, pad, y, cw, alpha);
            let name_y = dy + layout::ADD_NAME_Y;
            self.input_field(target, Hit::InputName, layout::INPUT_X, name_y, cw, &input.name, input.field == Some(super::InputField::Name), alpha);
            y = name_y + layout::INPUT_H + layout::INPUT_GAP;
            self.sub_label(target, s.api_key_label, pad, y, cw, alpha);
            let key_y = dy + layout::ADD_KEY_Y;
            self.input_field(target, Hit::InputKey, layout::INPUT_X, key_y, cw, &input.key, input.field == Some(super::InputField::Key), alpha);
            y = key_y + layout::INPUT_H + layout::INPUT_GAP;
            if team {
                // 团队版：组织 / 项目 ID（请求头 Bigmodel-Organization / Bigmodel-Project）
                self.sub_label(target, s.org_id_label, pad, y, cw, alpha);
                let org_y = dy + layout::ADD_ORG_Y;
                self.input_field(target, Hit::InputOrg, layout::INPUT_X, org_y, cw, &input.org, input.field == Some(super::InputField::Org), alpha);
                y = org_y + layout::INPUT_H + layout::INPUT_GAP;
                self.sub_label(target, s.project_id_label, pad, y, cw, alpha);
                let project_y = dy + layout::ADD_PROJECT_Y;
                self.input_field(target, Hit::InputProject, layout::INPUT_X, project_y, cw, &input.project, input.field == Some(super::InputField::Project), alpha);
                y = project_y + layout::INPUT_H + 12.0;
            } else {
                y += 6.0;
            }
            let pair_w = 88.0 * 2.0 + 12.0;
            let bx = pad + (cw - pair_w) / 2.0;
            self.pill_button(target, Hit::SaveAccount, bx, y, 88.0, 30.0, s.save, alpha, true);
            self.pill_button(target, Hit::Back, bx + 100.0, y, 88.0, 30.0, s.cancel, alpha, false);
            return;
        }
        // 单账号：有则显示账号卡片，叉掉后回到添加；无则显示添加按钮
        if let Some(acc) = model.account {
            let platform = if acc.platform == Platform::Cn {
                s.platform_cn
            } else {
                s.platform_intl
            };
            // 团队版在平台名牌并列标注——账号卡无编辑入口，保存后仍可辨识
            let platform_owned;
            let platform = if acc.team {
                platform_owned = format!("{platform} | {}", s.team_badge);
                &platform_owned
            } else {
                platform
            };
            // 版本 / 等级来自用量数据，无数据时占位
            let (version, tier) = match model.snapshot {
                Some(snap) => {
                    let t = snap.tier.label();
                    let tier = if t.is_empty() {
                        snap.plan_label.clone().unwrap_or_else(|| "—".into())
                    } else {
                        t.to_string()
                    };
                    (snap.plan_version.label().to_string(), tier)
                }
                None => ("—".to_string(), "—".to_string()),
            };
            self.account_card(target, Hit::RemoveAccount(acc.index), acc.name, platform, &version, &tier, pad, y, cw, alpha);
            y += 40.0 + 8.0;
            if matches!(model.error, Some(crate::api::FetchError::Auth)) {
                self.text(target, s.key_invalid, pad + 2.0, y - 4.0, cw, 16.0, 12.0, 400, self.theme.danger, alpha);
                y += 18.0;
            }
        } else {
            // 添加账号独占一行：占满内容区，视觉对称
            self.pill_button(target, Hit::AddAccount, pad, y + 2.0, cw, 30.0, s.add_account, alpha, false);
            y += 36.0;
        }

        // ── 轮询间隔：分段第 5 段「自定义」，选中时下方展开输入行 ──
        y = self.section_label(target, s.poll_interval, pad, y, w, alpha, true);
        let presets: [(Hit, &str); 5] = [
            (Hit::IntervalPreset(layout::INTERVAL_PRESETS[0]), s.interval_1m),
            (Hit::IntervalPreset(layout::INTERVAL_PRESETS[1]), s.interval_5m),
            (Hit::IntervalPreset(layout::INTERVAL_PRESETS[2]), s.interval_15m),
            (Hit::IntervalPreset(layout::INTERVAL_PRESETS[3]), s.interval_30m),
            (Hit::CustomizeInterval, s.interval_custom),
        ];
        let cur = model.poll_interval_secs;
        let is_preset = layout::INTERVAL_PRESETS.contains(&cur);
        y = self.segmented_raw(
            target,
            &presets,
            |h| match h {
                Hit::IntervalPreset(v) => *v == cur,
                Hit::CustomizeInterval => !is_preset,
                _ => false,
            },
            pad,
            y,
            cw,
            alpha,
        );
        if panel.customizing_interval {
            // y 钉在 layout::interval_input_y，与光标、高度公式同源
            let iy = dy + layout::interval_input_y(model.accounts_count > 0, panel.account_error);
            let input = &panel.input;
            self.input_field(target, Hit::InputInterval, layout::INPUT_X, iy, 96.0, &input.interval, input.field == Some(super::InputField::Interval), alpha);
            self.text(target, s.interval_custom_unit, pad + 104.0, iy + 6.0, 40.0, 16.0, 12.0, 400, self.theme.text_secondary, alpha);
            self.outline_button(target, Hit::ApplyInterval, w - pad - 56.0, iy - 1.0, 56.0, 28.0, s.apply, alpha);
            y = iy + 38.0;
        } else {
            y += 10.0;
        }

        // ── 通用：语言 / 外观 / 开机自启 ──
        y = self.section_label(target, s.settings_general, pad, y, w, alpha, true);
        y = self.sub_label(target, s.language, pad, y, cw, alpha);
        let langs: [(Hit, &str); 3] = [
            (Hit::Language(LanguageChoice::System), s.follow_system),
            (Hit::Language(LanguageChoice::Zh), "中文"),
            (Hit::Language(LanguageChoice::En), "English"),
        ];
        // 配置字符串 → 选项枚举：未配置 / 未知值都归「跟随系统」
        let cur_lang = match model.language {
            Some("zh") => LanguageChoice::Zh,
            Some("en") => LanguageChoice::En,
            _ => LanguageChoice::System,
        };
        y = self.segmented_raw(target, &langs, |h| matches!(h, Hit::Language(v) if *v == cur_lang), pad, y, cw, alpha);
        y += 2.0;
        y = self.sub_label(target, s.appearance_section, pad, y, cw, alpha);
        let themes: [(Hit, &str); 3] = [
            (Hit::Appearance(AppearanceChoice::System), s.follow_system),
            (Hit::Appearance(AppearanceChoice::Light), s.theme_light),
            (Hit::Appearance(AppearanceChoice::Dark), s.theme_dark),
        ];
        let cur_theme = match model.appearance {
            Some("light") => AppearanceChoice::Light,
            Some("dark") => AppearanceChoice::Dark,
            _ => AppearanceChoice::System,
        };
        y = self.segmented_raw(target, &themes, |h| matches!(h, Hit::Appearance(v) if *v == cur_theme), pad, y, cw, alpha);
        y += 2.0;
        y = self.toggle_row(target, Hit::ToggleAutostart, s.autostart, "", model.autostart, pad, y, cw, alpha);

        // ── 通知 ──
        y = self.section_label(target, s.notifications, pad, y, w, alpha, true);
        y = self.toggle_row(target, Hit::ToggleThreshold, s.notify_threshold, s.notify_threshold_desc, model.threshold_enabled, pad, y, cw, alpha);
        y = self.toggle_row(target, Hit::ToggleReset5h, s.notify_reset_5h_opt, s.notify_reset_5h_desc, model.reset_5h_enabled, pad, y, cw, alpha);
        y = self.toggle_row(target, Hit::ToggleResetWeekly, s.notify_reset_weekly_opt, s.notify_reset_weekly_desc, model.reset_weekly_enabled, pad, y, cw, alpha);

        // ── 关于：检查更新 + 版本，位于底部 ──
        let update_label = match model.update {
            Some(Ok(info)) if crate::service::update::is_newer(&info.tag, env!("CARGO_PKG_VERSION")) => {
                format!("{} · {}", s.check_update, info.tag)
            }
            Some(Ok(_)) => s.up_to_date.into(),
            Some(Err(_)) => s.err_update.into(),
            None => s.check_update.into(),
        };
        y = self.section_label(target, "", pad, y, w, alpha, true);
        // 左「当前版本」12px，右描边小按钮——字号一致、视觉平衡
        let ver_line = s.version_label.replace("{v}", env!("CARGO_PKG_VERSION"));
        self.text(target, &ver_line, pad, y + 7.0, w - pad * 2.0 - 124.0, 16.0, 12.0, 400, self.theme.text_tertiary, alpha);
        self.outline_button(target, Hit::CheckUpdate, w - pad - 104.0, y + 1.0, 104.0, 28.0, &update_label, alpha);
    }

    // ── 绘制小部件 ──

    /// 区块主标题：墨色强调块 + 标题；子标签不带块以区分层级
    #[allow(clippy::too_many_arguments)]
    unsafe fn section_label(&mut self, target: &ID2D1HwndRenderTarget, label: &str, x: f32, y: f32, _w: f32, alpha: f32, rule: bool) -> f32 {
        let mut ny = y;
        if rule {
            self.divider(target, x, ny + 2.0, _w - x * 2.0, alpha);
            ny += 12.0;
        }
        if !label.is_empty() {
            let bar = self.brush(target, self.theme.text_primary, alpha * 0.9);
            target.FillRectangle(
                &D2D_RECT_F { left: x, top: ny + 1.0, right: x + 3.0, bottom: ny + 14.0 },
                &bar,
            );
            self.text(target, label, x + 7.0, ny, _w - 7.0, 17.0, 13.0, 600, self.theme.text_tertiary, alpha);
            ny + 21.0
        } else {
            // 纯分隔无标题，只留少量空隙给紧随内容
            ny + 6.0
        }
    }

    /// 区块内子项标签：13/400 次级色、无强调块
    /// 字阶：16 导航 / 13·500 主标题 / 13·400 控件标签 / 12·400 描述
    unsafe fn sub_label(&mut self, target: &ID2D1HwndRenderTarget, label: &str, x: f32, y: f32, w: f32, alpha: f32) -> f32 {
        self.text(target, label, x, y + 1.0, w, 17.0, 13.0, 400, self.theme.text_secondary, alpha);
        y + 21.0
    }

    /// 自绘输入框；光标用系统 caret——CreateCaret，IME 候选窗跟随其定位
    #[allow(clippy::too_many_arguments)]
    unsafe fn input_field(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        x: f32,
        y: f32,
        w: f32,
        content: &str,
        active: bool,
        alpha: f32,
    ) {
        let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + layout::INPUT_H };
        let fill = self.brush(target, self.theme.track, alpha);
        target.FillRectangle(&rect, &fill);
        let edge_color = if active { self.theme.action } else { self.theme.border };
        let edge = self.brush(target, edge_color, alpha);
        target.DrawRectangle(&rect, &edge, 1.2, None);
        // 只显示末尾可视部分（等宽 7.3px/字符）
        let max_chars = (((w - 12.0) / 7.3).floor() as usize).max(1);
        let vis: String = content.chars().rev().take(max_chars).collect::<Vec<_>>().into_iter().rev().collect();
        self.text_rect_opts(target, &vis, &D2D_RECT_F { left: x + 6.0, top: y + 6.0, right: x + w - 4.0, bottom: y + 22.0 }, 12.0, 400, self.theme.text_primary, alpha, Align::Left, true);
        self.hits.push((hit, D2D_RECT_F { left: x - 4.0, top: y - 4.0, right: x + w + 4.0, bottom: y + 30.0 }));
    }

    unsafe fn divider(&mut self, target: &ID2D1HwndRenderTarget, x: f32, y: f32, width: f32, alpha: f32) {
        let b = self.brush(target, self.theme.border, alpha * 0.7);
        self.line(target, x, y, x + width, y, &b, 1.0);
    }

    /// 小票撕线：指标区与余额行之间的虚线分隔
    unsafe fn dashed_divider(&mut self, target: &ID2D1HwndRenderTarget, x: f32, y: f32, width: f32, alpha: f32) {
        if self.dash_style.is_none() {
            self.dash_style = self.factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_FLAT,
                    endCap: D2D1_CAP_STYLE_FLAT,
                    dashCap: D2D1_CAP_STYLE_FLAT,
                    dashStyle: D2D1_DASH_STYLE_DASH,
                    ..Default::default()
                },
                None,
            ).ok();
        }
        let b = self.brush(target, self.theme.border, alpha * 0.8);
        match self.dash_style.clone() {
            Some(style) => target.DrawLine(
                Vector2 { X: x, Y: y },
                Vector2 { X: x + width, Y: y },
                &b,
                1.0,
                &style,
            ),
            None => self.line(target, x, y, x + width, y, &b, 1.0),
        }
    }

    #[allow(clippy::too_many_arguments)]
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
    ) -> f32 {
        self.text(target, title, x, y + 1.0, w - 56.0, 18.0, 13.0, 400, self.theme.text_primary, alpha);
        let mut ty = y + 19.0;
        if !desc.is_empty() {
            self.text(target, desc, x, ty, w - 56.0, 14.0, 12.0, 400, self.theme.text_tertiary, alpha);
            ty += 14.0;
        }
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
        let knob = th - 4.0;
        let kx = if on { tx + tw - knob - 2.0 } else { tx + 2.0 };
        let kb = self.brush(target, [1.0, 1.0, 1.0, 1.0], alpha);
        let ellipse = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
            point: Vector2 { X: kx + knob / 2.0, Y: cy + th / 2.0 },
            radiusX: knob / 2.0,
            radiusY: knob / 2.0,
        };
        target.FillEllipse(&ellipse, &kb);
        // 命中区只覆盖开关本体并含 8px 容差——点击行文字/空白不翻转
        self.hits.push((hit, D2D_RECT_F { left: tx - 8.0, top: cy - 6.0, right: tx + tw + 8.0, bottom: cy + th + 6.0 }));
        ty + 9.0
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn segmented_raw(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        items: &[(Hit, &str)],
        selected: impl Fn(&Hit) -> bool,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 {
        let h = 30.0;
        let n = items.len().max(1) as f32;
        let seg_w = w / n;
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
            let rect = D2D_RECT_F { left: tx, top: y, right: tx + seg_w, bottom: y + h };
            self.text_aligned_vc(target, label, &rect, 13.0, 400, color, alpha, Align::Center, false);
            self.hits.push((*hit, D2D_RECT_F { left: tx, top: y, right: tx + seg_w, bottom: y + h }));
        }
        y + h + 10.0
    }

    /// 单账号卡片：名称 + 平台/版本/等级三枚名牌，右上删除
    #[allow(clippy::too_many_arguments)]
    unsafe fn account_card(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        remove: Hit,
        name: &str,
        platform: &str,
        version: &str,
        tier: &str,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) {
        let h = 40.0;
        let card = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        let fill = self.brush(target, self.theme.track, alpha);
        target.FillRoundedRectangle(&card, &fill);
        let edge = self.brush(target, self.theme.border, alpha * 0.8);
        target.DrawRoundedRectangle(&card, &edge, 1.0, None);
        // 保头截断给名牌留位，名牌永不被挤掉
        let name_disp = ellipsize_px(name, 15.0, 104.0);
        self.text_aligned_vc(
            target,
            &name_disp,
            &D2D_RECT_F { left: x + 12.0, top: y, right: x + w - 60.0, bottom: y + h },
            15.0,
            500,
            self.theme.text_primary,
            alpha,
            Align::Left,
            false,
        );
        let name_w = est_width(&name_disp, 15.0);
        // 名牌行紧随名称；三枚统一描边样式，仅等级牌按档位取墨阶色
        let mut bx = x + 12.0 + name_w + 10.0;
        let by = y + (h - 17.0) / 2.0;
        let max_bx = x + w - 56.0; // 给右侧 × 留位
        let pw = est_width(platform, 10.5) + 14.0;
        if bx + pw <= max_bx {
            self.badge(target, platform, bx, by, pw, self.theme.border, self.theme.text_secondary, alpha, false);
            bx += pw + 6.0;
        }
        let vw = est_width(version, 10.5) + 14.0;
        if bx + vw <= max_bx && version != "—" {
            self.badge(target, version, bx, by, vw, self.theme.border, self.theme.text_secondary, alpha, true);
            bx += vw + 6.0;
        }
        if bx + 40.0 <= max_bx && tier != "—" {
            let (edge, fg) = self.tier_badge_colors(tier);
            let tw = est_width(tier, 10.5) + 14.0;
            self.badge(target, tier, bx, by, tw, edge, fg, alpha, false);
        }
        self.x_button(target, remove, x + w - 24.0, y + 15.0);
    }

    /// 等级配色墨阶梯度：Max 最深、Pro 次之、Lite 最浅
    fn tier_badge_colors(&self, tier: &str) -> ([f32; 4], [f32; 4]) {
        let t = tier.trim().to_ascii_lowercase();
        if t.contains("max") {
            (self.theme.text_primary, self.theme.text_primary)
        } else if t.contains("pro") {
            (self.theme.text_secondary, self.theme.text_secondary)
        } else {
            (self.theme.border, self.theme.text_tertiary)
        }
    }

    /// 名牌：透明底 + hairline 描边 + 居中文字，颜色由调用方给
    #[allow(clippy::too_many_arguments)]
    unsafe fn badge(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &str,
        x: f32,
        y: f32,
        w: f32,
        edge_color: [f32; 4],
        fg: [f32; 4],
        alpha: f32,
        mono: bool,
    ) {
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + 17.0 },
            radiusX: 2.5,
            radiusY: 2.5,
        };
        let edge = self.brush(target, edge_color, alpha * 0.9);
        target.DrawRoundedRectangle(&r, &edge, 1.0, None);
        let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + 17.0 };
        self.text_aligned_vc(target, label, &rect, 10.5, 400, fg, alpha, Align::Center, mono);
    }

    /// 按钮：primary 为 Ink 填充，次级 Linen
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
    ) {
        let hovered = self.hover == Some(hit);
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        let (base, fill_alpha, fg) = if primary {
            // 主按钮 hover 轻微透纸——alpha 呼吸，不变色
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
        self.text_aligned(target, label, &rect, 13.0, 400, fg, alpha, Align::Center, false);
        self.hits.push((hit, D2D_RECT_F { left: x - 4.0, top: y - 4.0, right: x + w + 4.0, bottom: y + h + 4.0 }));
    }

    /// 描边小按钮：透明底 + hairline 边框，与版本号等同行文字视觉平衡。
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
    ) {
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
        self.text_aligned(target, label, &rect, 12.0, 400, fg, alpha, Align::Center, false);
        self.hits.push((hit, D2D_RECT_F { left: x - 4.0, top: y - 4.0, right: x + w + 4.0, bottom: y + h + 4.0 }));
    }

    unsafe fn icon_button(&mut self, target: &ID2D1HwndRenderTarget, hit: Hit, cx: f32, cy: f32, r: f32, spin: f32) {
        let hovered = self.hover == Some(hit);
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
        // refresh-cw：两段弧 + 箭头头；spin 为旋转角
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
    }

    /// 设置入口滑杆图标；细线在 16px 下清晰，齿轮会糊
    unsafe fn sliders(&mut self, target: &ID2D1HwndRenderTarget, hit: Hit, cx: f32, cy: f32, r: f32) {
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
        let half = r * 0.62; // 线半长，放大到与刷新图标的视觉量级一致
        let dot_r = r * 0.16;
        // 三行：y 偏移与圆点 x 位置错落
        let rows = [(-0.40f32, -0.20f32), (0.0, 0.22), (0.40, -0.12)];
        for (dy, dx) in rows {
            let ly = cy + dy * r;
            self.line(target, cx - half, ly, cx + half, ly, &stroke, 1.5);
            let (px, py) = (cx + dx * r, ly);
            // 圆点：底色挖空 + 线色描边，骑在线上的空心圆
            let he = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                point: Vector2 { X: px, Y: py },
                radiusX: dot_r,
                radiusY: dot_r,
            };
            target.FillEllipse(&he, &hole);
            target.DrawEllipse(&he, &stroke, 1.5, None);
        }
        self.hits.push((hit, D2D_RECT_F { left: cx - r - 4.0, top: cy - r - 4.0, right: cx + r + 4.0, bottom: cy + r + 4.0 }));
    }

    unsafe fn back_arrow(&mut self, target: &ID2D1HwndRenderTarget, hit: Hit, x: f32, y: f32) {
        // 中性灰细线箭头——Ember 只留给文字强调，不做图标
        let stroke = self.brush(target, self.theme.text_secondary, 1.0);
        let (cx, cy) = (x + 8.0, y + 6.0);
        self.line(target, cx + 5.0, cy - 6.0, cx - 4.0, cy, &stroke, 1.8);
        self.line(target, cx - 4.0, cy, cx + 5.0, cy + 6.0, &stroke, 1.8);
        self.hits.push((hit, D2D_RECT_F { left: x - 6.0, top: y - 6.0, right: x + 24.0, bottom: y + 20.0 }));
    }

    unsafe fn x_button(&mut self, target: &ID2D1HwndRenderTarget, hit: Hit, x: f32, y: f32) {
        let stroke = self.brush(target, self.theme.text_tertiary, 1.0);
        self.line(target, x, y, x + 10.0, y + 10.0, &stroke, 1.4);
        self.line(target, x + 10.0, y, x, y + 10.0, &stroke, 1.4);
        self.hits.push((hit, D2D_RECT_F { left: x - 6.0, top: y - 6.0, right: x + 16.0, bottom: y + 16.0 }));
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn line(
        &self,
        target: &ID2D1HwndRenderTarget,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        brush: &ID2D1SolidColorBrush,
        width: f32,
    ) {
        target.DrawLine(
            Vector2 { X: x0, Y: y0 },
            Vector2 { X: x1, Y: y1 },
            brush,
            width,
            None,
        );
    }

    /// 纯色刷子：逐次创建，失败退回兜底黑刷；alpha 乘进颜色分量
    unsafe fn brush(&mut self, target: &ID2D1HwndRenderTarget, c: [f32; 4], alpha: f32) -> ID2D1SolidColorBrush {
        let color = D2D1_COLOR_F {
            r: c[0], g: c[1], b: c[2], a: (c[3] * alpha).clamp(0.0, 1.0),
        };
        // 逐次创建：存在多刷并行交替，单刷 SetColor 会被覆盖——滑杆曾整支隐形
        target
            .CreateSolidColorBrush(&color, None)
            .unwrap_or_else(|_| self.black.clone().expect("fallback brush"))
    }

    #[allow(clippy::too_many_arguments)]
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
    ) {
        let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h };
        self.text_rect(target, s, &rect, size, weight, color, alpha);
    }

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
    ) {
        let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h };
        self.text_rect_opts(target, s, &rect, size, weight, color, alpha, Align::Right, true);
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn text_rect(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        rect: &D2D_RECT_F,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
    ) {
        self.text_rect_opts(target, s, rect, size, weight, color, alpha, Align::Left, false);
    }

    /// 文本绘制的完整选项版：对齐 + 字体族
    #[allow(clippy::too_many_arguments)]
    unsafe fn text_rect_opts(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        rect: &D2D_RECT_F,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
        align: Align,
        mono: bool,
    ) {
        self.text_aligned(target, s, rect, size, weight, color, alpha, align, mono);
    }

    /// 按枚举对齐绘制文本，段落对齐保持顶部；mono 选择等宽字体。
    #[allow(clippy::too_many_arguments)]
    unsafe fn text_aligned(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        rect: &D2D_RECT_F,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
        align: Align,
        mono: bool,
    ) {
        let Some(fmt) = self.format(size, weight, mono) else { return };
        let align_set = match align {
            Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        };
        let _ = fmt.SetTextAlignment(align_set);
        let w16: Vec<u16> = s.encode_utf16().collect();
        // 空串跳过：无可绘制内容，也省一次刷子创建
        if !w16.is_empty() {
            let brush = self.brush(target, color, alpha);
            target.DrawText(
                &w16,
                &fmt,
                rect as *const D2D_RECT_F,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS(0),
                windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        let _ = fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
    }

    /// 垂直居中版 text_aligned；临时改共享 format 对齐，绘制后立即还原
    #[allow(clippy::too_many_arguments)]
    unsafe fn text_aligned_vc(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &str,
        rect: &D2D_RECT_F,
        size: f32,
        weight: u16,
        color: [f32; 4],
        alpha: f32,
        align: Align,
        mono: bool,
    ) {
        let Some(fmt) = self.format(size, weight, mono) else { return };
        let align_set = match align {
            Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        };
        let _ = fmt.SetTextAlignment(align_set);
        let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        let w16: Vec<u16> = s.encode_utf16().collect();
        // 空串跳过：无可绘制内容，也省一次刷子创建
        if !w16.is_empty() {
            let brush = self.brush(target, color, alpha);
            target.DrawText(
                &w16,
                &fmt,
                rect as *const D2D_RECT_F,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS(0),
                windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        let _ = fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
        let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
    }

    /// 应用 logo（assets/logo.svg 的矢量重绘）：圆角磁贴 + 白色 Z 字形
    /// 几何按 30×30 viewBox 构建一次，绘制时以矩阵缩放平移，任意 DPI 无损。
    unsafe fn logo(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        x: f32,
        y: f32,
        size: f32,
        alpha: f32,
    ) {
        // 磁贴底，圆角比例约 4/30
        let tile = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + size, bottom: y + size },
            radiusX: size * (4.0 / 30.0),
            radiusY: size * (4.0 / 30.0),
        };
        let tb = self.brush(target, self.theme.logo_tile, alpha);
        target.FillRoundedRectangle(&tile, &tb);

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
    }

    /// 构建白色 Z 字形路径（坐标取自 logo.svg 的三段图形单位）
    fn build_logo_glyph(&self) -> Option<ID2D1PathGeometry> {
        unsafe {
            let geo = self.factory.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            // 上横杠：右端斜切 + 圆角过渡
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
            // 下横杠：左端斜切 + 圆角过渡
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

/// 单字符宽度模型（est_width / ellipsize_px 共用）：ASCII ≈ 0.58×字号，
/// 全角 CJK ≈ 1.0×字号
fn char_w(c: char, size: f32) -> f32 {
    if c.is_ascii() { size * 0.58 } else { size }
}

/// 估算比例字体文本像素宽，名牌排布用
fn est_width(s: &str, size: f32) -> f32 {
    s.chars().map(|c| char_w(c, size)).sum()
}

/// 按像素预算保头截断，省略号收尾
fn ellipsize_px(s: &str, size: f32, max_w: f32) -> String {
    let ellipsis_w = size * 0.7;
    let budget = (max_w - ellipsis_w).max(0.0);
    let mut w = 0.0;
    let mut out = String::new();
    for c in s.chars() {
        let cw = char_w(c, size);
        if w + cw > budget {
            out.push('…');
            return out;
        }
        out.push(c);
        w += cw;
    }
    out
}

/// 错误 → 本地化展示文案（api 层只给英文技术细节）
fn error_text(s: &Strings, e: &FetchError) -> String {
    match e {
        FetchError::Auth => s.err_auth.to_string(),
        FetchError::EmptyLimits => s.err_empty.to_string(),
        FetchError::Api(detail) => with_detail(s.err_api, detail),
        FetchError::Network(detail) => with_detail(s.err_network, detail),
    }
}

/// 「前缀: 细节」拼接；detail 为空仅前缀
fn with_detail(prefix: &str, detail: &str) -> String {
    if detail.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}: {detail}")
    }
}

impl Default for AnimState {
    fn default() -> Self {
        Self::new()
    }
}
