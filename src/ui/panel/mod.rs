//! 弹出面板

pub mod anim;
pub mod layout;
pub mod model;
pub mod render;
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
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use crate::platform::wide;
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::render::Renderer;
use crate::ui::panel::theme::PANEL_WIDTH;

const PANEL_WND_CLASS: &str = "QuotifyPanelWnd";

const TIMER_ANIM: usize = 1;
const TIMER_CLOSE_DEBOUNCE: usize = 2;
pub(crate) const TIMER_OUTSIDE_CHECK: usize = 3;

/// DPI 探测失败时的兜底值
pub(crate) const FALLBACK_DPI: f32 = 1.5;

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
    AccountPicker,
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
    pub interval: String,
    pub proxy: String,
    pub peak_start: String,
    pub peak_end: String,
    pub name: String,
    pub key: String,
    pub org: String,
    pub project: String,
}

pub struct Panel {
    pub hwnd: Option<HWND>,
    pub mode: PanelMode,
    pub view: PanelView,
    pub hovered: bool,
    pub(crate) anchor: Option<RECT>,
    pub renderer: Option<Renderer>,
    pub adding_account: bool,
    pub pending_platform: crate::api::client::Platform,
    pub pending_team: bool,
    pub input: PanelInput,
    pub customizing_interval: bool,
    pub(crate) anim_x: i32,
    pub(crate) anim_w: i32,
    pub(crate) anim_full_h: i32,
    pub(crate) anim_bottom: i32,
    pub(crate) caret_ctx: (bool, bool),
    /// 用户拖动过窗口；重开面板前 place 保持拖后位置而非锚点
    pub(crate) dragged: bool,
    /// 本次按下的屏幕坐标；松手时位移小于阈值视为点击而非拖动
    pub(crate) press_at: Option<(i32, i32)>,
    /// 手动拖动进行中：光标相对窗口左上的偏移。
    /// 不用系统 HTCAPTION 模态拖动——Win11 对拖到顶边强制贴靠预览
    pub(crate) drag_offset: Option<(i32, i32)>,
    class_registered: bool,
    pub(crate) main_h: i32,
    pub(crate) account_error: bool,
    /// 检查到比当前更新的版本
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
            hovered: false,
            anchor: None,
            renderer: None,
            adding_account: false,
            pending_platform: crate::api::client::Platform::Cn,
            pending_team: false,
            input: PanelInput::default(),
            customizing_interval: false,
            anim_x: 0,
            anim_w: 0,
            anim_full_h: 0,
            anim_bottom: 0,
            main_h: 300,
            account_error: false,
            update_available: false,
            caret_ctx: (false, false),
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
            // 逐段对照 draw_settings 的 y 累加链（dy=0）；间隔行展开 +40、收起 +10：
            PanelView::Settings => {
                // 有账号：卡片 48 + 常驻添加按钮行 36；无账号：仅添加按钮行
                let account_block = if accounts > 0 { 84 } else { 38 };
                // 鉴权失败提示行
                let error_line = if self.account_error { 18 } else { 0 };
                let base = 42 // 顶部留白 12 + 导航行 30，返回箭头 + 居中标题
                    + 21 // 账号区标题，实绘不带分隔线
                    + account_block
                    + error_line
                    + 33 // 轮询区标题：分隔线上隙 12 + 标题 21
                    + 40 // 间隔分段控件：段体 30 + 段后间距 10
                    + 33 // 通用区标题
                    + 63 // 语言行：sub_label 21 + segmented 40 + 行后 2
                    + 63 // 外观行，同语言行
                    + 28 // 开机自启开关行：标题 19 + 行后 9，无描述行
                    + 33 // 网络代理区标题：同轮询区标题
                    + 21 // 代理子标签
                    + 26 // 代理输入框
                    + 6 // 输入框后下隙，提示文字为框内占位
                    + 33 // 通知区标题：分隔线上隙 12 + 标题 21
                    + 126 // 三个通知开关行：标题 19 + 描述 14 + 行后 9 = 42 × 3
                    + 67 // 高峰区间区：标题 33 + 输入行 26 + 下隙 8
                    + 73 // 配置管理区：标题 33 + 按钮 28 + 行后 12
                    + 18 // 关于区纯分隔，无标题：上隙 12 + 下隙 6
                    + 29 // 版本行：描边按钮顶偏移 1 + 高 28
                    + 16; // 底部余量，按钮边框需完整呈现
                base + if self.customizing_interval { 40 } else { 10 }
            }
            // 逐段对照 draw_account_picker 的 y 累加链（dy=0）
            PanelView::AccountPicker => {
                // 顶部留白 12 + 导航行 30，返回箭头 + 居中标题
                42 + accounts as i32 * 44 // 账号行：名称 + 右侧徽标单行
                    + 12 // 底部余量
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
                    eprintln!("[Quotify] 面板窗口创建失败: {e}");
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
            let (x, y) = if self.dragged {
                // 拖动后保持用户位置，只重算高度
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
            // 拖动后高度受当前位置到工作区底边的空间限制
            let max_h = if self.dragged {
                (mi.rcWork.bottom - y - 8).max(self.px(200))
            } else {
                (mi.rcWork.bottom - mi.rcWork.top - 16).max(self.px(200))
            };
            let h = self.px(logical_h).min(max_h);
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
        }
    }

    /// 定位并显示，淡入由 TIMER_ANIM 推进。
    pub fn show_at(&mut self, parent: HWND, anchor: RECT, accounts: usize) {
        let Some(hwnd) = self.ensure_window(parent) else {
            return;
        };
        // 先于 place 读取：place 的 SetWindowPos 带 SWP_SHOWWINDOW 会置可见位
        let fresh = unsafe { !IsWindowVisible(hwnd).as_bool() };
        self.anchor = Some(anchor);
        // 拖动仅临时查看；面板重新弹出时回到托盘锚点
        self.dragged = false;
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

    fn begin_hide(&mut self, hwnd: HWND) {
        self.mode = PanelMode::Hidden;
        self.hovered = false;
        self.adding_account = false;
        self.clear_input(hwnd);
        // 直接隐藏：自绘收缩/淡出会与 DWM 过渡叠加闪烁
        // 此处不 trim 工作集：高频悬停反复 trim/软缺页，静止内存由轮询路径保证
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            let _ = KillTimer(Some(hwnd), TIMER_OUTSIDE_CHECK);
            let _ = KillTimer(Some(hwnd), TIMER_ANIM);
        }
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
        self.view = PanelView::Main;
        self.adding_account = false;
        if let Some(h) = self.hwnd {
            self.clear_input(h);
        } else {
            self.input.field = None;
        }
        self.show_at(parent, anchor, accounts);
    }

    /// 结束输入状态，销毁光标与 IME 上下文
    pub(crate) fn clear_input(&mut self, hwnd: HWND) {
        self.input.field = None;
        unsafe {
            let _ = DestroyCaret();
            // 摘除 IME 上下文：裸窗口挂上后要收回，避免游离
            use windows::Win32::UI::Input::Ime::{
                HIMC, ImmAssociateContext, ImmDestroyContext, ImmGetContext,
            };
            let ctx = ImmGetContext(hwnd);
            if !ctx.is_invalid() {
                let _ = ImmAssociateContext(hwnd, HIMC(std::ptr::null_mut()));
                let _ = ImmDestroyContext(ctx);
            }
        }
    }

    /// 聚焦输入字段：系统 caret + IME 组合窗跟随光标。
    pub fn focus_input(&mut self, hwnd: HWND, field: InputField) {
        self.input.field = Some(field);
        self.mode = PanelMode::Pinned;
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hwnd));
            let caret_h = self.px(16);
            let _ = CreateCaret(hwnd, None, 1, caret_h);
            let _ = ShowCaret(Some(hwnd));
            self.update_caret();
            self.attach_ime(hwnd);
        }
    }

    /// 挂 IME 上下文并把组合窗定位到光标处。
    unsafe fn attach_ime(&mut self, hwnd: HWND) {
        unsafe {
            use windows::Win32::Foundation::POINT;
            use windows::Win32::UI::Input::Ime::{
                CFS_POINT, COMPOSITIONFORM, ImmAssociateContext, ImmCreateContext, ImmGetContext,
                ImmReleaseContext, ImmSetCompositionWindow,
            };
            let _ = ImmAssociateContext(hwnd, ImmCreateContext());
            let Some(field) = self.input.field else {
                return;
            };
            let (buf, bx, by) = self.caret_anchor(field);
            let x = bx + 6.0 + text_width(buf);
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

    /// 光标/IME 共用锚点；x/y 取自 layout，设置页各框随分区伸缩
    fn caret_anchor(&self, field: InputField) -> (&str, f32, f32) {
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
        } + layout::CARET_Y_OFFSET;
        let (buf, bx) = match field {
            InputField::Interval => (self.input.interval.as_str(), layout::INPUT_X),
            InputField::Proxy => (self.input.proxy.as_str(), layout::INPUT_X),
            InputField::PeakStart => (self.input.peak_start.as_str(), layout::PEAK_START_X),
            InputField::PeakEnd => (self.input.peak_end.as_str(), layout::PEAK_END_X),
            InputField::Name => (self.input.name.as_str(), layout::INPUT_X),
            InputField::Key => (self.input.key.as_str(), layout::INPUT_X),
            InputField::Org => (self.input.org.as_str(), layout::INPUT_X),
            InputField::Project => (self.input.project.as_str(), layout::INPUT_X),
        };
        (buf, bx, y)
    }

    /// 按字段内容计算光标位置，与 input_field 绘制对齐。
    pub fn update_caret(&self) {
        let Some(field) = self.input.field else {
            return;
        };
        let (buf, bx, by) = self.caret_anchor(field);
        let x = bx + 6.0 + text_width(buf);
        // by 已含 CARET_Y_OFFSET，框内垂直居中
        let y = by;
        unsafe {
            let _ = SetCaretPos((x * self.dpi).round() as i32, (y * self.dpi).round() as i32);
        }
    }
}

/// 取显示器有效 DPI（百分比 / 96）。
unsafe fn dpi_of(monitor: HMONITOR) -> Option<f32> {
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
                    // take 成局部值，避免与后续 &Panel / 模型组装的不可变借用冲突
                    let mut renderer = app.panel.renderer.take().or_else(Renderer::new);
                    if let Some(r) = renderer.as_mut() {
                        let model = PanelModel::from_app(app);
                        let view = app.panel.view;
                        let dpi = app.panel.dpi;
                        r.paint(hwnd, &rect, &app.panel, &model, view, dpi);
                    }
                    app.panel.renderer = renderer;
                }
                LRESULT(0)
            }
            WM_TIMER => {
                let id = wparam.0;
                match id {
                    TIMER_ANIM => on_anim_tick(hwnd),
                    TIMER_CLOSE_DEBOUNCE => {
                        let app = app_from_tray(hwnd);
                        if let Some(app) = app {
                            let _ = KillTimer(Some(hwnd), TIMER_CLOSE_DEBOUNCE);
                            // 到期时光标已回托盘则取消收回——悬停语义优先
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
                                // 子控件同样算在面板内
                                let in_panel = w == hwnd || GetAncestor(w, GA_ROOT) == hwnd;
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
                // 手动拖动跟随：光标减按下偏移，钳制在工作区内
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
                    // 四方向都可越出屏幕，仅保留窗口一角在工作区内可抓回
                    let x = (cursor.x - ox).clamp(
                        mi.rcWork.left - w + 64,
                        (mi.rcWork.right - 64).max(mi.rcWork.left - w + 64),
                    );
                    let y = (cursor.y - oy).clamp(
                        mi.rcWork.top - (wr.bottom - wr.top) + 64,
                        mi.rcWork.bottom - 48,
                    );
                    let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
                    return LRESULT(0);
                }
                let app = app_from_tray(hwnd);
                if let Some(app) = app {
                    if !app.panel.hovered {
                        app.panel.hovered = true;
                        let mut tm = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            ..Default::default()
                        };
                        let _ = TrackMouseEvent(&mut tm);
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
                        let mut tm = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            ..Default::default()
                        };
                        let _ = TrackMouseEvent(&mut tm);
                    } else {
                        app.panel.hovered = false;
                        if app.panel.mode == PanelMode::Preview {
                            app.panel.request_close();
                        }
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let mut app = app_from_tray(hwnd);
                // 手动拖动结束：恢复完整高度并记拖动态
                if let Some(app) = app.as_mut()
                    && app.panel.drag_offset.take().is_some()
                {
                    let _ = ReleaseCapture();
                    app.panel.dragged = true;
                    let n = app.config.accounts.len();
                    let logical_h = app.panel.view_height(n);
                    app.panel.place(hwnd, logical_h, true);
                    let _ = InvalidateRect(Some(hwnd), None, true);
                    return LRESULT(0);
                }
                let app = app_from_tray(hwnd);
                if let Some(app) = app {
                    // 按下→松手位移过大视为拖动尾程，不触发点击
                    let moved_far = app.panel.press_at.take().is_some_and(|(px, py)| {
                        let mut cursor = POINT::default();
                        let _ = GetCursorPos(&mut cursor);
                        (cursor.x - px).abs() + (cursor.y - py).abs() > 8
                    });
                    if !moved_far {
                        let (x, y) = (x_of(lparam) / app.panel.dpi, y_of(lparam) / app.panel.dpi);
                        let hit = app.panel.renderer.as_ref().and_then(|r| r.hit_at(x, y));
                        if let Some(hit) = hit {
                            crate::app::handle_panel_hit(app, hit, hwnd);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_CHAR => {
                let app = app_from_tray(hwnd);
                if let Some(app) = app
                    && app.panel.input.field.is_some()
                {
                    let ch = (wparam.0 & 0xFFFF) as u16;
                    let mut confirm = false;
                    {
                        let input = &mut app.panel.input;
                        let field = input.field;
                        let buf = match field {
                            Some(InputField::Proxy) => &mut input.proxy,
                            Some(InputField::PeakStart) => &mut input.peak_start,
                            Some(InputField::PeakEnd) => &mut input.peak_end,
                            Some(InputField::Name) => &mut input.name,
                            Some(InputField::Key) => &mut input.key,
                            Some(InputField::Org) => &mut input.org,
                            Some(InputField::Project) => &mut input.project,
                            // Interval 走兜底臂，保持无输入态也可敲键
                            _ => &mut input.interval,
                        };
                        match char::from_u32(ch as u32) {
                            Some('\r') | Some('\n') => confirm = true,
                            Some('\u{8}') => {
                                buf.pop();
                            }
                            Some('\u{16}') => {
                                if let Some(text) = read_clipboard_text() {
                                    for c in text.chars() {
                                        if !c.is_control() && buf.len() < 128 {
                                            buf.push(c);
                                        }
                                    }
                                }
                            }
                            Some(c) if !c.is_control() && (c as u32) != 127 && buf.len() < 128 => {
                                buf.push(c);
                            }
                            _ => {}
                        }
                    }
                    if confirm {
                        crate::app::confirm_panel_input(app, hwnd);
                    }
                    app.panel.update_caret();
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
                    if hit_none {
                        let mut wr = RECT::default();
                        let _ = GetWindowRect(hwnd, &mut wr);
                        app.panel.drag_offset = Some((cursor.x - wr.left, cursor.y - wr.top));
                        let _ = SetCapture(hwnd);
                    }
                }
                LRESULT(0)
            }
            windows::Win32::UI::WindowsAndMessaging::WM_EXITSIZEMOVE => {
                let app = app_from_tray(hwnd);
                if let Some(app) = app {
                    // 拖动结束：保持当前位置恢复完整高度
                    app.panel.dragged = true;
                    let n = app.config.accounts.len();
                    let logical_h = app.panel.view_height(n);
                    app.panel.place(hwnd, logical_h, true);
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

fn app_from_tray(hwnd: HWND) -> Option<&'static mut crate::app::App> {
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

/// 等宽 12px 字号下的字符经验宽度
fn text_width(s: &str) -> f32 {
    s.chars()
        .map(|c| if c.is_ascii() { 7.3 } else { 12.5 })
        .sum()
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

fn x_of(l: LPARAM) -> f32 {
    (l.0 & 0xFFFF) as u16 as i16 as f32
}

fn y_of(l: LPARAM) -> f32 {
    ((l.0 >> 16) & 0xFFFF) as u16 as i16 as f32
}
