//! 账号切换弹窗

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    HMONITOR, InvalidateRect, MONITOR_DEFAULTTONEAREST, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

use crate::ui::float_wnd::{FloatKind, FloatWnd, float_wndproc};
use crate::ui::panel::render::popup as view;
use crate::ui::panel::{dpi_of, monitor_info};

const POPUP_WND_CLASS: &str = "QuotifyAccountPopup";

/// 账号切换弹窗
pub struct AccountPopup {
    pub wnd: FloatWnd,
    /// 宿主面板：开合时联动重绘其 chevron。
    panel: Option<HWND>,
    /// 监视器派生缓存：(句柄, dpi, 工作区)；面板换屏自动失效，
    /// 显示配置与 DPI 变更由失效钩子处理
    monitor: Option<(HMONITOR, f32, RECT)>,
}

impl AccountPopup {
    pub fn new() -> Self {
        Self {
            wnd: FloatWnd::new(),
            panel: None,
            monitor: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.wnd.is_open()
    }

    /// 显示配置或 DPI 变更后失效派生缓存，下次对位重取
    pub fn invalidate_monitor(&mut self) {
        self.monitor = None;
    }

    /// 面板左侧弹出
    /// 贴近触发按钮定位，越出工作区左界改放右侧，纵向以面板顶对齐并
    /// 夹回工作区。
    pub fn open(&mut self, tray: HWND, panel_hwnd: HWND, accounts: usize) {
        let was_open = self.is_open();
        // 点击不激活：前台留在面板，跑马灯不因点弹窗而冻结。
        let Some(h) =
            self.wnd
                .ensure_window(tray, POPUP_WND_CLASS, Some(popup_wndproc), "账号弹窗", true)
        else {
            return;
        };
        // 状态记录与面板重绘先于 show_at：其 SW_SHOW 同步重入消息泵，
        // 泵后不得再用本引用。
        self.panel = Some(panel_hwnd);
        if !was_open {
            self.repaint_panel();
        }
        unsafe {
            let monitor = MonitorFromWindow(panel_hwnd, MONITOR_DEFAULTTONEAREST);
            // 派生缓存命中免 dpi 与工作区两次查询：拖动与高度动画的
            // 高频重对位只重取面板矩形。
            let (dpi, work) = match self.monitor {
                Some((m, dpi, work)) if m == monitor => (dpi, work),
                _ => {
                    let dpi = dpi_of(monitor).unwrap_or(crate::ui::panel::FALLBACK_DPI);
                    let work = monitor_info(monitor).rcWork;
                    self.monitor = Some((monitor, dpi, work));
                    (dpi, work)
                }
            };
            self.wnd.dpi = dpi;
            let mut pr = RECT::default();
            let _ = GetWindowRect(panel_hwnd, &mut pr);

            let w = (view::POPUP_W * dpi).round() as i32;
            let hgt = (view::popup_height(accounts) as f32 * dpi).round() as i32;
            let gap = (8.0 * dpi).round() as i32;
            let mut x = pr.left - w - gap;
            if x < work.left {
                // 右侧回退同样夹回左界：极窄工作区下宁与面板重叠不出屏。
                x = (pr.right + gap).min(work.right - w).max(work.left);
            }
            let y = pr.top.clamp(work.top, (work.bottom - hgt).max(work.top));

            self.wnd.show_at(FloatKind::Popup, h, x, y, w, hgt);
        }
    }

    pub fn close(&mut self) {
        let was_open = self.is_open();
        // 重绘标记先于隐藏：隐藏持捕获窗口会重入消息泵。
        if was_open {
            self.repaint_panel();
        }
        self.wnd.close();
    }

    /// 开合联动面板重绘：chevron 随弹窗可见性翻转。
    fn repaint_panel(&self) {
        if let Some(p) = self.panel {
            unsafe {
                let _ = InvalidateRect(Some(p), None, false);
            }
        }
    }

    /// 已开时随面板几何重对位，未开不动。
    pub fn reposition(&mut self, tray: HWND, panel_hwnd: HWND, accounts: usize) {
        if self.is_open() {
            self.open(tray, panel_hwnd, accounts);
        }
    }
}

/// 弹窗的窗口过程：公共臂走 float_wndproc，差异点由 FloatKind::Popup 注入。
pub extern "system" fn popup_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    float_wndproc(hwnd, msg, wparam, lparam, FloatKind::Popup)
}
