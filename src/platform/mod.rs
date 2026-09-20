//! Windows platform services

pub mod autostart;
pub mod clipboard;
pub mod instance;
pub mod menu_theme;
pub mod notify;
pub mod post;

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::UI::WindowsAndMessaging::{HICON, WM_APP};
use windows::core::PCWSTR;

/// str → UTF-16
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Ring capacity for recent log lines: 64 entries × 512 bytes ≈ 32 KiB
/// worst case, within the lightweight-memory red line.
const LOG_RING_CAP: usize = 64;
/// Per-entry byte cap for ring lines.
const LOG_ENTRY_MAX_BYTES: usize = 512;

/// Tail of recent log lines backing the diagnostics export's recent-logs
/// section.
static RECENT_LOGS: std::sync::Mutex<std::collections::VecDeque<String>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

/// Byte-level truncation landing on a char boundary: slicing into a
/// multi-byte sequence's interior would panic, so the cut backs off to
/// that character's start. Shared by the log ring and input-field
/// prefill clamping — one truncation policy, not one per caller.
pub(crate) fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Push one line into the ring, dropping the oldest entry when full.
fn ring_push(ring: &mut std::collections::VecDeque<String>, line: &str) {
    if ring.len() >= LOG_RING_CAP {
        ring.pop_front();
    }
    ring.push_back(truncate_bytes(line, LOG_ENTRY_MAX_BYTES).to_string());
}

/// Masks the account-name segment of a `\Users\<name>\` path as `***` so
/// diagnostics output carries no Windows account name; anything outside
/// that segment passes through unchanged. ASCII lowercasing is
/// byte-for-byte, so indices found in the lowered copy stay valid here.
pub(crate) fn redact_user_segment(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    let Some(start) = lower.find("\\users\\") else {
        return path.to_string();
    };
    let name_start = start + "\\users\\".len();
    let name_end = path[name_start..]
        .find('\\')
        .map_or(path.len(), |i| name_start + i);
    // An empty name is not a profile segment; pass through unchanged.
    if name_end == name_start {
        return path.to_string();
    }
    format!("{}***{}", &path[..name_start], &path[name_end..])
}

/// Unified diagnostic logging: the `[Quotify] ` prefix is single-sourced
/// here (call sites stay bare), and the whole prefixed line passes the
/// profile-name redaction before it leaves the process — the debug output
/// and the recent ring (the diagnostics export's tail) share the same
/// redacted line, so an embedded config path leaks no account name into
/// either sink.
pub fn log(msg: &str) {
    let line = redact_user_segment(&format!("[Quotify] {msg}"));
    ring_push(&mut crate::locks::borrow(&RECENT_LOGS), &line);
    let w = wide(&line);
    unsafe { OutputDebugStringW(PCWSTR(w.as_ptr())) };
}

/// Oldest-first snapshot clone of the recent log lines
pub fn recent_logs() -> Vec<String> {
    crate::locks::borrow(&RECENT_LOGS).iter().cloned().collect()
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

/// Load the embedded icon resource (resource ID 1; the matching .ico layer
/// is chosen by size). Two consumption modes share this single loader:
/// the tray path passes a concrete pixel size with shared = false and owns
/// the returned handle — the caller destroys it; the window-class path
/// passes px = 0 with shared = true for LR_DEFAULTSIZE | LR_SHARED, whose
/// handle belongs to the system — never destroy it. Resource loading
/// stays on the platform side so neither the window-class boilerplate nor
/// the tray code carries its own copy.
pub(crate) fn load_resource_icon(px: i32, shared: bool) -> Option<HICON> {
    let hinst = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok()? };
    #[allow(clippy::manual_dangling_ptr)]
    let resid = 1usize as *const u16;
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::LoadImageW(
            Some(hinst.into()),
            PCWSTR(resid),
            windows::Win32::UI::WindowsAndMessaging::IMAGE_ICON,
            px,
            px,
            if shared {
                windows::Win32::UI::WindowsAndMessaging::LR_DEFAULTSIZE
                    | windows::Win32::UI::WindowsAndMessaging::LR_SHARED
            } else {
                Default::default()
            },
        )
        .ok()
        .map(|h| HICON(h.0))
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
            hIcon: load_resource_icon(0, true).unwrap_or_default(),
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
            log(&format!("{log_tag}创建失败: {e}"));
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
        log(&format!("拒绝非 http(s) 链接: {url}"));
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
        log(&format!("打开链接失败: {url}"));
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
            "文件 ACL 加固失败[尽力而为]: {} ({e})",
            path.display()
        )),
    }
}

/// Best-effort tightening with logging: the load-time backfill path and the
/// write-overwrite path share it, keeping the semantics single-sourced.
fn tighten_best_effort(path: &std::path::Path, acl: &PrivateAcl) {
    if let Err(e) = set_restrictive_dacl(path, acl.acl) {
        log(&format!(
            "文件 ACL 加固失败[尽力而为]: {} ({e})",
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
            crate::platform::log(&format!("受限 ACL 构建失败，回退普通写入: {e}"));
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

    /// Ring mechanics on a local deque: over-capacity pushes drop the
    /// oldest, and the per-entry byte cap truncates onto char boundaries.
    #[test]
    fn ring_push_cap_and_truncation() {
        use super::{LOG_ENTRY_MAX_BYTES, LOG_RING_CAP, ring_push};
        let mut ring = std::collections::VecDeque::new();
        for i in 0..LOG_RING_CAP + 10 {
            ring_push(&mut ring, &format!("line-{i}"));
        }
        assert_eq!(ring.len(), LOG_RING_CAP);
        assert_eq!(ring.front().unwrap(), "line-10");
        assert_eq!(ring.back().unwrap(), &format!("line-{}", LOG_RING_CAP + 9));
        // Short lines pass through unchanged.
        ring_push(&mut ring, "ok");
        assert_eq!(ring.back().unwrap(), "ok");
        // 3-byte characters: byte 512 lands mid-character, so the cut
        // backs off to the boundary at 510 bytes (170 characters).
        let wide = "中".repeat(LOG_ENTRY_MAX_BYTES);
        ring_push(&mut ring, &wide);
        let last = ring.back().unwrap();
        assert!(last.len() <= LOG_ENTRY_MAX_BYTES);
        assert!(std::str::from_utf8(last.as_bytes()).is_ok());
        assert_eq!(last.chars().count(), LOG_ENTRY_MAX_BYTES / 3);
        assert!(wide.starts_with(last));
    }

    /// log() lands the prefixed line in the shared ring and the snapshot
    /// sees it; containment assertions stay robust against other tests
    /// logging concurrently.
    #[test]
    fn log_ring_snapshot_has_prefix() {
        super::log("环测 marker-log-ring");
        assert!(
            super::recent_logs()
                .iter()
                .any(|l| l == "[Quotify] 环测 marker-log-ring")
        );
    }

    /// The profile-name segment is masked and nothing else: non-ASCII
    /// names, case-variant separators, non-profile paths, and an empty
    /// name segment
    #[test]
    fn redact_user_segment_masks_profile_name() {
        use super::redact_user_segment;
        assert_eq!(
            redact_user_segment(r"C:\Users\滕腾\Downloads\Quotify\config.toml"),
            r"C:\Users\***\Downloads\Quotify\config.toml"
        );
        assert_eq!(
            redact_user_segment(r"c:\users\bob\config.toml"),
            r"c:\users\***\config.toml"
        );
        assert_eq!(redact_user_segment(r"C:\Users\bob"), r"C:\Users\***");
        assert_eq!(
            redact_user_segment(r"D:\Apps\Quotify\config.toml"),
            r"D:\Apps\Quotify\config.toml"
        );
        assert_eq!(
            redact_user_segment(r"C:\Users\\config.toml"),
            r"C:\Users\\config.toml"
        );
    }

    /// The whole log line passes the redaction before landing in the ring:
    /// an ACL-hardening failure log embedding the full config path must not
    /// leak the account name into the diagnostics export's recent-logs
    /// tail — the copy-side redaction of the Config line alone misses ring
    /// lines. Containment assertions stay robust against other tests
    /// logging concurrently.
    #[test]
    fn log_line_redacts_user_segment() {
        super::log(&format!(
            "环测 marker-log-redact: 无法加固 {}",
            r"C:\Users\bob\AppData\Roaming\Quotify\config.toml"
        ));
        assert!(
            super::recent_logs()
                .iter()
                .any(|l| l.contains(r"C:\Users\***\AppData") && !l.contains(r"\Users\bob"))
        );
    }

    /// A poisoned ring must not take the log path down: locks::borrow
    /// recovers the guard, so logging keeps working after a panic while
    /// holding the lock. The ring stays poisoned afterwards and every
    /// later access still goes through the same recovery.
    #[test]
    fn log_ring_poison_recovery() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = crate::locks::borrow(&super::RECENT_LOGS);
            panic!("deliberate poisoning");
        }));
        super::log("poison-marker");
        assert!(
            super::recent_logs()
                .iter()
                .any(|l| l == "[Quotify] poison-marker")
        );
    }
}
