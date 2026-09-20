//! Cross-thread result posting

use std::collections::VecDeque;
use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Background result channel: results are enqueued and then a payloadless
/// wake code is posted, which the UI thread drains on arrival; message
/// parameters carry no data, so a forged PostMessage at worst triggers one
/// harmless take — there is no address to forge and dereference.
pub struct Slot<T> {
    queue: Mutex<VecDeque<T>>,
}

impl<T: Send> Slot<T> {
    pub const fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
        }
    }

    /// Enqueue the result and wake the window thread. A failed post only
    /// loses the wake code; the result stays queued until the same
    /// channel's next wake takes it — the resident polling always has a
    /// next time. One-shot channels such as the update check and the news
    /// fetch could in theory linger if never triggered again, but under the
    /// single message loop a full queue is practically unreachable; and if
    /// the window is already destroyed the process is on its way out. No
    /// leak either way.
    pub fn post(&self, hwnd: HWND, msg: u32, value: T) {
        crate::locks::borrow(&self.queue).push_back(value);
        let _ = unsafe { PostMessageW(Some(hwnd), msg, WPARAM(0), Default::default()) };
    }

    /// Take the earliest enqueued result
    pub fn take(&self) -> Option<T> {
        crate::locks::borrow(&self.queue).pop_front()
    }
}

/// Wrapper making a Win32 kernel-handle value moveable into a worker
/// thread: kernel-object references are safe to use from any thread, but
/// HANDLE and friends do not implement Send. Invariant: only wrap handle
/// values — never pointers into this thread's memory. After wrapping, the
/// whole wrapper must be moved into a closure local before its field is
/// taken; a 2021 precise capture of just .0 bypasses the wrapper and
/// falls back to capturing the bare handle.
pub(crate) struct SendCell<T>(pub(crate) T);
// SAFETY: constructed only with kernel handle values, valid across threads.
unsafe impl<T> Send for SendCell<T> {}

/// One-shot background task binding a channel with its wake code: the wake
/// message is locked in at construction, so spawn and drain can never
/// mismatch message numbers; the caller declares it static, and draining
/// happens in the matching message arm of the window procedure.
pub struct BackgroundTask<T> {
    slot: Slot<T>,
    msg: u32,
}

impl<T: Send> BackgroundTask<T> {
    pub const fn new(msg: u32) -> Self {
        Self {
            slot: Slot::new(),
            msg,
        }
    }

    /// Start one background thread to run work and deliver the result
    /// through the slot; a false return means thread creation failed, and
    /// the caller must roll back the state gate it set beforehand, or it
    /// stays latched forever.
    pub fn spawn(&'static self, hwnd: HWND, work: impl FnOnce() -> T + Send + 'static) -> bool {
        let wrapper = SendCell(hwnd);
        let slot = &self.slot;
        let msg = self.msg;
        match std::thread::Builder::new()
            .name("quotify-post".to_string())
            .spawn(move || {
                let h = wrapper;
                let r = work();
                slot.post(h.0, msg, r);
            }) {
            Ok(_) => true,
            Err(e) => {
                crate::platform::log(&format!("后台线程创建失败: {e}"));
                false
            }
        }
    }

    /// Drain the posted-back results
    pub fn drain(&self) -> impl Iterator<Item = T> + '_ {
        std::iter::from_fn(|| self.slot.take())
    }
}
