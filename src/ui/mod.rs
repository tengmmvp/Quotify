//! UI layer

pub mod about;
pub mod float_wnd;
pub mod fmt;
pub mod i18n;
pub mod icon;
pub mod panel;
pub mod peak;
pub mod popup;
pub mod tray;

use windows::Win32::Foundation::LPARAM;

/// LPARAM low word → client-area x (physical pixels)
pub(crate) fn x_of(lparam: LPARAM) -> f32 {
    (lparam.0 & 0xFFFF) as u16 as i16 as f32
}

/// LPARAM high word → client-area y (physical pixels)
pub(crate) fn y_of(lparam: LPARAM) -> f32 {
    ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32
}
