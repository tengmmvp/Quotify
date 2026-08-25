//! 系统通知：基于托盘图标的气球通知（现代 Windows 自动以 Toast 样式呈现）。
//!
//! 不走 WinRT AppNotification 路线：那需要 AUMID + 开始菜单快捷方式 +
//! COM 激活注册，对「单文件绿色 exe」负担过重；气球通知零注册零依赖，
//! 展示效果在 Win10/11 上与 Toast 一致。

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    NIF_INFO, NIIF_LARGE_ICON, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};

/// 弹出一条通知（title + 正文）。`hwnd`/`id` 为托盘注册时使用的窗口与图标 id。
pub fn show(hwnd: HWND, tray_id: u32, title: &str, body: &str) {
    let title16: Vec<u16> = title.encode_utf16().take(63).collect();
    let body16: Vec<u16> = body.encode_utf16().take(255).collect();

    let mut nid = NOTIFYICONDATAW {
        hWnd: hwnd,
        uID: tray_id,
        uFlags: NIF_INFO,
        ..Default::default()
    };
    nid.szInfoTitle[..title16.len()].copy_from_slice(&title16);
    if title16.len() < nid.szInfoTitle.len() {
        nid.szInfoTitle[title16.len()] = 0;
    }
    nid.szInfo[..body16.len()].copy_from_slice(&body16);
    if body16.len() < nid.szInfo.len() {
        nid.szInfo[body16.len()] = 0;
    }
    nid.dwInfoFlags = NIIF_LARGE_ICON;
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}
