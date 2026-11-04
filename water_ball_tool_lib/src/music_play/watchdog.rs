//! 线程崩溃看门狗 / thread crash watchdog
//!
//! 被监视线程在线程闭包内持有一个 [`ThreadExitGuard`]，线程结束（包括 panic unwind）时
//! 守卫的 `Drop` 把该线程的槽位置为“已退出”并唤醒看门狗线程；看门狗因此只需一个线程，
//! 不需要轮询 `JoinHandle::is_finished()`。
//!
//! A watched thread owns a [`ThreadExitGuard`] inside its closure. When the thread ends (panic
//! unwind included) the guard's `Drop` marks that slot as exited and wakes the watchdog thread, so
//! a single watchdog thread suffices and no `JoinHandle::is_finished()` polling is needed.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

//AI=== 被监视线程槽位（位掩码）/ watched thread slots (bit mask)
/// 解码线程 / decode thread
pub const SLOT_DECODER: u32 = 1 << 0;
/// 前效果器线程 / pre-effect runtime thread
pub const SLOT_PRE_EFFECT: u32 = 1 << 1;
//AI_END===

#[derive(Default)]
struct WatchState {
    /// 已退出的线程槽位位掩码（看门狗取走后清零）。
    exited: u32,
    /// 关停标志：置位后看门狗退出。
    shutdown: bool,
}

/// 看门狗共享状态 / shared watchdog state
pub struct WatchNotify {
    inner: Mutex<WatchState>,
    cv: Condvar,
}

impl WatchNotify {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(WatchState::default()),
            cv: Condvar::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, WatchState> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 为被监视线程创建退出守卫（放进线程闭包，随线程结束 Drop）。
    /// Create the exit guard for a watched thread; it drops when the thread ends.
    pub fn exit_guard(self: &Arc<Self>, slot: u32) -> ThreadExitGuard {
        ThreadExitGuard {
            notify: Arc::clone(self),
            slot,
        }
    }

    /// 等待线程退出，返回退出槽位位掩码；关停时返回 `None`。
    /// Block until a watched thread exits; `None` means shutdown.
    pub fn wait_exit(&self) -> Option<u32> {
        let mut state = self.lock();
        while state.exited == 0 && !state.shutdown {
            state = self.cv.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
        if state.shutdown {
            return None;
        }
        let exited = state.exited;
        state.exited = 0;
        Some(exited)
    }

    /// 请求看门狗退出并唤醒它 / request the watchdog to exit and wake it.
    pub fn shutdown(&self) {
        let mut state = self.lock();
        state.shutdown = true;
        self.cv.notify_all();
    }
}

impl Default for WatchNotify {
    fn default() -> Self {
        Self {
            inner: Mutex::new(WatchState::default()),
            cv: Condvar::new(),
        }
    }
}

/// 线程退出守卫：`Drop`（含 panic unwind）时标记本线程槽位并唤醒看门狗。
/// Thread exit guard: on `Drop` (panic unwind included) it marks the slot and wakes the watchdog.
pub struct ThreadExitGuard {
    notify: Arc<WatchNotify>,
    slot: u32,
}

impl Drop for ThreadExitGuard {
    fn drop(&mut self) {
        let mut state = self.notify.lock();
        state.exited |= self.slot;
        self.notify.cv.notify_all();
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    //AI=== 线程退出（含 panic）都能唤醒看门狗 / both normal exit and panic wake the watchdog
    #[test]
    fn exit_guard_wakes_on_normal_exit_and_panic() {
        let notify = WatchNotify::new();
        let panicked = Arc::new(AtomicBool::new(false));

        let worker = {
            let notify = Arc::clone(&notify);
            std::thread::Builder::new()
                .name("watchdog-test-worker".to_string())
                .spawn(move || {
                    let _guard = notify.exit_guard(SLOT_DECODER);
                })
                .expect("spawn worker")
        };
        worker.join().expect("worker must not panic");

        assert_eq!(
            notify.wait_exit(),
            Some(SLOT_DECODER),
            "normal exit must wake the watchdog with the decoder slot"
        );

        let worker = {
            let notify = Arc::clone(&notify);
            let panicked = Arc::clone(&panicked);
            std::thread::Builder::new()
                .name("watchdog-test-panicker".to_string())
                .spawn(move || {
                    let _guard = notify.exit_guard(SLOT_PRE_EFFECT);
                    panicked.store(true, Ordering::Release);
                    panic!("intentional panic for the watchdog test");
                })
                .expect("spawn panicker")
        };
        assert!(worker.join().is_err(), "worker must panic");

        assert_eq!(
            notify.wait_exit(),
            Some(SLOT_PRE_EFFECT),
            "panic must wake the watchdog with the pre-effect slot"
        );
    }

    //AI=== 关停后看门狗立即返回 / shutdown makes the watchdog return immediately
    #[test]
    fn shutdown_returns_none() {
        let notify = WatchNotify::new();
        notify.shutdown();
        assert_eq!(notify.wait_exit(), None);
    }

    //AI=== 未退出时阻塞（用超时线程避免测试挂死）/ blocks while nothing exits
    #[test]
    fn wait_exit_blocks_until_shutdown() {
        let notify = WatchNotify::new();
        let waiter = {
            let notify = Arc::clone(&notify);
            std::thread::Builder::new()
                .name("watchdog-test-waiter".to_string())
                .spawn(move || notify.wait_exit())
                .expect("spawn waiter")
        };
        std::thread::sleep(Duration::from_millis(20));
        notify.shutdown();
        assert_eq!(
            waiter.join().expect("waiter must not panic"),
            None,
            "shutdown must release the waiter"
        );
    }
}
