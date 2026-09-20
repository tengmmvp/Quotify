//! Clipboard read/write

use windows::Win32::Foundation::HWND;
use windows::Win32::System::DataExchange::OpenClipboard;

/// Inner unit cap for clipboard reads: caller budgets sit far below this,
/// and the cap defends against a budget-less call materializing the whole
/// string and spiking memory.
const CLIPBOARD_MAX_UNITS: usize = 512 * 1024;

/// Clipboard format id for Unicode text: one module-level owner — the
/// windows-rs constant lives in a Win32_System_Ole feature this build
/// does not enable, and a per-function copy invites a silent typo that
/// routes Get/SetClipboardData to the wrong format.
const CF_UNICODETEXT: u32 = 13;

/// Open the clipboard with a brief retry loop: clipboard managers, history
/// features, and RDP sync routinely hold it open for a moment, and a
/// single-shot open makes the user's copy/paste a silent no-op — the
/// toolkit-standard contention handling (Chromium and peers retry too).
/// The sleeps run on the UI thread, so the budget stays tight (12ms worst
/// case — a pumped wait would re-enter the message loop this file's
/// callers carefully avoid). The owner argument follows OpenClipboard's own
/// semantics (None for reads).
fn open_clipboard_retried(owner: Option<HWND>) -> bool {
    for attempt in 0..5 {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return true;
        }
        if attempt < 4 {
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
    }
    false
}

/// Read Unicode text from the clipboard; max_units is the UTF-16 unit
/// budget, beyond which only the head is taken.
pub fn read_text(max_units: usize) -> Option<String> {
    unsafe {
        use windows::Win32::Foundation::HGLOBAL;
        use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData};
        use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

        if !open_clipboard_retried(None) {
            return None;
        }
        let result = GetClipboardData(CF_UNICODETEXT).ok().and_then(|h| {
            let hg = HGLOBAL(h.0);
            let ptr = GlobalLock(hg) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let max_units = (GlobalSize(hg) / 2).min(max_units.min(CLIPBOARD_MAX_UNITS));
            let mut len = 0usize;
            while len < max_units && *ptr.add(len) != 0 {
                len += 1;
            }
            // A budget cut may land between a surrogate pair: back off the
            // high half, or from_utf16_lossy emits a literal U+FFFD that
            // slips past the paste filter's control-character screen.
            if len == max_units
                && len > 0
                && crate::platform::is_utf16_high_surrogate(*ptr.add(len - 1))
            {
                len -= 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(hg);
            Some(s)
        });
        let _ = CloseClipboard();
        result
    }
}

/// Write Unicode text to the clipboard; owner is a window of this process:
/// per the EmptyClipboard contract, a NULL owner makes the subsequent
/// SetClipboardData fail on paths that honor the documented behavior.
/// Returns true only when the whole chain succeeds, which the caller uses
/// for its feedback.
pub fn write_text(owner: HWND, text: &str) -> bool {
    unsafe {
        use windows::Win32::Foundation::GlobalFree;
        use windows::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, SetClipboardData,
        };
        use windows::Win32::System::Memory::{
            GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
        };

        if !open_clipboard_retried(Some(owner)) {
            crate::platform::log("剪贴板打开失败");
            return false;
        }
        let ok = (|| {
            use windows::Win32::Foundation::HANDLE;
            // The staging allocation happens before EmptyClipboard: a
            // failure here (memory pressure) must not have already
            // destroyed the user's previous clipboard content — only the
            // SetClipboardData step, which cannot precede EmptyClipboard,
            // risks the old content, and that risk is inherent to the API.
            let units = crate::platform::wide(text);
            let bytes = units.len() * 2;
            let hg = match GlobalAlloc(GMEM_MOVEABLE, bytes) {
                Ok(h) => h,
                Err(_) => return false,
            };
            let dst = GlobalLock(hg);
            if dst.is_null() {
                // Ownership was not transferred to the system, so release
                // it ourselves to avoid a leak.
                let _ = GlobalFree(Some(hg));
                return false;
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), dst as *mut u16, units.len());
            let _ = GlobalUnlock(hg);
            let Ok(()) = EmptyClipboard() else {
                let _ = GlobalFree(Some(hg));
                return false;
            };
            if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hg.0))).is_err() {
                // Same as above: the system did not take over the handle.
                let _ = GlobalFree(Some(hg));
                return false;
            }
            true
        })();
        let _ = CloseClipboard();
        if !ok {
            crate::platform::log("剪贴板写入失败");
        }
        ok
    }
}
