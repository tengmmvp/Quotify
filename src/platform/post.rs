//! 跨线程回传

use std::collections::VecDeque;
use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// 后台结果通道：入队后发无载荷唤醒码，UI 线程排空取用；消息参数
/// 不承载数据，伪造 PostMessage 至多触发一次无害取用，无从伪造地址析构。
pub struct Slot<T> {
    queue: Mutex<VecDeque<T>>,
}

impl<T: Send> Slot<T> {
    pub const fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
        }
    }

    /// 结果入队并唤醒窗口线程。投递失败只丢唤醒码，结果留队待同一
    /// 通道的下次唤醒取走——常驻轮询必有下次；更新检查与动态拉取等
    /// 一次性通道理论上可在不再触发时滞留，但单消息循环下队列满载
    /// 实际不可达；窗口已销毁则进程将退，均无泄漏。
    pub fn post(&self, hwnd: HWND, msg: u32, value: T) {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(value);
        let _ = unsafe { PostMessageW(Some(hwnd), msg, WPARAM(0), Default::default()) };
    }

    /// 取出最早入队的结果
    pub fn take(&self) -> Option<T> {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }
}

/// 起后台线程执行 work，结果经 slot 回传；返回 false 为线程创建
/// 失败，调用方须回退先行置位的状态门闩，否则永久闩死。
pub fn spawn_post<T: Send + 'static>(
    slot: &'static Slot<T>,
    hwnd: HWND,
    msg: u32,
    work: impl FnOnce() -> T + Send + 'static,
) -> bool {
    // HWND 未实现 Send：包装后须先整体移入闭包局部再取字段——
    // 2021 精准捕获只捕 .0 会绕过包装，退回捕获裸句柄
    struct SendHwnd(HWND);
    unsafe impl Send for SendHwnd {}
    let wrapper = SendHwnd(hwnd);
    match std::thread::Builder::new()
        .name("quotify-post".to_string())
        .spawn(move || {
            let h = wrapper;
            let r = work();
            slot.post(h.0, msg, r);
        }) {
        Ok(_) => true,
        Err(e) => {
            crate::platform::log(&format!("[Quotify] 后台线程创建失败: {e}"));
            false
        }
    }
}
