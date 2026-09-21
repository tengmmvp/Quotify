//! Single instance

use crate::platform::msg::WM_APP_WAKE_INSTANCE;
use crate::platform::wide;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};
use windows::core::PCWSTR;

/// Tray hidden window class name
pub const TRAY_WND_CLASS: &str = "QuotifyTrayWnd";

enum GuardState {
    /// The primary mutex plus the claimed legacy-name handles; Drop closes
    /// them all.
    First(HANDLE, Vec<HANDLE>),
    AlreadyRunning,
}

pub struct InstanceGuard(GuardState);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if let GuardState::First(h1, rest) = &mut self.0 {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(*h1);
                for h in rest.iter() {
                    let _ = windows::Win32::Foundation::CloseHandle(*h);
                }
            }
        }
    }
}

/// Try to become the only instance.
/// The primary mutex carries a user fingerprint to isolate multiple users;
/// the fingerprint-less legacy name stays compatible with released builds.
/// Which name a legacy build holds depends on its launch context (when
/// Global creation failed it only managed Local), so once the primary mutex
/// is acquired the legacy name is probed and claimed in both namespaces.
/// When the fingerprint is unavailable, only the Local primary mutex is
/// used: an all-identical fingerprint value would lock across users, while
/// Local is session-isolated and naturally never collides.
/// Known edge, kept by design: Local isolates by session, not user — two
/// SID-query-failing users in ONE logon session would share the tag-0 name
/// and the second would yield to the first. That requires the own-process
/// token query to fail for both accounts, which stock Windows does not do;
/// the alternative (per-attempt name probing) adds a race for a
/// theoretical path.
pub fn acquire() -> InstanceGuard {
    let tag = user_tag();
    let scopes = tag_scopes(tag);
    let tag_v = tag.unwrap_or(0);
    unsafe {
        for main_scope in scopes {
            let main = wide(&format!(
                "{main_scope}\\{TRAY_WND_CLASS}.SingleInstance.{tag_v}"
            ));
            let h_main = match create_mutex(&main) {
                // The same user already has a newer build running
                // elsewhere: briefly wait out the window period before its
                // tray window is created so main's wake message can get
                // through; an empty wait means cross-session, so exit
                // silently. The owner check mirrors the legacy-name arms:
                // the tag is an unkeyed hash of the SID, so a local
                // attacker who enumerates the victim's SID can pre-create
                // the name — holding is only authoritative when the owner
                // really is this user.
                MutexSlot::Held(h) if kernel_object_owner_is_me(h) => {
                    wait_for_tray_window();
                    let _ = windows::Win32::Foundation::CloseHandle(h);
                    return InstanceGuard(GuardState::AlreadyRunning);
                }
                // Another owner holds the tag name (pre-created by an
                // attacker, or an unreadable owner): yield without
                // waiting — the next scope still gets its chance.
                MutexSlot::Held(h) => {
                    let _ = windows::Win32::Foundation::CloseHandle(h);
                    continue;
                }
                // Creation failure != nobody holds it: an elevated
                // instance's mutex is a high-integrity-level object, and a
                // medium-integrity process cannot obtain full access
                // (No-Write-Up). Probe with a minimal-rights open — the
                // same name existing means a same-tag instance of the same
                // user, again gated on the owner check.
                MutexSlot::Failed => {
                    if let Some(h) = open_mutex(&main) {
                        // The probe handle pins the mutex object just like
                        // the Held arm's: close it before exiting.
                        let mine = kernel_object_owner_is_me(h);
                        if mine {
                            wait_for_tray_window();
                        }
                        let _ = windows::Win32::Foundation::CloseHandle(h);
                        if mine {
                            return InstanceGuard(GuardState::AlreadyRunning);
                        }
                    }
                    continue;
                }
                MutexSlot::Acquired(h) => h,
            };
            let mut held: Vec<HANDLE> = Vec::new();
            let mut blocked = false;
            for probe in ["Global", "Local"] {
                let name = wide(&format!("{probe}\\{TRAY_WND_CLASS}.SingleInstance"));
                match create_mutex(&name) {
                    // A legacy build of the same user is running (this
                    // session or across sessions): exit either way — two
                    // instances persisting via tmp+rename would overwrite
                    // each other and lose accounts.
                    MutexSlot::Held(h) if kernel_object_owner_is_me(h) => {
                        wait_for_tray_window();
                        let _ = windows::Win32::Foundation::CloseHandle(h);
                        blocked = true;
                        break;
                    }
                    // Another user holds the name (different SID or
                    // unreadable): yield without holding.
                    MutexSlot::Held(h) => {
                        let _ = windows::Win32::Foundation::CloseHandle(h);
                    }
                    // Free means claim it: closes the detection blind spot
                    // of a later legacy/new build running in parallel under
                    // another context.
                    MutexSlot::Acquired(h) => held.push(h),
                    // Full-rights creation failing degrades to the same
                    // probe: when an elevated legacy build holds the legacy
                    // name and the new build runs unelevated, creation
                    // failure is the only visible sign.
                    MutexSlot::Failed => {
                        if let Some(h) = open_mutex(&name) {
                            if kernel_object_owner_is_me(h) {
                                wait_for_tray_window();
                                blocked = true;
                            }
                            let _ = windows::Win32::Foundation::CloseHandle(h);
                            if blocked {
                                break;
                            }
                        }
                    }
                }
            }
            if blocked {
                for h in held {
                    let _ = windows::Win32::Foundation::CloseHandle(h);
                }
                let _ = windows::Win32::Foundation::CloseHandle(h_main);
                return InstanceGuard(GuardState::AlreadyRunning);
            }
            return InstanceGuard(GuardState::First(h_main, held));
        }
        // Both namespaces failed to create: conservatively allow through
        // rather than leave the app completely unable to start; the null
        // handle makes Drop's CloseHandle fail silently, equally harmless.
        InstanceGuard(GuardState::First(HANDLE::default(), Vec::new()))
    }
}

/// Result of creating a named mutex
enum MutexSlot {
    Acquired(HANDLE),
    /// Already held; the handle is kept for owner probing and closed by the
    /// caller when done — leaving it open would pin the mutex object, and
    /// other instances would still report ALREADY_EXISTS after the holder
    /// exits.
    Held(HANDLE),
    /// Creation failed (e.g. no permission); retry in the other namespace
    Failed,
}

unsafe fn create_mutex(name: &[u16]) -> MutexSlot {
    unsafe {
        match CreateMutexW(None, false, PCWSTR(name.as_ptr())) {
            Ok(h) if GetLastError() == ERROR_ALREADY_EXISTS => MutexSlot::Held(h),
            Ok(h) => MutexSlot::Acquired(h),
            Err(_) => MutexSlot::Failed,
        }
    }
}

/// Raw bits for SYNCHRONIZE | READ_CONTROL: the type has no ready-made
/// constants, and the right bits are a fixed Win32 contract. Shared by
/// the mutex and event probes — the owner check needs READ_CONTROL, so
/// one side changing the mask without the other silently degrades the
/// other's owner gate to a fallback.
const PROBE_ACCESS: windows::Win32::System::Threading::SYNCHRONIZATION_ACCESS_RIGHTS =
    windows::Win32::System::Threading::SYNCHRONIZATION_ACCESS_RIGHTS(0x0010_0000 | 0x0002_0000);

/// Open a named mutex with minimal rights: CreateMutexW demands full
/// access, so being denied does not mean nobody holds it (No-Write-Up); a
/// named mutex's default DACL grants read and synchronize access broadly
/// (owner, SYSTEM, and the creator's logon session — enough for any
/// same-session probe), so a successful open means the object exists.
unsafe fn open_mutex(name: &[u16]) -> Option<HANDLE> {
    use windows::Win32::System::Threading::OpenMutexW;
    unsafe { OpenMutexW(PROBE_ACCESS, false, PCWSTR(name.as_ptr())).ok() }
}

/// Find this session's tray window by class: single lookup source for
/// presence probing and waking. FindWindow is session-isolated, so other
/// users' instances are invisible.
unsafe fn find_tray_window() -> Option<windows::Win32::Foundation::HWND> {
    let class = wide(TRAY_WND_CLASS);
    unsafe { FindWindowW(PCWSTR(class.as_ptr()), None).ok() }
}

/// Namespace policy shared by the primary mutex and the tray-ready
/// event: Global whenever a user fingerprint exists, Local always — the
/// event must live in every namespace the mutex may occupy, or a
/// duplicate's fast-path wait would look in the wrong place.
fn tag_scopes(tag: Option<u64>) -> Vec<&'static str> {
    let mut scopes = Vec::new();
    if tag.is_some() {
        scopes.push("Global");
    }
    scopes.push("Local");
    scopes
}

/// Tray-ready event names: the same fingerprint tag as the primary
/// mutex, one per namespace. The first instance signals each once its
/// tray window exists; a duplicate's startup wait becomes a
/// deterministic handshake instead of a re-query loop.
fn tray_ready_event_names() -> Vec<String> {
    let tag = user_tag();
    let tag_v = tag.unwrap_or(0);
    tag_scopes(tag)
        .into_iter()
        .map(|scope| format!("{scope}\\{TRAY_WND_CLASS}.TrayReady.{tag_v}"))
        .collect()
}

/// First-instance signal: the tray window exists. Manual-reset with an
/// initial signaled state — once ready it stays ready for the rest of
/// the instance's life, so duplicates need no reset protocol. The
/// handle is deliberately leaked: closing it would destroy the named
/// object (it is the only handle), and duplicates arriving at any later
/// moment must still find it — the handle dies with the process.
pub fn signal_tray_ready() {
    for name in tray_ready_event_names() {
        let name = wide(&name);
        unsafe {
            if let Ok(h) = windows::Win32::System::Threading::CreateEventW(
                None,
                true,
                true,
                PCWSTR(name.as_ptr()),
            ) {
                let _ = windows::Win32::System::Threading::SetEvent(h);
                // No CloseHandle: the handle is deliberately leaked (see
                // this function's doc).
            }
        }
    }
}

/// Wait out the startup window where the mutex is already held but the
/// tray window is not yet created. Fast path: a new-build holder's
/// tray-ready event — a deterministic handshake, no polling (legacy
/// holders never signal, and pre-window the object may not exist yet;
/// both fall to the query loop). The verdict is deliberately not
/// returned: every caller exits regardless; the wait itself is the point
/// (main wakes the found window unconditionally afterwards).
unsafe fn wait_for_tray_window() {
    for name in tray_ready_event_names() {
        let name = wide(&name);
        unsafe {
            // The shared PROBE_ACCESS mask: the wait needs SYNCHRONIZE, the
            // owner check below READ_CONTROL.
            if let Ok(h) = windows::Win32::System::Threading::OpenEventW(
                PROBE_ACCESS,
                false,
                PCWSTR(name.as_ptr()),
            ) {
                // Owner gate mirrors the mutex arms: the tag is an unkeyed
                // hash of the SID, so a local attacker can pre-create the
                // event name un-signaled and stall every duplicate for the
                // full timeout; only a same-owner signal is worth the wait.
                let mine = kernel_object_owner_is_me(h);
                if mine {
                    let _ = windows::Win32::System::Threading::WaitForSingleObject(h, 1500);
                }
                let _ = windows::Win32::Foundation::CloseHandle(h);
                if mine {
                    return;
                }
            }
        }
    }
    for attempt in 0..5 {
        if unsafe { find_tray_window() }.is_some() {
            return;
        }
        if attempt < 4 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// Whether the mutex holder is the current user: read the object's owner
/// SID and compare it against the process token's user SID only. The owner
/// implicitly holds READ_CONTROL, so the same user can always read it; a
/// read failure yields as if another user, never hurting multi-user
/// coexistence.
/// Known edge, kept by design: matching group SIDs would let a pre-created
/// Everyone-owned object pose as a same-user instance and deny other users'
/// launches, so an elevated same-user instance (whose mutex owner is the
/// Administrators group) is no longer recognized and a duplicate may run
/// alongside it.
unsafe fn kernel_object_owner_is_me(h: HANDLE) -> bool {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
    use windows::Win32::Security::{OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID};

    unsafe {
        let mut owner = PSID::default();
        // The SD out-parameter must be captured: owner points into it,
        // LocalFree it when done.
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let code = GetSecurityInfo(
            h,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            Some(&mut sd),
        );
        if code.is_err() {
            return false;
        }
        let ok = crate::platform::sid_belongs_to_token(owner);
        let _ = LocalFree(Some(HLOCAL(sd.0.cast())));
        ok
    }
}

/// User fingerprint: the FNV-1a hash of the process token's user SID —
/// usernames collide between identically named local/domain accounts while
/// the SID is unique; DefaultHasher promises no stability across compiler
/// versions, and a toolchain upgrade would leave old and new binaries each
/// holding a different tag in parallel. When the token is unavailable, the
/// caller falls back to the Local namespace. Computed once per process:
/// the SID never changes mid-run, and several naming paths (mutex, tray
/// events) would otherwise each pay a token query.
fn user_tag() -> Option<u64> {
    static TAG: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *TAG.get_or_init(|| {
        let (sid_buf, sid) = crate::platform::current_user_sid().ok()?;
        let len = unsafe { windows::Win32::Security::GetLengthSid(sid) } as usize;
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in unsafe { std::slice::from_raw_parts(sid.0.cast::<u8>(), len) } {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        drop(sid_buf);
        Some(h)
    })
}

impl InstanceGuard {
    pub fn is_first(&self) -> bool {
        matches!(self.0, GuardState::First(..))
    }

    /// Wake the running instance to show its panel: find this session's
    /// tray window by class and post the wake code. Stateless — the guard
    /// itself is not consulted.
    pub fn wake_existing(&self) {
        unsafe {
            if let Some(hwnd) = find_tray_window() {
                let _ = PostMessageW(
                    Some(hwnd),
                    WM_APP_WAKE_INSTANCE,
                    Default::default(),
                    Default::default(),
                );
            }
        }
    }
}
