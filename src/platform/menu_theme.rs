//! Tray menu follows the theme
//!
//! No official API exists for dark menus: uxtheme exports these only by
//! ordinal (135=SetPreferredAppMode, 136=FlushMenuThemes), available on
//! Win10 1809+; when they cannot be resolved, the system default light
//! menu stays.

use std::sync::OnceLock;

use windows::Win32::Foundation::FARPROC;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{PCSTR, PCWSTR};

use super::wide;

type SetMode = unsafe extern "system" fn(i32) -> i32;
type Flush = unsafe extern "system" fn();

/// Process-wide menu theme entry points, loaded once and cached; if either
/// ordinal is missing, the whole feature is unavailable.
fn entries() -> Option<(SetMode, Flush)> {
    static FNS: OnceLock<Option<(SetMode, Flush)>> = OnceLock::new();
    *FNS.get_or_init(|| unsafe {
        let lib = LoadLibraryW(PCWSTR(wide("uxtheme.dll").as_ptr())).ok()?;
        // GetProcAddress convention: a name pointer whose numeric value is
        // < 0x10000 is resolved as an ordinal.
        let set = GetProcAddress(lib, PCSTR(135usize as *const u8))?;
        let flush = GetProcAddress(lib, PCSTR(136usize as *const u8))?;
        Some((
            std::mem::transmute::<FARPROC, SetMode>(Some(set)),
            std::mem::transmute::<FARPROC, Flush>(Some(flush)),
        ))
    })
}

/// Locks the light/dark mode of menus popped from now on, in sync with the
/// panel's resolved appearance.
pub fn apply(dark: bool) {
    if let Some((set, flush)) = entries() {
        unsafe {
            // PreferredAppMode: Default=0/AllowDark=1/ForceDark=2/
            // ForceLight=3. Use the Force values to pin the target state
            // directly, without depending on the system's current
            // AppsUseLightTheme.
            set(if dark { 2 } else { 3 });
            flush();
        }
    }
}

#[cfg(test)]
mod tests {
    /// When uxtheme exists on the machine, apply can be called round-trip
    /// without panicking; when missing, it degrades silently.
    #[test]
    fn apply_roundtrip_smoke() {
        super::apply(true);
        super::apply(false);
    }
}
