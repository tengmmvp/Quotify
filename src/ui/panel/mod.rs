//! 弹出面板：悬停预览 / 点击锁定 / 移开收起的任务栏 flyout。
//!
//! 窗口为无边框 WS_POPUP + DWM 大圆角 + 整窗 alpha 淡入淡出；
//! 内容由 `render.rs` 的 D2D 渲染器逐帧绘制，动画由 WM_TIMER 驱动
//! （静止时无定时器，零 CPU 占用）。

pub mod anim;
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

use crate::ui::panel::render::Renderer;
use crate::ui::panel::theme::PANEL_WIDTH;

const PANEL_WND_CLASS: &str = "QuotifyPanelWnd";

const TIMER_ANIM: usize = 1;
const TIMER_CLOSE_DEBOUNCE: usize = 2;
const TIMER_OUTSIDE_CHECK: usize = 3;

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
}

/// 自绘输入状态（缓冲；光标由系统 caret 呈现）。
pub struct PanelInput {
    pub field: Option<InputField>,
    pub name: String,
    pub key: String,
    pub interval: String,
}

impl Default for PanelInput {
    fn default() -> Self {
        Self {
            field: None,
            name: String::new(),
            key: String::new(),
            interval: String::new(),
        }
    }
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
    /// 自绘输入状态
    pub input: PanelInput,
    /// 轮询间隔自定义模式（显示输入行）
    pub customizing_interval: bool,
    /// 展开动画的当前布局（物理像素；锚定底边做高度生长）
    pub(crate) anim_x: i32,
    pub(crate) anim_w: i32,
    pub(crate) anim_full_h: i32,
    pub(crate) anim_bottom: i32,
    class_registered: bool,
    hide_anim: bool,
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
            input: PanelInput::default(),
            customizing_interval: false,
            anim_x: 0,
            anim_w: 0,
            anim_full_h: 0,
            anim_bottom: 0,
            class_registered: false,
            hide_anim: false,
            outside_since: None,
            dpi: 1.5,
        }
    }

    /// 逻辑像素 → 物理像素。
    fn px(&self, logical: i32) -> i32 {
        (logical as f32 * self.dpi).round() as i32
    }

    /// 面板逻辑高度（随视图与账号数动态；自定义输入行展开时 +40）。
    fn view_height(&self, accounts: usize) -> i32 {
        match self.view {
            PanelView::Main => 380,
            PanelView::Settings if self.adding_account => 258,
            PanelView::Settings => {
                552 + 34 * accounts as i32 + if self.customizing_interval { 40 } else { 0 }
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
                    eprintln!("[quotify] 面板窗口创建失败: {e}");
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

    /// 定位并显示（淡入起点 alpha=0，动画由 TIMER_ANIM 推进）。
    /// `accounts` 为账号数（设置页高度随账号列表伸缩）。
    pub fn show_at(&mut self, parent: HWND, anchor: RECT, accounts: usize) {
        let Some(hwnd) = self.ensure_window(parent) else { return };
        self.anchor = Some(anchor);
        unsafe {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut mi);
            self.dpi = dpi_of(monitor).unwrap_or(1.5);

            let w = self.px(PANEL_WIDTH);
            // 高度不超过工作区（小屏 / 多账号时截断显示）
            let max_h = (mi.rcWork.bottom - mi.rcWork.top - 16).max(self.px(200));
            let h = self.px(self.view_height(accounts)).min(max_h);
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
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x, y, w, h,
                SWP_SHOWWINDOW | SWP_NOCOPYBITS,
            );
            // 记录布局供展开动画使用（锚定底边）
            self.anim_x = x;
            self.anim_w = w;
            self.anim_full_h = h;
            self.anim_bottom = y + h;
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            // 注意：不再使用 WS_EX_LAYERED 整窗 alpha——layered 与交换链
            // 呈现（HwndRenderTarget）不兼容，且其子控件更新代价高昂。
            self.hide_anim = false;
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

    fn begin_hide(&mut self, hwnd: HWND) {
        self.mode = PanelMode::Hidden;
        self.hovered = false;
        self.adding_account = false;
        self.clear_input(hwnd);
        self.hide_anim = true;
        // 直接隐藏（收起不做自绘动画——收缩/淡出都会与系统 DWM 过渡
        // 叠加产生闪烁；消失的顺滑交给系统）
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            let _ = KillTimer(Some(hwnd), TIMER_OUTSIDE_CHECK);
            let _ = KillTimer(Some(hwnd), TIMER_ANIM);
            crate::platform::trim_working_set();
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
    fn clear_input(&mut self, hwnd: HWND) {
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

    pub fn clear_input_pub(&mut self, hwnd: HWND) {
        self.clear_input(hwnd);
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
            self.update_caret(hwnd);
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
        let (buf, bx, by) = match field {
            InputField::Name => (&self.input.name, 20.0f32, 126.0f32),
            InputField::Key => (&self.input.key, 20.0, 176.0),
            InputField::Interval => (&self.input.interval, 20.0, 168.0),
        };
        let x = bx + 6.0 + text_width(buf);
        let pt = POINT {
            x: (x * self.dpi).round() as i32,
            y: ((by + 20.0) * self.dpi).round() as i32,
        };
        let ctx = ImmGetContext(hwnd);
        if !ctx.is_invalid() {
            let mut cf = COMPOSITIONFORM {
                dwStyle: CFS_POINT,
                ptCurrentPos: pt,
                rcArea: windows::Win32::Foundation::RECT::default(),
            };
            let _ = ImmSetCompositionWindow(ctx, &mut cf);
            let _ = ImmReleaseContext(hwnd, ctx);
        }
    }}

    /// 按当前字段内容计算光标位置（按字符实际宽度：ASCII 7.3 / 全角 12.5）。
    /// 坐标与 draw_settings 的 input_field 布局对齐。
    pub fn update_caret(&self, _hwnd: HWND) {
        let Some(field) = self.input.field else { return };
        // (框 x, 框 y)；间隔框左起 96 宽（与 draw_settings 对齐）
        let (buf, bx, by) = match field {
            InputField::Name => (&self.input.name, 20.0f32, 126.0f32),
            InputField::Key => (&self.input.key, 20.0, 176.0),
            InputField::Interval => (&self.input.interval, 20.0, 168.0),
        };
        let x = bx + 6.0 + text_width(buf);
        let y = by + 5.0;
        unsafe {
            let _ = SetCaretPos(
                (x * self.dpi).round() as i32,
                (y * self.dpi).round() as i32,
            );
        }
    }

    // ── 供 app 层使用的访问器 ──

    pub fn px_of(&self, logical: i32) -> i32 {
        self.px(logical)
    }

    pub fn view_height_pub(&self, accounts: usize) -> i32 {
        self.view_height(accounts)
    }

    #[allow(dead_code)]
    pub fn dpi_pub(&self) -> f32 {
        self.dpi
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
                        .or_else(|| Renderer::new());
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
                        eprintln!("[quotify] WM_PAINT 耗时 {}ms", dt.as_millis());
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => {
                let id = wparam.0 as usize;
                match id {
                    TIMER_ANIM => on_anim_tick(hwnd),
                    TIMER_CLOSE_DEBOUNCE => {
                        let app = app_from_tray(hwnd);
                        if let Some(app) = app {
                            let _ = KillTimer(Some(hwnd), TIMER_CLOSE_DEBOUNCE);
                            if !app.panel.hovered && app.panel.mode == PanelMode::Preview {
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
                                if in_panel || focus_in_panel {
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
                        let hit = r
                            .hits
                            .iter()
                            .find(|(_, rc)| x >= rc.left && x <= rc.right && y >= rc.top && y <= rc.bottom)
                            .map(|(h, _)| *h);
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
                        .and_then(|r| {
                            r.hits
                                .iter()
                                .find(|(_, rc)| x >= rc.left && x <= rc.right && y >= rc.top && y <= rc.bottom)
                                .map(|(h, _)| *h)
                        });
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
                if let Some(app) = app {
                    if app.panel.input.field.is_some() {
                        let ch = (wparam.0 & 0xFFFF) as u16;
                        let mut confirm = false;
                        {
                            let input = &mut app.panel.input;
                            let field = input.field;
                            let buf = match field {
                                Some(InputField::Name) => &mut input.name,
                                Some(InputField::Key) => &mut input.key,
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
                                Some(c) if !c.is_control() && (c as u32) != 127 => {
                                    // Unicode 直收：IME 上屏的中文经 WM_CHAR 到达
                                    if buf.len() < 128 {
                                        buf.push(c);
                                    }
                                }
                                _ => {}
                            }
                        }
                        if confirm {
                            crate::app::confirm_panel_input(app, hwnd);
                        }
                        app.panel.update_caret(hwnd);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
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
    let hiding = app.panel.hide_anim;
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
    if let Some(r) = app.panel.renderer.as_ref() {
        let _ = r;
    }
    // 展开动画：高度自 88% 生长到 100%（锚定底边，Win11 flyout 风格）；
    // 收起反向收缩。不依赖 layered，与硬件呈现兼容。
    if let Some(t) = app
        .panel
        .renderer
        .as_ref()
        .and_then(|r| r.anim.appear.as_ref())
    {
        let p = anim::ease_out_cubic(t.progress());
        let k = if hiding { 1.0 - p } else { p };
        let scale = 0.88 + 0.12 * k;
        let (x, w, full_h, bottom) = (
            app.panel.anim_x,
            app.panel.anim_w,
            app.panel.anim_full_h,
            app.panel.anim_bottom,
        );
        if w > 0 && full_h > 0 {
            let h = (full_h as f32 * scale).round() as i32;
            let y = bottom - h;
            let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOACTIVATE | SWP_NOCOPYBITS);
        }
    }
    let _ = InvalidateRect(Some(hwnd), None, false);

    if done {
        let _ = KillTimer(Some(hwnd), TIMER_ANIM);
        if hiding {
            let _ = ShowWindow(hwnd, SW_HIDE);
            let _ = KillTimer(Some(hwnd), TIMER_OUTSIDE_CHECK);
            // 收起后归还 D2D 等运行时占用的物理页，静止时保持低内存
            crate::platform::trim_working_set();
        }
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

/// 线性 [0..1] 颜色 → COLORREF（0x00BBGGRR）。
#[allow(dead_code)]
fn rgb_of(c: [f32; 4]) -> u32 {
    let r = (c[0].clamp(0.0, 1.0) * 255.0).round() as u32;
    let g = (c[1].clamp(0.0, 1.0) * 255.0).round() as u32;
    let b = (c[2].clamp(0.0, 1.0) * 255.0).round() as u32;
    r | (g << 8) | (b << 16)
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
        use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
        use windows::Win32::Foundation::HGLOBAL;

        const CF_UNICODETEXT: u32 = 13;
        OpenClipboard(None).ok()?;
        let result = GetClipboardData(CF_UNICODETEXT).ok().and_then(|h| {
            let hg = HGLOBAL(h.0);
            let ptr = GlobalLock(hg) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
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

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
