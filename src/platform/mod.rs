//! Windows platform services

pub mod autostart;
pub mod clipboard;
pub mod instance;
pub mod menu_theme;
pub mod notify;
pub mod post;

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;
use windows::core::PCWSTR;

/// str → UTF-16
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Unified diagnostic logging
pub fn log(msg: &str) {
    let w = wide(msg);
    unsafe { OutputDebugStringW(PCWSTR(w.as_ptr())) };
}

/// Whether the foreground window belongs to this program: shared by the
/// marquee presence check and the floating-window keep-alive sweep.
/// All of the process's windows live on the UI thread, so equality is
/// judged by thread id.
pub fn foreground_is_self() -> bool {
    unsafe {
        let fg = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
        if fg.is_invalid() {
            return false;
        }
        windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(fg, None)
            == windows::Win32::System::Threading::GetCurrentThreadId()
    }
}

/// Register the window class shared by the panel and the floating windows:
/// null background brush (fully self-drawn) plus the app icon.
/// Returns false on failure; an already-registered class of the same name
/// also takes that path, so callers guard re-entry with a registration
/// flag.
pub fn register_wnd_class(
    class: &str,
    wndproc: windows::Win32::UI::WindowsAndMessaging::WNDPROC,
) -> bool {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{RegisterClassW, WNDCLASSW};
    unsafe {
        let Ok(hinst) = GetModuleHandleW(None) else {
            return false;
        };
        let name = wide(class);
        let wc = WNDCLASSW {
            lpfnWndProc: wndproc,
            hInstance: hinst.into(),
            lpszClassName: PCWSTR(name.as_ptr()),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(std::ptr::null_mut()),
            hIcon: crate::ui::icon::app_icon(hinst.into()).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc) != 0
    }
}

/// Create a TOPMOST | TOOLWINDOW popup top-level window with default corner
/// rounding; when noactivate is true, clicking it does not activate it.
/// Logs and returns None if window creation fails. Divergent configuration
/// (dropping the maximize box, disabling DWM transitions) is added by the
/// caller afterwards.
pub fn create_tool_window(
    parent: windows::Win32::Foundation::HWND,
    class: &str,
    log_tag: &str,
    noactivate: bool,
) -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };
    unsafe {
        let hinst = GetModuleHandleW(None).ok()?;
        let name = wide(class);
        let mut ex_style = WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
        if noactivate {
            ex_style |= WS_EX_NOACTIVATE;
        }
        let hwnd = CreateWindowExW(
            ex_style,
            PCWSTR(name.as_ptr()),
            PCWSTR(name.as_ptr()),
            WS_POPUP,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap_or_else(|e| {
            log(&format!("[Quotify] {log_tag}创建失败: {e}"));
            windows::Win32::Foundation::HWND::default()
        });
        if hwnd.is_invalid() {
            return None;
        }
        let pref = windows::Win32::Graphics::Dwm::DWMWCP_DEFAULT;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE>()
                as u32,
        );
        Some(hwnd)
    }
}

/// Centralized allocation of the WM_APP-range custom messages
pub mod msg {
    use super::WM_APP;
    /// Tray callback
    pub const WM_APP_TRAY: u32 = WM_APP + 1;
    /// Polling result posted back
    pub const WM_APP_POLL_RESULT: u32 = WM_APP + 2;
    /// Second launch wakes the existing instance
    pub const WM_APP_WAKE_INSTANCE: u32 = WM_APP + 3;
    /// Update-check result posted back
    pub const WM_APP_UPDATE_RESULT: u32 = WM_APP + 4;
    /// Repository news fetch result posted back
    pub const WM_APP_NEWS_RESULT: u32 = WM_APP + 5;
    /// Popup repositioning after a panel geometry change: dispatched
    /// asynchronously and received in a wndproc arm, so the caller's
    /// reference never crosses a pump.
    pub const WM_APP_REPOSITION_POPUP: u32 = WM_APP + 6;
    /// Tray content refresh: dispatched asynchronously and received in a
    /// wndproc arm at the pump point, so the caller's reference never
    /// crosses a pump.
    pub const WM_APP_TRAY_REFRESH: u32 = WM_APP + 7;
}

/// Open a link in the default browser
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
    // A return value <= 32 indicates failure.
    if r.0 as isize <= 32 {
        log(&format!("[Quotify] 打开链接失败: {url}"));
    }
}

/// Return the working set to keep memory low while idle; swapped-out pages
/// come back on demand.
pub fn trim_working_set() {
    unsafe {
        use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
        use windows::Win32::System::Threading::GetCurrentProcess;
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}

/// Modal save-file dialog; returns None on cancel
pub fn save_dialog(owner: HWND, default_name: &str) -> Option<std::path::PathBuf> {
    file_dialog(owner, default_name, true)
}

/// Modal open-file dialog; returns None on cancel
pub fn open_dialog(owner: HWND) -> Option<std::path::PathBuf> {
    file_dialog(owner, "", false)
}

/// Shared implementation of the legacy file dialogs
fn file_dialog(owner: HWND, default_name: &str, save: bool) -> Option<std::path::PathBuf> {
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
        OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    use windows::core::PWSTR;
    // The filter string ends with a double nul.
    let filter: Vec<u16> = "JSON (*.json)\0*.json\0All files (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let mut file = [0u16; 260];
    for (i, c) in default_name.encode_utf16().take(259).enumerate() {
        file[i] = c;
    }
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
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
    // A full buffer with no NUL is taken whole: unwrap_or(0) would treat an
    // exactly-filled path as an empty string.
    let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Some(std::path::PathBuf::from(String::from_utf16_lossy(
        &file[..len],
    )))
}

/// Tighten the DACL of an existing file: keep full control for the current
/// user and SYSTEM only, and strip every other inherited permissive ACE.
pub fn secure_file_acl(path: &std::path::Path) {
    match PrivateAcl::build() {
        Ok(acl) => tighten_best_effort(path, &acl),
        Err(e) => log(&format!(
            "[Quotify] 文件 ACL 加固失败[尽力而为]: {} ({e})",
            path.display()
        )),
    }
}

/// Best-effort tightening with logging: the load-time backfill path and the
/// write-overwrite path share it, keeping the semantics single-sourced.
fn tighten_best_effort(path: &std::path::Path, acl: &PrivateAcl) {
    if let Err(e) = set_restrictive_dacl(path, acl.acl) {
        log(&format!(
            "[Quotify] 文件 ACL 加固失败[尽力而为]: {} ({e})",
            path.display()
        ));
    }
}

/// Whether a UTF-16 unit is the high half of a surrogate pair: a budget cut
/// landing on one must back off a unit, or the lossy conversion emits a
/// literal U+FFFD — the tray tooltip write and the clipboard read share
/// this predicate (their surrounding truncation logic legitimately differs).
pub(crate) fn is_utf16_high_surrogate(u: u16) -> bool {
    (0xD800..0xDC00).contains(&u)
}

// The process's restricted-DACL product, built once: its only input is the
// process token's user SID, fixed for the process lifetime, so the
// seven-syscall build batch runs on the first persist only. The instance
// is leaked to 'static — one resident object reclaimed by the OS at
// process exit. Failures are not cached: each persist retries the build.
thread_local! {
    static PRIVATE_ACL: std::cell::OnceCell<&'static PrivateAcl> = const {
        std::cell::OnceCell::new()
    };
}

fn private_acl() -> windows::core::Result<&'static PrivateAcl> {
    PRIVATE_ACL.with(|c| {
        if let Some(acl) = c.get() {
            return Ok(*acl);
        }
        let acl: &'static PrivateAcl = Box::leak(Box::new(PrivateAcl::build()?));
        let _ = c.set(acl);
        Ok(acl)
    })
}

/// Create a file with a restricted DACL and write the full contents: the
/// file is reachable only by the current user and SYSTEM from the moment it
/// comes into existence, with no "persist first, tighten later" exposure
/// window. An existing file is truncated and rewritten; its old DACL on the
/// overwrite path is covered by after-the-fact tightening.
/// A failed ACL build falls back to a plain, un-tightened write:
/// persistence must not depend on the ACL APIs — a restricted token or
/// transient resource exhaustion would otherwise silently drop every config
/// save for the session, and the next successful save re-covers the file.
pub fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GENERIC_WRITE, GetLastError,
    };
    use windows::Win32::Storage::FileSystem::{
        CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
        FILE_SHARE_READ,
    };

    // Overwrite detection is left to CreateFileW's own report: when
    // CREATE_ALWAYS opens an existing file, GetLastError is set to
    // ERROR_ALREADY_EXISTS (even though a valid handle is returned), with
    // no probing window — a newly created file is born with the restricted
    // SD (the SA takes effect), so only the overwrite path needs
    // after-the-fact tightening.
    let acl = match private_acl() {
        Ok(acl) => Some(acl),
        Err(e) => {
            crate::platform::log(&format!("[Quotify] 受限 ACL 构建失败，回退普通写入: {e}"));
            None
        }
    };
    let sa = acl
        .as_ref()
        .map(|acl| windows::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: &acl.sd as *const _ as *mut core::ffi::c_void,
            bInheritHandle: false.into(),
        });
    let w = wide(&path.to_string_lossy());
    unsafe {
        let handle = CreateFileW(
            windows::core::PCWSTR(w.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_READ,
            sa.as_ref()
                .map(|sa| sa as *const windows::Win32::Security::SECURITY_ATTRIBUTES),
            CREATE_ALWAYS,
            FILE_FLAGS_AND_ATTRIBUTES(FILE_ATTRIBUTE_NORMAL.0),
            None,
        )
        .map_err(std::io::Error::other)?;
        // Read it back-to-back with creation: CREATE_ALWAYS's
        // already-exists report is only meaningful on the success path, and
        // any system call in between overwrites it.
        let preexisting = GetLastError() == ERROR_ALREADY_EXISTS;
        let bytes = contents.as_bytes();
        let ok = write_all(handle, bytes);
        let _ = CloseHandle(handle);
        ok?;
        // Overwriting an existing file keeps its old DACL; after-the-fact
        // tightening covers it.
        if preexisting && let Some(acl) = acl.as_ref() {
            tighten_best_effort(path, acl);
        }
        Ok(())
    }
}

/// Write everything through the WriteFile loop
unsafe fn write_all(handle: HANDLE, mut bytes: &[u8]) -> std::io::Result<()> {
    use windows::Win32::Storage::FileSystem::WriteFile;
    while !bytes.is_empty() {
        let mut written = 0u32;
        unsafe {
            WriteFile(handle, Some(bytes), Some(&mut written), None)
                .map_err(std::io::Error::other)?;
        }
        if written == 0 {
            return Err(std::io::Error::other("WriteFile 未推进"));
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

/// User SID of the process token; the u64 buffer guarantees alignment of
/// TOKEN_USER's pointer fields, and the SID lives as long as the buffer.
/// Shared by DACL construction and the single-instance fingerprint.
pub(crate) fn current_user_sid() -> windows::core::Result<(Vec<u64>, windows::Win32::Security::PSID)>
{
    use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        // The first query only fetches the buffer length; an
        // ERROR_INSUFFICIENT_BUFFER return is expected.
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u64; len.div_ceil(8) as usize];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        )
        .is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(token);
        ok.then(|| {
            let sid = (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid;
            (buf, sid)
        })
        .ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))
    }
}

/// Whether the SID belongs to the current process token (the user SID or
/// any group SID): objects created in an elevated context carry the
/// Administrators group SID as owner, so comparing against the user SID
/// alone would misjudge an elevated instance of the same user as another
/// user.
pub(crate) fn sid_belongs_to_token(sid: windows::Win32::Security::PSID) -> bool {
    use windows::Win32::Security::{
        EqualSid, GetTokenInformation, TOKEN_GROUPS, TOKEN_QUERY, TOKEN_USER, TokenGroups,
        TokenUser,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        // u64 buffer for alignment; shared by both query types and grown on
        // demand.
        let mut buf: Vec<u64> = Vec::new();
        let mut found = false;
        for ty in [TokenUser, TokenGroups] {
            let mut len = 0u32;
            let _ = GetTokenInformation(token, ty, None, 0, &mut len);
            if buf.len() * 8 < len as usize {
                buf.resize(len.div_ceil(8) as usize, 0);
            }
            if GetTokenInformation(token, ty, Some(buf.as_mut_ptr().cast()), len, &mut len).is_err()
            {
                continue;
            }
            if ty == TokenUser {
                found = EqualSid((*(buf.as_ptr() as *const TOKEN_USER)).User.Sid, sid).is_ok();
            } else {
                let g = &*(buf.as_ptr() as *const TOKEN_GROUPS);
                // Groups is declared as a 1-element array but carries
                // GroupCount entries (the classic variable-length tail):
                // indexing it directly panics on i >= 1, so walk the raw
                // slice.
                let groups = std::slice::from_raw_parts(g.Groups.as_ptr(), g.GroupCount as usize);
                for grp in groups {
                    if EqualSid(grp.Sid, sid).is_ok() {
                        found = true;
                        break;
                    }
                }
            }
            if found {
                break;
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(token);
        found
    }
}

/// Construction product of the restricted DACL: the security descriptor,
/// the ACL referenced inline, and the SYSTEM SID; Drop releases them all.
struct PrivateAcl {
    /// SECURITY_ATTRIBUTES wants the descriptor, not the bare ACL: the SD
    /// holds it inline and the DACL points at acl (not copied; it lives as
    /// long as this struct).
    sd: windows::Win32::Security::SECURITY_DESCRIPTOR,
    acl: *mut windows::Win32::Security::ACL,
    system_sid: windows::Win32::Security::PSID,
}

impl PrivateAcl {
    /// Build a protected DACL granting full control to the current user and
    /// SYSTEM only.
    fn build() -> windows::core::Result<Self> {
        use windows::Win32::Foundation::{GENERIC_ALL, HLOCAL, LocalFree, NO_ERROR};
        use windows::Win32::Security::Authorization::{
            EXPLICIT_ACCESS_W, SET_ACCESS, SetEntriesInAclW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN,
            TRUSTEE_W,
        };
        use windows::Win32::Security::{
            ACL, AllocateAndInitializeSid, FreeSid, NO_INHERITANCE, PSID, SECURITY_NT_AUTHORITY,
        };

        // The allocated SYSTEM SID and ACL must be accounted for on every
        // failure path: on failure, free the already-allocated parts before
        // returning.
        let mut system_sid = PSID(std::ptr::null_mut());
        let mut new_acl: *mut ACL = std::ptr::null_mut();
        let result =
            (|| -> windows::core::Result<windows::Win32::Security::SECURITY_DESCRIPTOR> {
                // The current user SID is taken straight from the process
                // token, sparing a second username-to-SID resolution.
                let (_sid_buf, user_sid) = current_user_sid()?;
                // S-1-5-18: local SYSTEM.
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
                // SET_ACCESS replaces wholesale: the new DACL holds only
                // these two ACEs, and together with PROTECTED it severs
                // parent-directory inheritance, discarding permissive ACEs
                // such as Authenticated Users.
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
                let code = unsafe { SetEntriesInAclW(Some(&entries), None, &mut new_acl) };
                if code != NO_ERROR {
                    return Err(windows::core::Error::from_hresult(
                        windows::core::HRESULT::from_win32(code.0),
                    ));
                }
                let mut sd = windows::Win32::Security::SECURITY_DESCRIPTOR::default();
                unsafe {
                    use windows::Win32::Security::{
                        InitializeSecurityDescriptor, PSECURITY_DESCRIPTOR,
                        SetSecurityDescriptorDacl,
                    };
                    use windows::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;
                    let p = PSECURITY_DESCRIPTOR(&mut sd as *mut _ as *mut core::ffi::c_void);
                    InitializeSecurityDescriptor(p, SECURITY_DESCRIPTOR_REVISION)?;
                    // The bare ACL must be attached to the descriptor's
                    // DACL slot; passing it to CreateFileW directly as the
                    // descriptor is rejected (error 1305).
                    SetSecurityDescriptorDacl(p, true, Some(new_acl), false)?;
                }
                Ok(sd)
            })();
        let sd = match result {
            Ok(sd) => sd,
            Err(e) => {
                unsafe {
                    if !system_sid.0.is_null() {
                        FreeSid(system_sid);
                    }
                    if !new_acl.is_null() {
                        // The ACL allocated by SetEntriesInAclW is freed
                        // with LocalFree.
                        let _ = LocalFree(Some(HLOCAL(new_acl.cast())));
                    }
                }
                return Err(e);
            }
        };
        Ok(Self {
            sd,
            acl: new_acl,
            system_sid,
        })
    }
}

impl Drop for PrivateAcl {
    fn drop(&mut self) {
        unsafe {
            if !self.system_sid.0.is_null() {
                windows::Win32::Security::FreeSid(self.system_sid);
            }
            if !self.acl.is_null() {
                let _ = windows::Win32::Foundation::LocalFree(Some(
                    windows::Win32::Foundation::HLOCAL(self.acl.cast()),
                ));
            }
        }
    }
}

/// Write a ready-made protected DACL onto an existing file; the caller
/// builds the ACL, so the overwrite path can share the same one with
/// creation.
fn set_restrictive_dacl(
    path: &std::path::Path,
    acl: *mut windows::Win32::Security::ACL,
) -> windows::core::Result<()> {
    use windows::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows::Win32::Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    };

    let w = wide(&path.to_string_lossy());
    let code = unsafe {
        SetNamedSecurityInfoW(
            windows::core::PCWSTR(w.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(acl),
            None,
        )
    };
    if code != windows::Win32::Foundation::NO_ERROR {
        return Err(windows::core::Error::from_hresult(
            windows::core::HRESULT::from_win32(code.0),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Real-disk regression: with the SD mis-wrapped, CreateFileW fails
    /// with 1305 and this test fails outright in a GUI-less environment —
    /// earlier unit tests never touched the disk and missed this blind
    /// spot.
    #[test]
    fn write_private_roundtrip() {
        let dir = std::env::temp_dir().join("quotify-write-private-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.toml");
        // Overwrite the existing file: the CREATE_ALWAYS path that reuses
        // the old DACL gets exercised too.
        std::fs::write(&p, "old").unwrap();
        super::write_private(&p, "new").expect("受限创建应成功");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        let _ = std::fs::remove_file(&p);
    }
}
