//! 弹出面板

pub mod anim;
pub mod layout;
pub mod model;
pub mod render;
pub mod text_edit;
pub mod theme;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HBRUSH, HMONITOR, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromWindow, ValidateRect,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForMonitor;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DELETE, VK_END,
    VK_HOME, VK_LEFT, VK_RIGHT, VK_Y, VK_Z,
};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use crate::platform::wide;
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::render::Renderer;
use crate::ui::panel::text_edit::EditState;
use crate::ui::panel::theme::PANEL_WIDTH;
use crate::ui::{x_of, y_of};

const PANEL_WND_CLASS: &str = "QuotifyPanelWnd";

/// 弹出/收起动画帧时钟
const TIMER_ANIM: usize = 1;
/// 预览失焦收回的去抖时钟
const TIMER_CLOSE_DEBOUNCE: usize = 2;
/// 预览/锁定态的光标离面巡检时钟
pub(crate) const TIMER_OUTSIDE_CHECK: usize = 3;
/// 分钟级重绘心跳
const TIMER_MINUTE_TICK: usize = 4;

/// DPI 探测失败时的兜底值
pub(crate) const FALLBACK_DPI: f32 = 1.5;

/// 单字段输入缓冲的字节上限
const INPUT_MAX_BYTES: usize = 128;

/// 面板的展示模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMode {
    Preview,
    Pinned,
    Hidden,
}

/// 面板当前展示的视图
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelView {
    Main,
    Settings,
}

/// 自绘输入的目标字段；设置页字段在前、添加页表单在后，各按所在分区顺序
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputField {
    /// 设置页：自定义轮询间隔
    Interval,
    /// 设置页：网络代理地址
    Proxy,
    /// 设置页：高峰区间开始
    PeakStart,
    /// 设置页：高峰区间结束
    PeakEnd,
    /// 添加页：账号名称
    Name,
    /// 添加页：API key
    Key,
    /// 添加页（团队版）：组织 ID
    Org,
    /// 添加页（团队版）：项目 ID
    Project,
}

/// 自绘输入缓冲；字段序同 InputField
#[derive(Default)]
pub struct PanelInput {
    pub field: Option<InputField>,
    pub edit: crate::ui::panel::text_edit::EditState,
    pub surrogate: Option<u16>,
    pub interval: String,
    pub proxy: String,
    pub peak_start: String,
    pub peak_end: String,
    pub name: String,
    pub key: String,
    pub org: String,
    pub project: String,
}

impl PanelInput {
    /// 激活字段缓冲的可变引用；无激活字段时兜底 interval
    pub(crate) fn active_buf(&mut self) -> &mut String {
        match self.field {
            Some(InputField::Interval) => &mut self.interval,
            Some(InputField::Proxy) => &mut self.proxy,
            Some(InputField::PeakStart) => &mut self.peak_start,
            Some(InputField::PeakEnd) => &mut self.peak_end,
            Some(InputField::Name) => &mut self.name,
            Some(InputField::Key) => &mut self.key,
            Some(InputField::Org) => &mut self.org,
            Some(InputField::Project) => &mut self.project,
            None => &mut self.interval,
        }
    }

    /// 激活字段缓冲的只读引用；无激活字段时兜底 interval
    pub(crate) fn active_str(&self) -> &str {
        match self.field {
            Some(InputField::Interval) => &self.interval,
            Some(InputField::Proxy) => &self.proxy,
            Some(InputField::PeakStart) => &self.peak_start,
            Some(InputField::PeakEnd) => &self.peak_end,
            Some(InputField::Name) => &self.name,
            Some(InputField::Key) => &self.key,
            Some(InputField::Org) => &self.org,
            Some(InputField::Project) => &self.project,
            None => &self.interval,
        }
    }
}

/// 输入框槽位命中 → 对应编辑字段；眼睛（RevealKey）与「自定义」按钮
/// 不属文本区，不映射
fn input_field_of_hit(hit: crate::ui::panel::render::Hit) -> Option<InputField> {
    use crate::ui::panel::render::Hit;
    match hit {
        Hit::InputInterval => Some(InputField::Interval),
        Hit::InputProxy => Some(InputField::Proxy),
        Hit::InputPeakStart => Some(InputField::PeakStart),
        Hit::InputPeakEnd => Some(InputField::PeakEnd),
        Hit::InputName => Some(InputField::Name),
        Hit::InputKey => Some(InputField::Key),
        Hit::InputOrg => Some(InputField::Org),
        Hit::InputProject => Some(InputField::Project),
        _ => None,
    }
}

/// 激活字段的输入框几何（x、宽、右端让位）；与 settings.rs 各 input_field
/// 调用点同源，眼睛让位 26、余量收边 4
pub(crate) fn field_geo(field: InputField) -> (f32, f32, f32) {
    let cw = PANEL_WIDTH as f32 - 2.0 * layout::CONTENT_PAD;
    match field {
        InputField::Interval => (layout::INPUT_X, 96.0, 4.0),
        InputField::PeakStart => (layout::PEAK_START_X, 64.0, 4.0),
        InputField::PeakEnd => (layout::PEAK_END_X, 64.0, 4.0),
        InputField::Key => (layout::INPUT_X, cw, 26.0),
        _ => (layout::INPUT_X, cw, 4.0),
    }
}

pub struct Panel {
    pub hwnd: Option<HWND>,
    pub mode: PanelMode,
    pub view: PanelView,
    pub(crate) scroll_dy: f32,
    pub(crate) scroll_max: f32,
    pub hovered: bool,
    pub(crate) anchor: Option<RECT>,
    pub renderer: Option<Renderer>,
    pub adding_account: bool,
    pub pending_platform: crate::api::Platform,
    pub pending_team: bool,
    pub input: PanelInput,
    pub(crate) key_revealed: bool,
    pub customizing_interval: bool,
    pub(crate) anim_x: i32,
    pub(crate) anim_w: i32,
    pub(crate) anim_full_h: i32,
    pub(crate) anim_bottom: i32,
    pub(crate) caret_ctx: (bool, bool),
    pub(crate) selecting: bool,
    pub(crate) text_clicks: Option<(u32, i32, i32, u8)>,
    vis_start: std::cell::Cell<usize>,
    ime_owned: bool,
    pub(crate) dragged: bool,
    pub(crate) press_at: Option<(i32, i32)>,
    pub(crate) drag_offset: Option<(i32, i32)>,
    class_registered: bool,
    pub(crate) main_h: i32,
    pub(crate) account_error: bool,
    pub update_available: bool,
    pub(crate) outside_since: Option<u64>,
    dpi: f32,
}

impl Panel {
    pub fn new() -> Self {
        Self {
            hwnd: None,
            mode: PanelMode::Hidden,
            view: PanelView::Main,
            scroll_dy: 0.0,
            scroll_max: 0.0,
            hovered: false,
            anchor: None,
            renderer: None,
            adding_account: false,
            pending_platform: crate::api::Platform::Cn,
            pending_team: false,
            input: PanelInput::default(),
            key_revealed: false,
            customizing_interval: false,
            anim_x: 0,
            anim_w: 0,
            anim_full_h: 0,
            anim_bottom: 0,
            main_h: 300,
            account_error: false,
            update_available: false,
            caret_ctx: (false, false),
            selecting: false,
            text_clicks: None,
            vis_start: std::cell::Cell::new(0),
            ime_owned: false,
            dragged: false,
            press_at: None,
            drag_offset: None,
            class_registered: false,
            outside_since: None,
            dpi: FALLBACK_DPI,
        }
    }

    /// 逻辑像素 → 物理像素
    pub(crate) fn px(&self, logical: i32) -> i32 {
        (logical as f32 * self.dpi).round() as i32
    }

    /// 面板当前视图的逻辑高度
    pub(crate) fn view_height(&self, accounts: usize) -> i32 {
        match self.view {
            // 动态：随指标行数 / 余额 / 副标题伸缩，由 sync_main_height 维护
            PanelView::Main => self.main_h,
            PanelView::Settings if self.adding_account => {
                layout::add_page_height(self.pending_team)
            }
            // 逐段对照 draw_settings 的 y 累加链（dy=0）；间隔行展开 +38（输入框 26 + 尾隙 12）：
            PanelView::Settings => {
                // 有账号：卡片 48 + 常驻添加按钮行 36；无账号：仅添加按钮行 36
                let account_block = if accounts > 0 { 84 } else { 36 };
                // 鉴权失败提示行
                let error_line = if self.account_error { 18 } else { 0 };
                let base = 42 // 顶部留白 12 + 导航行 30，返回箭头 + 居中标题
                    + 21 // 账号区标题，实绘不带分隔线
                    + account_block
                    + error_line
                    + 33 // 轮询区标题：分隔线上隙 12 + 标题 21
                    + 39 // 间隔分段控件：段体 30 + 段后间距 9
                    + 33 // 通知区标题：分隔线上隙 12 + 标题 21
                    + 126 // 三个通知开关行：标题 19 + 描述 14 + 行后 9 = 42 × 3
                    + 67 // 高峰区间区：标题 33 + 输入行 26 + 下隙 8
                    + 33 // 通用区标题
                    + 62 // 语言行：sub_label 21 + segmented 39 + 行后 2
                    + 62 // 外观行，同语言行
                    + 28 // 开机自启开关行：标题 19 + 行后 9，无描述行
                    + 33 // 网络代理区标题：同轮询区标题
                    + 21 // 代理子标签
                    + 26 // 代理输入框
                    + 6 // 输入框后下隙，提示文字为框内占位
                    + 70 // 配置管理区：标题 33 + 按钮 28 + 行后 9
                    + 18 // 关于区纯分隔，无标题：上隙 12 + 下隙 6
                    + 29 // 版本行：描边按钮顶偏移 1 + 高 28
                    + 12; // 底部余量，同顶部留白
                base + if self.customizing_interval { 38 } else { 0 }
            }
        }
    }

    fn ensure_window(&mut self, parent: HWND) -> Option<HWND> {
        if let Some(h) = self.hwnd {
            return Some(h);
        }
        unsafe {
            if !self.class_registered {
                let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok()?;
                let name = wide(PANEL_WND_CLASS);
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(panel_wndproc),
                    hInstance: hinst.into(),
                    lpszClassName: PCWSTR(name.as_ptr()),
                    hbrBackground: HBRUSH(std::ptr::null_mut()),
                    hIcon: crate::ui::icon::app_icon(hinst.into())?,
                    ..Default::default()
                };
                if RegisterClassW(&wc) == 0 {
                    return None;
                }
                self.class_registered = true;
            }
            let hinst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok()?;
            let name = wide(PANEL_WND_CLASS);
            let hwnd = match CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                PCWSTR(name.as_ptr()),
                PCWSTR(name.as_ptr()),
                WS_POPUP,
                0,
                0,
                0,
                0,
                Some(parent),
                None,
                Some(hinst.into()),
                None,
            ) {
                Ok(hwnd) => hwnd,
                Err(e) => {
                    crate::platform::log(&format!("[Quotify] 面板窗口创建失败: {e}"));
                    return None;
                }
            };
            // 剥掉最大化框：Win11 对可最大化窗口的「拖到顶边 = 贴靠最大化」
            // 会把拖动中的窗口弹回，无最大化能力则不参与
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            let nosnap = style & !(WS_MAXIMIZEBOX.0 as isize);
            let _ = SetWindowLongPtrW(hwnd, GWL_STYLE, nosnap);
            let pref = windows::Win32::Graphics::Dwm::DWMWCP_DEFAULT;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE>()
                    as u32,
            );
            // 禁用 DWM 位置过渡：弹出/拖动时的系统平滑会与自绘动画叠加显卡顿
            let disable: i32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWA_TRANSITIONS_FORCEDISABLED,
                &disable as *const i32 as *const core::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            );
            self.hwnd = Some(hwnd);
            Some(hwnd)
        }
    }

    /// 计算并应用面板几何
    pub(crate) fn place(&mut self, hwnd: HWND, logical_h: i32, show: bool) {
        unsafe {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut mi);
            self.dpi = dpi_of(monitor).unwrap_or(FALLBACK_DPI);

            let w = self.px(PANEL_WIDTH);
            // 拖动进行中（drag_offset）与拖过（dragged）都保持当前位置
            let moved = self.dragged || self.drag_offset.is_some();
            let (x, y) = if moved {
                let mut wr = RECT::default();
                let _ = GetWindowRect(hwnd, &mut wr);
                (wr.left, wr.top)
            } else if let Some(anchor) = self.anchor {
                let ax = (anchor.left + anchor.right) / 2;
                let mut x = ax - w / 2;
                let mut y = anchor.top - self.px(logical_h) - self.px(8);
                if x < mi.rcWork.left + 8 {
                    x = mi.rcWork.left + 8;
                }
                if x + w > mi.rcWork.right - 8 {
                    x = mi.rcWork.right - 8 - w;
                }
                if y < mi.rcWork.top + 8 {
                    y = mi.rcWork.top + 8;
                }
                (x, y)
            } else {
                (0, 0)
            };
            // 拖过（含拖动中）保持完整高、允许遮住任务栏——移动与重排交替
            // 改高会闪跳；锚点弹出仍以工作区为界压扁，防矮屏出屏
            let h = if moved {
                self.px(logical_h)
            } else {
                let max_h = (mi.rcWork.bottom - mi.rcWork.top - 16).max(self.px(200));
                self.px(logical_h).min(max_h)
            };
            let flags = if show {
                SWP_SHOWWINDOW | SWP_NOCOPYBITS
            } else {
                SWP_NOCOPYBITS
            };
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, flags);
            self.anim_x = x;
            self.anim_w = w;
            self.anim_full_h = h;
            self.anim_bottom = y + h;
            // 滚动按屏幕内真实可见高度计：面板浮于任务栏之上，遮住任务
            // 栏的部分同样可见，仅视口越出屏幕底的部分不算可见
            let visible = h.min((mi.rcMonitor.bottom - y).max(0));
            self.refresh_scroll(logical_h, visible);
        }
    }

    /// 视口物理高变化后重算滚动上限，并把偏移钳回范围（可视逻辑高 = 物理高 / dpi）
    fn refresh_scroll(&mut self, logical_h: i32, physical_h: i32) {
        let visible = physical_h as f32 / self.dpi;
        self.scroll_max = (logical_h as f32 - visible).max(0.0);
        self.scroll_dy = if self.view == PanelView::Main {
            0.0
        } else {
            self.scroll_dy.min(self.scroll_max)
        };
    }

    /// 定位并显示，淡入由 TIMER_ANIM 推进。
    pub fn show_at(&mut self, parent: HWND, anchor: RECT, accounts: usize) {
        let Some(hwnd) = self.ensure_window(parent) else {
            return;
        };
        // 先于 place 读取：place 的 SetWindowPos 带 SWP_SHOWWINDOW 会置可见位
        let fresh = unsafe { !IsWindowVisible(hwnd).as_bool() };
        self.anchor = Some(anchor);
        // 拖动仅临时查看；面板重新弹出时回到托盘锚点，拖动残留一并清除
        self.dragged = false;
        self.drag_offset = None;
        let logical_h = self.view_height(accounts);
        self.place(hwnd, logical_h, true);
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            // 整窗不用 layered alpha——与 HwndRenderTarget 交换链呈现不兼容
            if fresh {
                if let Some(r) = self.renderer.as_mut() {
                    r.anim.appear = Some(anim::Tween::now(180));
                }
                start_anim(hwnd);
            }
            // 巡检首延给足弹出宽限，防弹出瞬间即被误收回
            SetTimer(Some(hwnd), TIMER_OUTSIDE_CHECK, 1200, None);
            SetTimer(Some(hwnd), TIMER_MINUTE_TICK, 60_000, None);
            let _ = InvalidateRect(Some(hwnd), None, true);
        }
    }

    /// 请求收起，仅预览模式生效，400ms 防抖
    pub fn request_close(&mut self) {
        if self.hovered || self.mode != PanelMode::Preview {
            return;
        }
        if let Some(h) = self.hwnd {
            unsafe { SetTimer(Some(h), TIMER_CLOSE_DEBOUNCE, 400, None) };
        }
    }

    /// 锚点 ±24px 的本地矩形判定——Shell_NotifyIconGetRect 跨进程同步，高频轮询会互锁
    pub(crate) fn cursor_near_anchor(&self) -> bool {
        let pt = unsafe {
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            p
        };
        self.anchor
            .map(|a| {
                pt.x >= a.left - 24
                    && pt.x <= a.right + 24
                    && pt.y >= a.top - 24
                    && pt.y <= a.bottom + 24
            })
            .unwrap_or(false)
    }

    pub(crate) fn begin_hide(&mut self, hwnd: HWND) {
        self.mode = PanelMode::Hidden;
        self.hovered = false;
        self.adding_account = false;
        self.key_revealed = false;
        // 收起即丢弃巡检计时与未完结的拖动，重开从干净状态起步
        self.outside_since = None;
        self.drag_offset = None;
        self.clear_input(hwnd);
        // 直接隐藏：自绘收缩/淡出会与 DWM 过渡叠加闪烁
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            let _ = KillTimer(Some(hwnd), TIMER_OUTSIDE_CHECK);
            let _ = KillTimer(Some(hwnd), TIMER_ANIM);
            let _ = KillTimer(Some(hwnd), TIMER_MINUTE_TICK);
        }
        // 收起后到下次打开前不再绘制，归还工作集把静止内存压回托盘档；
        // 重开时的软缺页按次一次性发生，换常驻低占用
        crate::platform::trim_working_set();
    }

    /// 左键：预览 ⇄ 锁定；已锁定 → 收起
    pub fn toggle_pin(&mut self, parent: HWND, anchor: RECT, accounts: usize) {
        match self.mode {
            PanelMode::Pinned => {
                if let Some(h) = self.hwnd {
                    self.begin_hide(h);
                }
            }
            _ => {
                // 悬停预览中锁定：面板本就显示，保持正在看的视图；
                // 从隐藏重开才回主视图，不停留在上次滚动的旧设置页
                if self.mode == PanelMode::Hidden {
                    self.reset_to_main();
                }
                self.mode = PanelMode::Pinned;
                self.show_at(parent, anchor, accounts);
                // 显示后立即激活——后台窗口拿不到键盘焦点，IME 异常
                if let Some(h) = self.hwnd {
                    unsafe {
                        let _ = SetForegroundWindow(h);
                    }
                }
            }
        }
    }

    pub fn show_preview(&mut self, parent: HWND, anchor: RECT, accounts: usize) {
        self.mode = PanelMode::Preview;
        self.reset_to_main();
        self.show_at(parent, anchor, accounts);
    }

    /// 打开面板前的复位：回主视图、清滚动偏移、添加表单与输入临时态；
    /// 预览/锁定两条打开路径共用，防收起后重开停在上次滚动的旧设置页。
    /// 间隔行展开态不清：view_height 有配套 +38 钩子，展开态须跨开合保留
    fn reset_to_main(&mut self) {
        self.view = PanelView::Main;
        self.scroll_dy = 0.0;
        self.adding_account = false;
        self.key_revealed = false;
        if let Some(h) = self.hwnd {
            self.clear_input(h);
        } else {
            self.input.field = None;
        }
    }

    /// 结束输入状态，销毁光标与 IME 上下文
    pub(crate) fn clear_input(&mut self, hwnd: HWND) {
        self.input.field = None;
        self.input.edit.reset();
        self.input.surrogate = None;
        self.vis_start.set(0);
        unsafe {
            let _ = DestroyCaret();
            // 摘除 IME 上下文：裸窗口挂上后要收回，避免游离
            use windows::Win32::UI::Input::Ime::{
                HIMC, ImmAssociateContext, ImmDestroyContext, ImmGetContext,
            };
            let ctx = ImmGetContext(hwnd);
            // 仅回收自建上下文；从未进入过输入态时窗口挂的是线程默认上下文，销毁会坏全局 IME
            if self.ime_owned && !ctx.is_invalid() {
                let _ = ImmAssociateContext(hwnd, HIMC(std::ptr::null_mut()));
                let _ = ImmDestroyContext(ctx);
            }
        }
        self.ime_owned = false;
    }

    /// 聚焦输入字段：系统光标 + IME 组合窗跟随光标。
    /// 切换字段须重置编辑状态（撤销栈跨字段会污染新缓冲）；
    /// 重复聚焦同一字段保留光标位置，供点击定位后不被弹回末尾
    pub fn focus_input(&mut self, hwnd: HWND, field: InputField) {
        let switched = self.input.field != Some(field);
        if switched {
            if let Some(old) = self.input.field {
                self.invalidate_field_line(hwnd, old);
            }
            self.input.edit.reset();
            self.vis_start.set(0);
        }
        self.input.field = Some(field);
        self.mode = PanelMode::Pinned;
        if switched {
            let buf = self.input.active_str().to_string();
            self.input.edit.caret_to_end(&buf);
        }
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hwnd));
            let caret_h = self.px(16);
            // 宽度随 DPI 放大：150% 缩放下 1 物理像素不足 1 逻辑像素，过细难辨
            let _ = CreateCaret(hwnd, None, self.px(1).max(1), caret_h);
            let _ = ShowCaret(Some(hwnd));
            let r = self.renderer.as_ref();
            self.update_caret(hwnd, r);
            self.attach_ime(hwnd);
        }
    }

    /// 挂 IME 上下文并把组合窗定位到光标处。
    unsafe fn attach_ime(&mut self, hwnd: HWND) {
        unsafe {
            use windows::Win32::UI::Input::Ime::{
                ImmAssociateContext, ImmCreateContext, ImmDestroyContext,
            };
            let old = ImmAssociateContext(hwnd, ImmCreateContext());
            // 换挂返回的旧上下文：自建的须销毁防泄漏；首次换下的默认上下文归系统，不可销毁
            if self.ime_owned && !old.is_invalid() {
                let _ = ImmDestroyContext(old);
            }
            self.ime_owned = true;
            self.sync_ime_pos(hwnd);
        }
    }

    /// 组合窗跟随光标移动；光标每次移动后调用，中文组合串贴住光标处
    unsafe fn sync_ime_pos(&self, hwnd: HWND) {
        unsafe {
            use windows::Win32::Foundation::POINT;
            use windows::Win32::UI::Input::Ime::{
                CFS_POINT, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext,
                ImmSetCompositionWindow,
            };
            let Some(field) = self.input.field else {
                return;
            };
            let Some(r) = self.renderer.as_ref() else {
                return;
            };
            let (_vis_start, cx) = self.caret_layout(r);
            let bx = field_geo(field).0;
            let by = self.caret_line_y(field);
            let x = bx + 6.0 + cx;
            // 组合窗锚在框底附近，框顶 +17
            let pt = POINT {
                x: (x * self.dpi).round() as i32,
                y: ((by + 12.0) * self.dpi).round() as i32,
            };
            let ctx = ImmGetContext(hwnd);
            if !ctx.is_invalid() {
                let cf = COMPOSITIONFORM {
                    dwStyle: CFS_POINT,
                    ptCurrentPos: pt,
                    rcArea: windows::Win32::Foundation::RECT::default(),
                };
                let _ = ImmSetCompositionWindow(ctx, &cf);
                let _ = ImmReleaseContext(hwnd, ctx);
            }
        }
    }

    /// 光标可视布局：返回（可视窗口起点 char 位次，光标在窗口内的 x）。
    /// 绘制与系统光标定位共用此函数，同一次计算同一结果，两侧位置
    /// 恒一致。未超宽从 0 起；超宽时优先沿用上次起点（粘滞，避免每次
    /// 取最小起点让点击处窗口回跳），光标出窗才二分调整。
    /// Key 掩码态圆点与字符一一对应，位次空间不变
    pub(crate) fn caret_layout(&self, renderer: &Renderer) -> (usize, f32) {
        let Some(field) = self.input.field else {
            return (0, 0.0);
        };
        let buf = self.field_display();
        let (_bx, w, tail) = field_geo(field);
        let avail = (w - 6.0 - tail).max(1.0);
        let chars: Vec<char> = buf.chars().collect();
        let caret = self.input.edit.caret.min(chars.len());
        let seg = |a: usize, b: usize| -> f32 {
            let s: String = chars[a.min(chars.len())..b.min(chars.len())]
                .iter()
                .collect();
            unsafe { renderer.measure_ro(&s, 12.0, 400, true) }
        };
        let full = seg(0, chars.len());
        if full <= avail {
            self.vis_start.set(0);
            return (0, seg(0, caret));
        }
        let sticky = self.vis_start.get().min(caret);
        if seg(sticky, caret) <= avail {
            self.vis_start.set(sticky);
            return (sticky, seg(sticky, caret));
        }
        // 粘滞起点装不下光标才右移；宽度随起点单调不增，二分最小满足者
        let mut lo = sticky;
        let mut hi = caret;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if seg(mid, caret) <= avail {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        self.vis_start.set(lo);
        (lo, seg(lo, caret))
    }

    /// 激活字段的显示串：Key 掩码态用等宽圆点（与渲染侧 mask 同规则），
    /// 其余字段即缓冲本身
    pub(crate) fn field_display(&self) -> String {
        if self.input.field == Some(InputField::Key) && !self.key_revealed {
            "•".repeat(self.input.key.chars().count())
        } else {
            self.input.active_str().to_string()
        }
    }

    /// 光标行锚点 y（逻辑像素，含框内垂直偏移与滚动平移）；
    /// 系统光标定位、IME 组合窗与行级失效共用
    fn caret_line_y(&self, field: InputField) -> f32 {
        let y = match field {
            InputField::Interval => {
                let (has_account, auth_error) = self.caret_ctx;
                layout::interval_input_y(has_account, auth_error)
            }
            InputField::Proxy => {
                let (has_account, auth_error) = self.caret_ctx;
                layout::proxy_input_y(has_account, auth_error, self.customizing_interval)
            }
            InputField::PeakStart | InputField::PeakEnd => {
                let (has_account, auth_error) = self.caret_ctx;
                layout::peak_input_y(has_account, auth_error, self.customizing_interval)
            }
            InputField::Name => layout::ADD_NAME_Y,
            InputField::Key => layout::ADD_KEY_Y,
            InputField::Org => layout::ADD_ORG_Y,
            InputField::Project => layout::ADD_PROJECT_Y,
        };
        // 内容上滚后锚点同步上移，系统光标与 IME 组合窗贴住可视位置
        y + layout::CARET_Y_OFFSET - self.scroll_dy
    }

    /// 文本区内 x（逻辑像素）→ char 位次：与 caret_layout 同一可视窗口，
    /// 取最近字符边界（点击定位光标与拖选用）
    pub(crate) fn caret_hit_test(&self, renderer: &Renderer, field: InputField, x: f32) -> usize {
        let disp = self.field_display();
        let chars: Vec<char> = disp.chars().collect();
        let (vis_start, _) = self.caret_layout(renderer);
        let (bx, _w, _tail) = field_geo(field);
        let base = bx + 6.0;
        let mut best = vis_start;
        let mut best_dist = f32::MAX;
        for i in vis_start..=chars.len() {
            let s: String = chars[vis_start..i].iter().collect();
            let edge = base + unsafe { renderer.measure_ro(&s, 12.0, 400, true) };
            let d = (edge - x).abs();
            if d < best_dist {
                best_dist = d;
                best = i;
            }
        }
        best
    }

    /// 激活字段文本区命中判定：光标定位点击只认文本框范围
    pub(crate) fn hit_text_zone(&self, x: f32, y: f32) -> bool {
        if let Some(f) = self.input.field {
            let (bx, w, tail) = field_geo(f);
            let top = self.caret_line_y(f) - layout::CARET_Y_OFFSET;
            x >= bx + 4.0 && x <= bx + w - tail && y >= top && y <= top + layout::INPUT_H
        } else {
            false
        }
    }

    /// 输入行局部失效：编辑小动作只重画激活输入框所在行，
    /// 软光栅代价最小，也避免全窗重绘的迟滞感
    pub(crate) fn invalidate_input_line(&self, hwnd: HWND) {
        if let Some(f) = self.input.field {
            self.invalidate_field_line(hwnd, f);
        }
    }

    /// 指定字段所在行的局部失效；字段切换时旧字段行（含激活边框与
    /// 选区高亮像素）须一并失效，否则残留到下一次全窗重绘
    fn invalidate_field_line(&self, hwnd: HWND, field: InputField) {
        let (bx, w, _tail) = field_geo(field);
        let line_y = self.caret_line_y(field) - layout::CARET_Y_OFFSET;
        let rect = RECT {
            left: self.px((bx - 2.0) as i32),
            top: self.px((line_y - 2.0) as i32),
            right: self.px((bx + w + 2.0) as i32),
            bottom: self.px((line_y + layout::INPUT_H + 2.0) as i32),
        };
        unsafe {
            let _ = InvalidateRect(Some(hwnd), Some(&rect), false);
        }
    }

    /// 按字段内容计算光标位置，与 input_field 绘制对齐（同一 caret_layout）。
    /// renderer 缺席（首帧前）时跳过定位，待下次调用补；组合窗随光标同步
    pub fn update_caret(&self, hwnd: HWND, renderer: Option<&Renderer>) {
        let Some(field) = self.input.field else {
            return;
        };
        let Some(r) = renderer else {
            return;
        };
        let by = self.caret_line_y(field);
        let (_vis_start, cx) = self.caret_layout(r);
        let bx = field_geo(field).0;
        let x = bx + 6.0 + cx;
        // by 已含 CARET_Y_OFFSET，框内垂直居中
        unsafe {
            let _ = SetCaretPos(
                (x * self.dpi).round() as i32,
                (by * self.dpi).round() as i32,
            );
            // 组合窗随光标重定位，中文组合串贴住光标
            self.sync_ime_pos(hwnd);
        }
    }
}

/// 取显示器有效 DPI（百分比 / 96）
pub(crate) unsafe fn dpi_of(monitor: HMONITOR) -> Option<f32> {
    unsafe {
        let mut cx = 0u32;
        let mut cy = 0u32;
        GetDpiForMonitor(
            monitor,
            windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI,
            &mut cx,
            &mut cy,
        )
        .ok()
        .map(|_| cx as f32 / 96.0)
    }
}

unsafe fn start_anim(hwnd: HWND) {
    unsafe {
        SetTimer(Some(hwnd), TIMER_ANIM, 16, None);
    }
}

/// 重挂 TME_LEAVE 离窗跟踪
pub(crate) unsafe fn track_leave(hwnd: HWND) {
    unsafe {
        let mut tm = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            ..Default::default()
        };
        let _ = TrackMouseEvent(&mut tm);
    }
}

/// 面板窗口过程
pub extern "system" fn panel_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                // 不走 BeginPaint：验证客户区后直接渲染
                let _ = ValidateRect(Some(hwnd), None);
                let app = app_from_tray(hwnd);
                if let Some(app) = app {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    // take 成局部值，避免与后续 &Panel / 模型组装的不可变借用冲突；
                    // 首建标记同时驱动下面的主题对齐
                    let cached = app.panel.renderer.take();
                    let fresh = cached.is_none();
                    let mut renderer = cached.or_else(|| Renderer::new(hwnd, &rect, app.panel.dpi));
                    let mut keep = true;
                    if let Some(r) = renderer.as_mut() {
                        // 仅首建时对齐配置外观：Renderer::new 兜底读系统外观，
                        // 配置强制浅/深色与系统相反时首帧会用错主题；日常外观
                        // 切换由 app 侧 apply_appearance 推送，跟随系统模式下
                        // 逐帧重建主题会反复读注册表
                        if fresh {
                            r.theme = crate::ui::panel::theme::Theme::new(
                                crate::app::resolved_appearance(
                                    app.config.general.appearance.as_deref(),
                                ),
                            );
                        }
                        let model = PanelModel::from_app(app);
                        let view = app.panel.view;
                        let dpi = app.panel.dpi;
                        keep = r.paint(hwnd, &rect, &app.panel, &model, view, dpi);
                    }
                    // paint 判定设备丢失时丢弃整个 Renderer，并立即请求下一帧
                    // 走 fresh 路径全量重建、重新对齐主题——不请求的话静止面板
                    // 要等分钟心跳才有下一帧，期间空白且命中全部失效
                    if keep {
                        app.panel.renderer = renderer;
                    } else {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => {
                let id = wparam.0;
                match id {
                    TIMER_ANIM => on_anim_tick(hwnd),
                    // 心跳只请求重绘：无数据事件时相对时间文案也能推进
                    TIMER_MINUTE_TICK => {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        LRESULT(0)
                    }
                    TIMER_CLOSE_DEBOUNCE => {
                        let app = app_from_tray(hwnd);
                        if let Some(app) = app {
                            let _ = KillTimer(Some(hwnd), TIMER_CLOSE_DEBOUNCE);
                            // 到期时鼠标指针已回托盘则取消收回——悬停语义优先
                            let near_tray = app.panel.cursor_near_anchor();
                            if !app.panel.hovered
                                && app.panel.mode == PanelMode::Preview
                                && !near_tray
                            {
                                app.panel.begin_hide(hwnd);
                            }
                        }
                        LRESULT(0)
                    }
                    TIMER_OUTSIDE_CHECK => {
                        let app = app_from_tray(hwnd);
                        if let Some(app) = app {
                            // 不能调 Shell_NotifyIconGetRect——跨进程同步调用，高频轮询互锁卡死
                            SetTimer(Some(hwnd), TIMER_OUTSIDE_CHECK, 200, None);
                            let preview = app.panel.mode == PanelMode::Preview;
                            let pinned = app.panel.mode == PanelMode::Pinned;
                            if (preview || pinned) && !app.panel.hovered {
                                let mut pt = POINT::default();
                                let _ = GetCursorPos(&mut pt);
                                let w = WindowFromPoint(pt);
                                // 子控件同样算在面板内；账号弹窗是面板的延伸，光标
                                // 移入其中同样不能触发面板收起
                                let in_popup = app
                                    .popup
                                    .hwnd
                                    .is_some_and(|ph| w == ph || GetAncestor(w, GA_ROOT) == ph);
                                let in_panel =
                                    in_popup || w == hwnd || GetAncestor(w, GA_ROOT) == hwnd;
                                // 正在输入则绝不收起
                                let focus_in_panel = app.panel.input.field.is_some()
                                    || windows::Win32::UI::Input::KeyboardAndMouse::GetFocus()
                                        == hwnd;
                                // 鼠标在托盘图标上
                                let near_tray = app.panel.cursor_near_anchor();
                                if in_panel || focus_in_panel || near_tray {
                                    app.panel.outside_since = None;
                                } else {
                                    let now =
                                        windows::Win32::System::SystemInformation::GetTickCount64();
                                    let since = *app.panel.outside_since.get_or_insert(now);
                                    let timeout: u64 = if preview { 300 } else { 2000 };
                                    if now - since > timeout {
                                        app.panel.outside_since = None;
                                        app.panel.begin_hide(hwnd);
                                    }
                                }
                            }
                        }
                        LRESULT(0)
                    }
                    _ => DefWindowProcW(hwnd, msg, wparam, lparam),
                }
            }
            WM_MOUSEMOVE => {
                let mut app = app_from_tray(hwnd);
                // 手动拖动跟随：鼠标指针减按下偏移，钳制在工作区内
                if let Some(app) = app.as_mut()
                    && let Some((ox, oy)) = app.panel.drag_offset
                {
                    let mut cursor = POINT::default();
                    let _ = GetCursorPos(&mut cursor);
                    let mut wr = RECT::default();
                    let _ = GetWindowRect(hwnd, &mut wr);
                    let w = wr.right - wr.left;
                    let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                    let mut mi = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    let _ = GetMonitorInfoW(monitor, &mut mi);
                    // 四方向都可越出屏幕，仅保留窗口一角（64 物理像素）
                    // 在工作区内可抓回，上下左右对称
                    let x = (cursor.x - ox).clamp(
                        mi.rcWork.left - w + 64,
                        (mi.rcWork.right - 64).max(mi.rcWork.left - w + 64),
                    );
                    let y_min = mi.rcWork.top - (wr.bottom - wr.top) + 64;
                    let y = (cursor.y - oy).clamp(y_min, (mi.rcWork.bottom - 64).max(y_min));
                    let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
                    return LRESULT(0);
                }
                let app = app_from_tray(hwnd);
                if let Some(app) = app {
                    // 拖选中：光标随鼠标指针推进扩选区
                    if app.panel.selecting {
                        let x = x_of(lparam) / app.panel.dpi;
                        if let (Some(f), Some(r)) =
                            (app.panel.input.field, app.panel.renderer.as_ref())
                        {
                            let pos = app.panel.caret_hit_test(r, f, x);
                            if pos != app.panel.input.edit.caret {
                                app.panel.input.edit.place(pos, true);
                                app.panel.update_caret(hwnd, app.panel.renderer.as_ref());
                                app.panel.invalidate_input_line(hwnd);
                            }
                        }
                        return LRESULT(0);
                    }
                    // 每次移动都重挂：TME_LEAVE 一次性且会被鼠标捕获打断，
                    // 仅在 !hovered 时挂载的话，捕获结束后 MOUSELEAVE 永远不来，hovered 卡 true
                    track_leave(hwnd);
                    if !app.panel.hovered {
                        app.panel.hovered = true;
                        let _ = KillTimer(Some(hwnd), TIMER_CLOSE_DEBOUNCE);
                    }
                    let (x, y) = (x_of(lparam) / app.panel.dpi, y_of(lparam) / app.panel.dpi);
                    if let Some(r) = app.panel.renderer.as_mut() {
                        let hit = r.hit_at(x, y);
                        if r.hover != hit {
                            r.hover = hit;
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                let app = app_from_tray(hwnd);
                if let Some(app) = app {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let w = WindowFromPoint(pt);
                    let still_here = w == hwnd || GetAncestor(w, GA_ROOT) == hwnd;
                    if still_here {
                        app.panel.hovered = true;
                        track_leave(hwnd);
                    } else {
                        app.panel.hovered = false;
                        if app.panel.mode == PanelMode::Preview {
                            app.panel.request_close();
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let app = app_from_tray(hwnd);
                // 仅设置页可滚；主视图内容恒短于视口，滚了也无内容可显
                if let Some(app) = app
                    && app.panel.view != PanelView::Main
                {
                    // 高字为有符号 delta；向后滚（负）看下方内容 → scroll_dy 增大
                    let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
                    // 三行制：每 120 delta 记 3 行 ≈ 53 逻辑 px
                    let next = (app.panel.scroll_dy - delta / 120.0 * 53.0)
                        .clamp(0.0, app.panel.scroll_max);
                    if next != app.panel.scroll_dy {
                        app.panel.scroll_dy = next;
                        // 焦点在输入框上时，系统光标随内容一起平移
                        if app.panel.input.field.is_some() {
                            let r = app.panel.renderer.as_ref();
                            app.panel.update_caret(hwnd, r);
                        }
                        let _ = InvalidateRect(Some(hwnd), None, true);
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let mut app = app_from_tray(hwnd);
                if let Some(app) = app.as_mut() {
                    // 拖选收尾：光标定位已就位，跳过命中分派——
                    // 拖选终点若落在别的输入框上，不应触发该框的
                    // 命中处理造成意外的字段切换
                    if app.panel.selecting {
                        app.panel.selecting = false;
                        let _ = ReleaseCapture();
                        app.panel.press_at = None;
                        track_leave(hwnd);
                        return LRESULT(0);
                    }
                    // 按下→松手位移超过阈值才算拖动；press_at 在 BUTTONDOWN 记录
                    let moved_far = app.panel.press_at.take().is_some_and(|(px, py)| {
                        let mut cursor = POINT::default();
                        let _ = GetCursorPos(&mut cursor);
                        (cursor.x - px).abs() + (cursor.y - py).abs() > 8
                    });
                    // 空白处按下即进入拖动态，松手须先按位移分流：原地点击不是拖动
                    if app.panel.drag_offset.take().is_some() {
                        let _ = ReleaseCapture();
                        if moved_far {
                            // 真拖动结束：保持当前位置恢复完整高度
                            app.panel.dragged = true;
                            let n = app.config.accounts.len();
                            let logical_h = app.panel.view_height(n);
                            app.panel.place(hwnd, logical_h, true);
                            let _ = InvalidateRect(Some(hwnd), None, true);
                        } else if app.panel.input.field.is_some() || app.panel.key_revealed {
                            // 空白点击结束输入态与明文查看；缓冲文本保留
                            app.panel.clear_input(hwnd);
                            app.panel.key_revealed = false;
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                        return LRESULT(0);
                    }
                    if !moved_far {
                        let (x, y) = (x_of(lparam) / app.panel.dpi, y_of(lparam) / app.panel.dpi);
                        let hit = app.panel.renderer.as_ref().and_then(|r| r.hit_at(x, y));
                        // 会进入/保持输入态的命中；其余点击一律结束输入，光标不再滞留框外。
                        // 列表收敛在 Hit::is_input_hit，新增输入框时与枚举同文件维护
                        let refocus = hit.is_some_and(|h| h.is_input_hit());
                        if let Some(hit) = hit {
                            crate::app::handle_panel_hit(app, hit, hwnd);
                        }
                        if !refocus && (app.panel.input.field.is_some() || app.panel.key_revealed) {
                            app.panel.clear_input(hwnd);
                            app.panel.key_revealed = false;
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                // 编辑键位分派：移动、选区、删词、撤销重做走 EditState；
                // 字符与组合键由 WM_CHAR 处理
                let vk = (wparam.0 & 0xFF) as u16;
                let app = app_from_tray(hwnd);
                if let Some(app) = app
                    && app.panel.input.field.is_some()
                    && [VK_DELETE, VK_END, VK_HOME, VK_LEFT, VK_RIGHT, VK_Y, VK_Z]
                        .iter()
                        .any(|k| vk == k.0)
                {
                    use windows::Win32::UI::Input::KeyboardAndMouse::{
                        GetKeyState, VK_CONTROL, VK_SHIFT,
                    };
                    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                    let input = &mut app.panel.input;
                    let acted = if vk == VK_LEFT.0 {
                        let t = input.active_str().to_string();
                        input.edit.move_left(&t, ctrl, shift);
                        true
                    } else if vk == VK_RIGHT.0 {
                        let t = input.active_str().to_string();
                        input.edit.move_right(&t, ctrl, shift);
                        true
                    } else if vk == VK_HOME.0 {
                        input.edit.move_home(shift);
                        true
                    } else if vk == VK_END.0 {
                        let t = input.active_str().to_string();
                        input.edit.move_end(&t, shift);
                        true
                    } else if vk == VK_DELETE.0 {
                        apply_edit(input, |e, t| e.delete(t, ctrl));
                        true
                    } else if vk == VK_Z.0 && ctrl {
                        // Shift+Z 与 Y 同为重做惯例
                        apply_edit(input, |e, t| {
                            if shift {
                                e.redo(t);
                            } else {
                                e.undo(t);
                            }
                        });
                        true
                    } else if vk == VK_Y.0 && ctrl {
                        apply_edit(input, EditState::redo);
                        true
                    } else {
                        false
                    };
                    if acted {
                        app.panel.update_caret(hwnd, app.panel.renderer.as_ref());
                        app.panel.invalidate_input_line(hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_CHAR => {
                // 字符编辑与剪贴板组合键；控制字符按 Windows 编辑控件惯例分派
                let app = app_from_tray(hwnd);
                if let Some(app) = app
                    && app.panel.input.field.is_some()
                {
                    let ch = (wparam.0 & 0xFFFF) as u16;
                    let mut confirm = false;
                    let key_masked =
                        app.panel.input.field == Some(InputField::Key) && !app.panel.key_revealed;
                    // UTF-16 代理对按高低半区送达：高半区暂存、低半区到达时
                    // 拼出增补平面字符；普通字符到达即清暂存，防陈旧高半区
                    // 与后续低半区错误拼合
                    let decoded = {
                        let cp = ch as u32;
                        if (0xD800..0xDC00).contains(&cp) {
                            app.panel.input.surrogate = Some(ch);
                            None
                        } else if (0xDC00..0xE000).contains(&cp) {
                            app.panel
                                .input
                                .surrogate
                                .take()
                                .map(|hi| 0x10000 + (((hi as u32) - 0xD800) << 10) + cp - 0xDC00)
                                .and_then(char::from_u32)
                        } else {
                            app.panel.input.surrogate = None;
                            char::from_u32(cp)
                        }
                    };
                    {
                        use windows::Win32::UI::Input::KeyboardAndMouse::{
                            GetKeyState, VK_CONTROL,
                        };
                        let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                        let input = &mut app.panel.input;
                        match decoded {
                            Some('\r') | Some('\n') => confirm = true,
                            Some('\u{8}') => {
                                apply_edit(input, |e, t| e.backspace(t, ctrl));
                            }
                            Some('\u{7F}') => {
                                // Ctrl+Backspace：删前一个词
                                apply_edit(input, |e, t| e.backspace(t, true));
                            }
                            Some('\u{1}') => {
                                let t = input.active_str().to_string();
                                input.edit.select_all(&t);
                            }
                            Some('\u{3}') if !key_masked => {
                                // 单行惯例：无选区复制全部
                                let t = input.active_str().to_string();
                                let s = input
                                    .edit
                                    .copy(&t)
                                    .map(str::to_string)
                                    .or_else(|| (!t.is_empty()).then_some(t));
                                if let Some(s) = s {
                                    write_clipboard_text(&s);
                                }
                            }
                            Some('\u{18}') if !key_masked => {
                                // 单行惯例：无选区剪切全部
                                let mut out = None;
                                apply_edit(input, |e, t| {
                                    if e.selection().is_none() {
                                        if t.is_empty() {
                                            return;
                                        }
                                        e.select_all(t);
                                    }
                                    out = e.cut(t);
                                });
                                if let Some(s) = out {
                                    write_clipboard_text(s.as_str());
                                }
                            }
                            Some('\u{16}') => {
                                if let Some(text) = read_clipboard_text() {
                                    apply_edit(input, |e, t| e.paste(t, &text, INPUT_MAX_BYTES));
                                }
                            }
                            Some(c) if !c.is_control() => {
                                apply_edit(input, |e, t| e.insert(t, c, INPUT_MAX_BYTES));
                            }
                            _ => {}
                        }
                    }
                    if confirm {
                        crate::app::confirm_panel_input(app, hwnd);
                    }
                    app.panel.update_caret(hwnd, app.panel.renderer.as_ref());
                    app.panel.invalidate_input_line(hwnd);
                }
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                let app = app_from_tray(hwnd);
                if let Some(app) = app
                    && app.panel.input.field.is_some()
                    // 焦点真已转走才清；若焦点绕回本窗口则保留输入态
                    && windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() != hwnd
                {
                    // 失焦不清输入会让面板被当作「正在输入」永不收起，IME 组合窗也会游离
                    app.panel.clear_input(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                // 任意空白处按下进入手动拖动，长设置页可拖到可视范围。
                // 不用系统 HTCAPTION 模态拖动：Win11 拖到屏幕顶边会强制贴靠预览
                let mut app = app_from_tray(hwnd);
                let (x, y) = if let Some(app) = app.as_ref() {
                    (x_of(lparam) / app.panel.dpi, y_of(lparam) / app.panel.dpi)
                } else {
                    (0.0, 0.0)
                };
                let hit_none = app
                    .as_ref()
                    .and_then(|a| a.panel.renderer.as_ref())
                    .map(|r| r.hit_at(x, y).is_none())
                    .unwrap_or(true);
                let mut cursor = POINT::default();
                let _ = GetCursorPos(&mut cursor);
                if let Some(app) = app.as_mut() {
                    app.panel.press_at = Some((cursor.x, cursor.y));
                    // 输入框文本区按下：定位光标并进入拖选，不进入空白拖动。
                    // 已聚焦的框直接定位；未聚焦的框按下即聚焦再定位点击处，
                    // 双击第二击即可选词。连击按 500ms/8px 判定：双击选词、
                    // 三击全选；Shift 按下时点击扩选而非重置
                    let active_zone = app.panel.hit_text_zone(x, y);
                    let target = if active_zone {
                        app.panel.input.field
                    } else {
                        app.panel
                            .renderer
                            .as_ref()
                            .and_then(|r| r.hit_at(x, y))
                            .and_then(input_field_of_hit)
                    };
                    if let Some(f) = target {
                        if !active_zone {
                            app.panel.focus_input(hwnd, f);
                        }
                        let Some(r) = app.panel.renderer.as_ref() else {
                            return LRESULT(0);
                        };
                        let now = windows::Win32::System::SystemInformation::GetTickCount();
                        let (px, py) = ((x * app.panel.dpi) as i32, (y * app.panel.dpi) as i32);
                        let count = match app.panel.text_clicks {
                            Some((t, cx, cy, n))
                                if now.wrapping_sub(t) <= 500
                                    && (px - cx).abs() + (py - cy).abs() <= 8 =>
                            {
                                n.saturating_add(1).min(3)
                            }
                            _ => 1,
                        };
                        app.panel.text_clicks = Some((now, px, py, count));
                        let pos = app.panel.caret_hit_test(r, f, x);
                        use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT};
                        let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                        match count {
                            2 => {
                                // 双击选词；Key 掩码态圆点无词义，退化为全选
                                let real = app.panel.input.active_str().to_string();
                                if f == InputField::Key && !app.panel.key_revealed {
                                    app.panel.input.edit.select_all(&real);
                                } else {
                                    let (a, b) = text_edit::word_range_at(&real, pos);
                                    app.panel.input.edit.place(a, false);
                                    app.panel.input.edit.place(b, true);
                                }
                            }
                            3 => {
                                let real = app.panel.input.active_str().to_string();
                                app.panel.input.edit.select_all(&real);
                            }
                            _ => {
                                app.panel.input.edit.place(pos, shift);
                            }
                        }
                        app.panel.update_caret(hwnd, app.panel.renderer.as_ref());
                        app.panel.selecting = true;
                        app.panel.invalidate_input_line(hwnd);
                        let _ = SetCapture(hwnd);
                        return LRESULT(0);
                    }
                    if hit_none {
                        let mut wr = RECT::default();
                        let _ = GetWindowRect(hwnd, &mut wr);
                        app.panel.drag_offset = Some((cursor.x - wr.left, cursor.y - wr.top));
                        let _ = SetCapture(hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                // lParam 为获得捕获的新窗口（wParam 未用）；空或他人持有即本窗口拖动已断线
                if HWND(lparam.0 as *mut _) != hwnd {
                    let app = app_from_tray(hwnd);
                    if let Some(app) = app {
                        // 捕获被系统/他窗夺走后不会再有 WM_LBUTTONUP，拖动状态须在此终结，
                        // 否则下次 MOUSEMOVE 仍按拖动跟随，形成幽灵拖动
                        app.panel.drag_offset = None;
                        app.panel.press_at = None;
                        app.panel.selecting = false;
                    }
                    // 捕获期间 TME 离窗跟踪被取消，此处补挂；鼠标指针已在外会立即收到 MOUSELEAVE
                    track_leave(hwnd);
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let app = app_from_tray(hwnd);
                if let Some(app) = app {
                    // 建议矩形已按新 DPI 算好位置尺寸，照设免内容拉伸；此时仍持旧 dpi
                    let suggested = &*(lparam.0 as *const RECT);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        suggested.left,
                        suggested.top,
                        suggested.right - suggested.left,
                        suggested.bottom - suggested.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    app.panel.dpi = ((wparam.0 >> 16) & 0xFFFF) as u32 as f32 / 96.0;
                    // 视口物理高变了，滚动上限随新 dpi 重算
                    let logical_h = app.panel.view_height(app.config.accounts.len());
                    app.panel
                        .refresh_scroll(logical_h, suggested.bottom - suggested.top);
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SETCURSOR => {
                let hit_hwnd = HWND(wparam.0 as *mut _);
                if hit_hwnd == hwnd && (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                    let app = app_from_tray(hwnd);
                    let hand = app
                        .and_then(|a| a.panel.renderer.as_ref())
                        .map(|r| r.hover.is_some())
                        .unwrap_or(false);
                    let cursor = if hand { IDC_HAND } else { IDC_ARROW };
                    if let Ok(c) = LoadCursorW(None, cursor) {
                        let _ = SetCursor(Some(c));
                        return LRESULT(1);
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_ACTIVATE as isize),
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// 动画帧
unsafe fn on_anim_tick(hwnd: HWND) -> LRESULT {
    unsafe {
        let app = app_from_tray(hwnd);
        let Some(app) = app else {
            let _ = KillTimer(Some(hwnd), TIMER_ANIM);
            return LRESULT(0);
        };
        let mut done = true;
        if let Some(r) = app.panel.renderer.as_mut() {
            if let Some(t) = &r.anim.appear {
                if t.finished() {
                    r.anim.appear = None;
                } else {
                    done = false;
                }
            }
            if r.spin_remaining() {
                done = false;
            }
        }
        let _ = InvalidateRect(Some(hwnd), None, false);

        if done {
            let _ = KillTimer(Some(hwnd), TIMER_ANIM);
        }
        LRESULT(0)
    }
}

/// 从子窗口 hwnd 回溯托盘窗口取 App
pub(crate) fn app_from_tray(hwnd: HWND) -> Option<&'static mut crate::app::App> {
    unsafe {
        let parent = GetWindowLongPtrW(hwnd, GWLP_HWNDPARENT);
        if parent == 0 {
            return None;
        }
        let p = GetWindowLongPtrW(HWND(parent as *mut _), GWLP_USERDATA);
        if p == 0 {
            return None;
        }
        Some(&mut *(p as *mut crate::app::App))
    }
}

/// 读剪贴板 Unicode 文本
fn read_clipboard_text() -> Option<String> {
    unsafe {
        use windows::Win32::Foundation::HGLOBAL;
        use windows::Win32::System::DataExchange::{
            CloseClipboard, GetClipboardData, OpenClipboard,
        };
        use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

        const CF_UNICODETEXT: u32 = 13;
        OpenClipboard(None).ok()?;
        let result = GetClipboardData(CF_UNICODETEXT).ok().and_then(|h| {
            let hg = HGLOBAL(h.0);
            let ptr = GlobalLock(hg) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let max_units = GlobalSize(hg) / 2;
            let mut len = 0usize;
            while len < max_units && *ptr.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(hg);
            Some(s)
        });
        let _ = CloseClipboard();
        result
    }
}

/// 写 Unicode 文本到剪贴板
fn write_clipboard_text(text: &str) {
    unsafe {
        use windows::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
        };
        use windows::Win32::System::Memory::{
            GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
        };

        const CF_UNICODETEXT: u32 = 13;
        let Ok(()) = OpenClipboard(None) else {
            return;
        };
        let ok = (|| {
            use windows::Win32::Foundation::HANDLE;
            let Ok(()) = EmptyClipboard() else {
                return false;
            };
            let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = units.len() * 2;
            let hg = match GlobalAlloc(GMEM_MOVEABLE, bytes) {
                Ok(h) => h,
                Err(_) => return false,
            };
            let dst = GlobalLock(hg);
            if dst.is_null() {
                return false;
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), dst as *mut u16, units.len());
            let _ = GlobalUnlock(hg);
            SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hg.0))).is_ok()
        })();
        let _ = CloseClipboard();
        let _ = ok;
    }
}

/// 在激活缓冲上执行修改型编辑：clone 出文本 → EditState 操作 → 写回。
/// EditState 与缓冲同属 PanelInput，直借两个字段互斥，clone 解耦
fn apply_edit(input: &mut PanelInput, f: impl FnOnce(&mut EditState, &mut String)) {
    let mut t = input.active_str().to_string();
    f(&mut input.edit, &mut t);
    *input.active_buf() = t;
}

/// 高度链钉位回归：期望值由 draw_settings 的 y 累加链推导而来，
/// 布局改动须同步更新
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_view_height_pinned() {
        let mut p = Panel::new();
        p.view = PanelView::Settings;
        assert_eq!(p.view_height(0), 797);
        assert_eq!(p.view_height(1), 845);
        p.customizing_interval = true;
        assert_eq!(p.view_height(0), 835);
        assert_eq!(p.view_height(1), 883);
        p.account_error = true;
        assert_eq!(p.view_height(1), 901);
    }
}
