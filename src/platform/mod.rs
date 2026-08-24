//! Windows 平台服务：开机自启、系统通知、单实例互斥。

pub mod autostart;
pub mod instance;
pub mod notify;

/// 归还工作集：面板收起 / 数据拉取完成后调用，把 D2D、TLS 等
/// 运行时换出的页面还给系统，保持静止时低内存（下次使用按需换入）。
pub fn trim_working_set() {
    unsafe {
        use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
        use windows::Win32::System::Threading::GetCurrentProcess;
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}
