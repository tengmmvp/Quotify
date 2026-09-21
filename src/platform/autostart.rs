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
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    points_to_exe(&value, &exe.display().to_string())
}

/// The executable path segment of a Run value: the quoted span when
/// quoted, otherwise up to the first whitespace — the rest is arguments.
fn exe_segment(value: &str) -> &str {
    let v = value.trim();
    if let Some(rest) = v.strip_prefix('"') {
        rest.split('"').next().unwrap_or("")
    } else {
        v.split_whitespace().next().unwrap_or("")
    }
}

/// Whether a Run value's exe segment is the given path, compared
/// case-insensitively with slashes normalized — hand-edited forms
/// (arguments, quoting, case) still count as enabled instead of being
/// overwritten on the next enable.
fn points_to_exe(value: &str, exe: &str) -> bool {
    let seg = exe_segment(value);
    !seg.is_empty()
        && seg
            .replace('/', "\\")
            .eq_ignore_ascii_case(&exe.replace('/', "\\"))
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

#[cfg(test)]
mod tests {
    const EXE: &str = r"C:\Apps\Quotify\Quotify.exe";

    /// A Run value in any of the three live forms — bare path, quoted,
    /// argument-carrying — reads as enabled, including case and slash
    /// variants.
    #[test]
    fn points_to_exe_matches_edited_forms() {
        assert!(super::points_to_exe(EXE, EXE));
        assert!(super::points_to_exe(&format!("\"{EXE}\""), EXE));
        assert!(super::points_to_exe(&format!("{EXE} --minimized"), EXE));
        assert!(super::points_to_exe(&format!("\"{EXE}\" --minimized"), EXE));
        assert!(super::points_to_exe(
            &format!("\"{}\"", EXE.to_ascii_lowercase()),
            EXE
        ));
        assert!(super::points_to_exe(&EXE.replace('\\', "/"), EXE));
    }

    /// A different exe, blank values, and a path merely prefixed by ours
    /// all stay disabled.
    #[test]
    fn points_to_exe_rejects_other_targets() {
        assert!(!super::points_to_exe("\"C:\\Other\\app.exe\"", EXE));
        assert!(!super::points_to_exe("", EXE));
        assert!(!super::points_to_exe("   ", EXE));
        assert!(!super::points_to_exe("\"\"", EXE));
        assert!(!super::points_to_exe(&format!("{EXE}.bak --arg"), EXE));
    }
}
