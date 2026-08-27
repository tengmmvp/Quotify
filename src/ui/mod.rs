//! UI 层

pub mod about;
pub mod fmt;
pub mod i18n;
pub mod icon;
pub mod panel;
pub mod peak;
pub mod popup;
pub mod tray;

use windows::Win32::Foundation::LPARAM;

/// LPARAM 低字 → 客户区 x（物理像素）
pub(crate) fn x_of(lparam: LPARAM) -> f32 {
    (lparam.0 & 0xFFFF) as u16 as i16 as f32
}

/// LPARAM 高字 → 客户区 y（物理像素）
pub(crate) fn y_of(lparam: LPARAM) -> f32 {
    ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32
}
