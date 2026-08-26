//! 面板内容渲染

#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::HashMap;

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F,
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_CAP_STYLE_FLAT, D2D1_DASH_STYLE_DASH, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES, D2D1CreateFactory, ID2D1Factory,
    ID2D1HwndRenderTarget, ID2D1PathGeometry, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::core::PCWSTR;
use windows_numerics::{Matrix3x2, Vector2};

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

/// 可点击元素标识，用于命中检测；按 UI 场景分组，序同 handle_panel_hit 臂序
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    // ── 主视图 ──
    Refresh,
    Settings,
    AccountSwitch,
    Retry,
    UsageInfo,

    // ── 导航 ──
    Back,

    // ── 设置 · 轮询间隔 ──
    IntervalPreset(u64),
    CustomizeInterval,
    ApplyInterval,
    InputInterval,

    // ── 设置 · 通用 ──
    Language(LanguageChoice),
    Appearance(AppearanceChoice),
    ToggleAutostart,

    // ── 设置 · 网络代理 ──
    InputProxy,

    // ── 设置 · 用量通知 ──
    ToggleThreshold,
    ToggleReset5h,
    ToggleResetWeekly,

    // ── 设置 · 高峰区间 ──
    InputPeakStart,
    InputPeakEnd,
    ApplyPeak,

    // ── 设置 · 账号 ──
    AddAccount,
    RemoveAccount(usize),
    PickAccount(usize),
    AccountType(ScopeChoice),
    InputName,
    InputKey,
    InputOrg,
    InputProject,
    SaveAccount,
    Platform(Platform),

    // ── 设置 · 配置管理与关于 ──
    ExportConfig,
    ImportConfig,
    CheckUpdate,
    OpenDownload,
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
        Self {
            appear: None,
            spin: 0.0,
        }
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
    /// 本帧待绘的峰谷说明卡片位置，绘制期由徽标填入、draw 尾部统一画以盖过数据行
    pending_tip: Option<(f32, f32, f32)>,
    font_fallback: bool,
    target_dpi: f32,
    anim_allowed: bool,
    logo_geo: Option<ID2D1PathGeometry>,
    bolt_geo: Option<ID2D1PathGeometry>,
    dash_style: Option<ID2D1StrokeStyle>,
}

impl Renderer {
    pub fn new() -> Option<Self> {
        unsafe {
            let factory: ID2D1Factory =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
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
                pending_tip: None,
                font_fallback: false,
                target_dpi: 0.0,
                anim_allowed: animations_allowed(),
                logo_geo: None,
                bolt_geo: None,
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
            // 单行布局：禁用自动换行，超宽由 ellipsize 截断负责，防文本折行溢出框外
            let _ = fmt.SetWordWrapping(
                windows::Win32::Graphics::DirectWrite::DWRITE_WORD_WRAPPING_NO_WRAP,
            );
            self.formats.insert(key, fmt.clone());
            Some(fmt)
        }
    }

    /// DWrite 真实测宽
    unsafe fn measure(&mut self, s: &str, size: f32, weight: u16, mono: bool) -> f32 {
        let Some(fmt) = self.format(size, weight, mono) else {
            return 0.0;
        };
        let w16: Vec<u16> = s.encode_utf16().collect();
        if w16.is_empty() {
            return 0.0;
        }
        let Ok(layout) = self.dwrite.CreateTextLayout(&w16, &fmt, 1.0e6, 1.0e6) else {
            return 0.0;
        };
        let mut m = windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS::default();
        if layout.GetMetrics(&mut m).is_ok() {
            m.widthIncludingTrailingWhitespace
        } else {
            0.0
        }
    }

    /// 真实度量保头截断：逐 cluster 宽累计 + 真实省略号宽
    unsafe fn ellipsize(
        &mut self,
        s: &str,
        size: f32,
        max_w: f32,
        weight: u16,
        mono: bool,
    ) -> String {
        if s.is_empty() {
            return String::new();
        }
        let ell_w = self.measure("…", size, weight, mono);
        let budget = (max_w - ell_w).max(0.0);
        let Some(fmt) = self.format(size, weight, mono) else {
            return s.to_string();
        };
        let w16: Vec<u16> = s.encode_utf16().collect();
        let Ok(layout) = self.dwrite.CreateTextLayout(&w16, &fmt, 1.0e6, 1.0e6) else {
            return s.to_string();
        };
        let mut cms = vec![
            windows::Win32::Graphics::DirectWrite::DWRITE_CLUSTER_METRICS::default();
            w16.len()
        ];
        let mut n = 0u32;
        if layout.GetClusterMetrics(Some(&mut cms), &mut n).is_err() {
            return s.to_string();
        }
        cms.truncate(n as usize);
        let mut w = 0.0f32;
        let mut units = 0usize;
        for cm in &cms {
            if w + cm.width > budget {
                break;
            }
            w += cm.width;
            units += cm.length as usize;
        }
        if units >= w16.len() {
            return s.to_string();
        }
        let mut out: Vec<u16> = w16[..units].to_vec();
        out.push(0x2026);
        String::from_utf16_lossy(&out)
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
            let Some(target) = self.ensure_target(hwnd, rect_phys, dpi) else {
                return;
            };
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
                    self.drop_device_resources();
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
        let bg_rect = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: w,
            bottom: h,
        };
        target.FillRectangle(&bg_rect, &bg_brush);

        match view {
            PanelView::Main => self.draw_main(target, model, w, h, dy, alpha),
            PanelView::Settings => self.draw_settings(target, panel, model, w, dy, alpha),
            PanelView::AccountPicker => self.draw_account_picker(target, model, w, dy, alpha),
        }

        // 峰谷说明卡片最后画，盖过数据行
        if let Some((x, y, tw)) = self.pending_tip.take() {
            let tip = model
                .strings
                .peak_tip
                .replace("{r}", &crate::ui::peak::fmt_range(model.peak_range));
            self.tip_card(target, x, y, tw, alpha, &tip);
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
        // 顶栏区域恒定 52 高：logo 恒 38px，logo/文本块/箭头/右按钮
        // 全部以行中心 y+26 对齐，等级有无不改变布局
        let (logo_size, logo_y) = (38.0, y + 7.0);
        self.logo(target, pad, logo_y, logo_size, alpha);
        let tx = pad + logo_size + 10.0;
        // 多账号时名称区还要给下拉箭头留位（前距 6 + 箭头 10 + 余量 2），长名称不再把箭头挤进右按钮区
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
        // 多账号时账号名后给下拉箭头，与 logo 中心对齐；命中区只覆盖箭头附近
        if model.accounts_count > 1 {
            let ax = tx + self.measure(&title_disp, 16.0, 500, false) + 6.0;
            self.chevron(target, ax, y + 26.0, self.theme.text_secondary, alpha);
            self.hits.push((
                Hit::AccountSwitch,
                D2D_RECT_F {
                    left: ax - 10.0,
                    top: y + 12.0,
                    right: ax + 20.0,
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
        // 顶栏恒定 52，与 sync_main_height 一致
        y += 52.0;

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
            (None, Some(e)) => {
                // 错误卡：danger 染底 + hairline 边，居中信息 + 重试
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
            _ => {
                if let Some(snap) = snap {
                    // 区块刊头：hairline + 墨色强调块 + 标题，与设置页同款
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
                    // 高峰期标题旁给琥珀黄闪电徽标，悬停展开说明
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

                    // 余额为国内版功能：撕线之下的「总额」行——刊头带强调块，右侧等宽数值
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
                        let line = format!("¥{:.2}", b.available);
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
                let footer_y = dy + h - 36.0;
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

        // 5px 直角细条，与面板锐利纸感一致；critical 转警示色
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
        if frac > 0.004 {
            // 最小可见长度 2px，避免 0.x% 时出现针尖
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
        let nav_title = if panel.adding_account {
            s.add_account
        } else {
            s.settings
        };
        let title_rect = D2D_RECT_F {
            left: pad,
            top: y,
            right: w - pad,
            bottom: y + 26.0,
        };
        self.text_aligned(
            target,
            nav_title,
            &title_rect,
            16.0,
            400,
            self.theme.text_primary,
            alpha,
            Align::Center,
            false,
        );
        y += 30.0;

        // ── 账号：设置页首项即数据来源；添加页此处标题用「版本」──
        let section = if panel.adding_account {
            s.platform_section
        } else {
            s.accounts_section
        };
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
            self.input_field(
                target,
                Hit::InputName,
                layout::INPUT_X,
                name_y,
                cw,
                &input.name,
                "",
                input.field == Some(super::InputField::Name),
                alpha,
            );
            y = name_y + layout::INPUT_H + layout::INPUT_GAP;
            self.sub_label(target, s.api_key_label, pad, y, cw, alpha);
            let key_y = dy + layout::ADD_KEY_Y;
            self.input_field(
                target,
                Hit::InputKey,
                layout::INPUT_X,
                key_y,
                cw,
                &input.key,
                "",
                input.field == Some(super::InputField::Key),
                alpha,
            );
            y = key_y + layout::INPUT_H + layout::INPUT_GAP;
            if team {
                // 团队版：组织 / 项目 ID（请求头 Bigmodel-Organization / Bigmodel-Project）
                self.sub_label(target, s.org_id_label, pad, y, cw, alpha);
                let org_y = dy + layout::ADD_ORG_Y;
                self.input_field(
                    target,
                    Hit::InputOrg,
                    layout::INPUT_X,
                    org_y,
                    cw,
                    &input.org,
                    "",
                    input.field == Some(super::InputField::Org),
                    alpha,
                );
                y = org_y + layout::INPUT_H + layout::INPUT_GAP;
                self.sub_label(target, s.project_id_label, pad, y, cw, alpha);
                let project_y = dy + layout::ADD_PROJECT_Y;
                self.input_field(
                    target,
                    Hit::InputProject,
                    layout::INPUT_X,
                    project_y,
                    cw,
                    &input.project,
                    "",
                    input.field == Some(super::InputField::Project),
                    alpha,
                );
                y = project_y + layout::INPUT_H + 12.0;
            } else {
                y += 6.0;
            }
            let pair_w = 88.0 * 2.0 + 12.0;
            let bx = pad + (cw - pair_w) / 2.0;
            self.pill_button(
                target,
                Hit::SaveAccount,
                bx,
                y,
                88.0,
                30.0,
                s.save,
                alpha,
                true,
            );
            self.pill_button(
                target,
                Hit::Back,
                bx + 100.0,
                y,
                88.0,
                30.0,
                s.cancel,
                alpha,
                false,
            );
            return;
        }
        // 当前活跃账号卡片（有则显示）；添加入口常驻，支持继续添加账号
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
            self.account_card(
                target,
                Hit::RemoveAccount(acc.index),
                acc.name,
                platform,
                &version,
                &tier,
                pad,
                y,
                cw,
                alpha,
            );
            y += 40.0 + 8.0;
            if matches!(model.error, Some(crate::api::FetchError::Auth)) {
                self.text(
                    target,
                    s.key_invalid,
                    pad + 2.0,
                    y - 4.0,
                    cw,
                    16.0,
                    12.0,
                    400,
                    self.theme.danger,
                    alpha,
                );
                y += 18.0;
            }
        }
        // 添加账号独占一行：占满内容区，视觉对称
        self.pill_button(
            target,
            Hit::AddAccount,
            pad,
            y + 2.0,
            cw,
            30.0,
            s.add_account,
            alpha,
            false,
        );
        y += 36.0;

        // ── 轮询间隔：分段第 5 段「自定义」，选中时下方展开输入行 ──
        y = self.section_label(target, s.poll_interval, pad, y, w, alpha, true);
        let presets: [(Hit, &str); 5] = [
            (
                Hit::IntervalPreset(layout::INTERVAL_PRESETS[0]),
                s.interval_1m,
            ),
            (
                Hit::IntervalPreset(layout::INTERVAL_PRESETS[1]),
                s.interval_5m,
            ),
            (
                Hit::IntervalPreset(layout::INTERVAL_PRESETS[2]),
                s.interval_15m,
            ),
            (
                Hit::IntervalPreset(layout::INTERVAL_PRESETS[3]),
                s.interval_30m,
            ),
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
            self.input_field(
                target,
                Hit::InputInterval,
                layout::INPUT_X,
                iy,
                96.0,
                &input.interval,
                "",
                input.field == Some(super::InputField::Interval),
                alpha,
            );
            self.text(
                target,
                s.interval_custom_unit,
                pad + 104.0,
                iy + 6.0,
                40.0,
                16.0,
                12.0,
                400,
                self.theme.text_secondary,
                alpha,
            );
            self.outline_button(
                target,
                Hit::ApplyInterval,
                w - pad - 56.0,
                iy - 1.0,
                56.0,
                28.0,
                s.apply,
                alpha,
            );
            y = iy + 38.0;
        } else {
            y += 10.0;
        }

        // ── 通知 ──
        y = self.section_label(target, s.notifications, pad, y, w, alpha, true);
        y = self.toggle_row(
            target,
            Hit::ToggleThreshold,
            s.notify_threshold,
            s.notify_threshold_desc,
            model.threshold_enabled,
            pad,
            y,
            cw,
            alpha,
        );
        y = self.toggle_row(
            target,
            Hit::ToggleReset5h,
            s.notify_reset_5h_opt,
            s.notify_reset_5h_desc,
            model.reset_5h_enabled,
            pad,
            y,
            cw,
            alpha,
        );
        y = self.toggle_row(
            target,
            Hit::ToggleResetWeekly,
            s.notify_reset_weekly_opt,
            s.notify_reset_weekly_desc,
            model.reset_weekly_enabled,
            pad,
            y,
            cw,
            alpha,
        );

        // ── 高峰区间：工作日生效，自定义起止 ──
        self.section_label(target, s.peak_section, pad, y, w, alpha, true);
        // y 钉在 layout::peak_input_y，与光标、高度公式同源
        let pky = dy
            + layout::peak_input_y(
                model.accounts_count > 0,
                panel.account_error,
                panel.customizing_interval,
            );
        // 当前配置值作为弱色占位提示，输入即覆盖，无需删除
        let start_buf = panel.input.peak_start.as_str();
        let end_buf = panel.input.peak_end.as_str();
        // 非法状态（格式错误或两端相等）给红框，提示等待修正；空缓冲表示未编辑不判
        let bad = |v: &str| !v.is_empty() && crate::ui::peak::parse_hhmm(v).is_none();
        let peak_bad = bad(start_buf)
            || bad(end_buf)
            || matches!(
                (
                    crate::ui::peak::parse_hhmm(start_buf),
                    crate::ui::peak::parse_hhmm(end_buf),
                ),
                (Some(s), Some(e)) if s == e
            );
        self.text(
            target,
            s.peak_start_label,
            pad,
            pky + 7.0,
            26.0,
            14.0,
            12.0,
            400,
            self.theme.text_tertiary,
            alpha,
        );
        self.input_field(
            target,
            Hit::InputPeakStart,
            layout::PEAK_START_X,
            pky,
            64.0,
            start_buf,
            model.peak_start_raw,
            panel.input.field == Some(super::InputField::PeakStart),
            alpha,
        );
        self.text(
            target,
            s.peak_end_label,
            pad + 100.0,
            pky + 7.0,
            26.0,
            14.0,
            12.0,
            400,
            self.theme.text_tertiary,
            alpha,
        );
        self.input_field(
            target,
            Hit::InputPeakEnd,
            layout::PEAK_END_X,
            pky,
            64.0,
            end_buf,
            model.peak_end_raw,
            panel.input.field == Some(super::InputField::PeakEnd),
            alpha,
        );
        self.outline_button(
            target,
            Hit::ApplyPeak,
            w - pad - 48.0,
            pky - 1.0,
            48.0,
            28.0,
            s.apply,
            alpha,
        );
        if peak_bad {
            let edge = self.brush(target, self.theme.danger, alpha);
            for bx in [layout::PEAK_START_X, layout::PEAK_END_X] {
                target.DrawRectangle(
                    &D2D_RECT_F {
                        left: bx,
                        top: pky,
                        right: bx + 64.0,
                        bottom: pky + layout::INPUT_H,
                    },
                    &edge,
                    1.4,
                    None,
                );
            }
        }
        y = pky + layout::INPUT_H + 8.0;

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
        y = self.segmented_raw(
            target,
            &langs,
            |h| matches!(h, Hit::Language(v) if *v == cur_lang),
            pad,
            y,
            cw,
            alpha,
        );
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
        y = self.segmented_raw(
            target,
            &themes,
            |h| matches!(h, Hit::Appearance(v) if *v == cur_theme),
            pad,
            y,
            cw,
            alpha,
        );
        y += 2.0;
        y = self.toggle_row(
            target,
            Hit::ToggleAutostart,
            s.autostart,
            "",
            model.autostart,
            pad,
            y,
            cw,
            alpha,
        );

        // ── 网络代理：地址留空直连，提示在框内占位 ──
        y = self.section_label(target, s.network_section, pad, y, w, alpha, true);
        self.sub_label(target, s.proxy_label, pad, y, cw, alpha);
        // y 钉在 layout::proxy_input_y，与光标、高度公式同源
        let py = dy
            + layout::proxy_input_y(
                model.accounts_count > 0,
                panel.account_error,
                panel.customizing_interval,
            );
        let proxy_disp: String = if panel.input.proxy.is_empty() {
            model.proxy.unwrap_or("").to_string()
        } else {
            panel.input.proxy.clone()
        };
        self.input_field(
            target,
            Hit::InputProxy,
            layout::INPUT_X,
            py,
            cw,
            &proxy_disp,
            s.proxy_hint,
            panel.input.field == Some(super::InputField::Proxy),
            alpha,
        );
        y = py + layout::INPUT_H + 6.0;

        // ── 配置管理：导出 / 导入，位于关于区之前 ──
        y = self.section_label(target, s.backup_section, pad, y, w, alpha, true);
        let pair_w = 104.0 * 2.0 + 12.0;
        let bx = pad + (cw - pair_w) / 2.0;
        self.outline_button(
            target,
            Hit::ExportConfig,
            bx,
            y,
            104.0,
            28.0,
            s.export_config,
            alpha,
        );
        self.outline_button(
            target,
            Hit::ImportConfig,
            bx + 116.0,
            y,
            104.0,
            28.0,
            s.import_config,
            alpha,
        );
        y += 28.0 + 12.0;

        // ── 关于：检查更新 + 版本，位于底部 ──
        let update_label = match model.update {
            Some(Ok(info))
                if crate::service::update::is_newer(&info.tag, env!("CARGO_PKG_VERSION")) =>
            {
                format!("{} · {}", s.check_update, info.tag)
            }
            Some(Ok(_)) => s.up_to_date.into(),
            Some(Err(_)) => s.err_update.into(),
            None => s.check_update.into(),
        };
        y = self.section_label(target, "", pad, y, w, alpha, true);
        // 有新版时先给一行下载入口；版本行保持设置页收尾
        if model.update_available {
            let bw = 104.0;
            self.pill_button(
                target,
                Hit::OpenDownload,
                (w - bw) / 2.0,
                y + 2.0,
                bw,
                30.0,
                s.go_download,
                alpha,
                true,
            );
            y += 38.0;
        }
        // 左「当前版本」12px，右描边小按钮——字号一致、视觉平衡；
        // 按钮宽随文案自适应，英文失败/新版本文案不折行
        let ver_line = s.version_label.replace("{v}", env!("CARGO_PKG_VERSION"));
        let btn_w = (self.measure(&update_label, 12.0, 400, false) + 28.0).max(104.0);
        self.text(
            target,
            &ver_line,
            pad,
            y + 7.0,
            w - pad * 2.0 - btn_w - 12.0,
            16.0,
            12.0,
            400,
            self.theme.text_tertiary,
            alpha,
        );
        self.outline_button(
            target,
            Hit::CheckUpdate,
            w - pad - btn_w,
            y + 1.0,
            btn_w,
            28.0,
            &update_label,
            alpha,
        );
    }

    // ── 绘制小部件 ──

    /// 区块主标题：墨色强调块 + 标题；子标签不带块以区分层级
    #[allow(clippy::too_many_arguments)]
    unsafe fn section_label(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &str,
        x: f32,
        y: f32,
        _w: f32,
        alpha: f32,
        rule: bool,
    ) -> f32 {
        let mut ny = y;
        if rule {
            self.divider(target, x, ny + 2.0, _w - x * 2.0, alpha);
            ny += 12.0;
        }
        if !label.is_empty() {
            let bar = self.brush(target, self.theme.text_primary, alpha * 0.9);
            target.FillRectangle(
                &D2D_RECT_F {
                    left: x,
                    top: ny + 1.0,
                    right: x + 3.0,
                    bottom: ny + 14.0,
                },
                &bar,
            );
            self.text(
                target,
                label,
                x + 7.0,
                ny,
                _w - 7.0,
                17.0,
                13.0,
                600,
                self.theme.text_tertiary,
                alpha,
            );
            ny + 21.0
        } else {
            // 纯分隔无标题，只留少量空隙给紧随内容
            ny + 6.0
        }
    }

    /// 区块内子项标签：13/400 次级色、无强调块
    /// 字阶：16 导航 / 13·500 主标题 / 13·400 控件标签 / 12·400 描述
    unsafe fn sub_label(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &str,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 {
        self.text(
            target,
            label,
            x,
            y + 1.0,
            w,
            17.0,
            13.0,
            400,
            self.theme.text_secondary,
            alpha,
        );
        y + 21.0
    }

    /// 自绘输入框；光标用系统 caret——CreateCaret，IME 候选窗跟随其定位。
    /// content 为空且未聚焦时显示弱色占位提示
    #[allow(clippy::too_many_arguments)]
    unsafe fn input_field(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        x: f32,
        y: f32,
        w: f32,
        content: &str,
        placeholder: &str,
        active: bool,
        alpha: f32,
    ) {
        let rect = D2D_RECT_F {
            left: x,
            top: y,
            right: x + w,
            bottom: y + layout::INPUT_H,
        };
        let fill = self.brush(target, self.theme.track, alpha);
        target.FillRectangle(&rect, &fill);
        let edge_color = if active {
            self.theme.action
        } else {
            self.theme.border
        };
        let edge = self.brush(target, edge_color, alpha);
        target.DrawRectangle(&rect, &edge, 1.2, None);
        let text_rect = D2D_RECT_F {
            left: x + 6.0,
            top: y + 6.0,
            right: x + w - 4.0,
            bottom: y + 22.0,
        };
        if content.is_empty() && !placeholder.is_empty() {
            self.text_rect_opts(
                target,
                placeholder,
                &text_rect,
                12.0,
                400,
                self.theme.text_tertiary,
                alpha,
                Align::Left,
                false,
            );
        } else {
            // 只显示末尾可视部分（等宽 7.3px/字符）
            let max_chars = (((w - 12.0) / 7.3).floor() as usize).max(1);
            let vis: String = content
                .chars()
                .rev()
                .take(max_chars)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            self.text_rect_opts(
                target,
                &vis,
                &text_rect,
                12.0,
                400,
                self.theme.text_primary,
                alpha,
                Align::Left,
                true,
            );
        }
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: x - 4.0,
                top: y - 4.0,
                right: x + w + 4.0,
                bottom: y + 30.0,
            },
        ));
    }

    unsafe fn divider(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        x: f32,
        y: f32,
        width: f32,
        alpha: f32,
    ) {
        let b = self.brush(target, self.theme.border, alpha * 0.7);
        self.line(target, x, y, x + width, y, &b, 1.0);
    }

    /// 小票撕线：指标区与余额行之间的虚线分隔
    unsafe fn dashed_divider(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        x: f32,
        y: f32,
        width: f32,
        alpha: f32,
    ) {
        if self.dash_style.is_none() {
            self.dash_style = self
                .factory
                .CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: D2D1_CAP_STYLE_FLAT,
                        endCap: D2D1_CAP_STYLE_FLAT,
                        dashCap: D2D1_CAP_STYLE_FLAT,
                        dashStyle: D2D1_DASH_STYLE_DASH,
                        ..Default::default()
                    },
                    None,
                )
                .ok();
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
        self.text(
            target,
            title,
            x,
            y + 1.0,
            w - 56.0,
            18.0,
            13.0,
            400,
            self.theme.text_primary,
            alpha,
        );
        let mut ty = y + 19.0;
        if !desc.is_empty() {
            self.text(
                target,
                desc,
                x,
                ty,
                w - 56.0,
                14.0,
                12.0,
                400,
                self.theme.text_tertiary,
                alpha,
            );
            ty += 14.0;
        }
        let (tw, th) = (38.0, 22.0);
        let tx = x + w - tw;
        let cy = (y + (ty - y - th) / 2.0).max(y);
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: tx,
                top: cy,
                right: tx + tw,
                bottom: cy + th,
            },
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
            point: Vector2 {
                X: kx + knob / 2.0,
                Y: cy + th / 2.0,
            },
            radiusX: knob / 2.0,
            radiusY: knob / 2.0,
        };
        target.FillEllipse(&ellipse, &kb);
        // 命中区只覆盖开关本体并含 8px 容差——点击行文字/空白不翻转
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: tx - 8.0,
                top: cy - 6.0,
                right: tx + tw + 8.0,
                bottom: cy + th + 6.0,
            },
        ));
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
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
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
            let color = if sel {
                self.theme.action_text
            } else {
                self.theme.text_secondary
            };
            let tx = x + i as f32 * seg_w;
            let rect = D2D_RECT_F {
                left: tx,
                top: y,
                right: tx + seg_w,
                bottom: y + h,
            };
            self.text_aligned_vc(
                target,
                label,
                &rect,
                13.0,
                400,
                color,
                alpha,
                Align::Center,
                false,
            );
            self.hits.push((
                *hit,
                D2D_RECT_F {
                    left: tx,
                    top: y,
                    right: tx + seg_w,
                    bottom: y + h,
                },
            ));
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
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        let fill = self.brush(target, self.theme.track, alpha);
        target.FillRoundedRectangle(&card, &fill);
        let edge = self.brush(target, self.theme.border, alpha * 0.8);
        target.DrawRoundedRectangle(&card, &edge, 1.0, None);
        // 徽标与关闭钮居右对齐，名称吃左侧剩余空间；从右界向左依次排 tier/version/platform
        let by = y + (h - 17.0) / 2.0;
        let mut bx = x + w - 56.0; // 徽标区右界，给右侧 × 留位
        if tier != "—" {
            let tw = self.measure(tier, 10.5, 400, false) + 14.0;
            bx -= tw;
            let (edge, fg) = self.tier_badge_colors(tier);
            self.badge(target, tier, bx, by, tw, edge, fg, alpha, false);
            bx -= 6.0;
        }
        if version != "—" {
            let vw = self.measure(version, 10.5, 400, true) + 14.0;
            bx -= vw;
            self.badge(
                target,
                version,
                bx,
                by,
                vw,
                self.theme.border,
                self.theme.text_secondary,
                alpha,
                true,
            );
            bx -= 6.0;
        }
        let pw = self.measure(platform, 10.5, 400, false) + 14.0;
        bx -= pw;
        self.badge(
            target,
            platform,
            bx,
            by,
            pw,
            self.theme.border,
            self.theme.text_secondary,
            alpha,
            false,
        );
        let name_max = (bx - 10.0 - (x + 12.0)).max(60.0);
        let name_disp = self.ellipsize(name, 15.0, name_max, 500, false);
        self.text_aligned_vc(
            target,
            &name_disp,
            &D2D_RECT_F {
                left: x + 12.0,
                top: y,
                right: x + 12.0 + name_max,
                bottom: y + h,
            },
            15.0,
            500,
            self.theme.text_primary,
            alpha,
            Align::Left,
            false,
        );
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
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + 17.0,
            },
            radiusX: 2.5,
            radiusY: 2.5,
        };
        let edge = self.brush(target, edge_color, alpha * 0.9);
        target.DrawRoundedRectangle(&r, &edge, 1.0, None);
        let rect = D2D_RECT_F {
            left: x,
            top: y,
            right: x + w,
            bottom: y + 17.0,
        };
        self.text_aligned_vc(
            target,
            label,
            &rect,
            10.5,
            400,
            fg,
            alpha,
            Align::Center,
            mono,
        );
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
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        let (base, fill_alpha, fg) = if primary {
            // 主按钮 hover 轻微透纸——alpha 呼吸，不变色
            (
                self.theme.action,
                if hovered { alpha * 0.86 } else { alpha },
                self.theme.action_text,
            )
        } else {
            // 次级：Linen，hover 沉一档到 Stone
            (
                if hovered {
                    self.theme.border
                } else {
                    self.theme.track
                },
                alpha,
                self.theme.text_primary,
            )
        };
        let b = self.brush(target, base, fill_alpha);
        target.FillRoundedRectangle(&r, &b);
        let rect = D2D_RECT_F {
            left: x,
            top: y + 5.0,
            right: x + w,
            bottom: y + h - 4.0,
        };
        self.text_aligned(
            target,
            label,
            &rect,
            13.0,
            400,
            fg,
            alpha,
            Align::Center,
            false,
        );
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: x - 4.0,
                top: y - 4.0,
                right: x + w + 4.0,
                bottom: y + h + 4.0,
            },
        ));
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
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        if hovered {
            let b = self.brush(target, self.theme.track, alpha);
            target.FillRoundedRectangle(&r, &b);
        }
        let edge = self.brush(target, self.theme.border, alpha * 0.9);
        target.DrawRoundedRectangle(&r, &edge, 1.0, None);
        let fg = if hovered {
            self.theme.accent
        } else {
            self.theme.text_secondary
        };
        let rect = D2D_RECT_F {
            left: x,
            top: y + 5.0,
            right: x + w,
            bottom: y + h - 4.0,
        };
        self.text_aligned(
            target,
            label,
            &rect,
            12.0,
            400,
            fg,
            alpha,
            Align::Center,
            false,
        );
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: x - 4.0,
                top: y - 4.0,
                right: x + w + 4.0,
                bottom: y + h + 4.0,
            },
        ));
    }

    unsafe fn icon_button(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        cx: f32,
        cy: f32,
        r: f32,
        spin: f32,
    ) {
        let hovered = self.hover == Some(hit);
        let ellipse = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
            point: Vector2 { X: cx, Y: cy },
            radiusX: r,
            radiusY: r,
        };
        let base = if hovered {
            self.theme.track
        } else {
            [0.0, 0.0, 0.0, 0.0]
        };
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
            segs.push((
                px,
                py,
                px + (fx * fc - fy * fs) * al,
                py + (fx * fs + fy * fc) * al,
            ));
            segs.push((
                px,
                py,
                px + (fx * fc + fy * fs) * al,
                py + (-fx * fs + fy * fc) * al,
            ));
        }
        for (x0, y0, x1, y1) in segs {
            let (ax, ay) = rot(x0, y0);
            let (bx, by) = rot(x1, y1);
            self.line(target, ax, ay, bx, by, &stroke, 1.6);
        }
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: cx - r - 4.0,
                top: cy - r - 4.0,
                right: cx + r + 4.0,
                bottom: cy + r + 4.0,
            },
        ));
    }

    /// 设置入口滑杆图标；细线在 16px 下清晰，齿轮会糊
    unsafe fn sliders(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        cx: f32,
        cy: f32,
        r: f32,
    ) {
        let hovered = self.hover == Some(hit);
        let base = if hovered {
            self.theme.track
        } else {
            [0.0, 0.0, 0.0, 0.0]
        };
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
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: cx - r - 4.0,
                top: cy - r - 4.0,
                right: cx + r + 4.0,
                bottom: cy + r + 4.0,
            },
        ));
    }

    unsafe fn back_arrow(&mut self, target: &ID2D1HwndRenderTarget, hit: Hit, x: f32, y: f32) {
        // 中性灰细线箭头——Ember 只留给文字强调，不做图标
        let stroke = self.brush(target, self.theme.text_secondary, 1.0);
        let (cx, cy) = (x + 8.0, y + 6.0);
        self.line(target, cx + 5.0, cy - 6.0, cx - 4.0, cy, &stroke, 1.8);
        self.line(target, cx - 4.0, cy, cx + 5.0, cy + 6.0, &stroke, 1.8);
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: x - 6.0,
                top: y - 6.0,
                right: x + 24.0,
                bottom: y + 20.0,
            },
        ));
    }

    unsafe fn x_button(&mut self, target: &ID2D1HwndRenderTarget, hit: Hit, x: f32, y: f32) {
        let stroke = self.brush(target, self.theme.text_tertiary, 1.0);
        self.line(target, x, y, x + 10.0, y + 10.0, &stroke, 1.4);
        self.line(target, x + 10.0, y, x, y + 10.0, &stroke, 1.4);
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: x - 6.0,
                top: y - 6.0,
                right: x + 16.0,
                bottom: y + 16.0,
            },
        ));
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
    unsafe fn brush(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        c: [f32; 4],
        alpha: f32,
    ) -> ID2D1SolidColorBrush {
        let color = D2D1_COLOR_F {
            r: c[0],
            g: c[1],
            b: c[2],
            a: (c[3] * alpha).clamp(0.0, 1.0),
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
        let rect = D2D_RECT_F {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        };
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
        let rect = D2D_RECT_F {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        };
        self.text_rect_opts(
            target,
            s,
            &rect,
            size,
            weight,
            color,
            alpha,
            Align::Right,
            true,
        );
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
        self.text_rect_opts(
            target,
            s,
            rect,
            size,
            weight,
            color,
            alpha,
            Align::Left,
            false,
        );
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
        let Some(fmt) = self.format(size, weight, mono) else {
            return;
        };
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
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        let _ = fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
    }

    /// 多行文案：临时开自动换行，长文按框宽折行（如空态副标题）。
    /// format 是缓存共享句柄，用后必须还原换行与对齐
    #[allow(clippy::too_many_arguments)]
    unsafe fn text_wrapped(
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
        let Some(fmt) = self.format(size, weight, mono) else {
            return;
        };
        let _ =
            fmt.SetWordWrapping(windows::Win32::Graphics::DirectWrite::DWRITE_WORD_WRAPPING_WRAP);
        let align_set = match align {
            Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        };
        let _ = fmt.SetTextAlignment(align_set);
        let w16: Vec<u16> = s.encode_utf16().collect();
        if !w16.is_empty() {
            let brush = self.brush(target, color, alpha);
            target.DrawText(
                &w16,
                &fmt,
                rect as *const D2D_RECT_F,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        let _ = fmt
            .SetWordWrapping(windows::Win32::Graphics::DirectWrite::DWRITE_WORD_WRAPPING_NO_WRAP);
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
        let Some(fmt) = self.format(size, weight, mono) else {
            return;
        };
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
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        let _ = fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
        let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
    }

    /// 应用 logo：圆角磁贴 + 白色 Z 字形
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
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + size,
                bottom: y + size,
            },
            radiusX: size * (4.0 / 30.0),
            radiusY: size * (4.0 / 30.0),
        };
        let tb = self.brush(target, self.theme.logo_tile, alpha);
        target.FillRoundedRectangle(&tile, &tb);

        if self.logo_geo.is_none() {
            self.logo_geo = self.build_logo_glyph();
        }
        let Some(geo) = self.logo_geo.clone() else {
            return;
        };
        let zb = self.brush(target, [1.0, 1.0, 1.0, 1.0], alpha);
        let m = Matrix3x2 {
            M11: size / 30.0,
            M12: 0.0,
            M21: 0.0,
            M22: size / 30.0,
            M31: x,
            M32: y,
        };
        target.SetTransform(&m);
        target.FillGeometry(&geo, &zb, None);
        target.SetTransform(&Matrix3x2::identity());
    }

    /// 构建白色 Z 字形路径（三段图形单位）
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

    /// 建立或复用 HwndRenderTarget；失败返回 None
    unsafe fn ensure_target(
        &mut self,
        hwnd: HWND,
        rect_phys: &RECT,
        dpi: f32,
    ) -> Option<ID2D1HwndRenderTarget> {
        let dpi = if dpi.is_finite() && dpi >= 1.0 {
            dpi
        } else {
            1.0
        };
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
                && self.target.as_ref().is_some_and(|t| {
                    t.Resize(&D2D_SIZE_U {
                        width: w_px,
                        height: h_px,
                    })
                    .is_ok()
                });
            if !resized {
                self.target = None;
                let pf = D2D1_PIXEL_FORMAT {
                    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                };
                // 软件渲染：硬件路径会拉起 d3d11+显卡驱动栈，驱动内部提交
                // 占 60MB+，远超本程序预算；窗口小，软光栅耗时可忽略
                let props = D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                    pixelFormat: pf,
                    dpiX: dpi * 96.0,
                    dpiY: dpi * 96.0,
                    ..Default::default()
                };
                let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                    hwnd,
                    pixelSize: D2D_SIZE_U {
                        width: w_px,
                        height: h_px,
                    },
                    presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                };
                match self.factory.CreateHwndRenderTarget(&props, &hwnd_props) {
                    Ok(target) => {
                        let black = target
                            .CreateSolidColorBrush(
                                &D2D1_COLOR_F {
                                    r: 0.0,
                                    g: 0.0,
                                    b: 0.0,
                                    a: 1.0,
                                },
                                None,
                            )
                            .ok();
                        self.black = black;
                        self.target = Some(target);
                        self.target_dpi = dpi;
                    }
                    Err(e) => {
                        crate::platform::log(&format!(
                            "[Quotify] CreateHwndRenderTarget 失败: {e}"
                        ));
                        return None;
                    }
                }
            }
        }
        self.target.clone()
    }

    /// 设备丢失时丢弃设备绑定资源，下帧整建
    fn drop_device_resources(&mut self) {
        self.target = None;
        self.black = None;
        self.logo_geo = None;
        self.bolt_geo = None;
        self.dash_style = None;
    }

    /// 账号切换页：导航行 + 账号列表 + 添加入口；当前项左竖条，hover 整行淡填充
    unsafe fn draw_account_picker(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        w: f32,
        dy: f32,
        alpha: f32,
    ) {
        let s = model.strings;
        let pad = 20.0;
        let mut y = dy + 12.0;

        // 导航行：返回 + 居中标题，同设置页
        self.back_arrow(target, Hit::Back, pad, y + 6.0);
        let title_rect = D2D_RECT_F {
            left: pad,
            top: y,
            right: w - pad,
            bottom: y + 26.0,
        };
        self.text_aligned(
            target,
            s.switch_account,
            &title_rect,
            16.0,
            400,
            self.theme.text_primary,
            alpha,
            Align::Center,
            false,
        );
        y += 30.0;

        let selected = model.account.map(|a| a.index);
        for (i, acc) in model.accounts.iter().enumerate() {
            if self.hover == Some(Hit::PickAccount(i)) {
                let row = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: pad - 6.0,
                        top: y + 4.0,
                        right: w - pad + 6.0,
                        bottom: y + 40.0,
                    },
                    radiusX: 6.0,
                    radiusY: 6.0,
                };
                let fill = self.brush(target, self.theme.track, alpha);
                target.FillRoundedRectangle(&row, &fill);
            }
            let cur = selected == Some(i);
            if cur {
                let bar = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: pad - 6.0,
                        top: y + 14.0,
                        right: pad - 3.0,
                        bottom: y + 30.0,
                    },
                    radiusX: 1.5,
                    radiusY: 1.5,
                };
                let b = self.brush(target, self.theme.accent, alpha);
                target.FillRoundedRectangle(&bar, &b);
            }
            // 右端徽标：平台最右，key 前缀徽标在其左；空 key 跳过。宽度先算后定名称可用区
            let platform = if acc.platform == crate::api::client::Platform::Cn {
                s.platform_cn
            } else {
                s.platform_intl
            };
            let platform_label = if acc.team {
                format!("{platform} · {}", s.team_badge)
            } else {
                platform.to_string()
            };
            let pw = self.measure(&platform_label, 10.5, 400, false) + 14.0;
            let px = w - pad - pw;
            self.badge(
                target,
                &platform_label,
                px,
                y + 13.5,
                pw,
                self.theme.border,
                self.theme.text_secondary,
                alpha,
                false,
            );
            // chars 截取规避多字节切片 panic；不足 4 位有多少显多少
            let prefix: String = acc.api_key.chars().take(4).collect();
            let mut name_right = px - 10.0;
            if !prefix.is_empty() {
                let key_label = format!("{prefix}…");
                let kw = self.measure(&key_label, 10.5, 400, true) + 14.0;
                let kx = px - 6.0 - kw;
                self.badge(
                    target,
                    &key_label,
                    kx,
                    y + 13.5,
                    kw,
                    self.theme.border,
                    self.theme.text_tertiary,
                    alpha,
                    true,
                );
                name_right = kx - 8.0;
            }
            let name_w = (name_right - (pad + 8.0)).max(60.0);
            let name = self.ellipsize(&acc.name, 14.0, name_w, 500, false);
            self.text(
                target,
                &name,
                pad + 8.0,
                y + 12.0,
                name_w,
                20.0,
                14.0,
                if cur { 600 } else { 500 },
                self.theme.text_primary,
                alpha,
            );
            self.hits.push((
                Hit::PickAccount(i),
                D2D_RECT_F {
                    left: pad - 6.0,
                    top: y,
                    right: w - pad + 6.0,
                    bottom: y + 44.0,
                },
            ));
            y += 44.0;
        }
    }

    /// 下拉小箭头（∨）
    unsafe fn chevron(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        x: f32,
        cy: f32,
        color: [f32; 4],
        alpha: f32,
    ) {
        let b = self.brush(target, color, alpha);
        let half = 5.0;
        self.line(target, x, cy - 3.0, x + half, cy + 3.0, &b, 1.8);
        self.line(
            target,
            x + half,
            cy + 3.0,
            x + half * 2.0,
            cy - 3.0,
            &b,
            1.8,
        );
    }

    /// 高峰徽标：闪电 + 文字整组右对齐，字号与区块标题一致、垂直居中
    unsafe fn peak_badge(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        title_y: f32,
        w: f32,
        alpha: f32,
        s: &Strings,
    ) {
        let pad = 20.0;
        // 与 12px 标题行视觉平衡：文字同 12px，闪电 14px 居中
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

    /// 7×13 单位闪电多边形
    fn build_bolt_glyph(&self) -> Option<ID2D1PathGeometry> {
        unsafe {
            let geo = self.factory.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            sink.BeginFigure(Vector2 { X: 4.5, Y: 0.0 }, D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(Vector2 { X: 0.5, Y: 7.5 });
            sink.AddLine(Vector2 { X: 3.2, Y: 7.5 });
            sink.AddLine(Vector2 { X: 2.5, Y: 13.0 });
            sink.AddLine(Vector2 { X: 6.5, Y: 5.0 });
            sink.AddLine(Vector2 { X: 3.8, Y: 5.0 });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close().ok()?;
            Some(geo)
        }
    }

    /// 峰谷说明卡片：不透明底圆角卡，盖在数据行上方
    unsafe fn tip_card(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
        tip: &str,
    ) {
        let pad = 20.0;
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
        // 底色取 bg 但不透明，避免透出被盖住的数据行
        let [r, g, b, _] = self.theme.bg;
        let fill = self.brush(target, [r, g, b, alpha], 1.0);
        target.FillRoundedRectangle(&card, &fill);
        let line = self.brush(target, self.theme.border, alpha);
        target.DrawRoundedRectangle(&card, &line, 1.0, None);
        self.text(
            target,
            tip,
            cx + 10.0,
            y + 8.0,
            cw - 20.0,
            32.0,
            11.0,
            400,
            self.theme.text_secondary,
            alpha,
        );
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
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
