//! 后台轮询线程

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::api::client::AccountSpec;
use crate::app::config::MIN_POLL_SECS;
pub(crate) use crate::platform::msg::WM_APP_POLL_RESULT;

/// 轮询间隔上限（秒）= 1 天
pub const MAX_POLL_SECS: u64 = 86400;

/// 读取间隔并夹取到合法区间
fn clamp_interval(secs: u64) -> u64 {
    secs.clamp(MIN_POLL_SECS, MAX_POLL_SECS)
}

/// 一次轮询的结果，跨线程传递给主线程。
pub enum PollOutcome {
    Success(Box<crate::api::UsageSnapshot>),
    Failure(Box<crate::api::FetchError>),
}

pub type PollTarget = Arc<Mutex<Option<AccountSpec>>>;
pub type PollInterval = Arc<Mutex<u64>>;

/// 后台轮询线程的所有者：发唤醒/停止信号，drop 时 join 回收。
pub struct Poller {
    wake: HANDLE,
    stop: HANDLE,
    refresh_requested: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Poller {
    /// 启动轮询线程
    pub fn spawn(
        hwnd: windows::Win32::Foundation::HWND,
        target: PollTarget,
        interval: PollInterval,
    ) -> Option<Self> {
        unsafe {
            let wake = CreateEventW(None, false, false, None).ok()?;
            let stop = CreateEventW(None, true, false, None).ok()?;
            let refresh_requested = Arc::new(AtomicBool::new(false));
            let flag = refresh_requested.clone();

            // Win32 句柄是内核对象引用、跨线程移动安全，但 HANDLE 未实现 Send
            struct SendHandle<T>(T);
            unsafe impl<T> Send for SendHandle<T> {}

            let (hwnd_s, wake_s, stop_s) = (SendHandle(hwnd), SendHandle(wake), SendHandle(stop));
            let thread = match std::thread::Builder::new()
                .name("quotify-poller".into())
                .spawn(move || {
                    // 先把 SendHandle 移入闭包局部，防 2021 精准捕获绕过包装破坏 Send
                    let (h, w, s) = (hwnd_s, wake_s, stop_s);
                    poll_loop(h.0, target, interval, w.0, s.0, flag)
                }) {
                Ok(t) => t,
                Err(_) => {
                    // 线程未起：Poller 不会诞生，Drop 不再兜底，须在此关闭两事件句柄防泄漏
                    let _ = windows::Win32::Foundation::CloseHandle(wake);
                    let _ = windows::Win32::Foundation::CloseHandle(stop);
                    return None;
                }
            };
            Some(Self {
                wake,
                stop,
                refresh_requested,
                thread: Some(thread),
            })
        }
    }

    /// 手动刷新：立即拉取一次。
    pub fn refresh_now(&self) {
        self.refresh_requested.store(true, Ordering::Release);
        unsafe {
            let _ = SetEvent(self.wake);
        };
    }

    /// 间隔或账号变更：不立即拉取，仅按当前间隔重排计时。
    pub fn reschedule(&self) {
        unsafe {
            let _ = SetEvent(self.wake);
        };
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop);
        };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.wake);
            let _ = windows::Win32::Foundation::CloseHandle(self.stop);
        }
    }
}

fn poll_loop(
    hwnd: windows::Win32::Foundation::HWND,
    target: PollTarget,
    interval: PollInterval,
    wake: HANDLE,
    stop: HANDLE,
    refresh_flag: Arc<AtomicBool>,
) {
    let handles = [stop, wake];
    let mut next_due = Instant::now();

    loop {
        let now = Instant::now();
        // 饱和上限取 u32::MAX - 1：u32::MAX 恰是 Win32 的 INFINITE，
        // 落在它上面会退化成永久等待；宁可提前醒来重算
        let wait_ms = if next_due > now {
            (next_due - now).as_millis().min(u32::MAX as u128 - 1) as u32
        } else {
            0
        };
        let wait = unsafe { WaitForMultipleObjects(&handles, false, wait_ms) };

        if wait == WAIT_OBJECT_0 {
            return; // stop
        }
        let manual = refresh_flag.swap(false, Ordering::AcqRel);
        let due = Instant::now() >= next_due;
        if !manual && !due {
            // 变更类唤醒不拉取，但按新间隔重排，否则新间隔要等旧周期走完才生效
            let secs = clamp_interval(*borrow(&interval));
            next_due = Instant::now() + Duration::from_secs(secs);
            continue;
        }

        let spec = {
            let guard = borrow(&target);
            match guard.clone() {
                Some(v) => v,
                None => {
                    // 无账号：挂起等下一次唤醒，不空转
                    next_due = Instant::now() + Duration::from_secs(60);
                    continue;
                }
            }
        };

        let outcome = match crate::api::client::fetch_usage(&spec) {
            Ok(s) => PollOutcome::Success(Box::new(s)),
            Err(e) => PollOutcome::Failure(Box::new(e)),
        };
        let boxed = Box::into_raw(Box::new(outcome));
        let posted = unsafe {
            PostMessageW(
                Some(hwnd),
                WM_APP_POLL_RESULT,
                WPARAM(boxed as usize),
                Default::default(),
            )
        };
        if posted.is_err() {
            // 投递失败（窗口已销毁等）：主线程不会取回指针，立即释放防泄漏
            drop(unsafe { Box::from_raw(boxed) });
        }

        let secs = clamp_interval(*borrow(&interval));
        next_due = Instant::now() + Duration::from_secs(secs);
    }
}

/// 毒化锁取回内部数据继续用
fn borrow<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_clamped_to_min_max() {
        assert_eq!(clamp_interval(0), MIN_POLL_SECS);
        assert_eq!(clamp_interval(9), MIN_POLL_SECS);
        assert_eq!(clamp_interval(300), 300);
        assert_eq!(clamp_interval(u64::MAX), MAX_POLL_SECS);
    }
}
