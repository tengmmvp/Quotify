//! 跨线程回传

use windows::Win32::Foundation::{HWND, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// 装箱结果并投递给窗口线程。投递失败[窗口已销毁等]时主线程不会
/// 取回指针，就地回收防泄漏。装箱值将跨线程抵达 UI 线程，Send
/// 约束把可跨线程性显式化，防未来误传非 Send 类型
pub fn post_boxed<T: Send>(hwnd: HWND, msg: u32, value: T) {
    let boxed = Box::into_raw(Box::new(value));
    let posted =
        unsafe { PostMessageW(Some(hwnd), msg, WPARAM(boxed as usize), Default::default()) };
    if posted.is_err() {
        drop(unsafe { Box::from_raw(boxed) });
    }
}

/// 起后台线程执行 work，结果装箱投递回窗口线程。
pub fn spawn_post<T: Send + 'static>(
    hwnd: HWND,
    msg: u32,
    work: impl FnOnce() -> T + Send + 'static,
) {
    // HWND 未实现 Send：包装后须先整体移入闭包局部再取字段——
    // 2021 精准捕获只捕 .0 会绕过包装，退回捕获裸句柄
    struct SendHwnd(HWND);
    unsafe impl Send for SendHwnd {}
    let wrapper = SendHwnd(hwnd);
    let _ = std::thread::Builder::new()
        .name("quotify-post".to_string())
        .spawn(move || {
            let h = wrapper;
            let r = work();
            post_boxed(h.0, msg, r);
        });
}
