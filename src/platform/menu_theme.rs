//! 托盘菜单跟随主题
//!
//! 菜单暗色无官方 API：uxtheme 仅按序号导出（135=SetPreferredAppMode、
//! 136=FlushMenuThemes），Win10 1809+ 可用；取不到时保持系统默认浅色菜单。

use std::sync::OnceLock;

use windows::Win32::Foundation::FARPROC;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{PCSTR, PCWSTR};

use super::wide;

type SetMode = unsafe extern "system" fn(i32) -> i32;
type Flush = unsafe extern "system" fn();

/// 进程级菜单主题入口，加载一次缓存；任一序号缺失则整体不可用
fn entries() -> Option<(SetMode, Flush)> {
    static FNS: OnceLock<Option<(SetMode, Flush)>> = OnceLock::new();
    *FNS.get_or_init(|| unsafe {
        let lib = LoadLibraryW(PCWSTR(wide("uxtheme.dll").as_ptr())).ok()?;
        // GetProcAddress 约定：名字指针数值 < 0x10000 时按序号解析
        let set = GetProcAddress(lib, PCSTR(135usize as *const u8))?;
        let flush = GetProcAddress(lib, PCSTR(136usize as *const u8))?;
        Some((
            std::mem::transmute::<FARPROC, SetMode>(Some(set)),
            std::mem::transmute::<FARPROC, Flush>(Some(flush)),
        ))
    })
}

/// 锁定此后弹出菜单的深浅，与面板 resolved 外观同步
pub fn apply(dark: bool) {
    if let Some((set, flush)) = entries() {
        unsafe {
            // PreferredAppMode：Default=0/AllowDark=1/ForceDark=2/ForceLight=3，
            // 用 Force 值直接钉住目标态，不依赖系统当前的 AppsUseLightTheme
            set(if dark { 2 } else { 3 });
            flush();
        }
    }
}

#[cfg(test)]
mod tests {
    /// 真机 uxtheme 存在时 apply 可往返调用不 panic；缺失时静默降级
    #[test]
    fn apply_roundtrip_smoke() {
        super::apply(true);
        super::apply(false);
    }
}
