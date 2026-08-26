//! 面板内容渲染

#![allow(unsafe_op_in_unsafe_fn)]

mod main;
mod picker;
mod settings;
mod widgets;

use std::collections::HashMap;

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget,
    ID2D1PathGeometry, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::core::PCWSTR;
use windows_numerics::Vector2;

use super::anim::{Tween, animations_allowed, ease_out_cubic};
use super::model::PanelModel;
use super::theme::Theme;
use super::{Panel, PanelView};

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

/// 可点击元素标识，用于命中检测；按 UI 场景分组，与 handle_panel_hit 的臂分组对应
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
    RevealKey,
    InputOrg,
    InputProject,
    SaveAccount,
    Platform(crate::api::Platform),

    // ── 设置 · 配置管理与关于 ──
    ExportConfig,
    ImportConfig,
    CheckUpdate,
    OpenDownload,
}

/// Hit 的谓词集中放在枚举旁维护；新增输入框类变体须同步收录
impl Hit {
    /// 点击后会进入/保持输入态或 key 明文查看态的命中；
    /// WM_LBUTTONUP 据此决定点击后是否结束输入态
    pub(crate) fn is_input_hit(&self) -> bool {
        matches!(
            self,
            Self::CustomizeInterval
                | Self::InputInterval
                | Self::InputProxy
                | Self::InputPeakStart
                | Self::InputPeakEnd
                | Self::AddAccount
                | Self::InputName
                | Self::InputKey
                | Self::RevealKey
                | Self::InputOrg
                | Self::InputProject
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

/// 淡入与刷新旋转两类动画的插值状态
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
    target: ID2D1HwndRenderTarget,
    black: ID2D1SolidColorBrush,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
    formats: HashMap<(u32, u16, bool), IDWriteTextFormat>,
    /// measure_ro 专用格式缓存：&self 上下文无法写 formats，
    /// 经 RefCell 提供内部可变性
    ro_formats: std::cell::RefCell<HashMap<(&'static str, u32, u16), IDWriteTextFormat>>,
    pub theme: Theme,
    pub hits: Vec<(Hit, D2D_RECT_F)>,
    pub hover: Option<Hit>,
    pub anim: AnimState,
    pending_tip: Option<(f32, f32, f32)>,
    font_fallback: bool,
    target_dpi: f32,
    anim_allowed: bool,
    logo_geo: Option<ID2D1PathGeometry>,
    bolt_geo: Option<ID2D1PathGeometry>,
    eye_geo: Option<ID2D1PathGeometry>,
    dash_style: Option<ID2D1StrokeStyle>,
}

impl Renderer {
    /// 创建渲染器；target 与 black 刷子成对随建，任一失败即放弃整个渲染器，
    /// 由调用方跳过本帧、待下次绘制消息重试——这是 black 恒为活句柄的契约来源
    pub fn new(hwnd: HWND, rect_phys: &RECT, dpi: f32) -> Option<Self> {
        unsafe {
            let factory: ID2D1Factory =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            let dpi = sane_dpi(dpi);
            let (target, black) = create_hwnd_target(&factory, hwnd, rect_phys, dpi)?;
            Some(Self {
                factory,
                dwrite,
                target,
                black,
                brushes: HashMap::new(),
                formats: HashMap::new(),
                ro_formats: std::cell::RefCell::new(HashMap::new()),
                theme: Theme::new(Theme::system_appearance()),
                hits: Vec::new(),
                hover: None,
                anim: AnimState::new(),
                pending_tip: None,
                font_fallback: false,
                target_dpi: dpi,
                anim_allowed: animations_allowed(),
                logo_geo: None,
                bolt_geo: None,
                eye_geo: None,
                dash_style: None,
            })
        }
    }

    /// 创建文本格式
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
        self.measure_with(&fmt, s)
    }

    /// 只读测宽：供 &self 上下文（光标定位与渲染共用 caret_layout）使用。
    /// format 按 (字号,字重,字体族) 缓存——拖选高频路径上每次鼠标移动
    /// 做多次测宽，逐次建 COM 对象纯属浪费
    pub(crate) unsafe fn measure_ro(&self, s: &str, size: f32, weight: u16, mono: bool) -> f32 {
        unsafe {
            let family = if mono {
                "Consolas"
            } else if self.font_fallback {
                "Segoe UI"
            } else {
                "Segoe UI Variable Text"
            };
            let key = (family, size.to_bits(), weight);
            if let Some(f) = self.ro_formats.borrow().get(&key) {
                return self.measure_with(f, s);
            }
            let Ok(fmt) = self.dwrite.CreateTextFormat(
                PCWSTR(wide(family).as_ptr()),
                None,
                windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT(weight as i32),
                windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STYLE_NORMAL,
                windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STRETCH_NORMAL,
                size,
                PCWSTR(wide("").as_ptr()),
            ) else {
                return 0.0;
            };
            self.ro_formats.borrow_mut().insert(key, fmt.clone());
            self.measure_with(&fmt, s)
        }
    }

    /// 按 format 建 layout 取宽
    unsafe fn measure_with(&self, fmt: &IDWriteTextFormat, s: &str) -> f32 {
        unsafe {
            let w16: Vec<u16> = s.encode_utf16().collect();
            if w16.is_empty() {
                return 0.0;
            }
            let Ok(layout) = self.dwrite.CreateTextLayout(&w16, fmt, 1.0e6, 1.0e6) else {
                return 0.0;
            };
            let mut m = windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() {
                m.widthIncludingTrailingWhitespace
            } else {
                0.0
            }
        }
    }

    /// 真实度量保头截断
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
    /// 返回 false 表示设备已丢失，调用方须丢弃整个 Renderer，下帧全量重建
    pub fn paint(
        &mut self,
        hwnd: HWND,
        rect_phys: &RECT,
        panel: &Panel,
        model: &PanelModel,
        view: PanelView,
        dpi: f32,
    ) -> bool {
        unsafe {
            // 整建失败只跳过本帧：Renderer 本体仍健康，保留待下次绘制消息重试
            let Some(target) = self.ensure_target(hwnd, rect_phys, dpi) else {
                return true;
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
                Ok(()) => true,
                Err(e) => {
                    // 设备丢失：target 与全部设备绑定刷子一并失效，残留只会让
                    // 面板永久空白——整个 Renderer 丢弃，下帧重建并重新对齐主题
                    crate::platform::log(&format!("[Quotify] EndDraw 失败: {e}"));
                    false
                }
            }
        }
    }

    /// 绘制一帧
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
        let dy = dy - panel.scroll_dy;

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

    // ── 文本原语 ──

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

    /// 纯色刷子：按量化颜色缓存复用，命中即克隆；alpha 乘进颜色分量。
    /// 每色独立实例缓存——单刷 SetColor 复用会被交替覆盖，滑杆图标整支隐形
    unsafe fn brush(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        c: [f32; 4],
        alpha: f32,
    ) -> ID2D1SolidColorBrush {
        let a = (c[3] * alpha).clamp(0.0, 1.0);
        let key = pack_color(c[0], c[1], c[2], a);
        if let Some(b) = self.brushes.get(&key) {
            return b.clone();
        }
        let color = D2D1_COLOR_F {
            r: c[0],
            g: c[1],
            b: c[2],
            a,
        };
        match target.CreateSolidColorBrush(&color, None) {
            Ok(b) => {
                self.brushes.insert(key, b.clone());
                b
            }
            // 创建失败只在设备濒死时发生：black 随 target 常驻必有活句柄，拿它
            // 绘制把错误递延给 EndDraw，本帧色偏可接受；随后 Renderer 整体
            // 丢弃重建，绝不 panic=abort 把瞬时设备丢失升级成进程终止
            Err(_) => self.black.clone(),
        }
    }

    /// 建立或复用 HwndRenderTarget；失败返回 None
    unsafe fn ensure_target(
        &mut self,
        hwnd: HWND,
        rect_phys: &RECT,
        dpi: f32,
    ) -> Option<ID2D1HwndRenderTarget> {
        let dpi = sane_dpi(dpi);
        let w_px = (rect_phys.right - rect_phys.left).max(1) as u32;
        let h_px = (rect_phys.bottom - rect_phys.top).max(1) as u32;
        // 尺寸/DPI 变化时处理；内部尺寸记录以 GetPixelSize 读数为准
        let sz = self.target.GetPixelSize();
        let need_rebuild = sz.width != w_px || sz.height != h_px || self.target_dpi != dpi;
        if need_rebuild {
            // 仅尺寸变化优先 Resize 复用——整建重分配后台缓冲引发顿挫；
            // Resize 不换设备，已建刷子继续有效
            let size_only = self.target_dpi == dpi;
            let resized = size_only
                && self
                    .target
                    .Resize(&D2D_SIZE_U {
                        width: w_px,
                        height: h_px,
                    })
                    .is_ok();
            if !resized {
                // 整建换 target：缓存刷子绑死创建它的旧 target，残留句柄会让
                // 整建后首帧绘制报错递延到 EndDraw 而白费一帧，先清缓存
                self.brushes.clear();
                match create_hwnd_target(&self.factory, hwnd, rect_phys, dpi) {
                    Some((target, black)) => {
                        self.target = target;
                        self.black = black;
                        self.target_dpi = dpi;
                    }
                    // 保留旧 target 原样跳过本帧，待下次绘制消息重试整建；失败细节由助手内记录
                    None => return None,
                }
            }
        }
        Some(self.target.clone())
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 颜色各通道量化 8bit 打包作刷子缓存键——与显示色深一致，无视觉损失
fn pack_color(r: f32, g: f32, b: f32, a: f32) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (q(r) << 24) | (q(g) << 16) | (q(b) << 8) | q(a)
}

/// 非法 DPI 兜底为 1.0，防 0/NaN 混进 target 建立参数
fn sane_dpi(dpi: f32) -> f32 {
    if dpi.is_finite() && dpi >= 1.0 {
        dpi
    } else {
        1.0
    }
}

/// 软件渲染 target 与 black 刷子成对建立；任一失败返回 None，
/// 首建与 DPI 整建两路共用，保证 target 存在时 black 必然就位
unsafe fn create_hwnd_target(
    factory: &ID2D1Factory,
    hwnd: HWND,
    rect_phys: &RECT,
    dpi: f32,
) -> Option<(ID2D1HwndRenderTarget, ID2D1SolidColorBrush)> {
    let w_px = (rect_phys.right - rect_phys.left).max(1) as u32;
    let h_px = (rect_phys.bottom - rect_phys.top).max(1) as u32;
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
    let target = match factory.CreateHwndRenderTarget(&props, &hwnd_props) {
        Ok(t) => t,
        Err(e) => {
            crate::platform::log(&format!("[Quotify] CreateHwndRenderTarget 失败: {e}"));
            return None;
        }
    };
    let black = match target.CreateSolidColorBrush(
        &D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        },
        None,
    ) {
        Ok(b) => b,
        Err(e) => {
            crate::platform::log(&format!("[Quotify] black 刷子创建失败: {e}"));
            return None;
        }
    };
    Some((target, black))
}

impl Default for AnimState {
    fn default() -> Self {
        Self::new()
    }
}
