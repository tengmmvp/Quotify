//! Background polling thread

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};

use crate::api::AccountSpec;
use crate::platform::msg::WM_APP_POLL_RESULT;

/// Default polling interval (seconds)
pub const DEFAULT_INTERVAL_SECS: u64 = 300;
/// Minimum polling interval (seconds)
pub const MIN_POLL_SECS: u64 = 10;
/// Maximum polling interval (seconds) = 1 day
pub const MAX_POLL_SECS: u64 = 86_400;
/// 429 exponential backoff base (seconds): data keeps changing while the
/// account is throttled, so quick retries strive for recovery.
const RATE_LIMIT_BACKOFF_BASE_SECS: u64 = 30;

/// Next wait after a 429: doubling per consecutive failure, capped.
/// The Retry-After header applies to this round only, taken as the max
/// against the exponential tier, never spilling into later rounds. The
/// header value is ≤ the backoff cap by construction (FetchError::throttled
/// owns the clamp), so it needs no re-clamping here.
fn backoff_wait(fails: u32, retry_after_secs: Option<u64>) -> u64 {
    let exp = RATE_LIMIT_BACKOFF_BASE_SECS
        .saturating_mul(1 << fails.min(4))
        .min(crate::api::client::RATE_LIMIT_BACKOFF_MAX_SECS);
    retry_after_secs.map_or(exp, |ra| ra.max(exp))
}

/// Read the interval and clamp it into the legal range
fn clamp_interval(secs: u64) -> u64 {
    secs.clamp(MIN_POLL_SECS, MAX_POLL_SECS)
}

/// One poll's result, handed to the main thread across threads.
pub enum PollOutcome {
    Success(Box<crate::api::UsageSnapshot>),
    Failure(Box<crate::api::FetchError>),
}

/// Envelope for posting a poll result back
pub struct PollMessage {
    pub generation: u64,
    pub outcome: PollOutcome,
}

/// Channel for posting poll results back: the polling thread enqueues, the
/// UI thread drains after being woken.
pub static POLL_SLOT: crate::platform::post::Slot<PollMessage> = crate::platform::post::Slot::new();

pub type PollTarget = Arc<Mutex<Option<AccountSpec>>>;
pub type PollInterval = Arc<Mutex<u64>>;
pub type PollGeneration = Arc<AtomicU64>;

/// Owner of the background polling thread: sends the wake/stop signals and
/// joins to reclaim on drop.
pub struct Poller {
    wake: HANDLE,
    stop: HANDLE,
    refresh_requested: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Poller {
    /// Spawn the polling thread
    pub fn spawn(
        hwnd: windows::Win32::Foundation::HWND,
        target: PollTarget,
        interval: PollInterval,
        generation: PollGeneration,
    ) -> Option<Self> {
        unsafe {
            let wake = CreateEventW(None, false, false, None).ok()?;
            // Partial-init rollback is symmetric with the thread-spawn arm
            // below: a failed stop-event creation must close the already
            // created wake handle, or it leaks with no log on this path.
            let stop = match CreateEventW(None, true, false, None) {
                Ok(s) => s,
                Err(e) => {
                    crate::platform::log(&format!("轮询停止事件创建失败: {e}"));
                    let _ = windows::Win32::Foundation::CloseHandle(wake);
                    return None;
                }
            };
            let refresh_requested = Arc::new(AtomicBool::new(false));
            let flag = refresh_requested.clone();

            // Kernel handles cross threads through the shared SendCell
            // wrapper (same invariant as the post side's).
            let (hwnd_s, wake_s, stop_s) = (
                crate::platform::post::SendCell(hwnd),
                crate::platform::post::SendCell(wake),
                crate::platform::post::SendCell(stop),
            );
            let thread = match std::thread::Builder::new()
                .name("quotify-poller".into())
                .spawn(move || {
                    // Move the SendHandle into a closure local first, so
                    // the 2021 precise capture cannot bypass the wrapper
                    // and break Send.
                    let (h, w, s) = (hwnd_s, wake_s, stop_s);
                    poll_loop(h.0, target, interval, w.0, s.0, flag, generation)
                }) {
                Ok(t) => t,
                Err(e) => {
                    // If the thread fails to start, the Poller is never
                    // born and Drop provides no fallback, so both event
                    // handles must be closed here to avoid a leak.
                    crate::platform::log(&format!("轮询线程创建失败，托盘将无数据: {e}"));
                    let _ = windows::Win32::Foundation::CloseHandle(wake);
                    let _ = windows::Win32::Foundation::CloseHandle(stop);
                    return None;
                }
            };
            Some(Self {
                wake,
                stop,
                refresh_requested,
                thread: Some(thread),
            })
        }
    }

    /// Wake entry for pulling once immediately: takes effect at once on
    /// manual refresh, interval/proxy changes, or account switches; the
    /// post-power-resume catch-up pull goes through here too — signaling
    /// without also setting the flag would let the pull's own re-anchoring
    /// consume the signal when sleep straddles an in-flight pull, and the
    /// catch-up would be lost.
    pub fn refresh_now(&self) {
        self.refresh_requested.store(true, Ordering::Release);
        unsafe {
            let _ = SetEvent(self.wake);
        };
    }

    /// Abandon the polling thread without joining or closing anything:
    /// session-end exit only — the process dies with the session
    /// regardless, and Drop's join bound (one in-flight request's agent
    /// timeout, up to ~15s) would stall the session teardown and risk the
    /// OS force-killing the process before the tray cleanup runs. Stop is
    /// signaled first so a thread at the wait point exits promptly.
    pub fn detach(self) {
        unsafe {
            let _ = SetEvent(self.stop);
        }
        std::mem::forget(self);
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop);
        };
        if let Some(t) = self.thread.take() {
            // The join upper bound is one in-flight request's agent-global
            // timeout (15s for the main endpoint): only after the request
            // returns does the loop reach the wait point again and see
            // stop.
            let _ = t.join();
        }
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.wake);
            let _ = windows::Win32::Foundation::CloseHandle(self.stop);
        }
    }
}

/// Permission to re-anchor by interval: the interval changed and no
/// backoff is in effect — during backoff the cadence belongs to the
/// backoff anchor alone, and re-anchoring would collide with it early.
/// Shared by the post-fetch path and the not-yet-due wake path.
fn interval_retunable(fails: u32, interval: u64, last_secs: u64) -> bool {
    interval != last_secs && fails == 0
}

/// Anchoring action after one fetch round
enum Schedule {
    /// Re-anchor by the given seconds: success and automatic failure
    Reset(u64),
    /// Manual failure gets no catch-up pull: the schedule only pushes out,
    /// never earlier than floor; when the interval changed and no backoff
    /// is active, re-anchor by the new interval — re-anchor only, no
    /// catch-up pull.
    Defer { reanchor: bool, floor: u64 },
}

/// Next schedule and consecutive-429 count for one round's result: the
/// cadence rules in a single place. A manual failure does not rewrite the
/// automatic state and leaves the count untouched; a 429's wait floor is
/// the larger of the backoff tier and Retry-After — the server's directive
/// must take effect; a non-429 manual failure falls back to the regular
/// interval.
fn schedule_next(
    err: Option<&crate::api::FetchError>,
    manual: bool,
    fails: u32,
    interval: u64,
    last_secs: u64,
) -> (Schedule, u32) {
    let interval_changed = interval_retunable(fails, interval, last_secs);
    match err {
        None => (Schedule::Reset(interval), 0),
        Some(crate::api::FetchError::Throttled {
            retry_after_secs, ..
        }) => {
            let wait = backoff_wait(fails, *retry_after_secs);
            if manual {
                (
                    Schedule::Defer {
                        // The queue-jump never fires early: the consumer
                        // applies push_out(floor) right after any re-anchor,
                        // so a Retry-After directive still holds. Allowing
                        // the re-anchor matters for the plain-backoff case
                        // (no header): a just-selected interval takes over
                        // from now instead of waiting out the old
                        // interval's anchor.
                        reanchor: interval_changed,
                        floor: wait,
                    },
                    fails,
                )
            } else {
                (Schedule::Reset(wait), fails + 1)
            }
        }
        Some(_) => {
            if manual {
                (
                    Schedule::Defer {
                        reanchor: interval_changed,
                        floor: interval,
                    },
                    fails,
                )
            } else {
                (Schedule::Reset(interval), 0)
            }
        }
    }
}

/// Paired holder of the monotonic anchor and the wall anchor
struct Due {
    next: Instant,
    wall: chrono::DateTime<chrono::Utc>,
}

impl Due {
    fn now() -> Self {
        Self {
            next: Instant::now(),
            wall: chrono::Utc::now(),
        }
    }

    /// Move both anchors forward together by secs seconds
    fn set(&mut self, secs: u64) {
        self.next = Instant::now() + Duration::from_secs(secs);
        self.wall = chrono::Utc::now() + chrono::Duration::seconds(secs as i64);
    }

    /// Due once either anchor has passed: the wall anchor covers the
    /// monotonic clock stalling during sleep
    fn expired(&self) -> bool {
        Instant::now() >= self.next || chrono::Utc::now() >= self.wall
    }

    /// Take each anchor's max with now+secs: the schedule only pushes
    /// out, never earlier. Reserved for the manual-failure
    /// queue-jump semantics — regular re-anchoring goes through set.
    fn push_out(&mut self, secs: u64) {
        let n = Instant::now() + Duration::from_secs(secs);
        let w = chrono::Utc::now() + chrono::Duration::seconds(secs as i64);
        self.next = self.next.max(n);
        self.wall = self.wall.max(w);
    }
}

/// The polling loop
fn poll_loop(
    hwnd: windows::Win32::Foundation::HWND,
    target: PollTarget,
    interval: PollInterval,
    wake: HANDLE,
    stop: HANDLE,
    refresh_flag: Arc<AtomicBool>,
    generation: PollGeneration,
) {
    let handles = [stop, wake];
    let mut last_secs = 0u64;
    // Dual-anchor due point: the monotonic clock leads with the wall clock
    // running in parallel — sleep can stall the monotonic clock, and after
    // wake the wall anchor rules it due for an immediate catch-up pull.
    // Paired updates go through one method so a new schedule point can
    // never update just one anchor.
    let mut due = Due::now();

    // Consecutive-429 count: zeroed by success, a non-429 result, or a
    // source-switch generation, handing the wait cadence back to plain
    // exponential backoff. A throttled account's count must not starve a
    // newly switched healthy account.
    let mut backoff_fails: u32 = 0;
    let mut last_gen = u64::MAX;

    loop {
        let now = Instant::now();
        // The wait takes the earlier of the monotonic and wall anchors:
        // the monotonic clock stalls during sleep, so even with the resume
        // broadcast also lost, we cannot oversleep on a stale monotonic
        // anchor.
        // The saturation cap is u32::MAX - 1: u32::MAX happens to be
        // Win32's INFINITE, and landing on it would degenerate into an
        // infinite wait.
        let wall_ms = (due.wall - chrono::Utc::now()).num_milliseconds().max(0) as u64;
        let wait_ms = if due.next > now {
            ((due.next - now).as_millis() as u64)
                .min(wall_ms)
                .min(u32::MAX as u64 - 1) as u32
        } else {
            0
        };
        let wait = unsafe { WaitForMultipleObjects(&handles, false, wait_ms) };

        if wait == WAIT_OBJECT_0 {
            return; // stop
        }
        let manual = refresh_flag.swap(false, Ordering::AcqRel);
        if !manual && !due.expired() {
            let secs = clamp_interval(*borrow(&interval));
            if interval_retunable(backoff_fails, secs, last_secs) {
                due.set(secs);
                last_secs = secs;
            }
            continue;
        }

        // The generation number must be read before cloning the account:
        // the main thread writes target first and bumps the generation
        // after; the reverse order would let old-account data slip in
        // carrying the new number. Reading the number first means seeing
        // the new number implies seeing the new target.
        // A generation change is a source switch: zero the count, reset the
        // anchor to now, and let the new account's schedule be decided by
        // the fetch after the switch, inheriting no backoff wait from the
        // old account.
        let gen_at_start = generation.load(Ordering::Acquire);
        if gen_at_start != last_gen {
            backoff_fails = 0;
            last_gen = gen_at_start;
            due.set(0);
        }
        let spec = {
            let guard = borrow(&target);
            match guard.clone() {
                Some(v) => v,
                None => {
                    // No account: suspend until the next wake instead of
                    // spinning.
                    due.set(60);
                    continue;
                }
            }
        };
        let fetched = crate::api::client::fetch_usage(&spec);
        let interval_secs = clamp_interval(*borrow(&interval));
        // Cadence and count computed at this single point: judge by
        // borrowing fetched, then move it into the post-back.
        let (schedule, fails_now) = schedule_next(
            fetched.as_ref().err(),
            manual,
            backoff_fails,
            interval_secs,
            last_secs,
        );
        backoff_fails = fails_now;
        let outcome = match fetched {
            Ok(snap) => PollOutcome::Success(Box::new(snap)),
            Err(e) => PollOutcome::Failure(Box::new(e)),
        };
        POLL_SLOT.post(
            hwnd,
            WM_APP_POLL_RESULT,
            PollMessage {
                generation: gen_at_start,
                outcome,
            },
        );

        match schedule {
            Schedule::Reset(wait) => due.set(wait),
            // A manual failure is a pure queue jump: the user gets no
            // second catch-up pull, and the schedule only pushes out, never
            // earlier. The exception is a changed interval with no backoff
            // active: the old anchor was set by the old interval, and
            // keeping it would swallow the new interval until the old
            // anchor expires — re-anchor to the new interval from now,
            // re-anchor only, no catch-up pull.
            Schedule::Defer { reanchor, floor } => {
                if reanchor {
                    due.set(interval_secs);
                }
                due.push_out(floor);
            }
        }
        // last_secs records the current interval each round (backoff rounds
        // included) and is used only to detect interval changes. The
        // backoff wait never enters this variable; mixed in, it would be
        // mistaken for an interval change and re-anchor away the backoff
        // anchor.
        last_secs = interval_secs;
    }
}

use crate::locks::borrow;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_clamped_to_min_max() {
        assert_eq!(clamp_interval(0), MIN_POLL_SECS);
        assert_eq!(clamp_interval(9), MIN_POLL_SECS);
        assert_eq!(clamp_interval(300), 300);
        assert_eq!(clamp_interval(u64::MAX), MAX_POLL_SECS);
    }

    /// Backoff sequence pinned: 30→60→120→240→300 capped; the header
    /// applies to this round only and only lengthens, never shortens
    #[test]
    fn backoff_sequence_and_retry_after_priority() {
        assert_eq!(backoff_wait(0, None), 30);
        assert_eq!(backoff_wait(1, None), 60);
        assert_eq!(backoff_wait(2, None), 120);
        assert_eq!(backoff_wait(3, None), 240);
        assert_eq!(backoff_wait(4, None), 300);
        assert_eq!(backoff_wait(9, None), 300);
        // A header longer than the exponential tier wins; a shorter one
        // does not shorten the tier; 0 does not constitute an immediate
        // retry
        assert_eq!(backoff_wait(0, Some(45)), 45);
        assert_eq!(backoff_wait(0, Some(0)), 30);
        assert_eq!(backoff_wait(3, Some(45)), 240);
        assert_eq!(backoff_wait(3, Some(0)), 240);
    }

    /// Cadence matrix pinned: success clears the count and re-anchors to
    /// the interval; automatic 429 increments the count and waits by the
    /// backoff tier; manual failure leaves the count and only pushes out;
    /// interval re-anchoring requires no backoff in effect
    #[test]
    fn schedule_next_matrix() {
        let throttled = |ra: Option<u64>| {
            crate::api::FetchError::throttled(crate::api::ErrorInfo::local("429"), ra)
        };
        let failure = crate::api::FetchError::Network("offline".into());
        let (interval, last) = (300u64, 300u64);

        // Success: re-anchor to the interval, clear the count
        assert!(matches!(
            schedule_next(None, false, 2, interval, last),
            (Schedule::Reset(300), 0)
        ));

        // Automatic 429: increment the count, wait by the backoff tier
        assert!(matches!(
            schedule_next(Some(&throttled(None)), false, 1, interval, last),
            (Schedule::Reset(60), 2)
        ));
        // Automatic non-429: clear the count, use the regular interval
        assert!(matches!(
            schedule_next(Some(&failure), false, 1, interval, last),
            (Schedule::Reset(300), 0)
        ));

        // Manual 429: count untouched, push-out floor is the larger of the
        // backoff tier and the header
        assert!(matches!(
            schedule_next(Some(&throttled(Some(45))), true, 0, interval, last),
            (
                Schedule::Defer {
                    reanchor: false,
                    floor: 45
                },
                0
            )
        ));
        // Manual 429 with a just-selected interval: the re-anchor is safe
        // (push_out floors it after), so the new interval takes over now —
        // with no header the plain backoff floor must not hold the old
        // anchor for a full old interval
        assert!(matches!(
            schedule_next(Some(&throttled(None)), true, 0, 60, last),
            (
                Schedule::Defer {
                    reanchor: true,
                    floor: 30
                },
                0
            )
        ));
        // Manual non-429: push-out floor is the regular interval
        assert!(matches!(
            schedule_next(Some(&failure), true, 3, interval, last),
            (
                Schedule::Defer {
                    reanchor: false,
                    floor: 300
                },
                3
            )
        ));

        // Manual failure with a changed interval: re-anchoring to the new
        // interval is allowed only without backoff
        assert!(matches!(
            schedule_next(Some(&failure), true, 0, 60, last),
            (
                Schedule::Defer {
                    reanchor: true,
                    floor: 60
                },
                0
            )
        ));
        assert!(matches!(
            schedule_next(Some(&failure), true, 2, 60, last),
            (
                Schedule::Defer {
                    reanchor: false,
                    floor: 60
                },
                2
            )
        ));
    }

    /// Dual-anchor expiry pinned with fixed anchor pairs: due once
    /// either anchor passes — the wall arm covers a monotonic clock
    /// stalled by sleep, the monotonic arm keeps the schedule when the
    /// wall clock rolls back
    #[test]
    fn due_expires_on_either_anchor() {
        let mono_ahead = Instant::now() + Duration::from_secs(3600);
        let wall_ahead = chrono::Utc::now() + chrono::Duration::seconds(3600);
        // Both anchors ahead: not due
        assert!(
            !Due {
                next: mono_ahead,
                wall: wall_ahead
            }
            .expired()
        );
        // Monotonic passed while the wall anchor is far ahead (the wall
        // clock rolled back): still due — the monotonic anchor leads
        assert!(
            Due {
                next: Instant::now() - Duration::from_secs(1),
                wall: wall_ahead
            }
            .expired()
        );
        // Wall passed while the monotonic anchor is far ahead (sleep
        // stalled the monotonic clock): still due — the catch-up arm
        assert!(
            Due {
                next: mono_ahead,
                wall: chrono::Utc::now() - chrono::Duration::seconds(1)
            }
            .expired()
        );
    }

    /// set re-anchors both clocks together at now+secs, earlier points
    /// included (unlike push_out); each new anchor lands inside the
    /// before/after now-window of the call
    #[test]
    fn due_set_moves_both_anchors_together() {
        let mut d = Due {
            next: Instant::now() + Duration::from_secs(3600),
            wall: chrono::Utc::now() + chrono::Duration::seconds(3600),
        };
        let (m0, w0) = (Instant::now(), chrono::Utc::now());
        d.set(60);
        let (m1, w1) = (Instant::now(), chrono::Utc::now());
        assert!(d.next >= m0 + Duration::from_secs(60) && d.next <= m1 + Duration::from_secs(60));
        assert!(
            d.wall >= w0 + chrono::Duration::seconds(60)
                && d.wall <= w1 + chrono::Duration::seconds(60)
        );
        assert!(!d.expired());
        // set(0) lands both anchors on now: immediately due (the
        // source-switch reset path)
        d.set(0);
        assert!(d.expired());
    }

    /// push_out only pushes later, never earlier: each anchor keeps the
    /// max with now+secs — a far anchor survives a short floor
    /// untouched, and an expired pair moves out to the floor
    #[test]
    fn due_push_out_never_earlier() {
        // Far anchors + short floor: both anchors keep their old values
        let mut d = Due {
            next: Instant::now() + Duration::from_secs(3600),
            wall: chrono::Utc::now() + chrono::Duration::seconds(3600),
        };
        let (n0, w0) = (d.next, d.wall);
        d.push_out(5);
        assert_eq!(d.next, n0);
        assert_eq!(d.wall, w0);
        assert!(!d.expired());

        // Expired anchors + floor 30: both move out to ≈now+30
        let mut d = Due {
            next: Instant::now() - Duration::from_secs(1),
            wall: chrono::Utc::now() - chrono::Duration::seconds(1),
        };
        let (m0, w0) = (Instant::now(), chrono::Utc::now());
        d.push_out(30);
        let (m1, w1) = (Instant::now(), chrono::Utc::now());
        assert!(d.next >= m0 + Duration::from_secs(30) && d.next <= m1 + Duration::from_secs(30));
        assert!(
            d.wall >= w0 + chrono::Duration::seconds(30)
                && d.wall <= w1 + chrono::Duration::seconds(30)
        );
        assert!(!d.expired());
    }
}
