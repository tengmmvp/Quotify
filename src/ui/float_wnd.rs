//! 浮动窗口骨架

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetForegroundWindow, HTCLIENT, IDC_ARROW,
    IDC_HAND, IsWindowVisible, KillTimer, LoadCursorW, RegisterClassW, SW_HIDE, SW_SHOW,
    SWP_NOZORDER, SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    WM_ERASEBKGND, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_SETCURSOR, WM_TIMER, WNDCLASSW,
    WNDPROC, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::PCWSTR;

use crate::platform::wide;
use crate::ui::panel::app_from_tray;
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::render::Renderer;
use crate::ui::panel::{PanelMode, track_leave};
use crate::ui::{x_of, y_of};

/// 关闭巡检：前台被夺走即收起[弹窗叠加面板已收起判定]
const TIMER_SWEEP: usize = 1;
/// 弹出动画帧时钟
pub(crate) const TIMER_ANIM: usize = 2;

/// 浮动窗口种类：wndproc 公共臂据此分派到具体窗口
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FloatKind {
    Popup,
    About,
}

/// 浮动窗口骨架： hwnd/renderer/dpi 与建类标记，公共生命周期方法承载
pub struct FloatWnd {
    pub hwnd: Option<HWND>,
    pub renderer: Option<Renderer>,
    pub(crate) dpi: f32,
    class_registered: bool,
}

impl FloatWnd {
    pub fn new() -> Self {
        Self {
            hwnd: None,
            renderer: None,
            dpi: crate::ui::panel::FALLBACK_DPI,
            class_registered: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.hwnd
            .is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() })
    }

    /// 注册窗口类并建窗；类只注册一次，窗口销毁后复用注册标记
    pub(crate) fn ensure_window(
        &mut self,
        parent: HWND,
        class: &str,
        wndproc: WNDPROC,
        log_tag: &str,
    ) -> Option<HWND> {
        if let Some(h) = self.hwnd {
            return Some(h);
        }
        unsafe {
            if !self.class_registered {
                let hinst = GetModuleHandleW(None).ok()?;
                let name = wide(class);
                let wc = WNDCLASSW {
                    lpfnWndProc: wndproc,
                    hInstance: hinst.into(),
                    lpszClassName: PCWSTR(name.as_ptr()),
                    hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(std::ptr::null_mut()),
                    hIcon: crate::ui::icon::app_icon(hinst.into())?,
                    ..Default::default()
                };
                if RegisterClassW(&wc) == 0 {
                    return None;
                }
                self.class_registered = true;
            }
            let hinst = GetModuleHandleW(None).ok()?;
            let name = wide(class);
            let hwnd = CreateWindowExW(
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
            )
            .unwrap_or_else(|e| {
                crate::platform::log(&format!("[Quotify] {log_tag}创建失败: {e}"));
                HWND::default()
            });
            if hwnd.is_invalid() {
                return None;
            }
            let pref = windows::Win32::Graphics::Dwm::DWMWCP_DEFAULT;
            let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE>()
                    as u32,
            );
            self.hwnd = Some(hwnd);
            Some(hwnd)
        }
    }

    /// 定位算完后共用的现身序列：重置悬停与淡入、上屏、抢前台、起双定时器
    pub(crate) fn show_at(&mut self, h: HWND, x: i32, y: i32, w: i32, hgt: i32) {
        unsafe {
            let _ = SetWindowPos(h, None, x, y, w, hgt, SWP_NOZORDER);
            if let Some(r) = self.renderer.as_mut() {
                r.hover = None;
                r.anim.appear = Some(crate::ui::panel::anim::Tween::now(180));
            }
            let _ = ShowWindow(h, SW_SHOW);
            // 自身必须成为前台：点击外部才构成关闭信号（同托盘菜单的前台要求）
            let _ = SetForegroundWindow(h);
            SetTimer(Some(h), TIMER_SWEEP, 200, None);
            SetTimer(Some(h), TIMER_ANIM, 16, None);
            let _ = InvalidateRect(Some(h), None, false);
        }
    }

    pub fn close(&mut self) {
        if let Some(h) = self.hwnd {
            unsafe {
                let _ = ShowWindow(h, SW_HIDE);
                let _ = KillTimer(Some(h), TIMER_SWEEP);
                let _ = KillTimer(Some(h), TIMER_ANIM);
            }
        }
        // 动画期间的 alpha 档位画刷随收起清空，缓存不跨开合累积
        if let Some(r) = self.renderer.as_mut() {
            r.clear_brush_cache();
        }
    }
}

/// 骨架在 App 上的落点随种类而异：弹窗挂 popup、关于窗挂 about
fn float_of(app: &mut crate::app::App, kind: FloatKind) -> &mut FloatWnd {
    match kind {
        FloatKind::Popup => &mut app.popup.wnd,
        FloatKind::About => &mut app.about.wnd,
    }
}

/// 收起走外层 close：关于窗要连带复位展开态，不能只收骨架
fn close_float(app: &mut crate::app::App, kind: FloatKind) {
    match kind {
        FloatKind::Popup => app.popup.close(),
        FloatKind::About => app.about.close(),
    }
    // 浮窗全收后面板也隐藏时即刻归还工作集——轮询回传只在数据到达时
    // 触发，长间隔下静止窗口会一直占着内存
    if app.panel.mode == PanelMode::Hidden && !app.popup.is_open() && !app.about.is_open() {
        crate::platform::trim_working_set();
    }
}

/// 浮动窗口的窗口过程公共臂；差异点（stale 判定、paint 调用、点击分派
/// 目标）按 kind 就地分流。两窗的 wndproc 是薄包装
pub(crate) fn float_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    kind: FloatKind,
) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                // 不走 BeginPaint：验证客户区后直接渲染
                let _ = ValidateRect(Some(hwnd), None);
                if let Some(app) = app_from_tray(hwnd) {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let dpi = float_of(app, kind).dpi;
                    let cached = float_of(app, kind).renderer.take();
                    let fresh = cached.is_none();
                    let mut renderer = cached.or_else(|| Renderer::new(hwnd, &rect, dpi));
                    let mut keep = true;
                    if let Some(r) = renderer.as_mut() {
                        if fresh {
                            r.theme = crate::ui::panel::theme::Theme::new(
                                crate::ui::panel::theme::resolved(
                                    app.config.general.appearance.as_deref(),
                                ),
                            );
                            // 首开时 renderer 尚未建成，open 里的淡入没设上，这里补
                            r.anim.appear = Some(crate::ui::panel::anim::Tween::now(180));
                        }
                        let model = PanelModel::from_app(app);
                        keep = match kind {
                            FloatKind::Popup => r.paint_popup(hwnd, &rect, &model, dpi),
                            FloatKind::About => {
                                let expanded = app.about.news_expanded;
                                let egg = app.about.egg.as_ref().map(|t| t.progress());
                                let eaten = app.about.egg_eaten;
                                r.paint_about(hwnd, &rect, &model, expanded, egg, eaten, dpi)
                            }
                        };
                    }
                    if keep {
                        float_of(app, kind).renderer = renderer;
                    } else {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => match wparam.0 {
                TIMER_SWEEP => {
                    let app = app_from_tray(hwnd);
                    let stale = match app.as_ref() {
                        // 前台被夺走即失锚；弹窗还锚在面板上，面板收起同判
                        Some(app) => {
                            GetForegroundWindow() != hwnd
                                || (kind == FloatKind::Popup && app.panel.mode == PanelMode::Hidden)
                        }
                        None => true,
                    };
                    if stale && let Some(app) = app {
                        close_float(app, kind);
                    }
                    LRESULT(0)
                }
                TIMER_ANIM => {
                    let mut app = app_from_tray(hwnd);
                    let mut egg_alive = false;
                    if let Some(a) = app.as_mut()
                        && kind == FloatKind::About
                    {
                        let finished = a.about.egg.as_ref().is_some_and(|t| t.finished());
                        if finished {
                            a.about.egg = None;
                            a.about.egg_eaten = true;
                        }
                        egg_alive = a.about.egg.is_some();
                    }
                    let appear_done = app.as_ref().is_none_or(|a| {
                        let f = match kind {
                            FloatKind::Popup => &a.popup.wnd,
                            FloatKind::About => &a.about.wnd,
                        };
                        f.renderer
                            .as_ref()
                            .and_then(|r| r.anim.appear.as_ref().map(|t| t.finished()))
                            .unwrap_or(true)
                    });
                    if appear_done && !egg_alive {
                        let _ = KillTimer(Some(hwnd), TIMER_ANIM);
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            },
            WM_MOUSEMOVE => {
                // 每次移动重挂 TME_LEAVE：一次性通知，靠反复挂载续命
                track_leave(hwnd);
                if let Some(app) = app_from_tray(hwnd) {
                    let dpi = float_of(app, kind).dpi;
                    let (x, y) = (x_of(lparam) / dpi, y_of(lparam) / dpi);
                    if let Some(r) = float_of(app, kind).renderer.as_mut() {
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
                // 离窗清除残留高亮
                if let Some(app) = app_from_tray(hwnd)
                    && let Some(r) = float_of(app, kind).renderer.as_mut()
                    && r.hover.take().is_some()
                {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if let Some(app) = app_from_tray(hwnd) {
                    let dpi = float_of(app, kind).dpi;
                    let (x, y) = (x_of(lparam) / dpi, y_of(lparam) / dpi);
                    let hit = float_of(app, kind)
                        .renderer
                        .as_ref()
                        .and_then(|r| r.hit_at(x, y));
                    // 命中统一走面板的 handle_panel_hit；分派目标随窗而异：
                    // 弹窗的 PickAccount 臂要选账号收弹窗并重排面板，目标须是
                    // 面板 hwnd；关于窗尾部的 InvalidateRect 重绘自身，目标即自身
                    let target = match kind {
                        FloatKind::Popup => app.panel.hwnd,
                        FloatKind::About => Some(hwnd),
                    };
                    if let (Some(hit), Some(t)) = (hit, target) {
                        crate::app::handle_panel_hit(app, hit, t);
                    }
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SETCURSOR => {
                let hit_hwnd = HWND(wparam.0 as *mut _);
                if hit_hwnd == hwnd && (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                    let hand = app_from_tray(hwnd)
                        .and_then(|a| float_of(a, kind).renderer.as_ref())
                        .and_then(|r| r.hover)
                        .is_some_and(|h| h != crate::ui::panel::render::Hit::AboutLogo);
                    let cursor = if hand { IDC_HAND } else { IDC_ARROW };
                    if let Ok(c) = LoadCursorW(None, cursor) {
                        let _ = SetCursor(Some(c));
                        return LRESULT(1);
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
