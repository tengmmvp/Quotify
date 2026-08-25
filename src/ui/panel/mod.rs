//! 弹出面板：悬停预览 / 点击锁定 / 移开收起的任务栏 flyout。
//!
//! 窗口为无边框 WS_POPUP + DWM 大圆角 + 内容级上浮+渐入（不再用
//! WS_EX_LAYERED 整窗 alpha）；
//! 内容由 `render.rs` 的 D2D 渲染器逐帧绘制，动画由 WM_TIMER 驱动
//! （静止时无定时器，零 CPU 占用）。

pub mod anim;
pub mod layout;
pub mod render;
pub mod theme;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, InvalidateRect, MonitorFromWindow, HBRUSH, HMONITOR,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, ValidateRect,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForMonitor;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::platform::wide;
use crate::ui::panel::render::Renderer;
use crate::ui::panel::theme::PANEL_WIDTH;

const PANEL_WND_CLASS: &str = "QuotifyPanelWnd";

const TIMER_ANIM: usize = 1;
const TIMER_CLOSE_DEBOUNCE: usize = 2;
const TIMER_OUTSIDE_CHECK: usize = 3;

/// DPI 探测失败时的兜底值（150%——常见笔记本缩放）
pub(crate) const FALLBACK_DPI: f32 = 1.5;

/// 面板的展示模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMode {
    /// 悬停预览：鼠标离开（含防抖窗口）后自动收起
    Preview,
    /// 点击锁定：点图标或点面板外关闭
    Pinned,
    Hidden,
}

/// 面板当前展示的视图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelView {
    Main,
    Settings,
}

/// 自绘输入的目标字段。不使用系统 EDIT：其在本环境与输入法钩子、
/// 前台锁定、ghost 的兼容问题无解；键盘消息直达面板窗口过程自管理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputField {
    Name,
    Key,
    Interval,
    /// 团队版：组织 ID
    Org,
    /// 团队版：项目 ID
    Project,
}

/// 自绘输入状态（缓冲；光标由系统 caret 呈现）。
#[derive(Default)]
pub struct PanelInput {
    pub field: Option<InputField>,
    pub name: String,
    pub key: String,
    pub interval: String,
    pub org: String,
    pub project: String,
}

pub struct Panel {
    pub hwnd: Option<HWND>,
    pub mode: PanelMode,
    pub view: PanelView,
    /// 鼠标在面板客户区内
    pub hovered: bool,
    pub(crate) anchor: Option<RECT>,
    pub renderer: Option<Renderer>,
    /// 设置视图：添加账号子状态（显示输入行）
    pub adding_account: bool,
    pub pending_platform: crate::api::client::Platform,
    /// 添加账号：团队版（展开组织/项目输入行；仅国内站）
    pub pending_team: bool,
    /// 自绘输入状态
    pub input: PanelInput,
    /// 轮询间隔自定义模式（显示输入行）
    pub customizing_interval: bool,
    /// 展开动画的当前布局（物理像素；relayout 同步）
    pub(crate) anim_x: i32,
    pub(crate) anim_w: i32,
    pub(crate) anim_full_h: i32,
    pub(crate) anim_bottom: i32,
    /// 光标/IME 定位上下文（has_account, auth_error）：设置页「自定义间隔」
    /// 输入框的 y 随账号块与鉴权错误行伸缩，而 Panel 不持有 config，
    /// 由 app 层在 relayout_panel 时回写
    /// `panel.caret_ctx = (accounts > 0, panel.account_error)`。
    pub(crate) caret_ctx: (bool, bool),
    class_registered: bool,
    /// 主视图动态高度（逻辑像素；按指标行数/余额/副标题由 app 侧计算）
    pub(crate) main_h: i32,
    /// 账号当前处于鉴权失败状态（设置页账号卡下显示修复提示，高度联动）
    pub(crate) account_error: bool,
    /// Pinned 模式下鼠标离开面板/托盘区的起始时刻（持续在外 2s 才收起）
    pub(crate) outside_since: Option<u64>,
    /// 显示器缩放（逻辑 → 物理像素）
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
            caret_ctx: (false, false),
            class_registered: false,
            outside_since: None,
            dpi: FALLBACK_DPI,
        }
    }

    /// 逻辑像素 → 物理像素。
    pub(crate) fn px(&self, logical: i32) -> i32 {
        (logical as f32 * self.dpi).round() as i32
    }

    /// 面板逻辑高度（含单账号卡片 48；自定义输入行展开时 +40）。
    pub(crate) fn view_height(&self, accounts: usize) -> i32 {
        match self.view {
            // 动态：随指标行数 / 余额 / 副标题伸缩（sync_main_height 维护）
            PanelView::Main => self.main_h,
            // 添加页：账号类型双分段（平台/个人团队）+ 名称/key；
            // 团队版追加组织/项目两行输入（高度由 layout 统一给出）
            PanelView::Settings if self.adding_account => layout::add_page_height(self.pending_team),
            // 设置页随内容伸缩：账号卡(48) vs 添加按钮(38)、key 失效提示行(18)、
            // 自定义间隔输入行展开时 +40（收起时 +10）。各项逐段对照
            // draw_settings 的 y 累加链（dy=0 静止态）：
            PanelView::Settings => {
                // 有账号：卡片 40 + 卡后间距 8；无账号：添加按钮行 36（+2 余量）
                let account_block = if accounts > 0 { 48 } else { 38 };
                // key 失效提示行（卡片下方 danger 弱字一行）
                let error_line = if self.account_error { 18 } else { 0 };
                let base = 42 // 顶部留白 12 + 导航行 30（返回箭头 + 居中标题）
                    + 33 // 账号区标题 21 + 12 余量（该区实绘不带分隔线）
                    + account_block
                    + error_line
                    + 33 // 轮询区标题（分隔线上隙 12 + 标题 21）
                    + 40 // 间隔分段控件（段体 30 + 段后间距 10）
                    + 63 // 语言行（sub_label 21 + segmented 40 + 行后 2）
                    + 63 // 外观行（同语言行）
                    + 28 // 开机自启开关行（标题 19 + 行后 9，无描述行）
                    + 33 // 通知区标题（分隔线上隙 12 + 标题 21）
                    + 126 // 三个通知开关行（标题 19 + 描述 14 + 行后 9 = 42 × 3）
                    + 18 // 关于区纯分隔（无标题：上隙 12 + 下隙 6）
                    + 29 // 版本行（描边按钮顶偏移 1 + 高 28）
                    + 30; // 底部余量（按钮边框需完整呈现）
                base + if self.customizing_interval { 40 } else { 10 }
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
                // 注意：创建时不带 WS_EX_LAYERED——创建即 layered 的窗口在
                // DWM 下走 per-pixel alpha 合成，GDI/D2D 绘制不写 alpha 字节，
                // 内容会全透明不可见。layered 在 ShowWindow 之后动态附加。
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                PCWSTR(name.as_ptr()),
                PCWSTR(name.as_ptr()),
                WS_POPUP,
                0, 0, 0, 0,
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
            // Win11 系统默认小圆角（约 8px，贴近编辑风的锐利纸感）
            let pref = windows::Win32::Graphics::Dwm::DWMWCP_DEFAULT;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            );
            self.hwnd = Some(hwnd);
            Some(hwnd)
        }
    }

    /// 计算并应用面板几何：监视器工作区探测 + DPI 同步 + 锚点上方
    /// 水平居中 + 三向夹取 + SetWindowPos + 回写动画基准。
    /// 首次弹出（show=true，带 SWP_SHOWWINDOW）与尺寸变化重排
    /// （app 层 relayout_panel，show=false）共用，消除两份定位代码。
    /// `logical_h` 为期望逻辑高度，内部做工作区高度夹取。
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
            // 高度不超过工作区（小屏 / 多账号时截断显示）
            let max_h = (mi.rcWork.bottom - mi.rcWork.top - 16).max(self.px(200));
            let h = self.px(logical_h).min(max_h);
            let (x, y) = match self.anchor {
                Some(anchor) => {
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
                }
                None => (0, 0),
            };
            let flags = if show { SWP_SHOWWINDOW | SWP_NOCOPYBITS } else { SWP_NOCOPYBITS };
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, flags);
            // 记录布局供展开动画使用（锚定底边）
            self.anim_x = x;
            self.anim_w = w;
            self.anim_full_h = h;
            self.anim_bottom = y + h;
        }
    }

    /// 定位并显示（淡入起点 alpha=0，动画由 TIMER_ANIM 推进）。
    /// `accounts` 为账号数（设置页高度随账号列表伸缩）。
    pub fn show_at(&mut self, parent: HWND, anchor: RECT, accounts: usize) {
        let Some(hwnd) = self.ensure_window(parent) else { return };
        self.anchor = Some(anchor);
        let logical_h = self.view_height(accounts);
        self.place(hwnd, logical_h, true);
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            // 注意：不再使用 WS_EX_LAYERED 整窗 alpha——layered 与交换链
            // 呈现（HwndRenderTarget）不兼容，且其子控件更新代价高昂。
            if let Some(r) = self.renderer.as_mut() {
                r.anim.appear = Some(anim::Tween::now(180));
            }
            start_anim(hwnd);
            // Pinned 模式点面板外关闭的外部检查：首次延迟给足弹出宽限
            // （避免唤醒/点击弹出瞬间就因「鼠标不在面板」被收回），
            // 首次触发后转高频巡检
            SetTimer(Some(hwnd), TIMER_OUTSIDE_CHECK, 1200, None);
            let _ = InvalidateRect(Some(hwnd), None, true);
        }
    }

    /// 请求收起（预览模式 + 400ms 防抖）。
    pub fn request_close(&mut self) {
        if self.hovered || self.mode != PanelMode::Preview {
            return;
        }
        if let Some(h) = self.hwnd {
            unsafe { SetTimer(Some(h), TIMER_CLOSE_DEBOUNCE, 400, None) };
        }
    }

    /// 光标是否在托盘图标锚点附近（±24px，纯本地矩形比较——
    /// Shell_NotifyIconGetRect 是跨进程同步调用，高频轮询会与 Explorer 互锁）。
    pub(crate) fn cursor_near_anchor(&self) -> bool {
        let pt = unsafe {
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            p
        };
        self.anchor
            .map(|a| {
                pt.x >= a.left - 24 && pt.x <= a.right + 24
                    && pt.y >= a.top - 24 && pt.y <= a.bottom + 24
            })
            .unwrap_or(false)
    }

    fn begin_hide(&mut self, hwnd: HWND) {
        self.mode = PanelMode::Hidden;
        self.hovered = false;
        self.adding_account = false;
        self.clear_input(hwnd);
        // 直接隐藏（收起不做自绘动画——收缩/淡出都会与系统 DWM 过渡
        // 叠加产生闪烁；消失的顺滑交给系统）。注意此处不再 trim 工作集：
        // 高频悬停会反复 trim/软缺页换回；静止内存由轮询路径的 trim 保证
        // （最迟一个轮询周期内必触发）。
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            let _ = KillTimer(Some(hwnd), TIMER_OUTSIDE_CHECK);
            let _ = KillTimer(Some(hwnd), TIMER_ANIM);
        }
    }

    /// 左键：预览 ⇄ 锁定；已锁定 → 收起。
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
                // 社区标准（PowerToys/launcher 同类）：点击瞬间进程持有
                // 前台权，显示后立即激活面板——后台窗口的 EDIT 拿不到
                // 焦点、IME 行为异常，这是输入卡顿的根源
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

    /// 结束输入状态（销毁光标与 IME 上下文）。
    pub(crate) fn clear_input(&mut self, hwnd: HWND) {
        self.input.field = None;
        unsafe {
            let _ = DestroyCaret();
            // 摘除 IME 上下文（裸窗口挂上后要收回，避免游离）
            use windows::Win32::UI::Input::Ime::{
                ImmAssociateContext, ImmDestroyContext, ImmGetContext, HIMC,
            };
            let ctx = ImmGetContext(hwnd);
            if !ctx.is_invalid() {
                let _ = ImmAssociateContext(hwnd, HIMC(std::ptr::null_mut()));
                let _ = ImmDestroyContext(ctx);
            }
        }
    }

    /// 聚焦某个输入字段：系统 caret + IME 上下文（组合窗跟随光标，
    /// 拼音在 IME 内组合而不是字母透传进缓冲）。
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
    unsafe fn attach_ime(&mut self, hwnd: HWND) { unsafe {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::Input::Ime::{
            ImmAssociateContext, ImmCreateContext, ImmGetContext, ImmReleaseContext,
            ImmSetCompositionWindow, COMPOSITIONFORM, CFS_POINT,
        };
        let _ = ImmAssociateContext(hwnd, ImmCreateContext());
        let Some(field) = self.input.field else { return };
        let (buf, by) = self.caret_anchor(field);
        let x = layout::INPUT_X + 6.0 + text_width(buf);
        // 组合窗锚点保持在框底附近（框顶 +17，与历史行为一致）
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
    }}

    /// 光标/IME 共用锚点：(字段缓冲, 输入框顶 y + CARET_Y_OFFSET)。
    /// y 统一取 layout——添加页四个输入框是固定常量；设置页的自定义
    /// 间隔框随账号块/鉴权错误行伸缩（上下文 caret_ctx 由 app 层回写）。
    fn caret_anchor(&self, field: InputField) -> (&str, f32) {
        let y = match field {
            InputField::Name => layout::ADD_NAME_Y,
            InputField::Key => layout::ADD_KEY_Y,
            InputField::Interval => {
                let (has_account, auth_error) = self.caret_ctx;
                layout::interval_input_y(has_account, auth_error)
            }
            InputField::Org => layout::ADD_ORG_Y,
            InputField::Project => layout::ADD_PROJECT_Y,
        } + layout::CARET_Y_OFFSET;
        let buf = match field {
            InputField::Name => self.input.name.as_str(),
            InputField::Key => self.input.key.as_str(),
            InputField::Interval => self.input.interval.as_str(),
            InputField::Org => self.input.org.as_str(),
            InputField::Project => self.input.project.as_str(),
        };
        (buf, y)
    }

    /// 按当前字段内容计算光标位置（按字符实际宽度：ASCII 7.3 / 全角 12.5）。
    /// 坐标与 draw_settings 的 input_field 布局对齐（见 caret_anchor）。
    pub fn update_caret(&self) {
        let Some(field) = self.input.field else { return };
        let (buf, by) = self.caret_anchor(field);
        let x = layout::INPUT_X + 6.0 + text_width(buf);
        // by 已含 CARET_Y_OFFSET（框内垂直居中）
        let y = by;
        unsafe {
            let _ = SetCaretPos(
                (x * self.dpi).round() as i32,
                (y * self.dpi).round() as i32,
            );
        }
    }

}

/// 取显示器有效 DPI（百分比 / 96）。
unsafe fn dpi_of(monitor: HMONITOR) -> Option<f32> { unsafe {
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
}}

/// 动画时钟：确保 TIMER_ANIM 在跑。
unsafe fn start_anim(hwnd: HWND) { unsafe {
    SetTimer(Some(hwnd), TIMER_ANIM, 16, None);
}}

/// 面板窗口过程。
pub extern "system" fn panel_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                #[cfg(debug_assertions)]
                let t0 = std::time::Instant::now();
                // 硬件呈现（交换链）自绘整窗：验证客户区后直接渲染
                let _ = ValidateRect(Some(hwnd), None);
                let app = app_from_tray(hwnd);
                if let Some(app) = app {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let mut renderer = app
                        .panel
                        .renderer
                        .take()
                        .or_else(Renderer::new);
                    if let Some(r) = renderer.as_mut() {
                        let view = app.panel.view;
                        let dpi = app.panel.dpi;
                        r.paint(hwnd, &rect, app, view, dpi);
                    }
                    app.panel.renderer = renderer;
                }
                #[cfg(debug_assertions)]
                {
                    let dt = t0.elapsed();
                    if dt.as_millis() > 30 {
                        eprintln!("[Quotify] WM_PAINT 耗时 {}ms", dt.as_millis());
                    }
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
                            // 防抖到期时光标已回到托盘图标上：收回意图取消
                            // （悬停语义优先），后续交给外部巡检定时器继续盯
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
                            // 首次触发（宽限期结束）后转巡检。注意：这里不能调
                            // Shell_NotifyIconGetRect——它是跨进程同步调用（向
                            // Explorer 发消息），高频轮询 + 面板激活状态下会互锁卡死
                            SetTimer(Some(hwnd), TIMER_OUTSIDE_CHECK, 200, None);
                            let preview = app.panel.mode == PanelMode::Preview;
                            let pinned = app.panel.mode == PanelMode::Pinned;
                            if (preview || pinned) && !app.panel.hovered {
                                let mut pt = POINT::default();
                                let _ = GetCursorPos(&mut pt);
                                let w = WindowFromPoint(pt);
                                // 子控件同样算在面板内
                                let in_panel = w == hwnd || GetAncestor(w, GA_ROOT) == hwnd;
                                // 输入状态中（正在打字）绝不收起
                                let focus_in_panel = app.panel.input.field.is_some()
                                    || windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == hwnd;
                                // 鼠标悬停在托盘图标上（锚点矩形判定）
                                let near_tray = app.panel.cursor_near_anchor();
                                if in_panel || focus_in_panel || near_tray {
                                    app.panel.outside_since = None;
                                } else {
                                    let now = windows::Win32::System::SystemInformation::GetTickCount64();
                                    let since = *app.panel.outside_since.get_or_insert(now);
                                    // Preview 300ms（悬停移开即收）/ Pinned 2s
                                    // （给「托盘 → 面板」的移动留时间）
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
                    // 鼠标消息坐标是物理像素，命中区以逻辑像素记录（渲染
                    // 时 target 设了 DPI），匹配前先归一到逻辑
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
                    // 鼠标移到子控件（EDIT 输入框）上也会触发 MOUSELEAVE——
                    // 若根窗口仍是本面板则视为「还在面板内」，继续跟踪
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let w = WindowFromPoint(pt);
                    let still_here = w == hwnd
                        || GetAncestor(w, GA_ROOT) == hwnd;
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
                let app = app_from_tray(hwnd);
                if let Some(app) = app {
                    // 同 WM_MOUSEMOVE：物理 → 逻辑
                    let (x, y) = (x_of(lparam) / app.panel.dpi, y_of(lparam) / app.panel.dpi);
                    let hit = app
                        .panel
                        .renderer
                        .as_ref()
                        .and_then(|r| r.hit_at(x, y));
                    if let Some(hit) = hit {
                        crate::app::handle_panel_hit(app, hit, hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_CHAR => {
                // 自绘输入：键盘直达面板（无 EDIT 子窗口，绕开输入法
                // 钩子与前台锁定的全部兼容问题）
                let app = app_from_tray(hwnd);
                if let Some(app) = app && app.panel.input.field.is_some() {
                    let ch = (wparam.0 & 0xFFFF) as u16;
                    let mut confirm = false;
                    {
                        let input = &mut app.panel.input;
                        let field = input.field;
                        let buf = match field {
                            Some(InputField::Name) => &mut input.name,
                            Some(InputField::Key) => &mut input.key,
                            Some(InputField::Org) => &mut input.org,
                            Some(InputField::Project) => &mut input.project,
                            _ => &mut input.interval,
                        };
                        match char::from_u32(ch as u32) {
                            Some('\r') | Some('\n') => confirm = true,
                            Some('\u{8}') => {
                                buf.pop();
                            }
                            Some('\u{16}') => {
                                // Ctrl+V：粘贴剪贴板文本（中文输入的补充路径）
                                if let Some(text) = read_clipboard_text() {
                                    for c in text.chars() {
                                        if !c.is_control() && buf.len() < 128 {
                                            buf.push(c);
                                        }
                                    }
                                }
                            }
                            // Unicode 直收：IME 上屏的中文经 WM_CHAR 到达
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
            WM_ERASEBKGND => LRESULT(1), // 全量交给 D2D
            WM_SETCURSOR => {
                // 只处理面板本体的命中：可点击元素手型，其余箭头
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
            WM_MOUSEACTIVATE => {
                // 点击激活面板：EDIT 子控件依赖窗口激活才能获得键盘焦点
                LRESULT(MA_ACTIVATE as isize)
            }
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// 动画帧：推进弹出/收起/旋转，完毕即停表（静止零占用）。
unsafe fn on_anim_tick(hwnd: HWND) -> LRESULT { unsafe {
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
    // 弹出只做内容级变换（渲染器内部的上浮 + 透明度）——不做逐帧
    // 窗口高度生长：高度变化会强制 HwndRenderTarget 每帧重建（重新
    // 分配缓冲），正是弹出瞬间「duang」顿挫的来源。收起同样直接隐藏。
    let _ = InvalidateRect(Some(hwnd), None, false);

    if done {
        let _ = KillTimer(Some(hwnd), TIMER_ANIM);
    }
    LRESULT(0)
}}

/// 面板窗口 → 所属 App（经 owner 托盘窗口的 GWLP_USERDATA）。
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

/// 等宽 12px 字号下的文本像素宽（ASCII 7.3 / 全角 CJK 12.5）。
fn text_width(s: &str) -> f32 {
    s.chars().map(|c| if c.is_ascii() { 7.3 } else { 12.5 }).sum()
}

/// 读剪贴板 Unicode 文本（Ctrl+V 粘贴）。
fn read_clipboard_text() -> Option<String> {
    unsafe {
        use windows::Win32::System::DataExchange::{
            CloseClipboard, GetClipboardData, OpenClipboard,
        };
        use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
        use windows::Win32::Foundation::HGLOBAL;

        const CF_UNICODETEXT: u32 = 13;
        OpenClipboard(None).ok()?;
        let result = GetClipboardData(CF_UNICODETEXT).ok().and_then(|h| {
            let hg = HGLOBAL(h.0);
            let ptr = GlobalLock(hg) as *const u16;
            if ptr.is_null() {
                return None;
            }
            // NUL 结尾扫描加分配上界（GlobalSize / 2 个 u16）——剪贴板
            // 数据由外部进程写入，防无终止符的脏数据导致越界读
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

/// lParam 的有符号低位（GET_X_LPARAM 等价，windows-rs 未导出该宏）。
fn x_of(l: LPARAM) -> f32 {
    (l.0 & 0xFFFF) as u16 as i16 as f32
}

fn y_of(l: LPARAM) -> f32 {
    ((l.0 >> 16) & 0xFFFF) as u16 as i16 as f32
}
