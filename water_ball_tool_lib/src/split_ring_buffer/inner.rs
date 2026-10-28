use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};

use crate::split_ring_buffer::error::Kind;

#[inline]
pub(crate) fn lock_ignore_poison<U>(m: &Mutex<U>) -> MutexGuard<'_, U> {
    // 不使用 `unwrap()`：中毒时取回内部守卫继续使用。
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[inline]
pub(crate) fn wait_ignore_poison<'a, U>(c: &Condvar, g: MutexGuard<'a, U>) -> MutexGuard<'a, U> {
    // 不使用 `unwrap()`：中毒时取回内部守卫继续使用。
    c.wait(g).unwrap_or_else(|e| e.into_inner())
}

pub(crate) struct Endpoint {
    pub(crate) pos: AtomicUsize,
    pub(crate) lock: Mutex<()>,
    pub(crate) cvar: Condvar,
    pub(crate) waiting: AtomicUsize,
    pub(crate) count: AtomicUsize,
    pub(crate) closed: AtomicBool,
}

impl Endpoint {
    fn new() -> Self {
        Self {
            pos: AtomicUsize::new(0),
            lock: Mutex::new(()),
            cvar: Condvar::new(),
            waiting: AtomicUsize::new(0),
            count: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        }
    }

    #[inline]
    pub(crate) fn enter_wait(&self) {
        self.waiting.fetch_add(1, Ordering::AcqRel);
    }
    #[inline]
    pub(crate) fn leave_wait(&self) {
        self.waiting.fetch_sub(1, Ordering::AcqRel);
    }
    #[inline]
    pub(crate) fn has_waiters(&self) -> bool {
        self.waiting.load(Ordering::Relaxed) > 0
    }

    pub(crate) fn notify_one_locked(&self) {
        if self.waiting.load(Ordering::Acquire) > 0 {
            self.cvar.notify_one();
        }
    }

    pub(crate) fn notify_all_locked(&self) {
        if self.waiting.load(Ordering::Acquire) > 0 {
            self.cvar.notify_all();
        }
    }

    #[inline]
    pub(crate) fn inc_count(&self) {
        self.count.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn dec_count_and_close(&self) -> bool {
        if self.count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.closed.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }

    #[inline]
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

type Cells<T> = Box<[UnsafeCell<MaybeUninit<T>>]>;

fn make_cells<T>(cap: usize) -> Cells<T> {
    (0..cap)
        .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

pub(crate) struct Inner<T> {
    a: UnsafeCell<Cells<T>>,
    b: UnsafeCell<Cells<T>>,
    cap: usize,
    pub(crate) read: Endpoint,
    pub(crate) write: Endpoint,
    pub(crate) wait_mutex: Mutex<()>,
    clear_pending: AtomicBool,
    truncate_pending: AtomicBool,
    b_dropping: AtomicBool,
    in_flight: AtomicUsize,
    clear_cvar: Condvar,
    mutation_mutex: Mutex<()>,
}

// SAFETY: 手写 `Send`。`UnsafeCell` / `MaybeUninit` 抑制自动推导。
// - `T: Send` 足够：元素由写端 move 入、读端 move 出，跨线程一次转移。
// - 同一槽位同一时刻只被一方访问；同端锁串行化。
// - `clear` / `truncate_back` 在 `in_flight == 0` 时修改结构，期间读写端被
//   `clear_pending` / `truncate_pending` 阻止进入。
unsafe impl<T: Send> Send for Inner<T> {}

// SAFETY: 手写 `Sync`。`UnsafeCell` / `MaybeUninit` 抑制自动推导。
// - `T: Send` 足够（无需 `T: Sync`）：同一槽位同一时刻只被一方读或写。
// - 同端锁串行化；读写端区间分离。
// - `clear_pending` / `truncate_pending` / `b_dropping` / `in_flight` 与
//   `clear_cvar` / `mutation_mutex` 协调交换、截断与 drop。
unsafe impl<T: Send> Sync for Inner<T> {}

impl<T> Inner<T> {
    pub(crate) fn with_capacity(cap: usize) -> Self {
        assert!(cap > 0, "ring buffer capacity must be > 0");
        Self {
            a: UnsafeCell::new(make_cells(cap)),
            b: UnsafeCell::new(make_cells(cap)),
            cap,
            read: Endpoint::new(),
            write: Endpoint::new(),
            wait_mutex: Mutex::new(()),
            clear_pending: AtomicBool::new(false),
            truncate_pending: AtomicBool::new(false),
            b_dropping: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            clear_cvar: Condvar::new(),
            mutation_mutex: Mutex::new(()),
        }
    }

    pub(crate) fn len(&self) -> usize {
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let used = w.wrapping_sub(r);
        if used > self.cap { 0 } else { used }
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.read.pos.load(Ordering::Acquire) == self.write.pos.load(Ordering::Acquire)
    }

    #[inline]
    pub(crate) fn is_full(&self) -> bool {
        self.len() == self.cap
    }

    pub(crate) fn reader_should_wait(&self) -> bool {
        self.clear_pending.load(Ordering::Acquire)
            || self.truncate_pending.load(Ordering::Acquire)
            || self.len() == 0
    }

    pub(crate) fn writer_should_wait(&self) -> bool {
        self.clear_pending.load(Ordering::Acquire)
            || self.truncate_pending.load(Ordering::Acquire)
            || self.is_full()
    }

    pub(crate) fn enter(&self) -> Option<InFlightGuard<'_, T>> {
        if self.clear_pending.load(Ordering::Acquire)
            || self.truncate_pending.load(Ordering::Acquire)
        {
            return None;
        }
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        if self.clear_pending.load(Ordering::Acquire)
            || self.truncate_pending.load(Ordering::Acquire)
        {
            if self.in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
                self.wake_clear_waiter();
            }
            return None;
        }
        Some(InFlightGuard { inner: self })
    }

    fn wake_clear_waiter(&self) {
        let _g = lock_ignore_poison(&self.wait_mutex);
        self.clear_cvar.notify_all();
    }

    pub(crate) fn pop_one(&self) -> Option<T> {
        let _guard = self.enter()?;
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        if r == w {
            return None;
        }
        let idx = r % self.cap;
        let val = unsafe {
            // SAFETY: `enter` 已成功，保证 `clear_pending == false` 且
            // `truncate_pending == false`，`in_flight > 0`，因此 `clear` /
            // `truncate_back` 不会在此刻交换 `a`/`b` 或修改 `write_pos`；
            // 同端由端锁或 `&mut self` 串行化。
            // `idx = r % cap` 且 `r != w`，槽位位于已初始化区间 `[r, w)`。
            // `ptr::read` move 出 `MaybeUninit<T>`，`assume_init` 取出 `T`。
            // 读取后槽位回到未初始化状态，`push_one` 可覆盖。
            std::ptr::read((*self.a.get())[idx].get()).assume_init()
        };
        self.read.pos.store(r.wrapping_add(1), Ordering::Release);
        self.notify_writer_after_read();
        Some(val)
    }

    pub(crate) fn push_one(&self, value: T) -> Result<(), Kind<T>> {
        let _guard = match self.enter() {
            Some(g) => g,
            None => {
                if self.clear_pending.load(Ordering::Acquire) {
                    return Err(Kind::Clearing(value));
                }
                return Err(Kind::Truncating(value));
            }
        };
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let used = w.wrapping_sub(r);
        if used >= self.cap {
            return Err(Kind::Full(value));
        }
        let idx = w % self.cap;
        unsafe {
            // SAFETY: `enter` 成功保证 `clear` / `truncate_back` 不修改 `a`
            // 或 `write_pos`；同端由端锁或 `&mut self` 串行化。
            // `used < cap` 保证 `idx = w % cap` 不在 `[r, w)` 内，槽位空闲。
            // `ptr::write` 直接覆盖裸槽位（不 drop 旧值，槽位本就是 uninit）。
            // 写入后 `write.pos.store(Release)` 发布。
            std::ptr::write((*self.a.get())[idx].get(), MaybeUninit::new(value));
        }
        self.write.pos.store(w.wrapping_add(1), Ordering::Release);
        self.notify_reader_after_write();
        Ok(())
    }

    pub(crate) fn clear(&self) {
        let _m = lock_ignore_poison(&self.mutation_mutex);
        if self.truncate_pending.load(Ordering::Acquire) {
            return;
        }
        if self
            .clear_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let mut cp_guard = ClearPendingGuard::new(self);

        // 等 b_dropping 空闲并占用
        loop {
            if self
                .b_dropping
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
            let mut g = lock_ignore_poison(&self.wait_mutex);
            if self.b_dropping.load(Ordering::Acquire) {
                g = wait_ignore_poison(&self.clear_cvar, g);
            }
            drop(g);
        }
        let mut bd_guard = BDroppingGuard::new(self);

        // 等 in_flight == 0
        loop {
            if self.in_flight.load(Ordering::Acquire) == 0 {
                break;
            }
            let mut g = lock_ignore_poison(&self.wait_mutex);
            if self.in_flight.load(Ordering::Acquire) > 0 {
                g = wait_ignore_poison(&self.clear_cvar, g);
            }
            drop(g);
        }

        // 交换 a/b、重置 pos
        let (old_r, old_w) = unsafe {
            // SAFETY: clear_pending == true 阻止新 pop/push 进入；in_flight == 0
            // 保证已进入的底层操作均已退出；mutation_mutex 阻止并发 truncate_back。
            // `ptr::swap(a, b)` 交换两块 Cells 实例（Box 指针）；`swap(0)` 取回
            // 旧位置用于后续 drop 并归零新 a 的位置。
            std::ptr::swap(self.a.get(), self.b.get());
            let r = self.read.pos.swap(0, Ordering::AcqRel);
            let w = self.write.pos.swap(0, Ordering::AcqRel);
            (r, w)
        };
        bd_guard.set_range(old_r, old_w);

        // 清 clear_pending 并唤醒读写端 —— 读写窗口在此打开
        self.clear_pending.store(false, Ordering::Release);
        {
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.read.notify_all_locked();
            self.write.notify_all_locked();
            self.clear_cvar.notify_all();
        }
        cp_guard.mark_released();

        // 原地 drop b 中 [old_r, old_w) 元素并置 uninit
        unsafe {
            // SAFETY: 仅访问 `b`；读写端只访问 `a`（新空块）。`b` 是旧 `a`，
            // `[old_r, old_w)` 由写端初始化。`ptr::read` 后立即 `ptr::write(uninit)`，
            // 即使 `T::drop` panic 也不会二次 drop。
            let b = &*self.b.get();
            let mut idx = old_r;
            while idx != old_w {
                let slot = idx % self.cap;
                let slot_ptr = b[slot].get() as *mut T;
                let value = std::ptr::read(slot_ptr);
                std::ptr::write(b[slot].get(), MaybeUninit::uninit());
                drop(value);
                idx = idx.wrapping_add(1);
            }
        }

        self.b_dropping.store(false, Ordering::Release);
        {
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.clear_cvar.notify_all();
        }
        bd_guard.mark_released();
    }

    /// 保留 `[r, r + keep)`，丢弃尾部 `[r + keep, w)`，不交换 A/B。
    ///
    /// - 与 `clear` 通过 `mutation_mutex` 串行化，不能并发。
    /// - 与底层读写通过 `truncate_pending` + 等 `in_flight == 0` 互斥。
    /// - `clear` 或另一个 `truncate_back` 在进行时直接返回（无操作）。
    /// - `keep >= len` 时视为保留全部，不修改指针。
    pub(crate) fn truncate_back(&self, keep: usize) {
        let _m = lock_ignore_poison(&self.mutation_mutex);
        if self.clear_pending.load(Ordering::Acquire) {
            return;
        }
        if self
            .truncate_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let mut tp_guard = TruncatePendingGuard::new(self);

        // 等 in_flight == 0
        loop {
            if self.in_flight.load(Ordering::Acquire) == 0 {
                break;
            }
            let mut g = lock_ignore_poison(&self.wait_mutex);
            if self.in_flight.load(Ordering::Acquire) > 0 {
                g = wait_ignore_poison(&self.clear_cvar, g);
            }
            drop(g);
        }

        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let len = w.wrapping_sub(r);
        let keep_actual = keep.min(len);
        let new_w = r.wrapping_add(keep_actual);

        if new_w != w {
            unsafe {
                // SAFETY: truncate_pending == true 阻止新 pop/push 进入；
                // in_flight == 0 保证已进入的底层操作均已退出；
                // mutation_mutex 阻止并发 clear。因此 `a` 与 read/write.pos
                // 无并发访问。`[new_w, w)` 槽位已初始化；`ptr::read` 后立即
                // `ptr::write(uninit)`，即使 `T::drop` panic 也不会二次 drop。
                let a = &*self.a.get();
                let mut idx = new_w;
                while idx != w {
                    let slot = idx % self.cap;
                    let ptr = a[slot].get() as *mut T;
                    let val = std::ptr::read(ptr);
                    std::ptr::write(a[slot].get(), MaybeUninit::uninit());
                    drop(val);
                    idx = idx.wrapping_add(1);
                }
            }
            self.write.pos.store(new_w, Ordering::Release);
        }

        self.truncate_pending.store(false, Ordering::Release);
        {
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.read.notify_all_locked();
            self.write.notify_all_locked();
            self.clear_cvar.notify_all();
        }
        tp_guard.mark_released();
    }

    /// 取走 `[r, r + k)` 到 `out`，推进 `read_pos`。调用方已 `enter()`。
    pub(crate) fn drain_prefix(&self, out: &mut Vec<T>, k: usize) -> usize {
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let n = k.min(w.wrapping_sub(r));
        if n == 0 {
            return 0;
        }
        let a = unsafe {
            // SAFETY: 调用方已持有 InFlightGuard，`clear` / `truncate_back`
            // 不会交换 `a`/`b` 或修改 `write_pos`；同端由端锁或 `&mut self`
            // 串行化，无并发读者。
            &*self.a.get()
        };
        out.reserve(n);
        let start = r % self.cap;
        let first = (self.cap - start).min(n);
        for i in 0..first {
            unsafe {
                // SAFETY: 槽位位于 `[r, r + n)` 内，已初始化；
                // `ptr::read` move 出 `T`，立即置 `uninit` 防止二次 drop。
                let ptr = a[start + i].get() as *mut T;
                let val = std::ptr::read(ptr);
                std::ptr::write(a[start + i].get(), MaybeUninit::uninit());
                out.push(val);
            }
        }
        for i in 0..(n - first) {
            unsafe {
                // SAFETY: 回绕段仍位于 `[r, r + n)` 内，已初始化。
                let ptr = a[i].get() as *mut T;
                let val = std::ptr::read(ptr);
                std::ptr::write(a[i].get(), MaybeUninit::uninit());
                out.push(val);
            }
        }
        self.read.pos.store(r.wrapping_add(n), Ordering::Release);
        self.notify_writer_after_read();
        n
    }

    /// 丢弃 `[r, r + k)`，推进 `read_pos`。调用方已 `enter()`。
    pub(crate) fn release_prefix(&self, k: usize) -> usize {
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let n = k.min(w.wrapping_sub(r));
        if n == 0 {
            return 0;
        }
        let a = unsafe {
            // SAFETY: 同 `drain_prefix`。
            &*self.a.get()
        };
        let start = r % self.cap;
        let first = (self.cap - start).min(n);
        for i in 0..first {
            unsafe {
                // SAFETY: 槽位位于 `[r, r + n)` 内，已初始化。
                let ptr = a[start + i].get() as *mut T;
                let val = std::ptr::read(ptr);
                std::ptr::write(a[start + i].get(), MaybeUninit::uninit());
                drop(val);
            }
        }
        for i in 0..(n - first) {
            unsafe {
                // SAFETY: 回绕段仍位于 `[r, r + n)` 内，已初始化。
                let ptr = a[i].get() as *mut T;
                let val = std::ptr::read(ptr);
                std::ptr::write(a[i].get(), MaybeUninit::uninit());
                drop(val);
            }
        }
        self.read.pos.store(r.wrapping_add(n), Ordering::Release);
        self.notify_writer_after_read();
        n
    }

    fn notify_reader_after_write(&self) {
        if self.read.has_waiters() {
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.read.notify_one_locked();
        }
    }

    fn notify_writer_after_read(&self) {
        if self.write.has_waiters() {
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.write.notify_one_locked();
        }
    }

    pub(crate) fn close_reader(&self) {
        if self.read.dec_count_and_close() {
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.write.notify_all_locked();
        }
    }

    pub(crate) fn close_writer(&self) {
        if self.write.dec_count_and_close() {
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.read.notify_all_locked();
        }
    }
}

impl<T: Copy> Inner<T> {
    pub(crate) fn read_slice(&self, dst: &mut [T]) -> usize {
        if dst.is_empty() {
            return 0;
        }
        let _guard = match self.enter() {
            Some(g) => g,
            None => return 0,
        };
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let avail = w.wrapping_sub(r);
        if avail == 0 || avail > self.cap {
            return 0;
        }
        let n = dst.len().min(avail);
        let start = r % self.cap;
        let first = (self.cap - start).min(n);
        let a = unsafe {
            // SAFETY: InFlightGuard 保护下 clear / truncate 不会修改 a；同端
            // 由端锁或 &mut self 串行化。
            &*self.a.get()
        };
        for i in 0..first {
            dst[i] = unsafe {
                // SAFETY: 槽位位于 `[r, r + n)` 内，已初始化；`T: Copy` 下
                // `ptr::read + assume_init` 复制出 `T`，槽位保持 initialized。
                std::ptr::read(a[start + i].get()).assume_init()
            };
        }
        for i in 0..(n - first) {
            dst[first + i] = unsafe {
                // SAFETY: 回绕段仍位于 `[r, r + n)` 内，已初始化。
                std::ptr::read(a[i].get()).assume_init()
            };
        }
        self.read.pos.store(r.wrapping_add(n), Ordering::Release);
        self.notify_writer_after_read();
        n
    }

    pub(crate) fn write_slice(&self, src: &[T]) -> usize {
        if src.is_empty() {
            return 0;
        }
        let _guard = match self.enter() {
            Some(g) => g,
            None => return 0,
        };
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let used = w.wrapping_sub(r);
        if used > self.cap {
            return 0;
        }
        let free = self.cap - used;
        let n = src.len().min(free);
        if n == 0 {
            return 0;
        }
        let start = w % self.cap;
        let first = (self.cap - start).min(n);
        let a = unsafe {
            // SAFETY: 同 `read_slice`。
            &*self.a.get()
        };
        for i in 0..first {
            unsafe {
                // SAFETY: 槽位位于 `[w, w + n)` 内，空闲；
                // `ptr::write` 覆盖为 `MaybeUninit::new(src[i])`。
                std::ptr::write(a[start + i].get(), MaybeUninit::new(src[i]));
            }
        }
        for i in 0..(n - first) {
            unsafe {
                // SAFETY: 回绕段仍位于 `[w, w + n)` 内，空闲。
                std::ptr::write(a[i].get(), MaybeUninit::new(src[first + i]));
            }
        }
        self.write.pos.store(w.wrapping_add(n), Ordering::Release);
        self.notify_reader_after_write();
        n
    }
}

pub(crate) struct InFlightGuard<'a, T> {
    inner: &'a Inner<T>,
}

impl<T> Drop for InFlightGuard<'_, T> {
    fn drop(&mut self) {
        if self.inner.in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.wake_clear_waiter();
        }
    }
}

struct ClearPendingGuard<'a, T> {
    inner: &'a Inner<T>,
    released: bool,
}

impl<'a, T> ClearPendingGuard<'a, T> {
    fn new(inner: &'a Inner<T>) -> Self {
        Self {
            inner,
            released: false,
        }
    }
    fn mark_released(&mut self) {
        self.released = true;
    }
}

impl<T> Drop for ClearPendingGuard<'_, T> {
    fn drop(&mut self) {
        if !self.released {
            // panic 兜底：复位 clear_pending 并唤醒读写端与等待 clear 的写端。
            self.inner.clear_pending.store(false, Ordering::Release);
            let _g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.notify_all_locked();
            self.inner.write.notify_all_locked();
            self.inner.clear_cvar.notify_all();
        }
    }
}

struct TruncatePendingGuard<'a, T> {
    inner: &'a Inner<T>,
    released: bool,
}

impl<'a, T> TruncatePendingGuard<'a, T> {
    fn new(inner: &'a Inner<T>) -> Self {
        Self {
            inner,
            released: false,
        }
    }
    fn mark_released(&mut self) {
        self.released = true;
    }
}

impl<T> Drop for TruncatePendingGuard<'_, T> {
    fn drop(&mut self) {
        if !self.released {
            // panic 兜底：复位 truncate_pending 并唤醒读写端。
            // 若 panic 发生在 write_pos.store 之前，指针未变，未 drop 的元素
            // 由 Inner::Drop 处理；若发生在 store 之后，指针已正确。
            self.inner.truncate_pending.store(false, Ordering::Release);
            let _g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.notify_all_locked();
            self.inner.write.notify_all_locked();
            self.inner.clear_cvar.notify_all();
        }
    }
}

struct BDroppingGuard<'a, T> {
    inner: &'a Inner<T>,
    released: bool,
    old_r: usize,
    old_w: usize,
    has_range: bool,
}

impl<'a, T> BDroppingGuard<'a, T> {
    fn new(inner: &'a Inner<T>) -> Self {
        Self {
            inner,
            released: false,
            old_r: 0,
            old_w: 0,
            has_range: false,
        }
    }
    fn set_range(&mut self, old_r: usize, old_w: usize) {
        self.old_r = old_r;
        self.old_w = old_w;
        self.has_range = true;
    }
    fn mark_released(&mut self) {
        self.released = true;
    }
}

impl<T> Drop for BDroppingGuard<'_, T> {
    fn drop(&mut self) {
        if !self.released {
            if self.has_range {
                unsafe {
                    // SAFETY: 仅访问 `b`；`b_dropping == true` 阻止其他 `clear`
                    // 交换 `a`/`b`；读写端只访问 `a`。`ptr::write(b[slot], uninit)`
                    // 覆盖 `[old_r, old_w)` 内槽位；已 drop 的槽位再次置 uninit
                    // 无副作用，未 drop 的槽位被丢弃（泄漏资源但不破坏结构安全）。
                    let b = &*self.inner.b.get();
                    let mut idx = self.old_r;
                    while idx != self.old_w {
                        let slot = idx % self.inner.cap;
                        std::ptr::write(b[slot].get(), MaybeUninit::uninit());
                        idx = idx.wrapping_add(1);
                    }
                }
            }
            self.inner.b_dropping.store(false, Ordering::Release);
            let _g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.clear_cvar.notify_all();
        }
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        let r = self.read.pos.load(Ordering::Relaxed);
        let w = self.write.pos.load(Ordering::Relaxed);
        let a = unsafe {
            // SAFETY: `&mut self` 独占，`Inner` 正在被 drop，无并发访问；
            // `a` 与 `b` 两个 `Box` 在 `Inner` 完全 drop 后才释放。
            &*self.a.get()
        };
        let mut idx = r;
        while idx != w {
            let slot = idx % self.cap;
            unsafe {
                // SAFETY: 独占 `&mut self`，无并发；`idx` 位于 `[r, w)`，槽位
                // 已初始化。`drop_in_place` 把 `*mut MaybeUninit<T>` cast 成
                // `*mut T`（布局相同），就地 drop 元素，不读取值。
                std::ptr::drop_in_place(a[slot].get() as *mut T);
            }
            idx = idx.wrapping_add(1);
        }
        // b 由 clear / truncate_back 保证全 uninit；若从未 clear / truncate，
        // b 保持初始 uninit。故不遍历 b。
    }
}
