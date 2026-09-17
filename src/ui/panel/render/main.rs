//! 主视图

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget,
};
use windows_numerics::Matrix3x2;

use super::widgets::PACMAN_R;
use super::{
    Align, ERROR_SCROLL_BRACKET_WEIGHT, ERROR_SCROLL_PAUSE, ERROR_SCROLL_SIZE, ErrorScroll, Hit,
    Renderer, ScrollBlock, belt_width,
};
use crate::api::{ErrorCategory, ErrorDetail, ErrorInfo, FetchError, TOKEN_LEGS, TokenLeg};
use crate::ui::fmt;
use crate::ui::i18n::Strings;
use crate::ui::panel::layout;
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::theme::CARD_RADIUS;

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
const MCP_CELL_R: f32 = 2.0;

/// Token 卡片排版规格：数字与单位的字号、数字字重、单位间隙、卡内
/// 截断边距——测宽与绘制两侧同引
const TOKEN_NUM_SIZE: f32 = 20.0;
const TOKEN_NUM_WEIGHT: u16 = 700;
const TOKEN_UNIT_SIZE: f32 = 11.0;
const TOKEN_UNIT_GAP: f32 = 1.0;
const TOKEN_CARD_INSET: f32 = 4.0;

/// 能量条的待填区间序列（段序即绘制序）：数据段间未覆盖格位补
/// 轨道空格段；颜色由调用侧配对——数据段用段色、空格段用轨道色。
fn mcp_job_ranges(segs: &[(usize, usize, [f32; 4])]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut cursor = 0usize;
    for (start, end, _) in segs {
        if *start > cursor {
            ranges.push((cursor, *start));
        }
        ranges.push((*start, *end));
        cursor = *end;
    }
    if cursor < MCP_CELLS {
        ranges.push((cursor, MCP_CELLS));
    }
    ranges
}

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
/// 分段与全部测宽；绘制按缓存分段逐格填圆角矩形。
pub(super) struct McpCompCache {
    key: chrono::DateTime<chrono::Local>,
    inner_w: f32,
    segs: Vec<(usize, usize, [f32; 4])>,
    badges: Vec<McpBadge>,
    shown: usize,
    plus_w: f32,
}

/// 主视图轮询稳定文本的帧缓存：跑马灯 30fps 全窗重绘期间不重建
/// 顶栏副标题、行明细、百分比与余额串；倒计时行随秒变化，
/// 按整秒独立缓存。标题缓存与快照无关，放 Renderer 侧。
pub(super) struct MainTexts {
    /// (快照时间, 语言)：任一变化整缓存重建
    key: (chrono::DateTime<chrono::Local>, crate::ui::i18n::Lang),
    /// 顶栏副标题（版本 · 等级），缺则 None
    meta: Option<String>,
    /// 指标行文本，按绘制序（在场桶序）
    rows: Vec<RowTexts>,
    /// 余额行整串
    balance: Option<String>,
    /// 倒计时行的缓存秒：秒推进才重建各行 reset 分量
    resets_at: i64,
}

/// 指标行文本：明细与百分比轮询稳定，倒计时随秒推进
struct RowTexts {
    detail: Option<String>,
    percent: String,
    reset: Option<String>,
}

/// Token 卡片按快照时间缓存的排版产物：文本与测宽随轮询变，
/// 动画帧不重算
pub(super) struct TokenCells {
    key: chrono::DateTime<chrono::Local>,
    cells: [TokenCell; TOKEN_LEGS.len()],
}

/// 单卡排版产物：数字串、单位与各自测宽
struct TokenCell {
    num: String,
    unit: &'static str,
    nw: f32,
    uw: f32,
}

impl Renderer {
    /// 重建主文本缓存：轮询稳定部分（副标题、行明细、百分比、余额）
    /// 全量产，倒计时行取当秒初值。
    fn build_main_texts(
        s: &Strings,
        snap: &crate::api::UsageSnapshot,
        lang: crate::ui::i18n::Lang,
        now_sec: i64,
    ) -> MainTexts {
        let v = snap.plan_version.label();
        let tier = snap.tier_label();
        let meta = match (v.is_empty(), tier.is_empty()) {
            (false, false) => Some(format!("{v} · {tier}")),
            (false, true) => Some(v.to_string()),
            (true, false) => Some(tier.to_string()),
            (true, true) => None,
        };
        let used_of = |cur: f64, tot: f64| {
            s.used_of
                .replace("{cur}", &fmt::compact_number(cur))
                .replace("{tot}", &fmt::compact_number(tot))
        };
        let mut rows = Vec::new();
        for b in snap.present_buckets() {
            let (detail, percent) = match &b {
                crate::api::PresentBucket::FiveHour(b) | crate::api::PresentBucket::Weekly(b) => {
                    let detail = match (b.current, b.total) {
                        (Some(c), Some(t)) if t > 0.0 => Some(used_of(c, t)),
                        _ => None,
                    };
                    (detail, fmt::percent(b.used_percent))
                }
                crate::api::PresentBucket::Mcp(m) => (
                    (m.total > 0.0).then(|| used_of(m.current_value, m.total)),
                    fmt::percent(m.used_percent),
                ),
            };
            rows.push(RowTexts {
                detail,
                percent,
                reset: None,
            });
        }
        let balance = snap.balance.as_ref().map(|b| {
            let amount = if b.available.abs() >= 1e6 {
                fmt::compact_number(b.available)
            } else {
                format!("{:.2}", b.available)
            };
            format!("¥{amount}")
        });
        let mut texts = MainTexts {
            key: (snap.queried_at, lang),
            meta,
            rows,
            balance,
            resets_at: now_sec,
        };
        Self::refresh_resets(&mut texts.rows, s, snap, lang);
        texts
    }

    /// 倒计时行重建：秒推进触发，只写各行 reset。行序与行集同引
    /// present_buckets，构建与重建天然对齐。
    fn refresh_resets(
        rows: &mut [RowTexts],
        s: &Strings,
        snap: &crate::api::UsageSnapshot,
        lang: crate::ui::i18n::Lang,
    ) {
        let line = |r: Option<chrono::DateTime<chrono::Utc>>| {
            r.map(|r| s.resets_line.replace("{t}", &fmt::countdown(r, lang)))
        };
        for (row, b) in rows.iter_mut().zip(snap.present_buckets()) {
            row.reset = line(b.resets_at());
        }
    }

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
        // 主文本帧缓存（单一出口，函数尾放回）：跑马灯 30fps 全窗
        // 重绘期间轮询稳定串零重建；倒计时行按整秒独立重建。
        let mut texts = self.main_texts.take();
        if let Some(snap) = snap {
            let now_sec = chrono::Utc::now().timestamp();
            if texts
                .as_ref()
                .is_none_or(|t| t.key != (snap.queried_at, model.lang))
            {
                texts = Some(Self::build_main_texts(s, snap, model.lang, now_sec));
            } else if let Some(t) = texts.as_mut()
                && t.resets_at != now_sec
            {
                t.resets_at = now_sec;
                Self::refresh_resets(&mut t.rows, s, snap, model.lang);
            }
        } else {
            // 无快照即换账号或退场：缓存连同旧账号的副标题一并作废，
            // 加载态与错误卡不得残留上家内容。
            texts = None;
        }
        // ── 顶栏：账号名 + 套餐副标题 ──
        let title = model.account.map(|a| a.name).unwrap_or("Quotify");
        let (logo_size, logo_y) = (38.0, y + 7.0);
        self.logo(target, pad, logo_y, logo_size, alpha);
        let tx = pad + logo_size + 10.0;
        let chevron_w = if model.accounts_count > 1 { 18.0 } else { 0.0 };
        let tw = w - tx - 88.0 - chevron_w;
        // 标题截断缓存与快照无关：键为 (源串, 可用宽)，跑马灯重绘期
        // 零文本构建；单一出口，函数尾放回。
        let mut title_cache = self.title_cache.take();
        if title_cache
            .as_ref()
            .is_none_or(|(k, _)| k.0 != title || k.1 != tw)
        {
            let (d, dw) = self.ellipsize(title, 16.0, tw, 500, false);
            title_cache = Some(((title.to_string(), tw), (d, dw)));
        }
        let (title_disp, title_disp_w) = title_cache
            .as_ref()
            .map(|(_, (d, w))| (d.as_str(), *w))
            .unwrap();
        let meta = texts.as_ref().and_then(|t| t.meta.as_deref());
        let block_h = if meta.is_some() { 39.0 } else { 22.0 };
        let block_top = y + 26.0 - block_h / 2.0;
        self.text(
            target,
            title_disp,
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
            // chevron 紧跟标题尾：宽度直接用 ellipsize 的返回值，不再复测。
            let ax = tx + title_disp_w + 6.0;
            self.chevron(
                target,
                ax,
                y + 26.0,
                self.theme.text_secondary,
                alpha,
                model.popup_open,
            );
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
        if let Some(m) = meta {
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
        self.icon_button(target, Hit::Refresh, refresh_cx, btn_cy, self.anim.spin);
        self.sliders(target, Hit::Settings, settings_cx, btn_cy, btn_r);
        y += layout::MAIN_TOPBAR_H;

        // ── 主体三态：空/加载、错误卡、数据区 ──
        match (snap, model.error) {
            // 无快照：已配置=加载中，未配置=引导空态。
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
            // 无数据有错误：错误卡两行制——标题加大类、正文放码与提示。
            (None, Some(e)) => {
                let title = category_label(s, e.category());
                let body = error_card_body(s, e);
                let body_top = y + 10.0;
                let body_h = (dy + h - 16.0) - body_top;
                let has_body = !body.is_empty();
                let card_h = if has_body { 94.0 } else { 68.0 };
                let card_top = body_top + (body_h - card_h) / 2.0;
                let card = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: pad,
                        top: card_top,
                        right: w - pad,
                        bottom: card_top + card_h,
                    },
                    radiusX: CARD_RADIUS,
                    radiusY: CARD_RADIUS,
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
                self.text(
                    target,
                    title,
                    pad + 12.0,
                    card_top + 14.0,
                    w - pad * 2.0 - 24.0,
                    22.0,
                    13.0,
                    700,
                    self.theme.text_primary,
                    alpha,
                );
                if has_body {
                    let body_rect = D2D_RECT_F {
                        left: pad + 12.0,
                        top: card_top + 38.0,
                        right: w - pad - 12.0,
                        bottom: card_top + 78.0,
                    };
                    // 自动换行至多两行：单行文案照常一行，长文第二行
                    // 末按矩形截断。
                    self.text_wrapped(
                        target,
                        &body,
                        &body_rect,
                        12.0,
                        400,
                        self.theme.text_secondary,
                        alpha,
                        Align::Left,
                        false,
                    );
                }
            }
            // 有数据即以数据区为主体；同时带错误时旧数据照常展示，错误降级页脚。
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

                    // 行文本按绘制序取缓存：行序与行集同引 present_buckets，
                    // 索引越界即暴露两侧行集漂移。
                    for (ri, b) in snap.present_buckets().enumerate() {
                        let row = &texts.as_ref().unwrap().rows[ri];
                        match b {
                            crate::api::PresentBucket::FiveHour(b) => {
                                y = self.metric_row(
                                    target,
                                    s.five_hour,
                                    b.used_percent,
                                    row.detail.as_deref(),
                                    &row.percent,
                                    row.reset.as_deref(),
                                    y,
                                    w,
                                    alpha,
                                );
                            }
                            crate::api::PresentBucket::Weekly(b) => {
                                y = self.metric_row(
                                    target,
                                    s.weekly,
                                    b.used_percent,
                                    row.detail.as_deref(),
                                    &row.percent,
                                    row.reset.as_deref(),
                                    y,
                                    w,
                                    alpha,
                                );
                            }
                            crate::api::PresentBucket::Mcp(m) => {
                                y = self.metric_row(
                                    target,
                                    s.mcp_tools,
                                    m.used_percent,
                                    row.detail.as_deref(),
                                    &row.percent,
                                    row.reset.as_deref(),
                                    y,
                                    w,
                                    alpha,
                                );
                                // 构成区陈列判定单源于 has_mcp_details，与高度侧同引。
                                if snap.has_mcp_details() {
                                    y = self.mcp_composition(
                                        target,
                                        m,
                                        snap.queried_at,
                                        y,
                                        w,
                                        alpha,
                                    );
                                }
                            }
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
                        y = self.token_section(target, s, ts, snap.queried_at, ty, alpha);
                    }

                    if let (Some(_), Some(t)) = (&snap.balance, texts.as_ref()) {
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
                        if let Some(line) = t.balance.as_deref() {
                            self.text_mono_r(
                                target,
                                line,
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
                }
                // 页脚钉底；视口被压矮时装不下内容，钉内容底免叠数据区。
                let footer_y = dy + h.max(content_h) - layout::MAIN_FOOTER_H;
                // 错误摘要与旧快照成对在场：前缀段报数据年龄，无快照
                // 不出摘要。
                if let (Some(e), Some(snap)) = (model.error, snap) {
                    let avail = w - pad * 2.0;
                    let anim_on = self.animations_on();
                    // 缓存键是内容指纹：错误负载、快照时间、可视宽、语言
                    // 与动效开关全入哈希，任一变化即重建；命中时零测量零
                    // 分配，指纹直接哈希字段，无 Display 中间串。
                    let key = {
                        use std::hash::{Hash, Hasher};
                        let mut h = std::hash::DefaultHasher::new();
                        e.hash(&mut h);
                        snap.queried_at.hash(&mut h);
                        avail.to_bits().hash(&mut h);
                        model.lang.hash(&mut h);
                        anim_on.hash(&mut h);
                        h.finish()
                    };
                    let cached = self
                        .anim
                        .error_scroll
                        .as_ref()
                        .is_some_and(|sc| sc.key == key);
                    if !cached {
                        let head = s
                            .data_as_of
                            .replace("{t}", &snap.queried_at.format("%H:%M").to_string());
                        let tail = footer_error_tail(s, e);
                        let cat = category_label(s, e.category()).to_string();
                        // 布局预建：滚动帧直绘复用，不逐帧重建 COM 布局；
                        // 宽度从布局读，字号字重与直绘同源只写一处。
                        // 块界中括号加粗、内容常规：三块共享一对括号布局。
                        let size = ERROR_SCROLL_SIZE;
                        let bw = ERROR_SCROLL_BRACKET_WEIGHT;
                        let bracket = ScrollBlock::new(self.make_layout("[", size, bw, false));
                        let bracket_end = ScrollBlock::new(self.make_layout("] ", size, bw, false));
                        let mut blocks = [
                            ScrollBlock::new(self.make_layout(&head, size, 400, false)),
                            ScrollBlock::new(self.make_layout(&cat, size, 400, false)),
                            ScrollBlock::new(self.make_layout(&tail, size, 400, false)),
                        ];
                        let pair = bracket.w + bracket_end.w;
                        let over = belt_width(&blocks.each_ref().map(|b| b.w), pair) > avail;
                        let scrolling = over && anim_on;
                        if over && !anim_on {
                            // 减动效超宽不滚：逐块截断成静态串，前缀与
                            // 大类全串，尾段截断加省略号；同走布局路径，
                            // 宽沿用截断计算的值。
                            let rest0 = (avail - 3.0 * pair).max(0.0);
                            let (hd, hw) = self.ellipsize(&head, size, rest0, 400, false);
                            let rest = (rest0 - hw).max(0.0);
                            let (cd, cw) = self.ellipsize(&cat, size, rest, 400, false);
                            let rest = (rest - cw).max(0.0);
                            let (td, tw) = self.ellipsize(&tail, size, rest, 400, false);
                            let trunc = |s: &str, w: f32| ScrollBlock {
                                layout: self.make_layout(s, size, 400, false),
                                w,
                            };
                            blocks = [trunc(&hd, hw), trunc(&cd, cw), trunc(&td, tw)];
                        }
                        self.anim.error_scroll = Some(ErrorScroll {
                            key,
                            blocks,
                            bracket,
                            bracket_end,
                            scrolling,
                            frozen: false,
                            offset: 0.0,
                            resume_at: std::time::Instant::now() + ERROR_SCROLL_PAUSE,
                            // last 对齐恢复点：首个暂停不计入位移。
                            last: std::time::Instant::now() + ERROR_SCROLL_PAUSE,
                        });
                        // 挂钟由 WM_PAINT 收尾统一处理：渲染帧外层持有
                        // App 引用，重取引用会使其失效。
                    }
                    // 摘要可视区：左起 pad、占满全宽。
                    let clip = D2D_RECT_F {
                        left: pad,
                        top: footer_y,
                        right: pad + avail,
                        bottom: footer_y + 34.0,
                    };
                    target.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                    // take 出跑马灯状态：绘制全程直接引用缓存布局与截断
                    // 产物，帧内零克隆零分配；单一出口，函数尾放回。
                    let mut sc_taken = self.anim.error_scroll.take();
                    if let Some(sc) = sc_taken.as_mut() {
                        // 直绘路（滚动、静态与截断共线）：同色同透明度一把
                        // 刷子全程共用。
                        let brush = self.brush(target, self.theme.text_secondary, alpha);
                        let mut draw_pair = |x: f32| {
                            let mut bx = x;
                            for b in sc.blocks.iter() {
                                // 空块跳过：推进与绘制都不参与，周期无尾随空白；
                                // 截断预算耗尽的块宽为零，同此跳过。
                                if b.w <= 0.0 {
                                    continue;
                                }
                                if let Some(l) = sc.bracket.layout.as_ref() {
                                    self.draw_layout(target, l, bx, footer_y + 6.0, &brush);
                                }
                                if let Some(l) = b.layout.as_ref() {
                                    self.draw_layout(
                                        target,
                                        l,
                                        bx + sc.bracket.w,
                                        footer_y + 6.0,
                                        &brush,
                                    );
                                }
                                let end_x = bx + sc.bracket.w + b.w;
                                if let Some(l) = sc.bracket_end.layout.as_ref() {
                                    self.draw_layout(target, l, end_x, footer_y + 6.0, &brush);
                                }
                                bx = end_x + sc.bracket_end.w;
                            }
                        };
                        if sc.scrolling {
                            // 传送带平铺：两份文本以周期相接，左出右进无缝循环。
                            let period = sc.total_w();
                            let mut x = pad - sc.offset;
                            while x < pad + avail {
                                draw_pair(x);
                                x += period;
                            }
                        } else {
                            // 不超宽或截断后落定：宽度已缓存，直接绘制。
                            draw_pair(pad);
                        }
                    }
                    self.anim.error_scroll = sc_taken;
                    target.PopAxisAlignedClip();
                } else if let Some(fa) = self.anim.footer.as_ref().filter(|f| !f.tween.finished()) {
                    // 吃豆人换装
                    let (old_text, new_text, tween) =
                        (fa.old_text.clone(), fa.new_text.clone(), fa.tween);
                    let (old_w, new_w) = (fa.old_w, fa.new_w);
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
                        // alpha 量化 32 档：档位跨轮复用，刷子缓存
                        // 不随动画膨胀。
                        let a = (alpha * (1.0 - t * t) * 32.0).round() / 32.0;
                        let cb = self.brush(target, self.theme.text_tertiary, a);
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
                    let text = fmt::updated_text(s, model.lang, snap.queried_at);
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
        // 主文本与标题缓存无条件放回：单一出口，快照缺席臂原样归还。
        self.main_texts = texts;
        self.title_cache = title_cache;
    }

    /// 数据段区块头：分隔线 + 强调条 + 标题；返回标题行顶 y，段内后续
    /// 推进（usage 叠高峰徽标后走刊头行高、Token 接卡片、余额即文本
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
        // 两种段的标题距段起点同为 12：刊头是段起点起算，数据段是分隔线起算。
        let ty = y + if dashed {
            layout::MAIN_SECTION_HEAD
        } else {
            layout::MAIN_MASTHEAD_RULE_GAP
        };
        let bar = self.brush(target, self.theme.text_primary, alpha * 0.9);
        // 柱子与文字同框垂直居中
        target.FillRectangle(
            &D2D_RECT_F {
                left: pad,
                top: ty + 2.0,
                right: pad + 3.0,
                bottom: ty + 15.0,
            },
            &bar,
        );
        self.text_aligned_vc(
            target,
            label,
            &D2D_RECT_F {
                left: pad + 7.0,
                top: ty,
                right: pad + 7.0 + title_w,
                bottom: ty + 17.0,
            },
            12.0,
            600,
            self.theme.text_tertiary,
            alpha,
            Align::Left,
            false,
        );
        ty
    }

    /// 指标行：标签 + 百分比 + 进度条，底行左倒计时右已用明细；文本
    /// 串由 MainTexts 帧缓存预产，此处只绘制。返回下一行 y
    #[allow(clippy::too_many_arguments)]
    unsafe fn metric_row(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &str,
        used_percent: f64,
        detail: Option<&str>,
        pct: &str,
        resets: Option<&str>,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 {
        let pad = layout::CONTENT_PAD;
        // ≥90% 数值与进度条入危险色，阈值与托盘图标红档一致。
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
        self.text_mono_r(
            target,
            pct,
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
        // 占比不足 0.4% 不画填充；画出时保底 2px 宽。
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

        // 底行左右分栏：左 55% 倒计时，右侧对齐已用明细。
        if let Some(line) = resets {
            self.text(
                target,
                line,
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
            self.text_aligned(
                target,
                d,
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

    /// MCP 工具构成区：直角外框内一条圆角格能量条[满串 = 工具
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
        // 单一出口，函数尾放回。
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
        // 待填段：区间序列与缓存建时同源（mcp_job_ranges），数据段取
        // 段色、空格段取轨道色，逐格填圆角矩形。
        let track = self.theme.track;
        for r in mcp_job_ranges(segs) {
            let color = segs
                .iter()
                .find(|(s, e, _)| (*s, *e) == r)
                .map_or(track, |(_, _, c)| *c);
            let b = self.brush(target, color, alpha);
            for i in r.0..r.1 {
                let left = x0 + i as f32 * (MCP_CELL_W + MCP_CELL_GAP);
                let cell = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left,
                        top: bar_y,
                        right: left + MCP_CELL_W,
                        bottom: bar_y + cell_h,
                    },
                    radiusX: MCP_CELL_R,
                    radiusY: MCP_CELL_R,
                };
                target.FillRoundedRectangle(&cell, &b);
            }
        }

        let leg_y = bar_y + cell_h + layout::MAIN_MCP_LEGEND_ADV;
        let leg_h = layout::MAIN_MCP_LEGEND_H;
        let sw_top = leg_y + 4.0;
        let swatch = |x: f32| D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: sw_top,
                right: x + MCP_SWATCH,
                bottom: sw_top + MCP_SWATCH,
            },
            radiusX: MCP_CELL_R,
            radiusY: MCP_CELL_R,
        };
        let mut x = inner_x;
        if shown == 0 && !badges.is_empty() {
            // 首枚即超宽：截文本兜底，最大工具保底可见。
            let b = &badges[0];
            let sw = self.brush(target, b.color, alpha);
            target.FillRoundedRectangle(&swatch(x), &sw);
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
                target.FillRoundedRectangle(&swatch(x), &sw);
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
        // 工具降序；段界累计取整，段间无缝不重叠，末段吃满整串。
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
        // 容得下徽标间空隙 + 「+N」才继续装，尾部概括数即剩余工具数。
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

    /// 重建 Token 卡片缓存：格式化、测宽与超宽截断一次成型
    unsafe fn build_token_cells(
        &mut self,
        ts: &crate::api::TokenStats,
        queried_at: chrono::DateTime<chrono::Local>,
    ) -> TokenCells {
        let avail = layout::TOKEN_CARD_W - TOKEN_CARD_INSET;
        let cells = ts.legs.map(|v| {
            let (num, unit) = match v {
                Some(v) => fmt::compact_number_split(v),
                None => (fmt::MISSING.to_string(), ""),
            };
            // Consolas 只有 400/700 两档字重，700 明示粗体。
            let nw = self.measure(&num, TOKEN_NUM_SIZE, TOKEN_NUM_WEIGHT, true);
            let uw = if unit.is_empty() {
                0.0
            } else {
                self.measure_static(unit, TOKEN_UNIT_SIZE, 400, false)
            };
            // 荒谬大值可超卡宽：截尾入宽，不越描边压邻卡。
            let (num, nw) = if nw + uw + TOKEN_UNIT_GAP > avail {
                self.ellipsize(
                    &num,
                    TOKEN_NUM_SIZE,
                    avail - uw - TOKEN_UNIT_GAP,
                    TOKEN_NUM_WEIGHT,
                    true,
                )
            } else {
                (num, nw)
            };
            TokenCell { num, unit, nw, uw }
        });
        TokenCells {
            key: queried_at,
            cells,
        }
    }

    /// Token 三卡区块：排版缓存按快照命中，重建一次成型；单一出口，
    /// take 出的缓存函数尾必放回。返回区块底 y
    unsafe fn token_section(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        s: &Strings,
        ts: &crate::api::TokenStats,
        queried_at: chrono::DateTime<chrono::Local>,
        y: f32,
        alpha: f32,
    ) -> f32 {
        let pad = layout::CONTENT_PAD;
        let cy = y + layout::MAIN_TOKEN_CARD_ADV;
        let mut cells = self.token_cells.take();
        if cells.as_ref().is_none_or(|c| c.key != queried_at) {
            cells = Some(self.build_token_cells(ts, queried_at));
        }
        let cells = cells.unwrap();
        // 标签按档位穷尽分派：配对内聚，增删档编译器背书。
        for (i, (leg, _, _)) in TOKEN_LEGS.iter().enumerate() {
            let label = match leg {
                TokenLeg::Today => s.today_tokens,
                TokenLeg::Week => s.week_tokens,
                TokenLeg::Month => s.month_tokens,
            };
            self.token_card(
                target,
                label,
                &cells.cells[i],
                pad + i as f32 * (layout::TOKEN_CARD_W + layout::TOKEN_CARD_GAP),
                cy,
                alpha,
            );
        }
        self.token_cells = Some(cells);
        cy + layout::TOKEN_CARD_H
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

    /// Token 卡片：透明底细描边容器；数字与单位分离排版，标签沉底。
    unsafe fn token_card(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &str,
        cell: &TokenCell,
        x: f32,
        y: f32,
        alpha: f32,
    ) {
        let cw = layout::TOKEN_CARD_W;
        let rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + cw,
                bottom: y + layout::TOKEN_CARD_H,
            },
            radiusX: CARD_RADIUS,
            radiusY: CARD_RADIUS,
        };
        let edge = self.brush(target, self.theme.card_border, alpha);
        target.DrawRoundedRectangle(&rect, &edge, 1.0, None);
        // 数字与单位组合整体居中；单位上标挂数字右上角。
        let nx = x + (cw - cell.nw - cell.uw - TOKEN_UNIT_GAP) / 2.0;
        let num_rect = D2D_RECT_F {
            left: nx,
            top: y + layout::TOKEN_CARD_NUM_Y,
            right: nx + cell.nw,
            bottom: y + layout::TOKEN_CARD_NUM_Y + layout::TOKEN_CARD_NUM_H,
        };
        self.text_aligned(
            target,
            &cell.num,
            &num_rect,
            TOKEN_NUM_SIZE,
            TOKEN_NUM_WEIGHT,
            self.theme.text_primary,
            alpha,
            Align::Left,
            true,
        );
        if !cell.unit.is_empty() {
            self.text(
                target,
                cell.unit,
                nx + cell.nw + TOKEN_UNIT_GAP,
                y + layout::TOKEN_CARD_NUM_Y,
                cell.uw + 2.0,
                16.0,
                TOKEN_UNIT_SIZE,
                400,
                self.theme.text_secondary,
                alpha,
            );
        }
        let label_rect = D2D_RECT_F {
            left: x,
            top: y + layout::TOKEN_CARD_LABEL_Y,
            right: x + cw,
            bottom: y + layout::TOKEN_CARD_LABEL_Y + layout::TOKEN_CARD_LABEL_H,
        };
        self.text_aligned(
            target,
            label,
            &label_rect,
            10.0,
            400,
            self.theme.text_secondary,
            alpha,
            Align::Center,
            false,
        );
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
        // 徽标文字是 i18n 常量文本，走帧内去重测宽。
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
            radiusX: CARD_RADIUS,
            radiusY: CARD_RADIUS,
        };
        let [r, g, b, _] = self.theme.bg;
        let fill = self.brush(target, [r, g, b, 1.0], 1.0);
        target.FillRoundedRectangle(&card, &fill);
        let line = self.brush(target, self.theme.card_border, alpha);
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

/// 大类标题文案
fn category_label(s: &Strings, c: ErrorCategory) -> &'static str {
    match c {
        ErrorCategory::Unauthenticated => s.err_cat_auth,
        ErrorCategory::Request => s.err_cat_bad_request,
        ErrorCategory::Throttled => s.err_cat_rate_limited,
        ErrorCategory::Server => s.err_cat_server,
        ErrorCategory::Network => s.err_cat_network,
        ErrorCategory::Empty => s.err_cat_no_data,
    }
}

/// FetchError 的展示取材：官方错误信息与该类无官方提示时的兜底
/// 文案；信息为 None 的类走裸文案直出，无降级链。错误卡与页脚
/// 尾段同引，两视图的回退配对不分叉。
fn error_parts<'a>(e: &'a FetchError, s: &'a Strings) -> (Option<&'a ErrorInfo>, &'a str) {
    match e {
        FetchError::Unauthenticated(info) => (Some(info), s.err_auth_hint),
        FetchError::Empty => (None, ""),
        FetchError::Failure(info) => (Some(info), ""),
        FetchError::Network(d) => (None, d),
        FetchError::Throttled { info, .. } => (Some(info), ""),
    }
}

/// 错误卡正文降级链
/// 取材判定单源 `ErrorInfo::detail`，此处只做卡片模板拼装。
fn error_card_body(s: &Strings, e: &FetchError) -> String {
    // 空态是引导正文，不走降级链。
    if matches!(e, FetchError::Empty) {
        return s.err_empty_hint.to_string();
    }
    match error_parts(e, s) {
        (Some(info), fallback) => match info.detail() {
            ErrorDetail::CodeAndMsg { code, msg } => s
                .err_code_msg
                .replace("{code}", &code.to_string())
                .replace("{msg}", msg),
            ErrorDetail::CodeOnly { code } => s.err_code_only.replace("{code}", &code.to_string()),
            ErrorDetail::Msg(msg) => msg.into_owned(),
            ErrorDetail::Fallback => fallback.to_string(),
        },
        (None, raw) => raw.to_string(),
    }
}

/// 页脚摘要的后缀段
/// 取材判定与错误卡同源 `ErrorInfo::detail`，空串表示无尾段。
fn footer_error_tail(s: &Strings, e: &FetchError) -> String {
    match error_parts(e, s) {
        (Some(info), fallback) => match info.detail() {
            ErrorDetail::CodeAndMsg { msg: m, .. } => m.to_string(),
            ErrorDetail::Msg(m) => m.into_owned(),
            ErrorDetail::CodeOnly { code } => s.err_code_only.replace("{code}", &code.to_string()),
            ErrorDetail::Fallback if fallback.is_empty() => String::new(),
            ErrorDetail::Fallback => fallback.to_string(),
        },
        (None, raw) => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    /// 档位与回退天数、日志标签配对钉位
    #[test]
    fn token_legs_pinned() {
        assert_eq!(TOKEN_LEGS[0], (TokenLeg::Today, 0, "today"));
        assert_eq!(TOKEN_LEGS[1], (TokenLeg::Week, 6, "week"));
        assert_eq!(TOKEN_LEGS[2], (TokenLeg::Month, 29, "month"));
    }

    /// 能量条待填区间：数据段原样保留，未覆盖格位补轨道段，
    /// 首尾按需补齐，段序即绘制序
    #[test]
    fn mcp_ranges_cover_track_gaps() {
        // 无数据：整条都是轨道
        assert_eq!(mcp_job_ranges(&[]), vec![(0, MCP_CELLS)]);
        // 单段吃满：无轨道
        assert_eq!(mcp_job_ranges(&[(0, 20, C)]), vec![(0, 20)]);
        // 头部缺口
        assert_eq!(mcp_job_ranges(&[(5, 20, C)]), vec![(0, 5), (5, 20)]);
        // 中段缺口，数据段连续衔接
        assert_eq!(
            mcp_job_ranges(&[(2, 6, C), (6, 10, C), (14, 20, C)]),
            vec![(0, 2), (2, 6), (6, 10), (10, 14), (14, 20)]
        );
    }
}
