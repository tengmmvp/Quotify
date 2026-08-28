//! Windows 平台服务

pub mod autostart;
pub mod instance;
pub mod menu_theme;
pub mod notify;
pub mod post;

use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;
use windows::core::PCWSTR;

/// str → UTF-16
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 统一诊断日志
pub fn log(msg: &str) {
    let w = wide(msg);
    unsafe { OutputDebugStringW(PCWSTR(w.as_ptr())) };
}

/// WM_APP 系自定义消息统一分配
pub mod msg {
    use super::WM_APP;
    /// 托盘回调
    pub const WM_APP_TRAY: u32 = WM_APP + 1;
    /// 轮询结果回传
    pub const WM_APP_POLL_RESULT: u32 = WM_APP + 2;
    /// 二次启动唤醒已有实例
    pub const WM_APP_WAKE_INSTANCE: u32 = WM_APP + 3;
    /// 检查更新结果回传
    pub const WM_APP_UPDATE_RESULT: u32 = WM_APP + 4;
    /// 仓库动态拉取结果回传
    pub const WM_APP_NEWS_RESULT: u32 = WM_APP + 5;
}

/// 用默认浏览器打开链接
pub fn open_url(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        log(&format!("[Quotify] 拒绝非 http(s) 链接: {url}"));
        return;
    }
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
    let verb = wide("open");
    let w = wide(url);
    let r = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(w.as_ptr()),
            None,
            None,
            SW_SHOW,
        )
    };
    // 返回值 <= 32 表示失败
    if r.0 as isize <= 32 {
        log(&format!("[Quotify] 打开链接失败: {url}"));
    }
}

/// 归还工作集，静止时保持低内存；换出页面按需换回
pub fn trim_working_set() {
    unsafe {
        use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
        use windows::Win32::System::Threading::GetCurrentProcess;
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}

/// 模态保存文件对话框；取消返回 None
pub fn save_dialog(default_name: &str) -> Option<std::path::PathBuf> {
    file_dialog(default_name, true)
}

/// 模态打开文件对话框；取消返回 None
pub fn open_dialog() -> Option<std::path::PathBuf> {
    file_dialog("", false)
}

/// 传统文件对话框共用实现
fn file_dialog(default_name: &str, save: bool) -> Option<std::path::PathBuf> {
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
        OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    use windows::core::PWSTR;
    // 过滤串以双 nul 收尾
    let filter: Vec<u16> = "JSON (*.json)\0*.json\0All files (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let mut file = [0u16; 260];
    for (i, c) in default_name.encode_utf16().take(259).enumerate() {
        file[i] = c;
    }
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        Flags: OFN_PATHMUSTEXIST
            | OFN_NOCHANGEDIR
            | if save {
                OFN_OVERWRITEPROMPT
            } else {
                OFN_FILEMUSTEXIST
            },
        ..Default::default()
    };
    let ok = unsafe {
        if save {
            GetSaveFileNameW(&mut ofn)
        } else {
            GetOpenFileNameW(&mut ofn)
        }
    };
    if !ok.as_bool() {
        return None;
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(0);
    Some(std::path::PathBuf::from(String::from_utf16_lossy(
        &file[..len],
    )))
}

/// 收紧已存在文件的 DACL：仅保留当前用户与 SYSTEM 完全控制，其余继承来的
/// 宽松 ACE 全部移除。
pub fn secure_file_acl(path: &std::path::Path) {
    if let Err(e) = set_restrictive_dacl(path) {
        log(&format!(
            "[Quotify] 文件 ACL 加固失败[尽力而为]: {} ({e})",
            path.display()
        ));
    }
}

/// 构造受保护 DACL 并写入文件。
fn set_restrictive_dacl(path: &std::path::Path) -> windows::core::Result<()> {
    use windows::Win32::Foundation::{
        CloseHandle, GENERIC_ALL, HANDLE, HLOCAL, LocalFree, NO_ERROR,
    };
    use windows::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW, SetNamedSecurityInfoW,
        TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
    };
    use windows::Win32::Security::{
        ACL, AllocateAndInitializeSid, DACL_SECURITY_INFORMATION, FreeSid, GetTokenInformation,
        NO_INHERITANCE, PROTECTED_DACL_SECURITY_INFORMATION, PSID, SECURITY_NT_AUTHORITY,
        TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)? };

    // 分配的 SYSTEM SID、ACL 与令牌句柄要覆盖所有失败路径，统一在尾部清理
    let mut system_sid = PSID(std::ptr::null_mut());
    let mut new_acl: *mut ACL = std::ptr::null_mut();
    let result = (|| -> windows::core::Result<()> {
        // 当前用户 SID 直接取自进程令牌，免去用户名到 SID 的二次解析
        let mut len = 0u32;
        // 首查只为取缓冲长度，返回 ERROR_INSUFFICIENT_BUFFER 属预期
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut len) };
        // u64 缓冲保证 TOKEN_USER 指针字段的对齐
        let mut buf = vec![0u64; len.div_ceil(8) as usize];
        let user_sid = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buf.as_mut_ptr().cast()),
                len,
                &mut len,
            )?;
            (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid
        };
        // S-1-5-18 本地 SYSTEM
        unsafe {
            AllocateAndInitializeSid(
                &SECURITY_NT_AUTHORITY,
                1,
                18,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                &mut system_sid,
            )?;
        }
        let trustee = |sid: PSID| TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: windows::core::PWSTR(sid.0.cast()),
            ..Default::default()
        };
        // SET_ACCESS 整体替换：新 DACL 只含这两条 ACE，配合 PROTECTED
        // 切断父目录继承，Authenticated Users 等宽松 ACE 一并丢弃
        let entries = [
            EXPLICIT_ACCESS_W {
                grfAccessPermissions: GENERIC_ALL.0,
                grfAccessMode: SET_ACCESS,
                grfInheritance: NO_INHERITANCE,
                Trustee: trustee(user_sid),
            },
            EXPLICIT_ACCESS_W {
                grfAccessPermissions: GENERIC_ALL.0,
                grfAccessMode: SET_ACCESS,
                grfInheritance: NO_INHERITANCE,
                Trustee: trustee(system_sid),
            },
        ];
        let win_err = |code: windows::Win32::Foundation::WIN32_ERROR| -> windows::core::Error {
            windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code.0))
        };
        let code = unsafe { SetEntriesInAclW(Some(&entries), None, &mut new_acl) };
        if code != NO_ERROR {
            return Err(win_err(code));
        }
        let w = wide(&path.to_string_lossy());
        let code = unsafe {
            SetNamedSecurityInfoW(
                windows::core::PCWSTR(w.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(new_acl),
                None,
            )
        };
        if code != NO_ERROR {
            return Err(win_err(code));
        }
        Ok(())
    })();
    unsafe {
        if !system_sid.0.is_null() {
            FreeSid(system_sid);
        }
        if !new_acl.is_null() {
            // SetEntriesInAclW 分配的 ACL 由 LocalFree 释放
            let _ = LocalFree(Some(HLOCAL(new_acl.cast())));
        }
        let _ = CloseHandle(token);
    }
    result
}
