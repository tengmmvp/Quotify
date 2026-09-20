//! System tray

use crate::platform::log;
use crate::platform::msg::WM_APP_TRAY;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, HICON};

/// Message id of the explorer restart broadcast: registered once per process;
/// on arrival the tray icon is re-registered.
pub static TASKBAR_CREATED: std::sync::LazyLock<u32> = std::sync::LazyLock::new(|| unsafe {
    let name: Vec<u16> = "TaskbarCreated"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let msg = windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(
        windows::core::PCWSTR(name.as_ptr()),
    );
    // A failed registration returns 0, which collides with the menu teardown WM_NULL and
    // would falsely trigger re-registration; substitute a message id that can never be valid.
    if msg == 0 { u32::MAX } else { msg }
});

/// Tray icon wrapper: register / update / locate / remove.
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

/// Copies tooltip text into szTip; capacity is 128 including the terminator, and
/// overlong text is truncated. If the last unit
/// falls on the high surrogate of a pair, back off one unit so no lone surrogate remains.
fn write_tip(nid: &mut NOTIFYICONDATAW, tip: &str) {
    let mut end = 0usize;
    for (i, u) in tip.encode_utf16().take(127).enumerate() {
        nid.szTip[i] = u;
        end = i + 1;
    }
    if crate::platform::is_utf16_high_surrogate(nid.szTip[end.saturating_sub(1)]) {
        // Saturating: end==0 leaves nothing to back off from, and a bare
        // decrement would underflow (debug panic; release wrap).
        end = end.saturating_sub(1);
    }
    if end < nid.szTip.len() {
        nid.szTip[end] = 0;
    }
}

impl TrayIcon {
    /// Registers the tray icon; `hwnd` is the callback window. Registration carries an
    /// app-name tooltip — an icon with no
    /// text gets a forced empty box on Win11. Intermittent NIM_ADD failures do not
    /// discard the instance; the
    /// retry timer and TaskbarCreated take over.
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

    /// Updates the tray icon.
    /// Returns whether NIM_MODIFY succeeded; failures can be retried.
    pub fn update_icon(&self, hicon: HICON) -> bool {
        let mut nid = base_data(self.hwnd, self.id);
        nid.uFlags = NIF_ICON;
        nid.hIcon = hicon;
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() }
    }

    /// Updates the tooltip text
    pub fn set_tooltip(&self, tip: &str) -> bool {
        let mut nid = base_data(self.hwnd, self.id);
        nid.uFlags = NIF_TIP;
        write_tip(&mut nid, tip);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() }
    }

    /// Re-register after an explorer restart: called when the TaskbarCreated broadcast arrives.
    /// The old icon died with the taskbar process, so a plain add is enough;
    /// on failure it must wait for the next explorer restart.
    /// Returns whether NIM_ADD succeeded — NIM_ADD is a cross-process pump point;
    /// `registered` is written back
    /// by the caller after re-taking the reference, and this method never writes state itself.
    pub fn readd(&self, hicon: HICON, tip: &str) -> bool {
        let mut nid = base_data(self.hwnd, self.id);
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = hicon;
        write_tip(&mut nid, tip);
        let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &nid).as_bool() };
        if !ok {
            log(&format!(
                "[Quotify] 托盘重注册失败: {}",
                windows::core::HRESULT::from_thread()
            ));
        }
        ok
    }

    /// The tray icon's on-screen rectangle; the positioning anchor for the panel popup.
    pub fn rect(&self) -> Option<RECT> {
        let ident = NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.hwnd,
            uID: self.id,
            ..Default::default()
        };
        unsafe { Shell_NotifyIconGetRect(&ident).ok() }
    }

    /// Removes the tray icon: must be called on exit, or a ghost icon lingers in the taskbar.
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

/// Parses the callback: returns the notification code.
pub fn parse_callback(lparam: LPARAM) -> u32 {
    (lparam.0 as u32) & 0xFFFF
}

/// The v0 callback carries no coordinates, so context-menu positioning takes the current
/// cursor location.
pub fn context_menu_pos() -> POINT {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    pt
}
