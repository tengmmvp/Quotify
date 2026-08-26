//! 单实例

use crate::platform::msg::WM_APP_WAKE_INSTANCE;
use crate::platform::wide;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};
use windows::core::PCWSTR;

/// 托盘隐藏窗口类名
pub const TRAY_WND_CLASS: &str = "QuotifyTrayWnd";

enum GuardState {
    First(HANDLE),
    AlreadyRunning,
}

pub struct InstanceGuard(GuardState);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if let GuardState::First(h) = self.0 {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(h);
            }
        }
    }
}

/// 尝试成为唯一实例
pub fn acquire() -> InstanceGuard {
    unsafe {
        // 优先 Global 命名空间以跨会话生效；无权限时回退 Local
        for scope in ["Global", "Local"] {
            let name = wide(&format!("{scope}\\{TRAY_WND_CLASS}.SingleInstance"));
            if let Ok(h) = CreateMutexW(None, false, PCWSTR(name.as_ptr())) {
                // CreateMutex 成功时 GetLastError 可能是 ERROR_ALREADY_EXISTS
                let already = GetLastError() == ERROR_ALREADY_EXISTS;
                if already {
                    // 此分支拿到的 h 故意不关：本实例随即唤醒并退出，泄漏至进程终止无害
                    return InstanceGuard(GuardState::AlreadyRunning);
                }
                return InstanceGuard(GuardState::First(h));
            }
        }
        // 两种命名空间都创建失败：保守放行，不让应用完全无法启动；
        // 空句柄在 Drop 里 CloseHandle 会静默失败，同样无害
        InstanceGuard(GuardState::First(HANDLE::default()))
    }
}

impl InstanceGuard {
    pub fn is_first(&self) -> bool {
        matches!(self.0, GuardState::First(_))
    }

    /// 找到已运行实例的托盘窗口，请它弹出面板。
    pub fn wake_existing(&self) {
        let class = wide(TRAY_WND_CLASS);
        unsafe {
            if let Ok(hwnd) = FindWindowW(PCWSTR(class.as_ptr()), None) {
                let _ = PostMessageW(
                    Some(hwnd),
                    WM_APP_WAKE_INSTANCE,
                    Default::default(),
                    Default::default(),
                );
            }
        }
    }
}
