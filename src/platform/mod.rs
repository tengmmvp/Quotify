//! Windows 平台服务

pub mod autostart;
pub mod instance;
pub mod notify;

use windows::core::PCWSTR;
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;

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

/// 归还工作集，静止时保持低内存；换出页面按需换回
pub fn trim_working_set() {
    unsafe {
        use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
        use windows::Win32::System::Threading::GetCurrentProcess;
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}
