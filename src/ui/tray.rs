//! 系统托盘

use crate::platform::log;
use crate::platform::msg::WM_APP_TRAY;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::HICON;

/// v4 通知码（lParam LOWORD）
pub const NIN_POPUPOPEN: u32 = 0x0406;
pub const NIN_POPUPCLOSE: u32 = 0x0407;

/// 托盘图标封装：注册 / 更新 / 定位 / 移除
pub struct TrayIcon {
    hwnd: HWND,
    id: u32,
    registered: bool,
}

fn base_data(hwnd: HWND, id: u32) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: id,
        ..Default::default()
    }
}

impl TrayIcon {
    /// 注册托盘图标：`hwnd` 为接收回调消息的隐藏窗口。
    pub fn new(hwnd: HWND, hicon: HICON) -> Option<Self> {
        unsafe {
            let mut nid = base_data(hwnd, 1);
            // 不注册 NIF_TIP：tooltip 弹在光标上方，恰是置顶面板所在区域，必然被遮挡
            nid.uFlags = NIF_ICON | NIF_MESSAGE;
            nid.uCallbackMessage = WM_APP_TRAY;
            nid.hIcon = hicon;
            if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                log(&format!(
                    "[Quotify] 托盘 NIM_ADD 失败: {}",
                    windows::core::HRESULT::from_thread()
                ));
                return None;
            }
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            if Shell_NotifyIconW(NIM_SETVERSION, &nid).as_bool() {
                Some(Self {
                    hwnd,
                    id: 1,
                    registered: true,
                })
            } else {
                log("[Quotify] 托盘 SETVERSION 失败");
                // NIM_ADD 已成功：不撤销注册的话任务栏会残留幽灵图标
                let mut icon = Self {
                    hwnd,
                    id: 1,
                    registered: true,
                };
                icon.remove();
                None
            }
        }
    }

    /// 更新托盘图标
    pub fn update_icon(&self, hicon: HICON) {
        let mut nid = base_data(self.hwnd, self.id);
        nid.uFlags = NIF_ICON;
        nid.hIcon = hicon;
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// 托盘图标在屏幕上的矩形，面板弹出的定位锚点
    pub fn rect(&self) -> Option<RECT> {
        let ident = NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.hwnd,
            uID: self.id,
            ..Default::default()
        };
        unsafe { Shell_NotifyIconGetRect(&ident).ok() }
    }

    /// 移除托盘图标：退出时必须调用，否则任务栏残留幽灵图标。
    pub fn remove(&mut self) {
        if self.registered {
            let nid = base_data(self.hwnd, self.id);
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            self.registered = false;
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        self.remove();
    }
}

/// 解析 v4 回调：返回 (通知码, 图标id)。
pub fn parse_callback(lparam: LPARAM) -> (u32, u32) {
    let lo = (lparam.0 as u32) & 0xFFFF;
    let hi = ((lparam.0 as u32) >> 16) & 0xFFFF;
    (lo, hi)
}

/// v4 下 WM_CONTEXTMENU 的坐标在 wParam，为屏幕坐标。
pub fn context_menu_pos(wparam: WPARAM) -> POINT {
    POINT {
        x: (wparam.0 & 0xFFFF) as u16 as i16 as i32,
        y: ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
    }
}
