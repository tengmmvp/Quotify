//! 面板内容渲染

#![allow(unsafe_op_in_unsafe_fn)]

pub mod about;
mod main;
pub mod popup;
mod settings;
mod widgets;

use std::collections::HashMap;

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_NO_SNAP, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1PathGeometry,
    ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::core::PCWSTR;
use windows_numerics::Vector2;

use super::anim::{Tween, animations_allowed, ease_out_cubic};
use super::model::PanelModel;
use super::theme::Theme;
use super::{Panel, PanelView};
use crate::platform::wide;

/// 外观模式选择：System = 跟随系统
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppearanceChoice {
    System,
    Light,
    Dark,
}

impl AppearanceChoice {
    /// 配置字符串 → 选项：未配置 / 未知值都归「跟随系统」，
    /// 与 explicit_appearance 同口径，控件高亮不错档。
    pub fn from_setting(setting: Option<&str>) -> Self {
        match crate::ui::panel::theme::explicit_appearance(setting) {
            Some(crate::ui::panel::theme::Appearance::Light) => AppearanceChoice::Light,
            Some(crate::ui::panel::theme::Appearance::Dark) => AppearanceChoice::Dark,
            None => AppearanceChoice::System,
        }
    }

    /// 选项 → 配置字符串：System 写 None（跟随系统）。
    pub fn to_config(self) -> Option<String> {
        match self {
            AppearanceChoice::System => None,
            AppearanceChoice::Light => Some("light".to_string()),
            AppearanceChoice::Dark => Some("dark".to_string()),
        }
    }
}

/// 界面语言选择：System = 跟随系统
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageChoice {
    System,
    Zh,
    En,
}

impl LanguageChoice {
    /// 配置字符串 → 选项：未配置 / 未知值都归「跟随系统」，
    /// 与 explicit_lang 同口径，控件高亮不错档。
    pub fn from_setting(setting: Option<&str>) -> Self {
        match crate::ui::i18n::explicit_lang(setting) {
            Some(crate::ui::i18n::Lang::Zh) => LanguageChoice::Zh,
            Some(crate::ui::i18n::Lang::En) => LanguageChoice::En,
            None => LanguageChoice::System,
        }
    }

    /// 选项 → 配置字符串：System 写 None（跟随系统）。
    pub fn to_config(self) -> Option<String> {
        match self {
            LanguageChoice::System => None,
            LanguageChoice::Zh => Some("zh".to_string()),
            LanguageChoice::En => Some("en".to_string()),
        }
    }
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
    UsageInfo,

    // ── 导航 ──
    Back,
    ClosePanel,

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
    ApplyProxy,

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

    // ── 关于窗 ──
    LinkRepo,
    LinkIssues,
    CopyDiagnostics,
    NewsItem(usize),
    AboutLogo,
}

/// Hit 的谓词集中放在枚举旁维护；新增输入框类变体须同步收录
impl Hit {
    /// 点击后会进入/保持输入态或 key 明文查看态的命中；
    /// WM_LBUTTONUP 据此决定点击后是否结束输入态。
    /// 输入框槽位经 input_field_of_hit 穷尽判定，
    /// 此处只补四个非槽位但保持输入态的命中（应用按钮校验失败
    /// 保留字段供修改）
    pub(crate) fn is_input_hit(&self) -> bool {
        crate::ui::panel::input_field_of_hit(*self).is_some()
            || matches!(
                self,
                Self::CustomizeInterval | Self::AddAccount | Self::RevealKey | Self::ApplyProxy
            )
    }

    /// 模态类命中：分派必须走窗口过程层拦截直调两段式，进
    /// handle_panel_hit 即接线错误。新增模态变体时同步登记
    /// crate::app::dispatch_modal。
    pub(crate) fn is_modal(&self) -> bool {
        matches!(self, Self::ExportConfig | Self::ImportConfig)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

/// 面板各动画的插值状态：淡入、刷新旋转、页脚换装、错误跑马灯
pub struct AnimState {
    pub appear: Option<Tween>,
    pub spin: f32,
    pub footer: Option<FooterAnim>,
    pub error_scroll: Option<ErrorScroll>,
}

/// 页脚换装动画的单次状态
pub struct FooterAnim {
    pub tween: Tween,
    pub old_text: String,
    pub new_text: String,
    pub old_w: f32,
    pub new_w: f32,
}

/// 页脚错误摘要的滚动状态：摘要超面板宽时传送带循环，开头与每轮
/// 回到起点后暂停数秒。布局与宽度在创建时算好，滚动帧零测量零分配
/// 零 COM 对象重建
pub struct ErrorScroll {
    /// 内容指纹：错误负载、快照时间、可视宽、语言与动效开关的哈希，
    /// 任一变化即重建
    key: u64,
    /// 三块内容（前缀/大类/尾段）：布局与宽成对持有，消费端不重排；
    /// 减动效超宽时存截断串的布局
    blocks: [ScrollBlock; 3],
    /// 块界左右括号：加粗字重，三块共享
    bracket: ScrollBlock,
    bracket_end: ScrollBlock,
    /// 摘要是否超宽滚动：非滚动态只作缓存，tick 不保活
    scrolling: bool,
    /// 失焦冻结中：时钟已撤，激活消息唤醒；重挂判定与推进都绕开
    frozen: bool,
    /// 当前偏移，0..total_w
    offset: f32,
    /// 暂停截止时刻；滚动中为过去时刻
    resume_at: std::time::Instant,
    /// 上帧时刻，帧率随动画优先级变化，按实距推进
    last: std::time::Instant,
}

/// 跑马灯的单个内容块：布局与宽成对，推进与绘制同引一份
struct ScrollBlock {
    layout: Option<IDWriteTextLayout>,
    w: f32,
}

impl ScrollBlock {
    /// 建块并从布局读宽；布局建不出即空块
    unsafe fn new(layout: Option<IDWriteTextLayout>) -> Self {
        let w = layout.as_ref().map_or(0.0, |l| Renderer::layout_width(l));
        Self { layout, w }
    }
}

impl ErrorScroll {
    /// 滚动周期：在场块总宽加循环段间空隙
    pub(crate) fn total_w(&self) -> f32 {
        belt_width(
            &self.blocks.each_ref().map(|b| b.w),
            self.bracket.w + self.bracket_end.w,
        ) + ERROR_SCROLL_GAP
    }
}

/// 在场块（内容宽 + 括号对）的总宽：空块零贡献，周期不带尾随空白。
/// 溢出判定与滚动周期同引，块结构改动只改此处
pub(crate) fn belt_width(blocks: &[f32], pair: f32) -> f32 {
    blocks.iter().filter(|w| **w > 0.0).map(|w| *w + pair).sum()
}

/// 跑马灯帧间隔
/// 30px/s 下 33ms 每帧推进约 1px，与页脚换装同档减半整窗重绘开销。
pub(crate) const ERROR_SCROLL_FRAME_MS: u32 = 33;
/// 跑马灯滚动速度（逻辑 px/秒）
pub(crate) const ERROR_SCROLL_SPEED: f32 = 30.0;
/// 轮回到位后的暂停时长
pub(crate) const ERROR_SCROLL_PAUSE: std::time::Duration = std::time::Duration::from_secs(3);
/// 循环平铺的段间空隙（逻辑 px）
pub(crate) const ERROR_SCROLL_GAP: f32 = 48.0;
/// 跑马灯文案字号：三块内容与括号同号
pub(crate) const ERROR_SCROLL_SIZE: f32 = 12.0;
/// 块界括号字重（加粗）：内容常规字重 400
pub(crate) const ERROR_SCROLL_BRACKET_WEIGHT: u16 = 700;

/// 跑马灯的单拍结果
pub(crate) enum ScrollTick {
    /// 滚动帧，帧间隔 ERROR_SCROLL_FRAME_MS
    Frame,
    /// 暂停中，距恢复毫秒数
    Pause(u32),
    /// 前台门控冻结中，不推进不重绘；不保活，回焦由激活消息重启时钟
    Idle,
}

impl AnimState {
    pub fn new() -> Self {
        Self {
            appear: None,
            spin: 0.0,
            footer: None,
            error_scroll: None,
        }
    }
}

impl Renderer {
    /// 刷新按钮动画是否仍在进行
    pub fn spin_remaining(&mut self) -> bool {
        if !self.anim_allowed {
            self.anim.spin = 0.0;
            return false;
        }
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

    /// 挂起页脚换装动画：新旧串与测宽由调用方备好；随错误退场的
    /// 拆除由绘制入口按数据统一处理
    pub fn start_footer_anim(
        &mut self,
        old_text: String,
        old_w: f32,
        new_text: String,
        new_w: f32,
    ) {
        self.anim.footer = Some(FooterAnim {
            tween: crate::ui::panel::anim::Tween::now(3600),
            old_text,
            new_text,
            old_w,
            new_w,
        });
    }

    /// 跑马灯推进一帧
    /// 失焦先于暂停判定：冻结即刻生效，不为等恢复点多保活一拍；
    /// 暂停期一次定到恢复点零空转；非滚动态与减少动效时不保活；
    /// 回焦由激活消息重启时钟，续滚不跳变。
    pub fn error_scroll_tick(&mut self, active: bool) -> Option<ScrollTick> {
        let sc = self.anim.error_scroll.as_mut()?;
        if !sc.scrolling || !self.anim_allowed {
            return None;
        }
        let now = std::time::Instant::now();
        if !active {
            sc.frozen = true;
            sc.last = now;
            return Some(ScrollTick::Idle);
        }
        // 暂停期不推进 last：轮回到位时 last 已对齐恢复点，恢复首帧
        // 位移只计一帧间隔。
        if now < sc.resume_at {
            return Some(ScrollTick::Pause((sc.resume_at - now).as_millis() as u32));
        }
        sc.frozen = false;
        let dt = now.saturating_duration_since(sc.last).as_secs_f32();
        sc.last = now;
        sc.offset += ERROR_SCROLL_SPEED * dt;
        if sc.offset >= sc.total_w() {
            // 一轮滚完，文本开头回到左侧，暂停后再滚；last 同步到恢复点，
            // 暂停时长不计入首帧位移。
            sc.offset = 0.0;
            sc.resume_at = now + ERROR_SCROLL_PAUSE;
            sc.last = sc.resume_at;
        }
        Some(ScrollTick::Frame)
    }

    /// 跑马灯处于滚动且未冻结：时钟该在场的判定，重挂点同引。
    /// 冻结态不重挂——挂上也是一拍空转即撤。
    pub fn error_scrolling(&self) -> bool {
        self.anim
            .error_scroll
            .as_ref()
            .is_some_and(|sc| sc.scrolling && !sc.frozen)
    }

    /// 冻结的跑马灯回焦复活：last 重压到当前，防首帧把停摆时长一次
    /// 推进跳变；返回是否需要重新挂钟。
    pub fn resume_error_scroll(&mut self) -> bool {
        if let Some(sc) = self.anim.error_scroll.as_mut()
            && sc.scrolling
            && sc.frozen
        {
            sc.frozen = false;
            sc.last = std::time::Instant::now();
            return true;
        }
        false
    }

    /// 鼠标位置（逻辑像素）命中的可点击元素；未命中返回 None
    pub fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .find(|(_, rc)| x >= rc.left && x <= rc.right && y >= rc.top && y <= rc.bottom)
            .map(|(h, _)| *h)
    }

    /// 动画是否被允许
    pub fn animations_on(&self) -> bool {
        self.anim_allowed
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
    ro_formats: std::cell::RefCell<HashMap<(&'static str, u32, u16), IDWriteTextFormat>>,
    frame_measures: HashMap<(usize, usize, u32, u16, bool), f32>,
    pub theme: Theme,
    pub hits: Vec<(Hit, D2D_RECT_F)>,
    pub hover: Option<Hit>,
    pub anim: AnimState,
    pending_tip: Option<(f32, f32, f32)>,
    font_fallback: bool,
    target_dpi: f32,
    anim_allowed: bool,
    logo_geo: Option<(ID2D1PathGeometry, ID2D1PathGeometry)>,
    bolt_geo: Option<ID2D1PathGeometry>,
    eye_geo: Option<ID2D1PathGeometry>,
    pacman_geo: Option<(ID2D1PathGeometry, ID2D1PathGeometry)>,
    refresh_geo: Option<ID2D1PathGeometry>,
    u16_buf: std::cell::RefCell<Vec<u16>>,
    dash_style: Option<ID2D1StrokeStyle>,
    mcp_cache: Option<main::McpCompCache>,
    token_cells: Option<main::TokenCells>,
    main_texts: Option<main::MainTexts>,
    /// 顶栏标题截断缓存：键 (源串, 可用宽) → (截断串, 实宽)。只随
    /// 账号名与宽度变化，与快照无关，跨快照存活
    title_cache: Option<((String, f32), (String, f32))>,
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
                frame_measures: HashMap::new(),
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
                pacman_geo: None,
                refresh_geo: None,
                u16_buf: std::cell::RefCell::new(Vec::new()),
                dash_style: None,
                mcp_cache: None,
                token_cells: None,
                main_texts: None,
                title_cache: None,
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
    pub(crate) unsafe fn measure(&mut self, s: &str, size: f32, weight: u16, mono: bool) -> f32 {
        let Some(fmt) = self.format(size, weight, mono) else {
            return 0.0;
        };
        self.measure_with(&fmt, s)
    }

    /// 只读上下文的 format 取用：按 (族,字号,字重) 缓存，缺席时建一次。
    /// 供 &self 测宽路径（前缀宽表）共用——拖选高频路径上逐次建 COM
    /// 对象纯属浪费
    unsafe fn ro_format(&self, size: f32, weight: u16, mono: bool) -> Option<IDWriteTextFormat> {
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
                return Some(f.clone());
            }
            let fmt = self
                .dwrite
                .CreateTextFormat(
                    PCWSTR(wide(family).as_ptr()),
                    None,
                    windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT(weight as i32),
                    windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STYLE_NORMAL,
                    windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    PCWSTR(wide("").as_ptr()),
                )
                .ok()?;
            self.ro_formats.borrow_mut().insert(key, fmt.clone());
            Some(fmt)
        }
    }

    /// 前缀宽表：整串一次 CreateTextLayout + GetClusterMetrics，按簇的
    /// UTF-16 长度把簇宽摊到字符位次上累积——P[i] = 前 i 个字符的累计
    /// 宽，表长 = 字符数 + 1。任何「前缀宽度查询」都改为查表（索引或
    /// 二分），替代逐前缀新建 TextLayout 的测量；簇内字符边界（代理对/
    /// 组合记号归并簇）取簇末累计宽，与可视切片/截断只在字符边界取宽
    /// 的用法相容。DWrite 不可用时退化为全零表，与旧测量失败返 0 一致
    pub(crate) unsafe fn prefix_widths(
        &self,
        chars: &[char],
        size: f32,
        weight: u16,
        mono: bool,
    ) -> Vec<f32> {
        unsafe {
            let n = chars.len();
            let mut table = vec![0.0f32; n + 1];
            if n == 0 {
                return table;
            }
            let Some(fmt) = self.ro_format(size, weight, mono) else {
                return table;
            };
            // encode_utf16 的 &mut [u16] 参数按切片长度校验容量，裸 Vec
            // 长度恒 0 必 panic。栈上双字缓冲接住单字符再入列。
            let mut w16: Vec<u16> = Vec::with_capacity(n);
            for c in chars {
                let mut buf = [0u16; 2];
                w16.extend_from_slice(c.encode_utf16(&mut buf));
            }
            let Ok(layout) = self.dwrite.CreateTextLayout(&w16, &fmt, 1.0e6, 1.0e6) else {
                return table;
            };
            let mut cms = vec![
                windows::Win32::Graphics::DirectWrite::DWRITE_CLUSTER_METRICS::default();
                w16.len()
            ];
            let mut count = 0u32;
            if layout
                .GetClusterMetrics(Some(&mut cms), &mut count)
                .is_err()
            {
                return table;
            }
            // 每字符的 UTF-16 起始单位位次；簇宽记到簇末所在字符边界上。
            let mut starts: Vec<usize> = Vec::with_capacity(n + 1);
            let mut off = 0usize;
            for c in chars {
                starts.push(off);
                off += c.len_utf16();
            }
            starts.push(off);
            let mut w = 0.0f32;
            let mut units = 0usize;
            let mut ci = 0usize;
            for cm in &cms[..count as usize] {
                w += cm.width;
                units += cm.length as usize;
                while ci < n && starts[ci + 1] <= units {
                    table[ci + 1] = w;
                    ci += 1;
                }
            }
            // 度量覆不满串（异常短缺）时补平尾，保住单调不减。
            for t in table.iter_mut().skip(ci + 1) {
                *t = w;
            }
            table
        }
    }

    /// 帧内去重测宽：同帧内相同 ('static 文本, 字号, 字重, mono) 只建一次
    /// layout[族由 mono 代理，实参族仅两三种]。ellipsize 的省略号、区块
    /// 标题等每帧重复出现的常量文本走这里；收窄理由见 frame_measures 字段注释
    unsafe fn measure_static(
        &mut self,
        s: &'static str,
        size: f32,
        weight: u16,
        mono: bool,
    ) -> f32 {
        let key = (s.as_ptr() as usize, s.len(), size.to_bits(), weight, mono);
        if let Some(&w) = self.frame_measures.get(&key) {
            return w;
        }
        let w = self.measure(s, size, weight, mono);
        self.frame_measures.insert(key, w);
        w
    }

    /// 编码入共享暂存并返回借用
    fn encode_u16(&self, s: &str) -> std::cell::RefMut<'_, Vec<u16>> {
        let mut buf = self.u16_buf.borrow_mut();
        buf.clear();
        buf.extend(s.encode_utf16());
        buf
    }

    /// 按 format 建 layout 取宽
    unsafe fn measure_with(&self, fmt: &IDWriteTextFormat, s: &str) -> f32 {
        unsafe {
            if s.is_empty() {
                return 0.0;
            }
            let w16 = self.encode_u16(s);
            let Ok(layout) = self.dwrite.CreateTextLayout(&w16, fmt, 1.0e6, 1.0e6) else {
                return 0.0;
            };
            Self::layout_width(&layout)
        }
    }

    /// 真实度量保头截断；返回 (截断串, 该串实际宽)——调用点排版紧随
    /// 其后的元素（chevron、NEW 徽标）不必再 measure 复测。宽度取自
    /// 簇宽累计：不截断即全串簇宽和，截断则补省略号宽
    unsafe fn ellipsize(
        &mut self,
        s: &str,
        size: f32,
        max_w: f32,
        weight: u16,
        mono: bool,
    ) -> (String, f32) {
        if s.is_empty() {
            return (String::new(), 0.0);
        }
        // 省略号宽走帧内去重：同帧多次 ellipsize 同参时只建一次 layout。
        let ell_w = self.measure_static("…", size, weight, mono);
        let budget = (max_w - ell_w).max(0.0);
        let Some(fmt) = self.format(size, weight, mono) else {
            return (s.to_string(), 0.0);
        };
        let (layout, w16_len) = {
            let w16 = self.encode_u16(s);
            let len = w16.len();
            (self.dwrite.CreateTextLayout(&w16, &fmt, 1.0e6, 1.0e6), len)
        };
        let Ok(layout) = layout else {
            // 退化路径按整串测量兜底，宽 0 会让跟随元素叠上文本头。
            let w = self.measure_with(&fmt, s);
            return (s.to_string(), w);
        };
        let mut cms =
            vec![windows::Win32::Graphics::DirectWrite::DWRITE_CLUSTER_METRICS::default(); w16_len];
        let mut n = 0u32;
        if layout.GetClusterMetrics(Some(&mut cms), &mut n).is_err() {
            // 簇度量缺席时截断无从谈起，宽退回整串测量。
            let w = self.measure(s, size, weight, mono);
            return (s.to_string(), w);
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
        if units >= w16_len {
            return (s.to_string(), w);
        }
        // 暂存自 layout 创建后未被改写——会改写的调用都在提前 return
        // 的失败路径上，可安全读回切片。
        let mut out: Vec<u16> = self.u16_buf.borrow()[..units].to_vec();
        out.push(0x2026);
        let disp = String::from_utf16_lossy(&out);
        (disp, w + ell_w)
    }

    /// 按字号字重建文本布局：只读上下文取 format，供缓存的直绘文本
    /// 预建布局共用
    unsafe fn make_layout(
        &self,
        s: &str,
        size: f32,
        weight: u16,
        mono: bool,
    ) -> Option<IDWriteTextLayout> {
        if s.is_empty() {
            return None;
        }
        let fmt = self.ro_format(size, weight, mono)?;
        let w16 = self.encode_u16(s);
        self.dwrite.CreateTextLayout(&w16, &fmt, 1.0e6, 1.0e6).ok()
    }

    /// 预建布局直绘：复用缓存布局，绕开 DrawText 每次内部重建 COM 对象；
    /// 裁剪由调用点外层统一
    unsafe fn draw_layout(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        layout: &IDWriteTextLayout,
        x: f32,
        y: f32,
        brush: &ID2D1SolidColorBrush,
    ) {
        target.DrawTextLayout(
            Vector2 { X: x, Y: y },
            layout,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
        );
    }

    /// 建成布局的文本宽：先建布局再取宽的路径共用，
    /// 宽度与直绘同一来源，字号字重只写一处
    unsafe fn layout_width(layout: &IDWriteTextLayout) -> f32 {
        let mut m = windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS::default();
        if layout.GetMetrics(&mut m).is_ok() {
            m.widthIncludingTrailingWhitespace
        } else {
            0.0
        }
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
            // 整建失败只跳过本帧：Renderer 本体仍健康，保留待下次绘制消息重试。
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
            self.frame_measures.clear();
            target.BeginDraw();
            self.draw(&target, panel, model, view, &rect_logical);
            match target.EndDraw(None, None) {
                Ok(()) => true,
                Err(e) => {
                    // 设备丢失：target 与全部设备绑定刷子一并失效，残留只会让
                    // 面板永久空白——整个 Renderer 丢弃，下帧重建并重新对齐主题。
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

        // 弹出动画：内容上浮渐入；背景必须不透明全幅绘制，否则首帧露交换链、错位抖动。
        let (dy, alpha) = match &self.anim.appear {
            Some(t) if self.anim_allowed => {
                let p = ease_out_cubic(t.progress());
                ((1.0 - p) * 6.0, p)
            }
            _ => (0.0, 1.0),
        };
        let dy = dy - panel.scroll_dy;

        // 页脚动画存续单点：换装动画随错误退场（不清会藏在摘要背后
        // 空跑，错误恢复后从中途续播陈旧文案）；跑马灯只在主视图且
        // 错误与快照双在场时存续——拆掉跟随数据与视图，数据侧与各
        // 视图不再自觉维护。
        if model.error.is_some() {
            self.anim.footer = None;
        }
        if view != PanelView::Main || model.error.is_none() || model.snapshot.is_none() {
            self.anim.error_scroll = None;
        }

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
            PanelView::Main => {
                let content_h = panel.main_h as f32;
                self.draw_main(target, model, w, h, content_h, dy, alpha)
            }
            // 添加表单与设置页共用 draw_settings，内部按视图分流。
            PanelView::Settings | PanelView::AddForm => {
                self.draw_settings(target, panel, model, w, dy, alpha)
            }
        }

        // 峰谷说明卡片最后画，盖过数据行。
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
        self.text_aligned(
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
        self.text_aligned(
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

    /// 文本绘制原语：临时改共享 format 的对齐/换行/居中，用后还原——取得
    /// fmt 后不得提前 return，改态泄漏会错排后续同参文本
    #[allow(clippy::too_many_arguments)]
    unsafe fn text_raw(
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
        wrap: bool,
        vcenter: bool,
        nosnap: bool,
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
        if wrap {
            let _ = fmt
                .SetWordWrapping(windows::Win32::Graphics::DirectWrite::DWRITE_WORD_WRAPPING_WRAP);
        }
        if vcenter {
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
        let mut opts = D2D1_DRAW_TEXT_OPTIONS_CLIP;
        if nosnap {
            opts |= D2D1_DRAW_TEXT_OPTIONS_NO_SNAP;
        }
        // 空串跳过：无可绘制内容，也省一次刷子创建；刷子先建——
        // 编码暂存的借用存活期间不得再借 &mut self。
        if !s.is_empty() {
            let brush = self.brush(target, color, alpha);
            let w16 = self.encode_u16(s);
            target.DrawText(
                &w16,
                &fmt,
                rect as *const D2D_RECT_F,
                &brush,
                opts,
                windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        // 还原 format 默认态（LEADING/NO_WRAP/NEAR），与缓存创建时一致。
        if wrap {
            let _ = fmt.SetWordWrapping(
                windows::Win32::Graphics::DirectWrite::DWRITE_WORD_WRAPPING_NO_WRAP,
            );
        }
        if vcenter {
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        }
        let _ = fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
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
        self.text_raw(
            target, s, rect, size, weight, color, alpha, align, mono, false, false, false,
        );
    }

    /// 多行文案：临时开自动换行，长文按框宽折行
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
        self.text_raw(
            target, s, rect, size, weight, color, alpha, align, mono, true, false, false,
        );
    }

    /// 垂直居中版 text_aligned
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
        self.text_raw(
            target, s, rect, size, weight, color, alpha, align, mono, false, true, false,
        );
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
            // 丢弃重建，绝不 panic=abort 把瞬时设备丢失升级成进程终止。
            Err(_) => self.black.clone(),
        }
    }

    /// 收起时清空画刷缓存：键含动画 alpha 的量化档位，淡入每帧一档，
    /// 不清会随开合次数缓慢累积；下次打开首帧按需重建，成本可忽略
    pub(crate) fn clear_brush_cache(&mut self) {
        self.brushes.clear();
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
        // 尺寸/DPI 变化时处理；内部尺寸记录以 GetPixelSize 读数为准。
        let sz = self.target.GetPixelSize();
        let need_rebuild = sz.width != w_px || sz.height != h_px || self.target_dpi != dpi;
        if need_rebuild {
            // 仅尺寸变化优先 Resize 复用——整建重分配后台缓冲引发顿挫；
            // Resize 不换设备，已建刷子继续有效。
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
                // 整建后首帧绘制报错递延到 EndDraw 而白费一帧，先清缓存。
                self.brushes.clear();
                match create_hwnd_target(&self.factory, hwnd, rect_phys, dpi) {
                    Some((target, black)) => {
                        self.target = target;
                        self.black = black;
                        self.target_dpi = dpi;
                    }
                    // 保留旧 target 原样跳过本帧，待下次绘制消息重试整建；失败细节由助手内记录。
                    None => return None,
                }
            }
        }
        Some(self.target.clone())
    }
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
    // 占 60MB+，超出内存预算；窗口小，软光栅耗时可忽略。
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
