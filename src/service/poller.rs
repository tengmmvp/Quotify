//! 后台轮询线程

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};

use crate::api::AccountSpec;
use crate::platform::msg::WM_APP_POLL_RESULT;

/// 默认轮询间隔（秒）
pub const DEFAULT_INTERVAL_SECS: u64 = 300;
/// 轮询间隔下限（秒）
pub const MIN_POLL_SECS: u64 = 10;
/// 轮询间隔上限（秒）= 1 天
pub const MAX_POLL_SECS: u64 = 86_400;
/// 429 指数退避基数（秒）：受限期间数据持续变化，快速重试争取恢复。
const RATE_LIMIT_BACKOFF_BASE_SECS: u64 = 30;

/// 429 的下一次等待：连续失败翻倍封顶。
/// Retry-After 头只作用本次，与指数档取大，不外溢到后续；
/// 上限已在 client 侧钳过，此处不重复钳。
fn backoff_wait(fails: u32, retry_after_secs: Option<u64>) -> u64 {
    let exp = RATE_LIMIT_BACKOFF_BASE_SECS
        .saturating_mul(1 << fails.min(4))
        .min(crate::api::client::RATE_LIMIT_BACKOFF_MAX_SECS);
    retry_after_secs.map_or(exp, |ra| ra.max(exp))
}

/// 读取间隔并夹取到合法区间
fn clamp_interval(secs: u64) -> u64 {
    secs.clamp(MIN_POLL_SECS, MAX_POLL_SECS)
}

/// 一次轮询的结果，跨线程传递给主线程。
pub enum PollOutcome {
    Success(Box<crate::api::UsageSnapshot>),
    Failure(Box<crate::api::FetchError>),
}

/// 轮询结果的回传信封
pub struct PollMessage {
    pub generation: u64,
    pub outcome: PollOutcome,
}

/// 轮询结果的回传通道：轮询线程入队，UI 线程被唤醒后排空。
pub static POLL_SLOT: crate::platform::post::Slot<PollMessage> = crate::platform::post::Slot::new();

pub type PollTarget = Arc<Mutex<Option<AccountSpec>>>;
pub type PollInterval = Arc<Mutex<u64>>;
pub type PollGeneration = Arc<AtomicU64>;

/// 后台轮询线程的所有者：发唤醒/停止信号，drop 时 join 回收。
pub struct Poller {
    wake: HANDLE,
    stop: HANDLE,
    refresh_requested: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Poller {
    /// 启动轮询线程
    pub fn spawn(
        hwnd: windows::Win32::Foundation::HWND,
        target: PollTarget,
        interval: PollInterval,
        generation: PollGeneration,
    ) -> Option<Self> {
        unsafe {
            let wake = CreateEventW(None, false, false, None).ok()?;
            let stop = CreateEventW(None, true, false, None).ok()?;
            let refresh_requested = Arc::new(AtomicBool::new(false));
            let flag = refresh_requested.clone();

            // Win32 句柄是内核对象引用、跨线程移动安全，但 HANDLE 未实现 Send。
            struct SendHandle<T>(T);
            unsafe impl<T> Send for SendHandle<T> {}

            let (hwnd_s, wake_s, stop_s) = (SendHandle(hwnd), SendHandle(wake), SendHandle(stop));
            let thread = match std::thread::Builder::new()
                .name("quotify-poller".into())
                .spawn(move || {
                    // 先把 SendHandle 移入闭包局部，防 2021 精准捕获绕过包装破坏 Send。
                    let (h, w, s) = (hwnd_s, wake_s, stop_s);
                    poll_loop(h.0, target, interval, w.0, s.0, flag, generation)
                }) {
                Ok(t) => t,
                Err(e) => {
                    // 线程未起时 Poller 不会诞生，Drop 不再兜底，
                    // 须在此关闭两事件句柄防泄漏。
                    crate::platform::log(&format!("[Quotify] 轮询线程创建失败，托盘将无数据: {e}"));
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

    /// 立即拉取一次的唤醒入口：手动刷新、改间隔/代理/换账号后立即
    /// 生效，电源唤醒补拉同走此路——只打信号不置标志的话，睡眠跨在
    /// 途拉取时拉取自身的重锚会把信号吃掉，补拉落空。
    pub fn refresh_now(&self) {
        self.refresh_requested.store(true, Ordering::Release);
        unsafe {
            let _ = SetEvent(self.wake);
        };
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop);
        };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.wake);
            let _ = windows::Win32::Foundation::CloseHandle(self.stop);
        }
    }
}

/// 间隔重排许可：间隔已变且无退避在场——退避期间节奏归退避锚独占，
/// 重排会提前顶撞。拉取后与未到期唤醒两路同引。
fn interval_retunable(fails: u32, interval: u64, last_secs: u64) -> bool {
    interval != last_secs && fails == 0
}

/// 一轮拉取后的锚定动作
enum Schedule {
    /// 按给定秒数重锚：成功与自动失败
    Reset(u64),
    /// 手动失败不补拉：日程只推后不提前至 floor；间隔已变且非退避时
    /// 按新间隔重排，只重排不补拉
    Defer { reanchor: bool, floor: u64 },
}

/// 一轮结果的下次日程与连续 429 计数：节奏规则单点。
/// 手动失败不改写自动状态，计数不动；429 的等待下限取退避档与
/// Retry-After 的较大者——服务器指令必须生效；非 429 手动失败按
/// 常规间隔兜底。
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

/// 轮询循环
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
    // 双锚到期点：单调钟为主，墙钟并行——睡眠可能停走单调钟，
    // 唤醒后由墙锚判定立即补拉。成对更新走同一方法，防新调度点只更其一。
    let mut due = Due::now();

    /// 单调锚与墙锚的成对持有
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

        /// 双锚一同后移 secs 秒
        fn set(&mut self, secs: u64) {
            self.next = Instant::now() + Duration::from_secs(secs);
            self.wall = chrono::Utc::now() + chrono::Duration::seconds(secs as i64);
        }

        /// 双锚任一已过即到期：单调钟睡眠停走的兜底由墙锚承担
        fn expired(&self) -> bool {
            Instant::now() >= self.next || chrono::Utc::now() >= self.wall
        }

        /// 双锚各自与 now+secs 取大：日程只推后不提前。
        /// 手动失败的插队语义专用——常规重锚走 set。
        fn push_out(&mut self, secs: u64) {
            let n = Instant::now() + Duration::from_secs(secs);
            let w = chrono::Utc::now() + chrono::Duration::seconds(secs as i64);
            self.next = self.next.max(n);
            self.wall = self.wall.max(w);
        }
    }

    // 连续 429 计数：成功、非 429 结果或换源世代即清零，等待节奏归指数
    // 退避接管。被限流账号的计数不得饿死新切换的健康账号。
    let mut backoff_fails: u32 = 0;
    let mut last_gen = u64::MAX;

    loop {
        let now = Instant::now();
        // 等待取单调锚与墙锚的较早者：单调钟睡眠停走，唤醒广播再丢失
        // 也不至于按旧单调锚睡过头。
        // 饱和上限取 u32::MAX - 1：u32::MAX 恰是 Win32 的 INFINITE，
        // 落在它上面会退化成永久等待。
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

        // 取世代号须早于账号克隆：主线程先写 target 再 bump 世代，反序
        // 会让旧账号数据带新号混入；先取号则见新号必见新 target。
        // 世代变化即换源：计数归零，锚清到当前，新账号的日程由换源后的
        // 拉取结果决定，不继承旧账号的退避等待。
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
                    // 无账号：挂起等下一次唤醒，不空转。
                    due.set(60);
                    continue;
                }
            }
        };
        let fetched = crate::api::client::fetch_usage(&spec);
        let interval_secs = clamp_interval(*borrow(&interval));
        // 节奏与计数单点计算：借 fetched 判定，move 交给回传。
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
            // 手动失败是纯插队：不给用户补拉第二次，日程只推后不提前。
            // 例外是非退避中间隔已变：旧锚按旧间隔排定，沿用会吞掉新间隔
            // 直到旧锚到期，按新间隔从现在重排，只重排不补拉。
            Schedule::Defer { reanchor, floor } => {
                if reanchor {
                    due.set(interval_secs);
                }
                due.push_out(floor);
            }
        }
        // last_secs 每轮记当前间隔（含退避轮），只作间隔变化判定。退避
        // 等待不入此变量，掺入会被当间隔变化重排掉退避锚。
        last_secs = interval_secs;
    }
}

/// 毒化锁取回内部数据继续用
fn borrow<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

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

    /// 退避序列钉位：30→60→120→240→300 封顶；头只作用于本次且只延长不缩短
    #[test]
    fn backoff_sequence_and_retry_after_priority() {
        assert_eq!(backoff_wait(0, None), 30);
        assert_eq!(backoff_wait(1, None), 60);
        assert_eq!(backoff_wait(2, None), 120);
        assert_eq!(backoff_wait(3, None), 240);
        assert_eq!(backoff_wait(4, None), 300);
        assert_eq!(backoff_wait(9, None), 300);
        // 头长于指数按头，短于指数不缩短档位，0 不构成立即重试
        assert_eq!(backoff_wait(0, Some(45)), 45);
        assert_eq!(backoff_wait(0, Some(0)), 30);
        assert_eq!(backoff_wait(3, Some(45)), 240);
        assert_eq!(backoff_wait(3, Some(0)), 240);
    }

    /// 节奏矩阵钉位：成功清计数按间隔；自动 429 计数递增按退避档；
    /// 手动失败计数不动只推后，间隔重排须无退避在场
    #[test]
    fn schedule_next_matrix() {
        let throttled = |ra: Option<u64>| crate::api::FetchError::Throttled {
            info: crate::api::ErrorInfo::local("429"),
            retry_after_secs: ra,
        };
        let failure = crate::api::FetchError::Network("offline".into());
        let (interval, last) = (300u64, 300u64);

        // 成功：按间隔重锚，计数清零
        assert!(matches!(
            schedule_next(None, false, 2, interval, last),
            (Schedule::Reset(300), 0)
        ));

        // 自动 429：计数递增，等待按退避档
        assert!(matches!(
            schedule_next(Some(&throttled(None)), false, 1, interval, last),
            (Schedule::Reset(60), 2)
        ));
        // 自动非 429：计数清零，按常规间隔
        assert!(matches!(
            schedule_next(Some(&failure), false, 1, interval, last),
            (Schedule::Reset(300), 0)
        ));

        // 手动 429：计数不动，推后下限取退避档与头的较大者
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
        // 手动非 429：推后下限按常规间隔
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

        // 手动失败且间隔已变：非退避才允许按新间隔重排
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
}
