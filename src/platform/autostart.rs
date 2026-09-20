//! Autostart on boot

use windows_registry::CURRENT_USER;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "Quotify";

// windows-registry wraps Win32 errors as HRESULT: 0x8007XXXX carries the
// raw code in the low 16 bits. Either of these when deleting a nonexistent
// value counts as idempotent success.
const HKR_ERROR_FILE_NOT_FOUND: i32 = 0x8007_0002u32 as i32;
const HKR_ERROR_PATH_NOT_FOUND: i32 = 0x8007_0003u32 as i32;

/// Whether autostart is currently enabled
pub fn is_enabled() -> bool {
    let Ok(value) = CURRENT_USER
        .open(RUN_KEY)
        .and_then(|k| k.get_string(VALUE_NAME))
    else {
        return false;
    };
    let registered = value.trim().trim_matches('"');
    if registered.is_empty() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    registered
        .replace('/', "\\")
        .eq_ignore_ascii_case(&exe.display().to_string().replace('/', "\\"))
}

/// Enable/disable autostart
pub fn set_enabled(on: bool) -> Result<(), String> {
    let key = CURRENT_USER
        .create(RUN_KEY)
        .map_err(|e| format!("打开注册表 Run 键失败: {e}"))?;
    if on {
        let exe = std::env::current_exe().map_err(|e| format!("获取 exe 路径失败: {e}"))?;
        let quoted = format!("\"{}\"", exe.display());
        key.set_string(VALUE_NAME, &quoted)
            .map_err(|e| format!("写入自启注册表失败: {e}"))
    } else {
        // Deleting a nonexistent value also counts as success — idempotent.
        match key.remove_value(VALUE_NAME) {
            Ok(()) => Ok(()),
            Err(e)
                if e.code().0 == HKR_ERROR_FILE_NOT_FOUND
                    || e.code().0 == HKR_ERROR_PATH_NOT_FOUND =>
            {
                Ok(())
            }
            Err(e) => Err(format!("删除自启注册表失败: {e}")),
        }
    }
}
