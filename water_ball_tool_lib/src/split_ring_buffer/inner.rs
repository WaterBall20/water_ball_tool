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
    b_dropping: AtomicBool,
    in_flight: AtomicUsize,
    clear_cvar: Condvar,
}

// SAFETY: 手写 `Send`。`UnsafeCell` / `MaybeUninit` 抑制自动推导。
// - `T: Send` 足够：元素由写端 move 入、读端 move 出，跨线程一次转移。
// - 同一槽位同一时刻只被一方访问；同端锁串行化。
// - `clear` 在 `in_flight == 0` 时交换 `a`/`b`，交换期间读写端被
//   `clear_pending` 阻止进入。
unsafe impl<T: Send> Send for Inner<T> {}

// SAFETY: 手写 `Sync`。`UnsafeCell` / `MaybeUninit` 抑制自动推导。
// - `T: Send` 足够（无需 `T: Sync`）：同一槽位同一时刻只被一方读或写。
// - 同端锁串行化；读写端区间分离。
// - `clear_pending` / `b_dropping` / `in_flight` 与 `clear_cvar` 协调交换与 drop。
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
            b_dropping: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            clear_cvar: Condvar::new(),
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
        self.clear_pending.load(Ordering::Acquire) || self.len() == 0
    }

    pub(crate) fn writer_should_wait(&self) -> bool {
        self.clear_pending.load(Ordering::Acquire) || self.is_full()
    }

    fn enter(&self) -> Option<InFlightGuard<'_, T>> {
        if self.clear_pending.load(Ordering::Acquire) {
            return None;
        }
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        if self.clear_pending.load(Ordering::Acquire) {
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
            // `in_flight > 0`，因此 `clear` 不会在此刻交换 `a`/`b`；同端由
            // 端锁或 `&mut self` 串行化，无其他读者/写者并发访问 `idx`。
            // `idx = r % cap` 且 `r != w`，槽位位于已初始化区间 `[r, w)`，
            // 写端经 `write.pos.store(Release)` 发布；`ptr::read` move 出
            // `MaybeUninit<T>`（move 语义，不复制），`assume_init` 取出 `T`。
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
            None => return Err(Kind::Clearing(value)),
        };
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let used = w.wrapping_sub(r);
        if used >= self.cap {
            return Err(Kind::Full(value));
        }
        let idx = w % self.cap;
        unsafe {
            // SAFETY: `enter` 已成功，`clear` 不会交换 `a`/`b`；同端由端锁或
            // `&mut self` 串行化，无其他写者并发。
            // `used < cap` 保证 `idx = w % cap` 不在 `[r, w)` 内，槽位空闲。
            // `ptr::write` 直接覆盖裸槽位（不 drop 旧值，槽位本就是 uninit）。
            // 写入后 `write.pos.store(Release)` 发布，读端 `Acquire` 观测到
            // 新 `w` 后才读取该槽位。
            std::ptr::write((*self.a.get())[idx].get(), MaybeUninit::new(value));
        }
        self.write.pos.store(w.wrapping_add(1), Ordering::Release);
        self.notify_reader_after_write();
        Ok(())
    }

    pub(crate) fn clear(&self) {
        // 1. 抢占 clear_pending；已有 clear 在进行则直接返回。
        if self
            .clear_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let mut cp_guard = ClearPendingGuard::new(self);

        // 2. 等 b_dropping 空闲并占用
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

        // 3. 等 in_flight == 0
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

        // 4. 交换 a/b、重置 pos（in_flight == 0，无并发）
        let (old_r, old_w) = unsafe {
            // SAFETY: 此刻 `clear_pending == true`，新进入的 `pop`/`push`
            // 在 `enter` 中直接返回；`in_flight == 0` 说明所有已进入的底层操作
            // 均已退出。因此 `a`/`b` 与 `read.pos`/`write.pos` 均无并发访问。
            // - `ptr::swap(self.a.get(), self.b.get())`：交换两块 `Cells<T>`
            //   实例（Box 指针），`a` 现在是旧 `b`（空块），`b` 现在是旧 `a`。
            // - `read.pos.swap(0)` / `write.pos.swap(0)`：取回旧位置用于后续
            //   drop `b` 中 `[old_r, old_w)` 的元素，并把新 `a` 的位置归零。
            std::ptr::swap(self.a.get(), self.b.get());
            let r = self.read.pos.swap(0, Ordering::AcqRel);
            let w = self.write.pos.swap(0, Ordering::AcqRel);
            (r, w)
        };
        bd_guard.set_range(old_r, old_w);

        // 5. 清 clear_pending 并唤醒读写端 —— 读写窗口在此打开
        self.clear_pending.store(false, Ordering::Release);
        {
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.read.notify_all_locked();
            self.write.notify_all_locked();
            self.clear_cvar.notify_all();
        }
        cp_guard.mark_released();

        // 6. 原地 drop b 中 [old_r, old_w) 元素并置 uninit
        unsafe {
            // SAFETY: 此处仅访问 `b`。读写端只访问 `a`（`a` 是新空块，
            // `read.pos == write.pos == 0`），不会触及 `b`。`b` 是旧 `a`，
            // 其 `[old_r, old_w)` 区间内的槽位在交换前由写端初始化。
            // - `ptr::read(slot as *mut T)`：move 出 `T`。
            // - `ptr::write(slot, MaybeUninit::uninit())`：槽位立即标记为
            //   uninit，即使随后 `drop(value)` panic，槽位也不会被二次 drop。
            // - `drop(value)`：释放元素资源。
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

        // 7. 清 b_dropping 并唤醒等待的 clear
        self.b_dropping.store(false, Ordering::Release);
        {
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.clear_cvar.notify_all();
        }
        bd_guard.mark_released();
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
            // SAFETY: `a` 在 `enter` 成功期间不会被 `clear` 交换（`clear_pending`
            // 为 false 且 `in_flight > 0`）；同端由端锁或 `&mut self` 串行化，
            // 无并发读者。`&*self.a.get()` 取得 `&Cells<T>`，其生命周期受 `_guard`
            // 约束；槽位读写遵循 `[r, r + n)` 已初始化约定。
            &*self.a.get()
        };
        // SAFETY: 第一段 [start, start + first)，所有槽位在 [r, r + n) 已初始化，
        // 写端经 `write.pos.store(Release)` 发布；`T: Copy` 下 `assume_init`
        // 复制值，不消耗槽位状态。
        for i in 0..first {
            dst[i] = unsafe {
                // SAFETY: 槽位 `start + i` 位于 `[r, r + n)` 内，已初始化；
                // `ptr::read` 读取 `MaybeUninit<T>`，`assume_init` 复制出 `T`。
                std::ptr::read(a[start + i].get()).assume_init()
            };
        }
        // SAFETY: 第二段 [0, n - first)，回绕部分仍位于 [r, r + n) 内，已初始化。
        for i in 0..(n - first) {
            dst[first + i] = unsafe {
                // SAFETY: 槽位 `i` 位于 `[r, r + n)` 的回绕段内，已初始化；
                // `ptr::read` + `assume_init` 复制出 `T`。
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
            // SAFETY: 同 `read_slice`：`enter` 成功期间 `a` 不会被 `clear`
            // 交换；同端由端锁或 `&mut self` 串行化，无并发写者。
            &*self.a.get()
        };
        // SAFETY: 第一段 [start, start + first)，所有槽位在 [w, w + n) 空闲，
        // `used + n <= cap` 保证不与读区间 [r, w) 重叠。
        for i in 0..first {
            unsafe {
                // SAFETY: 槽位 `start + i` 位于 `[w, w + n)` 内，空闲；
                // `ptr::write` 直接覆盖为 `MaybeUninit::new(src[i])`。
                std::ptr::write(a[start + i].get(), MaybeUninit::new(src[i]));
            }
        }
        // SAFETY: 第二段 [0, n - first)，回绕部分仍位于 [w, w + n) 内，空闲。
        for i in 0..(n - first) {
            unsafe {
                // SAFETY: 槽位 `i` 位于 `[w, w + n)` 的回绕段内，空闲；
                // `ptr::write` 覆盖为 `MaybeUninit::new(src[first + i])`。
                std::ptr::write(a[i].get(), MaybeUninit::new(src[first + i]));
            }
        }
        self.write.pos.store(w.wrapping_add(n), Ordering::Release);
        self.notify_reader_after_write();
        n
    }
}

struct InFlightGuard<'a, T> {
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
            // panic 兜底：把 b 的 [old_r, old_w) 全部置 uninit，避免下次交换后
            // 读到残留。已 drop 的槽位再次置 uninit 无副作用；未 drop 的槽位
            // 泄漏资源但不破坏结构安全。
            if self.has_range {
                unsafe {
                    // SAFETY: 仅访问 `b`，且此刻 `b_dropping == true` 阻止其他
                    // `clear` 交换 `a`/`b`；读写端只访问 `a`，不触及 `b`。
                    // `ptr::write(b[slot], uninit)` 覆盖 `[old_r, old_w)` 内槽位；
                    // 已 drop 的槽位再次置 uninit 无副作用，未 drop 的槽位被丢弃
                    // （泄漏资源但不破坏结构安全）。
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
        // a 的 [read.pos, write.pos) 未读元素需要 drop。
        let r = self.read.pos.load(Ordering::Relaxed);
        let w = self.write.pos.load(Ordering::Relaxed);
        let a = unsafe {
            // SAFETY: `&mut self` 独占，`Inner` 正在被 drop，无并发访问；
            // `a` 与 `b` 两个 `Box` 在 `Inner` 被完全 drop 后才释放。
            &*self.a.get()
        };
        let mut idx = r;
        while idx != w {
            let slot = idx % self.cap;
            unsafe {
                // SAFETY: 独占 `&mut self`，无并发；`idx` 位于 `[r, w)`，
                // 槽位已由写端初始化。`drop_in_place` 把 `*mut MaybeUninit<T>`
                // cast 成 `*mut T`（布局相同），就地 drop 元素，不读取值。
                std::ptr::drop_in_place(a[slot].get() as *mut T);
            }
            idx = idx.wrapping_add(1);
        }
        // b 由 clear 保证全 uninit（panic 时由 BDroppingGuard 兜底置 uninit）；
        // 若从未 clear，b 保持初始 uninit。故不遍历 b。
    }
}
