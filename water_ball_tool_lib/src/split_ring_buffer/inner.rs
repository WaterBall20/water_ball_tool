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
    /// `reject_pending_writers` 代际计数。阻塞写端入口记录当前值，若后续
    /// 发现值变化则返回拒绝错误。
    rejection_epoch: AtomicUsize,
}

// SAFETY: 手写 `Send`。`UnsafeCell` / `MaybeUninit` 抑制自动推导。
// - `T: Send` 足够：元素由写端 move 入、读端 move 出，跨线程一次转移。
// - 同一槽位同一时刻只被一方访问；同端锁串行化。
// - `clear` / `truncate_back` / `reject_pending_writers` 在 `in_flight == 0`
//   时修改结构，期间读写端被 `clear_pending` / `truncate_pending` 阻止进入。
unsafe impl<T: Send> Send for Inner<T> {}

// SAFETY: 手写 `Sync`。`UnsafeCell` / `MaybeUninit` 抑制自动推导。
// - `T: Send` 足够（无需 `T: Sync`）：同一槽位同一时刻只被一方读或写。
// - 同端锁串行化；读写端区间分离。
// - `clear_pending` / `truncate_pending` / `b_dropping` / `in_flight` 与
//   `clear_cvar` / `mutation_mutex` 协调交换、截断与 drop。
// - `rejection_epoch` 是原子计数，读写端以 `Acquire` / `Release` 读取。
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
            rejection_epoch: AtomicUsize::new(0),
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

    /// 当前拒绝代际。阻塞写端入口记录，循环内比较。
    #[inline]
    pub(crate) fn current_rejection_epoch(&self) -> usize {
        self.rejection_epoch.load(Ordering::Acquire)
    }

    /// 检查是否已被拒绝。若 `rejection_epoch` 与入口记录不同则返回 `true`。
    #[inline]
    pub(crate) fn rejection_changed(&self, my_epoch: usize) -> bool {
        self.rejection_epoch.load(Ordering::Acquire) != my_epoch
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
            // `truncate_back` / `reject_pending_writers` 不会在此刻交换
            // `a`/`b` 或修改 `write_pos`；同端由端锁或 `&mut self` 串行化。
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
            // SAFETY: `enter` 成功保证 `clear` / `truncate_back` /
            // `reject_pending_writers` 不修改 `a` 或 `write_pos`；同端由端锁
            // 或 `&mut self` 串行化。`used < cap` 保证 `idx = w % cap` 不在
            // `[r, w)` 内，槽位空闲。`ptr::write` 直接覆盖裸槽位（不 drop 旧
            // 值，槽位本就是 uninit）。写入后 `write.pos.store(Release)` 发布。
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

        self.acquire_b_dropping(&mut cp_guard);
        self.wait_in_flight_zero();

        let (old_r, old_w) = unsafe {
            // SAFETY: clear_pending == true 阻止新 pop/push 进入；in_flight == 0
            // 保证已进入的底层操作均已退出；mutation_mutex 阻止并发 truncate /
            // reject。`ptr::swap(a, b)` 交换两块 Cells 实例（Box 指针）；
            // `swap(0)` 取回旧位置用于后续 drop 并归零新 a 的位置。
            std::ptr::swap(self.a.get(), self.b.get());
            let r = self.read.pos.swap(0, Ordering::AcqRel);
            let w = self.write.pos.swap(0, Ordering::AcqRel);
            (r, w)
        };
        // 由 BDroppingGuard 记录 old_r / old_w，用于 panic 兜底
        self.set_b_range(&mut cp_guard, old_r, old_w);

        self.open_rw_window_and_notify(&mut cp_guard);
        self.drop_b_range(old_r, old_w);
        self.release_b_dropping();
    }

    /// 清空缓冲区并拒绝所有当前 pending 的阻塞写端，让它们返回
    /// `Kind::WriteRejected` / `Kind::WriteRejectedValue`。之后进入的写端不受
    /// 影响。
    ///
    /// 本方法**不获取 `write.lock`**：与 `write_all`（持写端锁直至完成并在
    /// `write_cvar` 等待）互斥由内部机制保证：
    /// - `mutation_mutex` 串行化 `clear` / `truncate_back` / 本方法；
    /// - `clear_pending` 阻止新的 `pop_one` / `push_one` 进入；
    /// - 等 `in_flight == 0` 保证所有已进入的底层操作退出；
    /// - 交换 `a`/`b` 与重置 `read.pos` / `write.pos` 在此期间无并发。
    /// 若获取 `write.lock`，会与 `write_all` 形成死锁：`write_all` 持锁等待
    /// `write_cvar`，而本方法拿不到锁无法 `notify_all`。
    pub(crate) fn reject_pending_writers(&self) {
        let _m = lock_ignore_poison(&self.mutation_mutex);

        // 与 truncate_back 互斥
        if self.truncate_pending.load(Ordering::Acquire) {
            return;
        }

        // 与 clear / 另一个 reject 互斥
        if self
            .clear_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let mut cp_guard = ClearPendingGuard::new(self);

        // 等 b_dropping 空闲并占用（防与 clear 的 drop b 并发）
        self.acquire_b_dropping(&mut cp_guard);
        // 等所有底层操作退出
        self.wait_in_flight_zero();

        // 交换 a/b、重置 pos（此时无并发访问）
        let (old_r, old_w) = unsafe {
            // SAFETY: clear_pending == true 阻止新 pop/push 进入；in_flight == 0
            // 保证已进入的底层操作均已退出；mutation_mutex 阻止并发 truncate /
            // clear。`ptr::swap(a, b)` 交换两块 Cells 实例（Box 指针）；`swap(0)`
            // 取回旧位置用于后续 drop 并归零新 a 的位置。
            std::ptr::swap(self.a.get(), self.b.get());
            let r = self.read.pos.swap(0, Ordering::AcqRel);
            let w = self.write.pos.swap(0, Ordering::AcqRel);
            (r, w)
        };
        self.set_b_range(&mut cp_guard, old_r, old_w);

        // 关键：递增 epoch 与 notify_all(write) 必须在同一 wait_mutex 临界区内，
        // 且发生在 clear_pending 复位之前。
        // - epoch 递增后，被唤醒的写端循环开头检查到 epoch 变化，返回拒绝；
        // - clear_pending 仍为 true，新写端 enter 返回 None 进入 wait；
        //   clear_pending 复位后，新写端进入时记录的 my_epoch 已是新值，不受影响。
        // - 若先复位 clear_pending 再递增 epoch，新写端可能在递增之前拿锁进入
        //   wait 并记录旧 epoch，然后被递增拒绝——语义错误。
        {
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.rejection_epoch.fetch_add(1, Ordering::AcqRel);
            self.write.notify_all_locked();
        }

        // 复位 clear_pending，唤醒读写端与等待 clear / truncate 的写端
        self.open_rw_window_and_notify(&mut cp_guard);

        // 读写端已恢复；本线程独占 b，drop 其中 [old_r, old_w) 元素
        self.drop_b_range(old_r, old_w);

        // 复位 b_dropping，唤醒等待 clear / truncate 的线程
        self.release_b_dropping();
    }

    // ----- clear / reject_pending_writers 共用的辅助步骤 -----

    fn acquire_b_dropping(&self, cp_guard: &mut ClearPendingGuard<'_, T>) {
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
        cp_guard.b_acquired = true;
    }

    fn wait_in_flight_zero(&self) {
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
    }

    fn set_b_range(&self, cp_guard: &mut ClearPendingGuard<'_, T>, old_r: usize, old_w: usize) {
        cp_guard.old_r = old_r;
        cp_guard.old_w = old_w;
        cp_guard.has_range = true;
    }

    fn open_rw_window_and_notify(&self, cp_guard: &mut ClearPendingGuard<'_, T>) {
        self.clear_pending.store(false, Ordering::Release);
        {
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.read.notify_all_locked();
            self.write.notify_all_locked();
            self.clear_cvar.notify_all();
        }
        cp_guard.mark_released();
    }

    fn drop_b_range(&self, old_r: usize, old_w: usize) {
        unsafe {
            // SAFETY: 仅访问 `b`；读写端只访问 `a`（新空块）。`b` 是旧 `a`，
            // `[old_r, old_w)` 由写端初始化。`ptr::read` 后立即
            // `ptr::write(uninit)`，即使 `T::drop` panic 也不会二次 drop。
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
    }

    fn release_b_dropping(&self) {
        self.b_dropping.store(false, Ordering::Release);
        let _g = lock_ignore_poison(&self.wait_mutex);
        self.clear_cvar.notify_all();
    }

    /// 保留 `[r, r + keep)`，丢弃尾部 `[r + keep, w)`，不交换 A/B。
    ///
    /// 调用方不得持 `write.lock`：`write_all` 会持写端锁等待空位，本方法若
    /// 也持该锁，两者将互相等待（本方法本应释放空间）。
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
                // mutation_mutex 阻止并发 clear / reject。因此 `a` 与
                // read/write.pos 无并发访问。`[new_w, w)` 槽位已初始化；
                // `ptr::read` 后立即 `ptr::write(uninit)`，即使 `T::drop`
                // panic 也不会二次 drop。
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
            // SAFETY: 调用方已持有 InFlightGuard，`clear` / `truncate_back` /
            // `reject_pending_writers` 不会交换 `a`/`b` 或修改 `write_pos`；
            // 同端由端锁或 `&mut self` 串行化，无并发读者。
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
            // SAFETY: InFlightGuard 保护下 clear / truncate / reject 不会修改
            // a；同端由端锁或 &mut self 串行化。
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

/// `clear` / `reject_pending_writers` 的 panic 兜底。
///
/// 未 `mark_released` 时在 drop 中：
/// - 若已记录 `b` 范围（交换发生），把 `b` 的 `[old_r, old_w)` 全部置 uninit，
///   避免下次交换后读到残留；
/// - 复位 `clear_pending`，唤醒所有等待方；
/// - 若已占用 `b_dropping`，复位它并唤醒等待 clear 的线程。
struct ClearPendingGuard<'a, T> {
    inner: &'a Inner<T>,
    released: bool,
    b_acquired: bool,
    has_range: bool,
    old_r: usize,
    old_w: usize,
}

impl<'a, T> ClearPendingGuard<'a, T> {
    fn new(inner: &'a Inner<T>) -> Self {
        Self {
            inner,
            released: false,
            b_acquired: false,
            has_range: false,
            old_r: 0,
            old_w: 0,
        }
    }
    fn mark_released(&mut self) {
        self.released = true;
    }
}

impl<T> Drop for ClearPendingGuard<'_, T> {
    fn drop(&mut self) {
        if !self.released {
            if self.has_range {
                unsafe {
                    // SAFETY: 仅访问 `b`；`b_dropping == true` 阻止其他 clear /
                    // reject 交换 `a`/`b`；读写端只访问 `a`。
                    // `ptr::write(b[slot], uninit)` 覆盖 `[old_r, old_w)` 内
                    // 槽位；已 drop 的槽位再次置 uninit 无副作用，未 drop 的
                    // 槽位被丢弃（泄漏资源但不破坏结构安全）。
                    let b = &*self.inner.b.get();
                    let mut idx = self.old_r;
                    while idx != self.old_w {
                        let slot = idx % self.inner.cap;
                        std::ptr::write(b[slot].get(), MaybeUninit::uninit());
                        idx = idx.wrapping_add(1);
                    }
                }
            }
            self.inner.clear_pending.store(false, Ordering::Release);
            let _g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.notify_all_locked();
            self.inner.write.notify_all_locked();
            self.inner.clear_cvar.notify_all();
            if self.b_acquired {
                self.inner.b_dropping.store(false, Ordering::Release);
            }
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
        // b 由 clear / truncate_back / reject_pending_writers 保证全 uninit；
        // 若从未执行，b 保持初始 uninit。故不遍历 b。
    }
}
