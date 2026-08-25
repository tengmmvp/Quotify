//! 后台轮询线程：按可变间隔拉取用量，结果 PostMessage 回主线程。
//!
//! 唤醒语义：`wake` 事件被设置时立即拉取一次并重置计时（手动刷新 /
//! 间隔变更共用）；自然到期也拉取。线程只在等待与请求时占用，无轮询空转。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::api::client::AccountSpec;

/// 一次轮询的结果（跨线程传递给主线程）。
pub enum PollOutcome {
    Success(Box<crate::api::UsageSnapshot>),
    Failure(Box<crate::api::FetchError>),
}

/// poller 眼中的「当前要查什么」。主线程在配置/选中账号变更时更新。
pub type PollTarget = Arc<Mutex<Option<AccountSpec>>>;
/// 当前轮询间隔（秒）。主线程在设置变更时更新。
pub type PollInterval = Arc<Mutex<u64>>;

/// 回传结果用的自定义消息。
pub const WM_APP_POLL_RESULT: u32 = WM_APP + 2;

pub struct Poller {
    wake: HANDLE,
    stop: HANDLE,
    refresh_requested: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Poller {
    /// 启动轮询线程。`hwnd` 接收 `WM_APP_POLL_RESULT`（wparam 为
    /// `Box::into_raw(Box<PollOutcome>)`，主线程负责取回并释放）。
    pub fn spawn(hwnd: windows::Win32::Foundation::HWND, target: PollTarget, interval: PollInterval) -> Option<Self> {
        unsafe {
            let wake = CreateEventW(None, false, false, None).ok()?;
            let stop = CreateEventW(None, true, false, None).ok()?;
            let refresh_requested = Arc::new(AtomicBool::new(false));
            let flag = refresh_requested.clone();

            // Win32 句柄是内核对象引用，跨线程移动安全；HANDLE 裸指针
            // 包装不实现 Send，这里显式声明。
            struct SendHandle<T>(T);
            unsafe impl<T> Send for SendHandle<T> {}

            let (hwnd_s, wake_s, stop_s) = (SendHandle(hwnd), SendHandle(wake), SendHandle(stop));
            let thread = std::thread::Builder::new()
                .name("quotify-poller".into())
                .spawn(move || {
                    // 先移动整个 SendHandle 到闭包局部，避免 2021 精准捕获
                    // 直接捕获内部的裸 HWND/HANDLE 导致闭包不满足 Send
                    let (h, w, s) = (hwnd_s, wake_s, stop_s);
                    poll_loop(h.0, target, interval, w.0, s.0, flag)
                })
                .ok()?;
            Some(Self { wake, stop, refresh_requested, thread: Some(thread) })
        }
    }

    /// 手动刷新：立即拉取一次。
    pub fn refresh_now(&self) {
        self.refresh_requested.store(true, Ordering::Release);
        unsafe { let _ = SetEvent(self.wake); };
    }

    /// 间隔或账号变更：不立即拉取，仅重置计时。
    pub fn reschedule(&self) {
        unsafe { let _ = SetEvent(self.wake); };
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        unsafe { let _ = SetEvent(self.stop); };
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
    let mut had_target = false;

    loop {
        let now = Instant::now();
        let wait_ms = if next_due > now {
            (next_due - now).as_millis().min(u32::MAX as u128) as u32
        } else {
            0
        };
        let wait = unsafe { WaitForMultipleObjects(&handles, false, wait_ms) };

        if wait == WAIT_OBJECT_0 {
            return; // stop
        }
        let _ = WAIT_TIMEOUT;
        // wake 或超时：判断是否需要拉取
        let manual = refresh_flag.swap(false, Ordering::AcqRel);
        let due = Instant::now() >= next_due;
        if !manual && !due {
            continue;
        }

        let spec = {
            let guard = target.lock().unwrap();
            match guard.clone() {
                Some(v) => v,
                None => {
                    // 无账号：挂起等下一次唤醒，不空转
                    if had_target {
                        had_target = false;
                    }
                    next_due = Instant::now() + Duration::from_secs(60);
                    continue;
                }
            }
        };
        had_target = true;

        let outcome = match crate::api::client::fetch_usage(&spec) {
            Ok(s) => PollOutcome::Success(Box::new(s)),
            Err(e) => PollOutcome::Failure(Box::new(e)),
        };
        let boxed = Box::into_raw(Box::new(outcome));
        unsafe {
            let _ = PostMessageW(
                Some(hwnd),
                WM_APP_POLL_RESULT,
                WPARAM(boxed as usize),
                Default::default(),
            );
        }

        let secs = interval.lock().unwrap().clone().max(10);
        next_due = Instant::now() + Duration::from_secs(secs);
    }
}
