//! Windows 平台服务：开机自启、系统通知、单实例互斥。
//!
//! 同时承载全仓共享的小工具：`wide`（UTF-16 转换，此前三文件各一份
//! 拷贝）、`log`（统一诊断日志）、WM_APP 系消息号（跨模块契约，
//! 此前分散四处靠人肉递增防撞号）。

pub mod autostart;
pub mod instance;
pub mod notify;

use windows::core::PCWSTR;
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;

/// str → UTF-16（含 NUL 终止），供 Win32 宽字符 API 使用。
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 统一诊断日志。release 为 windows 子系统、无 stderr，eprintln 全部
/// 静默丢失；走 OutputDebugStringW（DebugView / 事件查看器可见），
/// 零依赖零文件。**禁止打印 API key 等凭据**（toml 解析错误只报位置，
/// 不回显源行——源行可能含 key）。
pub fn log(msg: &str) {
    let w = wide(msg);
    unsafe { OutputDebugStringW(PCWSTR(w.as_ptr())) };
}

/// WM_APP 系自定义消息统一分配（tray / poller / instance / update 四方契约）。
pub mod msg {
    use super::WM_APP;
    /// 托盘回调（Shell_NotifyIcon）
    pub const WM_APP_TRAY: u32 = WM_APP + 1;
    /// 轮询结果回传（wparam = Box<PollOutcome> 指针）
    pub const WM_APP_POLL_RESULT: u32 = WM_APP + 2;
    /// 二次启动唤醒已有实例
    pub const WM_APP_WAKE_INSTANCE: u32 = WM_APP + 3;
    /// 检查更新结果回传（wparam = Box<ReleaseInfo-ish> 指针）
    pub const WM_APP_UPDATE_RESULT: u32 = WM_APP + 4;
}

/// 归还工作集：面板收起 / 数据拉取完成后调用，把 D2D、TLS 等
/// 运行时换出的页面还给系统，保持静止时低内存（下次使用按需换入）。
pub fn trim_working_set() {
    unsafe {
        use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
        use windows::Win32::System::Threading::GetCurrentProcess;
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}
