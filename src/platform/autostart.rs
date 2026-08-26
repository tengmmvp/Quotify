//! 开机自启

use windows_registry::CURRENT_USER;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "Quotify";

// Win32 错误码：删除不存在的值时注册表层可能报这两种，均按幂等成功处理。
const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PATH_NOT_FOUND: i32 = 3;

/// 当前自启是否开启
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

/// 开启/关闭自启
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
        // 值不存在时删除也视为成功，幂等
        match key.remove_value(VALUE_NAME) {
            Ok(()) => Ok(()),
            Err(e) if e.code().0 == ERROR_FILE_NOT_FOUND || e.code().0 == ERROR_PATH_NOT_FOUND => {
                Ok(())
            }
            Err(e) => Err(format!("删除自启注册表失败: {e}")),
        }
    }
}
