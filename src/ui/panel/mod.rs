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
    MonitorFromPoint, MonitorFromWindow, ValidateRect,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForMonitor;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DELETE, VK_END,
    VK_HOME, VK_LEFT, VK_RIGHT, VK_Y, VK_Z,
};
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
/// 锁定态的光标离面巡检时钟
pub(crate) const TIMER_OUTSIDE_CHECK: usize = 3;
/// 分钟级重绘心跳
const TIMER_MINUTE_TICK: usize = 4;
/// 巡检离面到自动收回的等待窗口
const OUTSIDE_HIDE_MS: u64 = 2000;

/// DPI 探测失败时的兜底值
pub(crate) const FALLBACK_DPI: f32 = 1.5;

/// 单字段输入缓冲的字节上限
const INPUT_MAX_BYTES: usize = 128;

/// 面板的展示模式：只有锁定与隐藏两态，由左键单击切换
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMode {
    Pinned,
    Hidden,
}

/// 面板当前展示的视图
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelView {
    Main,
    Settings,
    AddForm,
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

/// 一次高度过渡：起点高、目标高、计时与发起视图
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeightAnim {
    pub(crate) from: i32,
    pub(crate) to: i32,
    pub(crate) tween: anim::Tween,
    pub(crate) view: PanelView,
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

/// 输入框槽位命中 → 对应编辑字段；非输入变体显式列出，新增 Hit 变体
/// 漏登记即编译错误。眼睛（RevealKey）与「自定义」按钮不属文本区，不映射
pub(crate) fn input_field_of_hit(hit: crate::ui::panel::render::Hit) -> Option<InputField> {
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
        // 文本区之外的命中一概不映射输入字段
        Hit::Refresh
        | Hit::Settings
        | Hit::AccountSwitch
        | Hit::Retry
        | Hit::UsageInfo
        | Hit::Back
        | Hit::ClosePanel
        | Hit::IntervalPreset(_)
        | Hit::CustomizeInterval
        | Hit::ApplyInterval
        | Hit::Language(_)
        | Hit::Appearance(_)
        | Hit::ToggleAutostart
        | Hit::ToggleThreshold
        | Hit::ToggleReset5h
        | Hit::ToggleResetWeekly
        | Hit::ApplyPeak
        | Hit::AddAccount
        | Hit::RemoveAccount(_)
        | Hit::PickAccount(_)
        | Hit::AccountType(_)
        | Hit::RevealKey
        | Hit::SaveAccount
        | Hit::Platform(_)
        | Hit::ExportConfig
        | Hit::ImportConfig
        | Hit::CheckUpdate
        | Hit::OpenDownload
        | Hit::LinkRepo
        | Hit::LinkIssues
        | Hit::CopyDiagnostics
        | Hit::NewsItem(_)
        | Hit::AboutLogo => None,
    }
}

/// 激活字段的输入框几何（x、宽、右端让位）；与 settings.rs 各 input_field
/// 调用点同源，眼睛让位 26、余量收边 4。臂穷尽无兜底：新增 InputField
/// 变体即编译错误
pub(crate) fn field_geo(field: InputField) -> (f32, f32, f32) {
    let cw = PANEL_WIDTH as f32 - 2.0 * layout::CONTENT_PAD;
    match field {
        InputField::Interval => (layout::INPUT_X, 96.0, 4.0),
        InputField::Proxy => (layout::INPUT_X, cw, 4.0),
        InputField::PeakStart => (layout::PEAK_START_X, 64.0, 4.0),
        InputField::PeakEnd => (layout::PEAK_END_X, 64.0, 4.0),
        InputField::Name => (layout::INPUT_X, cw, 4.0),
        InputField::Key => (layout::INPUT_X, cw, 26.0),
        InputField::Org => (layout::INPUT_X, cw, 4.0),
        InputField::Project => (layout::INPUT_X, cw, 4.0),
    }
}

/// 单次输入布局产物：可视窗口 + 光标 x + 整串前缀宽表。同一鼠标事件内
/// 光标定位、命中换算与渲染切片共用一份，各处不再各自重建 TextLayout
#[derive(Clone)]
pub(crate) struct CaretLayout {
    /// 可视窗口起点 char 位次
    pub(crate) vis_start: usize,
    /// 光标在窗口内的 x
    pub(crate) cx: f32,
    /// 前 i 字符的累计宽（field_display 串），表长 = 字符数 + 1
    pub(crate) widths: Vec<f32>,
}

/// 光标宽表缓存条目：键 (字段, 显示串, 光标位次, 粘滞起点) + 产物
type CaretCacheEntry = (InputField, String, usize, usize, CaretLayout);

impl CaretLayout {
    /// chars[a..b] 的宽；位次自动钳入表内，越界不 panic
    pub(crate) fn seg(&self, a: usize, b: usize) -> f32 {
        let n = self.widths.len() - 1;
        self.widths[b.min(n)] - self.widths[a.min(n)]
    }
}

pub struct Panel {
    pub hwnd: Option<HWND>,
    pub mode: PanelMode,
    pub view: PanelView,
    pub(crate) scroll_dy: f32,
    pub(crate) scroll_max: f32,
    pub(crate) anchor: Option<RECT>,
    pub renderer: Option<Renderer>,
    pub pending_platform: crate::api::Platform,
    pub pending_team: bool,
    pub layout_team: bool,
    pub input: PanelInput,
    pub(crate) key_revealed: bool,
    pub customizing_interval: bool,
    pub layout_customizing: bool,
    pub(crate) caret_ctx: (bool, bool),
    pub(crate) selecting: bool,
    pub(crate) text_clicks: Option<(u32, i32, i32, u8)>,
    vis_start: std::cell::Cell<usize>,
    last_input_at: std::cell::Cell<std::time::Instant>,
    blink_drawn: std::cell::Cell<bool>,
    ime_owned: bool,
    pub(crate) dragged: bool,
    pub(crate) press_at: Option<(i32, i32)>,
    pub(crate) drag_offset: Option<(i32, i32)>,
    class_registered: bool,
    pub(crate) main_h: i32,
    pub(crate) account_error: bool,
    pub(crate) anim_period: u32,
    pub(crate) height_anim: Option<HeightAnim>,
    /// 分钟心跳当前周期：倒计时末分钟切秒拍，跨 tick 记忆避免重设。
    minute_period: std::cell::Cell<u32>,
    pub(crate) outside_since: Option<u64>,
    painted: bool,
    caret_cache: std::cell::RefCell<Option<CaretCacheEntry>>,
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
            anchor: None,
            renderer: None,
            pending_platform: crate::api::Platform::Cn,
            pending_team: false,
            layout_team: false,
            input: PanelInput::default(),
            key_revealed: false,
            customizing_interval: false,
            layout_customizing: false,
            main_h: 300,
            account_error: false,
            anim_period: 0,
            height_anim: None,
            minute_period: std::cell::Cell::new(0),
            caret_ctx: (false, false),
            selecting: false,
            text_clicks: None,
            vis_start: std::cell::Cell::new(0),
            last_input_at: std::cell::Cell::new(std::time::Instant::now()),
            blink_drawn: std::cell::Cell::new(true),
            ime_owned: false,
            dragged: false,
            press_at: None,
            drag_offset: None,
            class_registered: false,
            outside_since: None,
            painted: false,
            caret_cache: std::cell::RefCell::new(None),
            dpi: FALLBACK_DPI,
        }
    }

    /// 逻辑像素 → 物理像素
    pub(crate) fn px(&self, logical: i32) -> i32 {
        (logical as f32 * self.dpi).round() as i32
    }

    /// 面板当前视图的逻辑高度
    pub(crate) fn view_height(&self, accounts: usize) -> i32 {
        self.view_height_for(self.layout_team, self.layout_customizing, accounts)
    }

    /// 指定布局组合的逻辑高度：收缩过渡启动时按目标布局求终点
    pub(crate) fn view_height_for(&self, team: bool, customizing: bool, accounts: usize) -> i32 {
        match self.view {
            // 动态：随指标行数 / 余额 / 副标题伸缩，由 sync_main_height 维护
            PanelView::Main => self.main_h,
            PanelView::AddForm => layout::add_page_height(team),
            // 整页高度由 layout 的段常量链求和，与 draw_settings 的 y 推进链同源
            PanelView::Settings => {
                layout::settings_view_height(accounts > 0, self.account_error, customizing)
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
            // 拖动进行中（drag_offset）与拖过（dragged）都保持当前位置；
            // 首开前隐藏窗口悬在默认位（主屏），托盘在副屏时按窗口取屏
            // 首帧会用错 DPI 与工作区，非拖动态一律按锚点定屏
            let moved = self.dragged || self.drag_offset.is_some();
            let monitor = if !moved && let Some(a) = self.anchor {
                MonitorFromPoint(
                    POINT {
                        x: (a.left + a.right) / 2,
                        y: a.top,
                    },
                    MONITOR_DEFAULTTONEAREST,
                )
            } else {
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)
            };
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut mi);
            self.dpi = dpi_of(monitor).unwrap_or(FALLBACK_DPI);

            // 高度过渡进行中窗口几何归动画帧独占，外部重排让位；
            // 跨视图过渡已被 relayout 终止，拖动中无动画不判。
            if !show
                && self.drag_offset.is_none()
                && self.height_anim.is_some_and(|a| a.view == self.view)
            {
                return;
            }

            let w = self.px(PANEL_WIDTH);
            // 当前窗口矩形一次取回：拖动定位、动画起点与跳变判定共用
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let cur_h = wr.bottom - wr.top;
            // 拖过（含拖动中）保持完整高、允许遮住任务栏——移动与重排交替
            // 改高会闪跳；锚点弹出仍以工作区为界压扁，防矮屏出屏
            let target_h = if moved {
                self.px(logical_h)
            } else {
                let max_h = (mi.rcWork.bottom - mi.rcWork.top - 16).max(self.px(200));
                self.px(logical_h).min(max_h)
            };
            // 展开走过渡动画渐扩揭露；收缩由切换点收窗渐裁不经此路。
            // 弹出、拖动中与减动效直接落位；拖过照常过渡，帧保持位置。
            let animate = !show
                && self.drag_offset.is_none()
                && cur_h < target_h
                && IsWindowVisible(hwnd).as_bool()
                && anim::animations_allowed();
            if animate {
                self.height_anim = Some(HeightAnim {
                    from: cur_h,
                    to: target_h,
                    tween: anim::Tween::now(150),
                    view: self.view,
                });
                let _ = SetTimer(Some(hwnd), TIMER_ANIM, 16, None);
                self.anim_period = 16;
            } else {
                self.height_anim = None;
            }
            let h = if animate { cur_h } else { target_h };
            let (x, y) = if moved {
                (wr.left, wr.top)
            } else if let Some(anchor) = self.anchor {
                let ax = (anchor.left + anchor.right) / 2;
                let mut x = ax - w / 2;
                let mut y = anchor.top - h - self.px(8);
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
            // 落位带 SWP_NOCOPYBITS：CopyBits 先写屏再被重绘覆盖，两次
            // 写屏错位即闪影；拖动平移正需内容跟随，例外不带。
            let flags = if show {
                SWP_SHOWWINDOW | SWP_NOCOPYBITS
            } else if moved {
                SWP_NOACTIVATE
            } else {
                SWP_NOACTIVATE | SWP_NOCOPYBITS
            };
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, flags);
            // 可见高按屏幕内真实视口计，越出屏幕底的部分不算；展开动画
            // 首帧停旧位，顶边须按目标高反推终态，否则估小放出假滚动。
            let final_y = if moved {
                y
            } else if let Some(anchor) = self.anchor {
                (anchor.top - target_h - self.px(8)).max(mi.rcWork.top + 8)
            } else {
                y
            };
            let visible = target_h.min((mi.rcMonitor.bottom - final_y).max(0));
            self.refresh_scroll(logical_h, visible);
        }
    }

    /// 目标更矮且动效可用时启动收缩过渡：布局保持出发态、窗口先收
    /// 渐裁，结束或打断由调用方追平。`logical_h` 接逻辑高度。
    pub(crate) fn begin_shrink_anim(&mut self, hwnd: HWND, logical_h: i32) -> bool {
        unsafe {
            // 拖动进行中不启动：拖动帧即时落位，与过渡帧互相打架；
            // 拖过（非拖动中）照常过渡，动画帧保持当前位置
            if self.drag_offset.is_some() {
                return false;
            }
            // 拖动态与 place 的 moved 分支同款完整高；锚点弹出以工作区
            // 为界压扁，防矮屏出屏
            let target_h = if self.dragged {
                self.px(logical_h)
            } else {
                let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let _ = GetMonitorInfoW(monitor, &mut mi);
                let max_h = (mi.rcWork.bottom - mi.rcWork.top - 16).max(self.px(200));
                self.px(logical_h).min(max_h)
            };
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let cur_h = wr.bottom - wr.top;
            if target_h >= cur_h || !IsWindowVisible(hwnd).as_bool() || !anim::animations_allowed()
            {
                return false;
            }
            self.height_anim = Some(HeightAnim {
                from: cur_h,
                to: target_h,
                tween: anim::Tween::now(150),
                view: self.view,
            });
            let _ = SetTimer(Some(hwnd), TIMER_ANIM, 16, None);
            self.anim_period = 16;
            true
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
        // 新的开合周期：首帧淡入资格重置
        self.painted = false;
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
            self.minute_period.set(60_000);
            let _ = InvalidateRect(Some(hwnd), None, true);
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
        self.key_revealed = false;
        self.outside_since = None;
        self.drag_offset = None;
        self.selecting = false;
        self.press_at = None;
        self.text_clicks = None;
        self.clear_input(hwnd);
        // 直接隐藏：自绘收缩/淡出会与 DWM 过渡叠加闪烁
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            let _ = KillTimer(Some(hwnd), TIMER_OUTSIDE_CHECK);
            let _ = KillTimer(Some(hwnd), TIMER_ANIM);
            let _ = KillTimer(Some(hwnd), TIMER_MINUTE_TICK);
            self.height_anim = None;
            // 收起即追平：下次开门的高度与行集按所选值走，不携带冻结残留
            self.layout_team = self.pending_team;
            self.layout_customizing = self.customizing_interval;
            self.minute_period.set(0);
        }
        // 动画期间的 alpha 档位画刷随收起清空，缓存不跨开合累积；
        // 悬停高亮与命中区一并清，重开首帧不带旧状态
        if let Some(r) = self.renderer.as_mut() {
            r.hover = None;
            r.hits.clear();
            r.clear_brush_cache();
        }
        // 收起后到下次打开前不再绘制，归还工作集把静止内存压回托盘档；
        // 重开时的软缺页按次一次性发生，换常驻低占用
        crate::platform::trim_working_set();
    }

    /// 左键：隐藏 → 锁定；已锁定 → 收起
    pub fn toggle_pin(&mut self, parent: HWND, anchor: RECT, accounts: usize) {
        match self.mode {
            PanelMode::Pinned => {
                if let Some(h) = self.hwnd {
                    self.begin_hide(h);
                }
            }
            PanelMode::Hidden => {
                // 从隐藏重开才回主视图，不停留在上次滚动的旧设置页
                self.reset_to_main();
                self.show_at(parent, anchor, accounts);
                // 置 Pinned 须在窗口就位后：建窗失败保持 Hidden，状态机可重试
                if let Some(h) = self.hwnd {
                    self.mode = PanelMode::Pinned;
                    // 显示后立即激活——后台窗口拿不到键盘焦点，IME 异常
                    unsafe {
                        let _ = SetForegroundWindow(h);
                    }
                }
            }
        }
    }

    /// 打开面板前的复位：回主视图、清滚动偏移、添加表单与输入临时态；
    /// 左键从隐藏重开时调用，防收起后重开停在上次滚动的旧设置页。
    /// 间隔行展开态不清：view_height 有配套 +38 钩子，展开态须跨开合保留
    fn reset_to_main(&mut self) {
        self.view = PanelView::Main;
        self.scroll_dy = 0.0;
        self.key_revealed = false;
        self.input.interval.clear();
        self.input.proxy.clear();
        self.input.peak_start.clear();
        self.input.peak_end.clear();
        if let Some(h) = self.hwnd {
            self.clear_input(h);
        } else {
            self.input.field = None;
        }
    }

    /// 退出输入态，摘除 IME 上下文。
    pub(crate) fn clear_input(&mut self, hwnd: HWND) {
        self.input.field = None;
        self.input.edit.reset();
        self.input.surrogate = None;
        self.vis_start.set(0);
        unsafe {
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

    /// 聚焦输入字段：进入输入态，IME 组合窗跟随光标。
    /// 切换字段重置编辑状态，撤销栈跨字段会污染新缓冲；
    /// 重复聚焦保留光标位置，点击定位不被弹回末尾。
    pub fn focus_input(&mut self, hwnd: HWND, field: InputField) {
        let switched = self.input.field != Some(field);
        if switched {
            // 切换即整窗重绘：清旧字段的激活边框与选区像素，首聚焦的
            // 光标也靠这次重绘现身。
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            self.input.edit.reset();
            // 陈旧高代理半区跨字段残留会与低半区错拼增补平面字符
            self.input.surrogate = None;
            self.vis_start.set(0);
        }
        self.input.field = Some(field);
        if switched {
            let buf = self.input.active_str().to_string();
            self.input.edit.caret_to_end(&buf);
        }
        self.note_caret_interaction();
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hwnd));
            // 动画时钟保活：光标闪烁的翻转重绘由 TIMER_ANIM 驱动。
            let _ = SetTimer(Some(hwnd), TIMER_ANIM, 16, None);
            self.anim_period = 16;
            // 布局只算一次：光标 x 与 IME 组合窗定位共用同一产物；
            // renderer 缺席即首帧前时以 0 兜底，下次 update_caret 补正。
            let cx = if let Some(r) = self.renderer.as_ref() {
                let cx = self.caret_layout(r).cx;
                self.sync_ime_pos(hwnd, cx);
                cx
            } else {
                0.0
            };
            if switched {
                self.attach_ime(hwnd, cx);
            }
        }
    }

    /// 挂 IME 上下文并把组合窗定位到光标处；cx 由调用点的布局产物传入
    unsafe fn attach_ime(&mut self, hwnd: HWND, cx: f32) {
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
            self.sync_ime_pos(hwnd, cx);
        }
    }

    /// 组合窗跟随光标移动；光标每次移动后调用，中文组合串贴住光标处。
    /// cx 由调用点随同一布局传入，不再独立重算
    unsafe fn sync_ime_pos(&self, hwnd: HWND, cx: f32) {
        unsafe {
            use windows::Win32::Foundation::POINT;
            use windows::Win32::UI::Input::Ime::{
                CFS_POINT, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext,
                ImmSetCompositionWindow,
            };
            let Some(field) = self.input.field else {
                return;
            };
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

    /// 光标可视布局：一次 TextLayout 建成前缀宽表，光标定位、命中
    /// 换算与渲染切片全查此表，绘制与 IME 定位共用，位置恒一致。
    /// 未超宽从 0 起；超宽沿用上次起点保持粘滞，防点击处窗口回跳，
    /// 光标出窗才二分调整。Key 掩码态圆点与字符一一对应，位次空间不变。
    pub(crate) fn caret_layout(&self, renderer: &Renderer) -> CaretLayout {
        let Some(field) = self.input.field else {
            return CaretLayout {
                vis_start: 0,
                cx: 0.0,
                widths: vec![0.0],
            };
        };
        let buf = self.field_display();
        let caret = self.input.edit.caret;
        let sticky = self.vis_start.get();
        if let Some((f, b, c, s, cl)) = self.caret_cache.borrow().as_ref()
            && *f == field
            && b == &buf
            && *c == caret
            && *s == sticky
        {
            return cl.clone();
        }
        let cl = self.compute_caret_layout(field, &buf, renderer);
        *self.caret_cache.borrow_mut() = Some((field, buf, caret, cl.vis_start, cl.clone()));
        cl
    }

    /// 宽表实算：缓存未命中时跑一次完整布局
    fn compute_caret_layout(
        &self,
        field: InputField,
        buf: &str,
        renderer: &Renderer,
    ) -> CaretLayout {
        let chars: Vec<char> = buf.chars().collect();
        let caret = self.input.edit.caret.min(chars.len());
        let (_bx, w, tail) = field_geo(field);
        let avail = (w - 6.0 - tail).max(1.0);
        let widths = unsafe { renderer.prefix_widths(&chars, 12.0, 400, true) };
        let full = widths[chars.len()];
        if full <= avail {
            self.vis_start.set(0);
            return CaretLayout {
                vis_start: 0,
                cx: widths[caret],
                widths,
            };
        }
        let sticky = self.vis_start.get().min(caret);
        if widths[caret] - widths[sticky] <= avail {
            self.vis_start.set(sticky);
            return CaretLayout {
                vis_start: sticky,
                cx: widths[caret] - widths[sticky],
                widths,
            };
        }
        // 粘滞起点装不下光标才右移；宽度随起点单调不增，二分最小满足者
        let mut lo = sticky;
        let mut hi = caret;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if widths[caret] - widths[mid] <= avail {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        self.vis_start.set(lo);
        CaretLayout {
            vis_start: lo,
            cx: widths[caret] - widths[lo],
            widths,
        }
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
    /// IME 组合窗与文本区命中判定共用。
    fn caret_line_y(&self, field: InputField) -> f32 {
        let y = match field {
            InputField::Interval => {
                let (has_account, auth_error) = self.caret_ctx;
                layout::interval_input_y(has_account, auth_error)
            }
            InputField::Proxy => {
                let (has_account, auth_error) = self.caret_ctx;
                layout::proxy_input_y(has_account, auth_error, self.layout_customizing)
            }
            InputField::PeakStart | InputField::PeakEnd => {
                let (has_account, auth_error) = self.caret_ctx;
                layout::peak_input_y(has_account, auth_error, self.layout_customizing)
            }
            InputField::Name => layout::ADD_NAME_Y,
            InputField::Key => layout::ADD_KEY_Y,
            InputField::Org => layout::ADD_ORG_Y,
            InputField::Project => layout::ADD_PROJECT_Y,
        };
        // 内容上滚后锚点同步上移，IME 组合窗贴住可视位置。
        y + layout::CARET_Y_OFFSET - self.scroll_dy
    }

    /// 文本区内 x（逻辑像素）→ char 位次：与 caret_layout 同一可视窗口，
    /// 取最近字符边界（点击定位光标与拖选用）；前缀宽全部查传入的宽表，
    /// 零布局创建
    pub(crate) fn caret_hit_test(&self, cl: &CaretLayout, field: InputField, x: f32) -> usize {
        let (bx, _w, _tail) = field_geo(field);
        let base = bx + 6.0;
        // 前缀宽度随位次单调不减[caret_layout 二分所依赖的同一性质]：
        // 最近边界只可能是最后一个宽度 <= x 的位次或其下一格，二分定位即可
        let target = x - base;
        let n = cl.widths.len() - 1;
        let vis_start = cl.vis_start;
        let mut lo = vis_start;
        let mut hi = n;
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if cl.seg(vis_start, mid) <= target {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        // 平局取较小位次；零宽字符等宽块内取末位[原线性取首位]，
        // 光标 x 相同仅退格/删词的语义位置不同，可视等价
        if lo < n
            && (cl.seg(vis_start, lo + 1) - target).abs() < (target - cl.seg(vis_start, lo)).abs()
        {
            lo + 1
        } else {
            lo
        }
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

    /// 按已算好的光标 x 挪 IME 组合窗，供同一事件内已持布局产物的
    /// 调用点复用。
    fn place_caret(&self, hwnd: HWND, cx: f32) {
        unsafe {
            self.sync_ime_pos(hwnd, cx);
        }
    }

    /// 光标闪烁相位：由上次交互时刻起算，亮灭各半秒取模判定，
    /// 交互归零光标立现。返回当前可见性与距下次翻转的毫秒数。
    pub(crate) fn caret_blink(&self) -> (bool, u32) {
        Self::blink_phase(self.last_input_at.get().elapsed().as_millis())
    }

    /// 相位判定与时间源解耦，供测试钉位。
    fn blink_phase(elapsed_ms: u128) -> (bool, u32) {
        const ON: u128 = 500;
        const OFF: u128 = 500;
        let total = ON + OFF;
        let t = elapsed_ms % total;
        if t < ON {
            (true, (ON - t) as u32)
        } else {
            (false, (total - t) as u32)
        }
    }

    /// 点击、键入、光标移动、聚焦统一重置闪烁相位。
    fn note_caret_interaction(&self) {
        self.last_input_at.set(std::time::Instant::now());
        // 相位归零必可见，与下一帧绘制一致，翻转检测不会误判多绘一帧。
        self.blink_drawn.set(true);
    }

    /// 按字段内容计算光标位置并挪 IME 组合窗，与 input_field 绘制共用
    /// 同一布局产物；同时重置闪烁相位，各调用点即光标与文本变化点。
    /// renderer 缺席即首帧前时跳过定位，待下次调用补。
    pub fn update_caret(&self, hwnd: HWND, renderer: Option<&Renderer>) {
        if self.input.field.is_none() {
            return;
        }
        self.note_caret_interaction();
        let Some(r) = renderer else {
            return;
        };
        let cx = self.caret_layout(r).cx;
        self.place_caret(hwnd, cx);
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

pub(crate) unsafe fn start_anim(hwnd: HWND) {
    unsafe {
        SetTimer(Some(hwnd), TIMER_ANIM, 16, None);
    }
    if let Some(app) = app_from_tray(hwnd) {
        app.panel.anim_period = 16;
    }
}

/// 分钟心跳随最近重置时刻变拍：末分钟切秒拍让读数逐秒走，重置后
/// 回分钟拍；周期变化才重设。
pub(crate) fn retune_minute(app: &crate::app::App, hwnd: HWND) {
    let now = chrono::Utc::now();
    let soonest = app.data.snapshot.as_ref().and_then(|s| {
        [s.five_hour.as_ref(), s.weekly.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|b| b.resets_at)
            .filter(|t| *t > now)
            .min()
    });
    let want = minute_period(soonest, now);
    if app.panel.minute_period.get() != want {
        unsafe {
            let _ = SetTimer(Some(hwnd), TIMER_MINUTE_TICK, want, None);
        }
        app.panel.minute_period.set(want);
    }
}

/// 周期判定与时间源解耦，供测试钉位。切分点与 countdown 显示切分
/// 对齐：剩余整 60 秒仍显示「N 分」走分拍，59 秒起切秒拍。
fn minute_period(
    soonest: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> u32 {
    if soonest.is_some_and(|t| t > now && (t - now).num_seconds() < 60) {
        1000
    } else {
        60_000
    }
}

/// 离面巡检的单步判定：present 为四路保活（在面板/弹窗、输入中、
/// 托盘锚区、拖动中）任一命中。在场即清计时；离面从首拍起算，超时
/// 返回 true（应收起）并清计时。纯函数，Win32 侧只负责采集布尔与时钟
fn outside_step(present: bool, now: u64, since: &mut Option<u64>) -> bool {
    if present {
        *since = None;
        return false;
    }
    since.get_or_insert(now);
    if now - since.unwrap() > OUTSIDE_HIDE_MS {
        *since = None;
        true
    } else {
        false
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
                // 不走 BeginPaint：验证客户区后直接渲染。失效一律整窗、不做
                // 局部矩形收窄：HwndRenderTarget 呈现整窗后台缓冲，收窄省不
                // 下绘制，编辑路径同样整窗 InvalidateRect
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
                                crate::ui::panel::theme::resolved(
                                    app.config.general.appearance.as_deref(),
                                ),
                            );
                            // 进程首开时 renderer 到 paint 才建，show_at 的淡入
                            // 设不上；且 TIMER_ANIM 先于 WM_PAINT 派发，空转一
                            // 拍即自灭——此处补回淡入并重挂时钟。设备丢失重建
                            // 走的也是 fresh，但内容本就在屏，凭 painted 排除
                            if !app.panel.painted {
                                r.anim.appear = Some(anim::Tween::now(180));
                                start_anim(hwnd);
                            }
                        }
                        let model = PanelModel::from_app(app);
                        let view = app.panel.view;
                        let dpi = app.panel.dpi;
                        keep = r.paint(hwnd, &rect, &app.panel, &model, view, dpi);
                        if keep {
                            app.panel.painted = true;
                        }
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
                    // 心跳推进全部相对时间文案：页脚[X 分钟前]在底部、指标行
                    // 重置倒计时在内容区中部，带状失效会让倒计时冻结；且
                    // WM_PAINT 本就整窗重绘，收窄失效矩形省不下绘制开销
                    TIMER_MINUTE_TICK => {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        if let Some(app) = app_from_tray(hwnd) {
                            retune_minute(app, hwnd);
                        }
                        LRESULT(0)
                    }
                    TIMER_OUTSIDE_CHECK => {
                        let app = app_from_tray(hwnd);
                        if let Some(app) = app {
                            // 先判模式再重挂：收起瞬间残留的一拍不再自我续命，
                            // 隐藏期巡检彻底停摆
                            if app.panel.mode == PanelMode::Pinned {
                                SetTimer(Some(hwnd), TIMER_OUTSIDE_CHECK, 200, None);
                                // 不能调 Shell_NotifyIconGetRect——跨进程同步调用，
                                // 高频轮询互锁卡死
                                let mut pt = POINT::default();
                                let _ = GetCursorPos(&mut pt);
                                let w = WindowFromPoint(pt);
                                // 子控件同样算在面板内；账号弹窗是面板的延伸，光标
                                // 移入其中同样不能触发面板收起
                                let in_popup = app
                                    .popup
                                    .wnd
                                    .hwnd
                                    .is_some_and(|ph| w == ph || GetAncestor(w, GA_ROOT) == ph);
                                let in_panel =
                                    in_popup || w == hwnd || GetAncestor(w, GA_ROOT) == hwnd;
                                // 正在输入则绝不收起。仅认输入态：开门即抢前台使
                                // GetFocus 恒真，会把「离面超时自动收」整个废掉
                                let typing = app.panel.input.field.is_some();
                                // 鼠标在托盘图标上
                                let near_tray = app.panel.cursor_near_anchor();
                                // 拖动中视为在场：窗口被钳在工作区边缘时光标
                                // 可能已滑出面板外，按住不放不该被收起
                                let dragging = app.panel.drag_offset.is_some();
                                let now =
                                    windows::Win32::System::SystemInformation::GetTickCount64();
                                let present = in_panel || typing || near_tray || dragging;
                                if outside_step(present, now, &mut app.panel.outside_since) {
                                    app.panel.begin_hide(hwnd);
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
                // 拖动分支走 as_mut 的 reborrow 后提前返回；此处 move 同一
                // 绑定——再取一次 app_from_tray 会构造两个活跃可变别名
                if let Some(app) = app {
                    // 拖选中：光标随鼠标指针推进扩选区。宽表查缓存命中（缓冲
                    // 与粘滞起点均不变），新光标 x 直接查表，不再二次布局
                    if app.panel.selecting {
                        let x = x_of(lparam) / app.panel.dpi;
                        if let (Some(f), Some(r)) =
                            (app.panel.input.field, app.panel.renderer.as_ref())
                        {
                            let cl = app.panel.caret_layout(r);
                            let pos = app.panel.caret_hit_test(&cl, f, x);
                            if pos != app.panel.input.edit.caret {
                                app.panel.input.edit.place(pos, true);
                                // 光标随指针推进也是交互，相位归零立现。
                                app.panel.note_caret_interaction();
                                // 窗内查表定位；越出可视窗（捕获使框外坐标可达）
                                // 须完整重排让窗口滚动，否则光标画出框外不自愈
                                let (_bx, fw, tail) = field_geo(f);
                                let avail = (fw - 6.0 - tail).max(1.0);
                                let cx = cl.seg(cl.vis_start, pos);
                                if cx <= avail {
                                    app.panel.place_caret(hwnd, cx);
                                } else {
                                    app.panel.update_caret(hwnd, app.panel.renderer.as_ref());
                                }
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        }
                        return LRESULT(0);
                    }
                    // TME_LEAVE 一次性且会被鼠标捕获打断，每次移动重挂；
                    // 真正离开时由 MOUSELEAVE 清悬停高亮
                    track_leave(hwnd);
                    // 在场即时重置离面计时：巡检 200ms 一拍，两拍之间的短暂
                    // 回访不该被算进离面时间
                    app.panel.outside_since = None;
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
                    // 校正过渡态：TME 触发瞬间光标可能恰在子控件上，误清会闪
                    let still_here = w == hwnd || GetAncestor(w, GA_ROOT) == hwnd;
                    if still_here {
                        // 误触发，重挂跟踪等真正的离开
                        track_leave(hwnd);
                    } else if let Some(r) = app.panel.renderer.as_mut()
                        && r.hover.is_some()
                    {
                        r.hover = None;
                        let _ = InvalidateRect(Some(hwnd), None, false);
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
                        // 焦点在输入框上时，IME 组合窗随内容一起平移。
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
                            // 模态命中统一拦截直调两段式并提前返回：对话框
                            // 嵌套泵会派发其他消息重取 App，泵后本臂持有的
                            // 引用已失效，不得再触碰，下方 clear_input 同样
                            // 不可用。输入态不跨弹框——弹框抢焦点即
                            // KILLFOCUS 清输入，缓冲文本保留与旧路径一致。
                            if hit.is_modal() {
                                crate::app::dispatch_modal(hit, hwnd);
                                return LRESULT(0);
                            }
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
                        let _ = InvalidateRect(Some(hwnd), None, false);
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
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                let app = app_from_tray(hwnd);
                if let Some(app) = app
                    // 焦点真已转走才清；若焦点绕回本窗口则保留
                    && windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() != hwnd
                {
                    // 明文查看随失焦收回，不给旁人余光；连击链一并重置：
                    // 点框→失焦→点回不该被计为双击
                    let was_revealed = app.panel.key_revealed;
                    app.panel.key_revealed = false;
                    app.panel.text_clicks = None;
                    // 失焦不清输入会让面板被当作「正在输入」永不收起，IME 组合窗也会游离
                    let was_typing = app.panel.input.field.is_some();
                    if was_typing {
                        app.panel.clear_input(hwnd);
                    }
                    if was_revealed || was_typing {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
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
                        // 命中换算与光标定位共用一次宽表布局
                        let cl = app.panel.caret_layout(r);
                        let pos = app.panel.caret_hit_test(&cl, f, x);
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
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        let _ = SetCapture(hwnd);
                        return LRESULT(0);
                    }
                    if hit_none {
                        let mut wr = RECT::default();
                        let _ = GetWindowRect(hwnd, &mut wr);
                        // 拖动接管位置，进行中的过渡让位并追平、调和焦点；
                        // 平移靠 CopyBits 不重绘，须显式失效防停在出发布局。
                        // 追平即落位：原地松开不走拖动路径，窗口会卡在
                        // 动画中间高度。
                        app.panel.height_anim = None;
                        app.panel.layout_team = app.panel.pending_team;
                        app.panel.layout_customizing = app.panel.customizing_interval;
                        crate::app::reconcile_fading_focus(app, hwnd);
                        let n = app.config.accounts.len();
                        let logical_h = app.panel.view_height(n);
                        app.panel.place(hwnd, logical_h, false);
                        let _ = InvalidateRect(Some(hwnd), None, false);
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
                    app.panel.height_anim = None;
                    app.panel.layout_team = app.panel.pending_team;
                    app.panel.layout_customizing = app.panel.customizing_interval;
                    crate::app::reconcile_fading_focus(app, hwnd);
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
        let mut footer_only = false;
        let mut repaint = false;
        if let Some(r) = app.panel.renderer.as_mut() {
            let appear_active = r.anim.appear.is_some();
            if let Some(t) = &r.anim.appear {
                if t.finished() {
                    r.anim.appear = None;
                    // 结束帧仍须重绘，画终态清掉中间态残影。
                    repaint = true;
                } else {
                    done = false;
                    repaint = true;
                }
            }
            let footer_active = r.anim.footer.as_ref().is_some_and(|f| !f.tween.finished());
            if let Some(f) = &r.anim.footer {
                if f.tween.finished() {
                    r.anim.footer = None;
                    // 结束帧画静态页脚，清掉滑动中间态。
                    repaint = true;
                } else {
                    done = false;
                    repaint = true;
                }
            }
            let spin_active = r.spin_remaining();
            if spin_active {
                done = false;
                repaint = true;
            }
            // 页脚换装独占时帧率减半：文字滑动 30fps 足够，整窗重绘的
            // CPU 砍半；淡入/旋转在场维持满帧
            footer_only = footer_active && !appear_active && !spin_active;
        }
        // 高度过渡：逐帧插值，锚定与夹取同 place，结束帧即终值；
        // NOCOPYBITS 防每帧 CopyBits 双写屏闪影，展开缓出、收缩缓入。
        if let Some(a) = app.panel.height_anim {
            done = false;
            footer_only = false;
            let p = if a.to > a.from {
                anim::ease_out_cubic(a.tween.progress())
            } else {
                anim::ease_in_cubic(a.tween.progress())
            };
            let h = a.from + ((a.to - a.from) as f32 * p).round() as i32;
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            // 拖动态保持当前位置只改高度；锚点弹出按托盘锚定、顶边夹取
            // 与 place 同款
            if app.panel.dragged || app.panel.drag_offset.is_some() {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    wr.left,
                    wr.top,
                    wr.right - wr.left,
                    h,
                    SWP_NOACTIVATE | SWP_NOCOPYBITS,
                );
                repaint = true;
            } else if let Some(anchor) = app.panel.anchor {
                let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let _ = GetMonitorInfoW(monitor, &mut mi);
                let y = (anchor.top - h - app.panel.px(8)).max(mi.rcWork.top + 8);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    wr.left,
                    y,
                    wr.right - wr.left,
                    h,
                    SWP_NOACTIVATE | SWP_NOCOPYBITS,
                );
                repaint = true;
            }
            if a.tween.finished() {
                app.panel.height_anim = None;
                // 收缩到位：生效布局追平所选值。窗口已在终点高度，
                // 重排直接落位不产生二次动画
                if app.panel.layout_team != app.panel.pending_team
                    || app.panel.layout_customizing != app.panel.customizing_interval
                {
                    app.panel.layout_team = app.panel.pending_team;
                    app.panel.layout_customizing = app.panel.customizing_interval;
                    crate::app::reconcile_fading_focus(app, hwnd);
                    crate::app::relayout_panel(app, hwnd);
                }
            }
        }
        let anim_active = !done;
        // 输入态光标闪烁：时钟保活，仅相位跨过翻转点才整窗重绘；
        // 焦点与输入态双门控，失焦不闪。
        let mut caret_wake: Option<u32> = None;
        let caret_active = app.panel.input.field.is_some()
            && windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == hwnd;
        if caret_active {
            done = false;
            let (on, wake_in) = app.panel.caret_blink();
            if on != app.panel.blink_drawn.get() {
                app.panel.blink_drawn.set(on);
                repaint = true;
            }
            caret_wake = Some(wake_in);
        }
        if repaint {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        if done {
            let _ = KillTimer(Some(hwnd), TIMER_ANIM);
            app.panel.anim_period = 0;
        } else {
            // 仅光标闪烁在场时一次定到翻转点，空闲期零空转；
            // 其余动画按原有帧率。
            let want = if footer_only {
                33
            } else if anim_active {
                16
            } else {
                caret_wake.unwrap_or(16).max(1)
            };
            if app.panel.anim_period != want {
                // 仅周期变化时重设：SetTimer 会重置计时起点
                SetTimer(Some(hwnd), TIMER_ANIM, want, None);
                app.panel.anim_period = want;
            }
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

/// 写 Unicode 文本到剪贴板；全链成功才返回 true，调用方据此反馈
pub(crate) fn write_clipboard_text(text: &str) -> bool {
    unsafe {
        use windows::Win32::Foundation::GlobalFree;
        use windows::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
        };
        use windows::Win32::System::Memory::{
            GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
        };

        const CF_UNICODETEXT: u32 = 13;
        let Ok(()) = OpenClipboard(None) else {
            crate::platform::log("[Quotify] 剪贴板打开失败");
            return false;
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
                // 所有权未转移给系统，须自释放防泄漏
                let _ = GlobalFree(Some(hg));
                return false;
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), dst as *mut u16, units.len());
            let _ = GlobalUnlock(hg);
            if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hg.0))).is_err() {
                // 同上：系统未接管句柄
                let _ = GlobalFree(Some(hg));
                return false;
            }
            true
        })();
        let _ = CloseClipboard();
        if !ok {
            crate::platform::log("[Quotify] 剪贴板写入失败");
        }
        ok
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
        p.layout_customizing = true;
        assert_eq!(p.view_height(0), 835);
        assert_eq!(p.view_height(1), 883);
        p.account_error = true;
        assert_eq!(p.view_height(1), 901);
    }

    /// 光标点击定位：等宽最近边界、越界钳入、零宽平局取末位、
    /// 可视窗口偏移下返回绝对位次
    #[test]
    fn caret_hit_positions() {
        let p = Panel::new();
        let field = InputField::Name;
        let base = field_geo(field).0 + 6.0;
        let cl = |widths: &[f32], vis_start: usize| CaretLayout {
            vis_start,
            cx: 0.0,
            widths: widths.to_vec(),
        };
        // 等宽表 [0,5,10,15,20]：点击两格之间取近者，等距取小位次
        let w = cl(&[0.0, 5.0, 10.0, 15.0, 20.0], 0);
        assert_eq!(p.caret_hit_test(&w, field, base + 2.0), 0);
        assert_eq!(p.caret_hit_test(&w, field, base + 7.6), 2);
        assert_eq!(p.caret_hit_test(&w, field, base + 7.4), 1);
        // 框内左右极端钳入首尾
        assert_eq!(p.caret_hit_test(&w, field, base - 100.0), 0);
        assert_eq!(p.caret_hit_test(&w, field, base + 1000.0), 4);
        // 零宽字符（位次 1→2 同宽）：点击该宽处取末位
        let zw = cl(&[0.0, 5.0, 5.0, 10.0], 0);
        assert_eq!(p.caret_hit_test(&zw, field, base + 5.0), 2);
        // 可视窗口起点偏移：返回绝对位次而非窗口内位次
        let vs = cl(&[0.0, 5.0, 10.0, 15.0, 20.0], 1);
        assert_eq!(p.caret_hit_test(&vs, field, base + 5.0), 2);
    }

    /// 光标闪烁相位：交互归零立现，亮灭各半秒，翻转剩余毫秒递减，
    /// 周期循环回开。
    #[test]
    fn caret_blink_phase() {
        assert_eq!(Panel::blink_phase(0), (true, 500));
        assert_eq!(Panel::blink_phase(499), (true, 1));
        assert_eq!(Panel::blink_phase(500), (false, 500));
        assert_eq!(Panel::blink_phase(999), (false, 1));
        assert_eq!(Panel::blink_phase(1000), (true, 500));
        assert_eq!(Panel::blink_phase(1499), (true, 1));
        assert_eq!(Panel::blink_phase(1500), (false, 500));
    }

    /// 心跳变拍切分：整 60 秒走分拍、59 秒切秒拍、过期与缺失走分拍。
    #[test]
    fn minute_period_boundaries() {
        use chrono::TimeZone;
        let now = chrono::Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap();
        let at = |s: i64| now + chrono::Duration::seconds(s);
        assert_eq!(minute_period(None, now), 60_000);
        assert_eq!(minute_period(Some(at(60)), now), 60_000);
        assert_eq!(minute_period(Some(at(59)), now), 1000);
        assert_eq!(minute_period(Some(at(1)), now), 1000);
        // 过期时刻走分拍，等新数据换新时刻
        assert_eq!(minute_period(Some(at(-5)), now), 60_000);
    }

    /// 巡检判定：在场清计时、离面首拍起算、严格大于才收、收后重开
    /// 重新起算（不沿用旧时间戳）
    #[test]
    fn outside_step_timing() {
        let mut since = None;
        // 在场：永不收起且清计时
        assert!(!outside_step(true, 100, &mut since));
        assert_eq!(since, None);
        // 离面首拍：起算不收
        assert!(!outside_step(false, 200, &mut since));
        assert_eq!(since, Some(200));
        // 恰到 2000ms：严格大于才收
        assert!(!outside_step(false, 2200, &mut since));
        // 超时：收起并清计时
        assert!(outside_step(false, 2201, &mut since));
        assert_eq!(since, None);
        // 中途回访清零后，再次离面从新时刻起算
        assert!(!outside_step(true, 3000, &mut since));
        assert!(!outside_step(false, 3100, &mut since));
        assert_eq!(since, Some(3100));
        assert!(!outside_step(false, 5100, &mut since));
        assert!(outside_step(false, 5101, &mut since));
    }
}
