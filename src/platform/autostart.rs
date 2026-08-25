//! 开机自启

use windows_registry::CURRENT_USER;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "Quotify";

/// 当前自启是否开启
pub fn is_enabled() -> bool {
    CURRENT_USER
        .open(RUN_KEY)
        .and_then(|k| k.get_string(VALUE_NAME))
        .is_ok_and(|v| !v.trim().is_empty())
}

/// 开启/关闭自启
pub fn set_enabled(on: bool) -> Result<(), String> {
    let key = CURRENT_USER
        .create(RUN_KEY)
        .map_err(|e| format!("打开注册表 Run 键失败: {e}"))?;
    if on {
        let exe = std::env::current_exe()
            .map_err(|e| format!("获取 exe 路径失败: {e}"))?;
        let quoted = format!("\"{}\"", exe.display());
        key.set_string(VALUE_NAME, &quoted)
            .map_err(|e| format!("写入自启注册表失败: {e}"))
    } else {
        // 值不存在时删除也视为成功，幂等
        match key.remove_value(VALUE_NAME) {
            Ok(()) => Ok(()),
            // ERROR_FILE_NOT_FOUND / ERROR_PATH_NOT_FOUND
            Err(e) if e.code().0 == 2 || e.code().0 == 3 => Ok(()),
            Err(e) => Err(format!("删除自启注册表失败: {e}")),
        }
    }
}
