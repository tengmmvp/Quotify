//! 系统托盘

use crate::platform::log;
use crate::platform::msg::WM_APP_TRAY;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, HICON};

/// explorer 重启广播的消息号：进程内注册一次，到达即重注册托盘图标
pub static TASKBAR_CREATED: std::sync::LazyLock<u32> = std::sync::LazyLock::new(|| unsafe {
    let name: Vec<u16> = "TaskbarCreated"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let msg = windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(
        windows::core::PCWSTR(name.as_ptr()),
    );
    // 注册失败返回 0 会与菜单收尾的 WM_NULL 撞车误触发重注册；换永不合法的消息号
    if msg == 0 { u32::MAX } else { msg }
});

/// 托盘图标封装：注册 / 更新 / 定位 / 移除。
pub struct TrayIcon {
    hwnd: HWND,
    id: u32,
    pub registered: bool,
}

fn base_data(hwnd: HWND, id: u32) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: id,
        ..Default::default()
    }
}

/// tooltip 文本拷入 szTip；上限 128 含结尾，超长截断
fn write_tip(nid: &mut NOTIFYICONDATAW, tip: &str) {
    for (i, u) in tip.encode_utf16().take(127).enumerate() {
        nid.szTip[i] = u;
    }
}

impl TrayIcon {
    /// 注册托盘图标，`hwnd` 为回调窗口；注册即带应用名 tooltip——无文本
    /// 的图标在 Win11 会被强弹空白框。NIM_ADD 间歇失败不丢弃实例，交由
    /// 重试定时器与 TaskbarCreated 接管。
    pub fn new(hwnd: HWND, hicon: HICON) -> Self {
        unsafe {
            let mut nid = base_data(hwnd, 1);
            nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
            nid.uCallbackMessage = WM_APP_TRAY;
            nid.hIcon = hicon;
            write_tip(&mut nid, "Quotify");
            let ok = Shell_NotifyIconW(NIM_ADD, &nid).as_bool();
            if !ok {
                log(&format!(
                    "[Quotify] 托盘 NIM_ADD 失败: {}",
                    windows::core::HRESULT::from_thread()
                ));
            }
            Self {
                hwnd,
                id: 1,
                registered: ok,
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

    /// 更新 tooltip 文本
    pub fn set_tooltip(&self, tip: &str) {
        let mut nid = base_data(self.hwnd, self.id);
        nid.uFlags = NIF_TIP;
        write_tip(&mut nid, tip);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// explorer 重启后的重注册：TaskbarCreated 广播到达时调用；旧图标
    /// 随任务栏进程消亡，直接补注册即可，失败需待 explorer 下次重启
    pub fn readd(&mut self, hicon: HICON, tip: &str) {
        let mut nid = base_data(self.hwnd, self.id);
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = hicon;
        write_tip(&mut nid, tip);
        unsafe {
            if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                self.registered = true;
            } else {
                log(&format!(
                    "[Quotify] 托盘重注册失败: {}",
                    windows::core::HRESULT::from_thread()
                ));
            }
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

/// 解析回调：返回通知码。
pub fn parse_callback(lparam: LPARAM) -> u32 {
    (lparam.0 as u32) & 0xFFFF
}

/// v0 回调不带坐标，右键菜单定位取当前光标位置
pub fn context_menu_pos() -> POINT {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    pt
}
