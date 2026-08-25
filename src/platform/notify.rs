//! 系统通知

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    NIF_INFO, NIIF_LARGE_ICON, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};

/// 弹出气泡通知
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
