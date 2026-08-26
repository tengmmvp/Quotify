//! Windows 平台服务

pub mod autostart;
pub mod instance;
pub mod menu_theme;
pub mod notify;

use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;
use windows::core::PCWSTR;

/// str → UTF-16
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 统一诊断日志
pub fn log(msg: &str) {
    let w = wide(msg);
    unsafe { OutputDebugStringW(PCWSTR(w.as_ptr())) };
}

/// WM_APP 系自定义消息统一分配
pub mod msg {
    use super::WM_APP;
    /// 托盘回调
    pub const WM_APP_TRAY: u32 = WM_APP + 1;
    /// 轮询结果回传
    pub const WM_APP_POLL_RESULT: u32 = WM_APP + 2;
    /// 二次启动唤醒已有实例
    pub const WM_APP_WAKE_INSTANCE: u32 = WM_APP + 3;
    /// 检查更新结果回传
    pub const WM_APP_UPDATE_RESULT: u32 = WM_APP + 4;
}

/// 用默认浏览器打开链接
pub fn open_url(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        log(&format!("[Quotify] 拒绝非 http(s) 链接: {url}"));
        return;
    }
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
    let verb = wide("open");
    let w = wide(url);
    let r = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(w.as_ptr()),
            None,
            None,
            SW_SHOW,
        )
    };
    // 返回值 <= 32 表示失败
    if r.0 as isize <= 32 {
        log(&format!("[Quotify] 打开链接失败: {url}"));
    }
}

/// 模态保存文件对话框；取消返回 None
pub fn save_dialog(default_name: &str) -> Option<std::path::PathBuf> {
    file_dialog(default_name, true)
}

/// 模态打开文件对话框；取消返回 None
pub fn open_dialog() -> Option<std::path::PathBuf> {
    file_dialog("", false)
}

/// 传统文件对话框共用实现
fn file_dialog(default_name: &str, save: bool) -> Option<std::path::PathBuf> {
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
        OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    use windows::core::PWSTR;
    // 过滤串以双 nul 收尾
    let filter: Vec<u16> = "JSON (*.json)\0*.json\0All files (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let mut file = [0u16; 260];
    for (i, c) in default_name.encode_utf16().take(259).enumerate() {
        file[i] = c;
    }
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        Flags: OFN_PATHMUSTEXIST
            | OFN_NOCHANGEDIR
            | if save {
                OFN_OVERWRITEPROMPT
            } else {
                OFN_FILEMUSTEXIST
            },
        ..Default::default()
    };
    let ok = unsafe {
        if save {
            GetSaveFileNameW(&mut ofn)
        } else {
            GetOpenFileNameW(&mut ofn)
        }
    };
    if !ok.as_bool() {
        return None;
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(0);
    Some(std::path::PathBuf::from(String::from_utf16_lossy(
        &file[..len],
    )))
}
