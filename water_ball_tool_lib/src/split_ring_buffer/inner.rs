use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
//AI=== 唤醒握手需要 SeqCst 屏障（见 notify_*_after_*）。
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering, fence};
//AI_END===
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
        //AI=== 等待方侧屏障：登记计数后必须让随后读取的读写位置排在它之后，
        // 与发布方 notify_*_after_* 的屏障一起构成 “SB + 双侧 SeqCst fence”，
        // 保证两侧不会同时看不到对方（丢唤醒 → 永久阻塞）。
        // Waiter-side barrier: the predicate read that follows registration must not be
        // reordered before it; with the publisher-side fence in notify_*_after_* this forms the
        // "SB + two SeqCst fences" pattern, so both sides cannot miss each other at once.
        fence(Ordering::SeqCst);
        //AI_END===
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

#[inline]
fn cells_data_ptr<T>(cells: &Cells<T>) -> *mut UnsafeCell<MaybeUninit<T>> {
    cells.as_ptr().cast_mut()
}

pub(crate) struct Inner<T> {
    a: UnsafeCell<Cells<T>>,
    /// 当前 A 数据区的原子指针；读端只通过它访问槽位，扩容交换时不会有数据竞争。
    ///
    /// Atomic pointer to the current A data area; readers access slots through it, so swapping
    /// the A/B boxes during a grow is race-free.
    a_ptr: AtomicPtr<UnsafeCell<MaybeUninit<T>>>,
    b: UnsafeCell<Cells<T>>,
    /// 实际分配长度（A/B 的槽位数）；调整大小时才改变。
    ///
    /// Physical allocation length (slots in A/B); only changes when growing past the allocation.
    alloc_cap: AtomicUsize,
    /// 当前逻辑容量；缩小/分配内增长只改这个值。
    ///
    /// Current logical capacity; shrink and grow-within-allocation only change this.
    cap: AtomicUsize,
    pub(crate) read: Endpoint,
    pub(crate) write: Endpoint,
    pub(crate) wait_mutex: Mutex<()>,
    clear_pending: AtomicBool,
    truncate_pending: AtomicBool,
    /// 扩容期间阻止新写端进入；读端不受影响。
    ///
    /// Blocks new writers while growing past the allocation; readers are not affected.
    resize_pending: AtomicBool,
    /// 扩容时的 seqlock：奇数表示 A/B 与 `alloc_cap` 正在交换。
    ///
    /// Grow seqlock: odd while A/B and `alloc_cap` are being swapped.
    resize_seq: AtomicUsize,
    /// 旧缓冲区的延迟释放队列：等所有在途操作退出后再释放。
    ///
    /// Retired old buffers, freed once no operation is in flight.
    retired: Mutex<Vec<Cells<T>>>,
    has_retired: AtomicBool,
    b_dropping: AtomicBool,
    in_flight: AtomicUsize,
    /// 在途写端数量；扩容只需等写端退出，读端可以继续。
    ///
    /// In-flight writers; a grow only waits for these, readers may continue.
    writers_in_flight: AtomicUsize,
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
        let a = make_cells(cap);
        let a_ptr = AtomicPtr::new(cells_data_ptr(&a));
        Self {
            a: UnsafeCell::new(a),
            a_ptr,
            b: UnsafeCell::new(make_cells(cap)),
            alloc_cap: AtomicUsize::new(cap),
            cap: AtomicUsize::new(cap),
            read: Endpoint::new(),
            write: Endpoint::new(),
            wait_mutex: Mutex::new(()),
            clear_pending: AtomicBool::new(false),
            truncate_pending: AtomicBool::new(false),
            resize_pending: AtomicBool::new(false),
            resize_seq: AtomicUsize::new(0),
            retired: Mutex::new(Vec::new()),
            has_retired: AtomicBool::new(false),
            b_dropping: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            writers_in_flight: AtomicUsize::new(0),
            clear_cvar: Condvar::new(),
            mutation_mutex: Mutex::new(()),
            rejection_epoch: AtomicUsize::new(0),
        }
    }

    pub(crate) fn len(&self) -> usize {
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let used = w.wrapping_sub(r);
        // 缩小容量时允许存量数据暂时超过逻辑容量（写端会等待读端排空）
        // After a shrink the buffered data may temporarily exceed the logical capacity (writers stall until drained).
        if used > self.alloc_cap.load(Ordering::Acquire) {
            0
        } else {
            used
        }
    }

    #[must_use]
    pub(crate) fn capacity(&self) -> usize {
        self.cap.load(Ordering::Acquire)
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.read.pos.load(Ordering::Acquire) == self.write.pos.load(Ordering::Acquire)
    }

    #[inline]
    pub(crate) fn is_full(&self) -> bool {
        self.len() >= self.cap.load(Ordering::Acquire)
    }

    pub(crate) fn reader_should_wait(&self) -> bool {
        self.clear_pending.load(Ordering::Acquire)
            || self.truncate_pending.load(Ordering::Acquire)
            || self.is_empty()
    }

    pub(crate) fn writer_should_wait(&self) -> bool {
        self.clear_pending.load(Ordering::Acquire)
            || self.truncate_pending.load(Ordering::Acquire)
            || self.resize_pending.load(Ordering::Acquire)
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

    pub(crate) fn enter_read(&self) -> Option<InFlightGuard<'_, T>> {
        self.enter_inner(false)
    }

    pub(crate) fn enter_write(&self) -> Option<InFlightGuard<'_, T>> {
        self.enter_inner(true)
    }

    fn enter_inner(&self, writer: bool) -> Option<InFlightGuard<'_, T>> {
        if self.clear_pending.load(Ordering::Acquire)
            || self.truncate_pending.load(Ordering::Acquire)
            || (writer && self.resize_pending.load(Ordering::Acquire))
        {
            return None;
        }
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        if writer {
            self.writers_in_flight.fetch_add(1, Ordering::AcqRel);
        }
        if self.clear_pending.load(Ordering::Acquire)
            || self.truncate_pending.load(Ordering::Acquire)
            || (writer && self.resize_pending.load(Ordering::Acquire))
        {
            if writer && self.writers_in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
                self.wake_clear_waiter();
            }
            if self.in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
                self.wake_clear_waiter();
            }
            return None;
        }
        Some(InFlightGuard {
            inner: self,
            writer,
        })
    }

    /// 取得一致的（当前 A 槽位切片，物理分配长度）对；扩容交换窗口内返回 `None`。
    ///
    /// 调用方必须持有 `InFlightGuard`：旧 A 在扩容后进入延迟释放队列，直到所有
    /// 在途操作退出，因此这里返回的切片引用在本次操作期间始终有效。
    ///
    /// Obtain a consistent (current A cells, physical allocation) pair; `None` during the
    /// grow swap window. The caller must hold an `InFlightGuard`: a grow retires the old A until
    /// no operation is in flight, so the returned slice stays valid for this operation.
    fn read_buffer(&self) -> Option<(&[UnsafeCell<MaybeUninit<T>>], usize)> {
        let seq = self.resize_seq.load(Ordering::Acquire);
        if seq & 1 != 0 {
            return None;
        }
        let alloc = self.alloc_cap.load(Ordering::Acquire);
        let ptr = self.a_ptr.load(Ordering::Acquire);
        // 读取数据后、复查序号前加 Acquire 屏障，防止弱内存序下把复查提前到 ptr/alloc 之前。
        // Acquire fence between reading the pair and re-reading the sequence, so the re-check
        // cannot be reordered before the pointer/capacity loads on weakly ordered CPUs.
        fence(Ordering::Acquire);
        if self.resize_seq.load(Ordering::Acquire) != seq {
            return None;
        }
        let cells = unsafe {
            // SAFETY: `ptr` 指向当前 A 的数据区，长度 `alloc`；旧分配由 `retired`
            // 延迟释放，本次操作持有 InFlightGuard，因此切片在操作期间有效。
            // `ptr` points to the current A data area with `alloc` slots; the old allocation is
            // retired until no operation is in flight, and this call holds an InFlightGuard.
            std::slice::from_raw_parts(ptr, alloc)
        };
        Some((cells, alloc))
    }

    fn wake_clear_waiter(&self) {
        let _g = lock_ignore_poison(&self.wait_mutex);
        self.clear_cvar.notify_all();
    }

    pub(crate) fn pop_one(&self) -> Option<T> {
        let _guard = self.enter_read()?;
        let Some((a, alloc)) = self.read_buffer() else {
            return None;
        };
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        if r == w {
            return None;
        }
        let idx = r % alloc;
        let val = unsafe {
            // SAFETY: `enter` 已成功，保证 `clear_pending == false` 且
            // `truncate_pending == false`，`in_flight > 0`，因此 `clear` /
            // `truncate_back` / `reject_pending_writers` 不会在此刻交换
            // `a`/`b` 或修改 `write_pos`；同端由端锁或 `&mut self` 串行化。
            // `idx = r % cap` 且 `r != w`，槽位位于已初始化区间 `[r, w)`。
            // `ptr::read` move 出 `MaybeUninit<T>`，`assume_init` 取出 `T`。
            // 读取后槽位回到未初始化状态，`push_one` 可覆盖。
            std::ptr::read(a[idx].get()).assume_init()
        };
        self.read.pos.store(r.wrapping_add(1), Ordering::Release);
        self.notify_writer_after_read();
        Some(val)
    }

    pub(crate) fn push_one(&self, value: T) -> Result<(), Kind<T>> {
        let Some(_guard) = self.enter_write() else {
            if self.resize_pending.load(Ordering::Acquire) {
                return Err(Kind::Resizing(value));
            }
            if self.clear_pending.load(Ordering::Acquire) {
                return Err(Kind::Clearing(value));
            }
            return Err(Kind::Truncating(value));
        };
        let alloc = self.alloc_cap.load(Ordering::Acquire);
        let cap = self.cap.load(Ordering::Acquire);
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let used = w.wrapping_sub(r);
        if used >= cap {
            return Err(Kind::Full(value));
        }
        let idx = w % alloc;
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
            self.a_ptr
                .store(cells_data_ptr(&*self.a.get()), Ordering::Release);
            let r = self.read.pos.swap(0, Ordering::AcqRel);
            let w = self.write.pos.swap(0, Ordering::AcqRel);
            (r, w)
        };
        // 由 BDroppingGuard 记录 old_r / old_w，用于 panic 兜底
        Self::set_b_range(&mut cp_guard, old_r, old_w);

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
            self.a_ptr
                .store(cells_data_ptr(&*self.a.get()), Ordering::Release);
            let r = self.read.pos.swap(0, Ordering::AcqRel);
            let w = self.write.pos.swap(0, Ordering::AcqRel);
            (r, w)
        };
        Self::set_b_range(&mut cp_guard, old_r, old_w);

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

    /// 只等写端退出；读端可以继续运行（扩容时使用）。
    ///
    /// Wait for writers only; readers may keep running (used by grow).
    fn wait_writers_zero(&self) {
        loop {
            if self.writers_in_flight.load(Ordering::Acquire) == 0 {
                break;
            }
            let mut g = lock_ignore_poison(&self.wait_mutex);
            if self.writers_in_flight.load(Ordering::Acquire) > 0 {
                g = wait_ignore_poison(&self.clear_cvar, g);
            }
            drop(g);
        }
    }

    fn wake_writers(&self) {
        let _g = lock_ignore_poison(&self.wait_mutex);
        self.write.notify_all_locked();
    }

    /// 旧缓冲区进入延迟释放队列，等在途操作全部退出后释放。
    ///
    /// Retire an old buffer; it is freed once no operation is in flight.
    fn retire(&self, cells: Cells<T>) {
        {
            let mut retired = lock_ignore_poison(&self.retired);
            retired.push(cells);
        }
        self.has_retired.store(true, Ordering::Release);
        if self.in_flight.load(Ordering::Acquire) == 0 {
            self.drain_retired();
        }
    }

    fn drain_retired(&self) {
        if !self.has_retired.load(Ordering::Acquire) {
            return;
        }
        let mut retired = lock_ignore_poison(&self.retired);
        retired.clear();
        self.has_retired.store(false, Ordering::Release);
    }

    fn set_b_range(cp_guard: &mut ClearPendingGuard<'_, T>, old_r: usize, old_w: usize) {
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
            let alloc = self.alloc_cap.load(Ordering::Acquire);
            let mut idx = old_r;
            while idx != old_w {
                let slot = idx % alloc;
                let slot_ptr = b[slot].get().cast::<T>();
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
                let alloc = self.alloc_cap.load(Ordering::Acquire);
                let mut idx = new_w;
                while idx != w {
                    let slot = idx % alloc;
                    let ptr = a[slot].get().cast::<T>();
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
        let Some((a, alloc)) = self.read_buffer() else {
            return 0;
        };
        out.reserve(n);
        let start = r % alloc;
        let first = (alloc - start).min(n);
        for i in 0..first {
            unsafe {
                // SAFETY: 槽位位于 `[r, r + n)` 内，已初始化；
                // `ptr::read` move 出 `T`，立即置 `uninit` 防止二次 drop。
                let ptr = a[start + i].get().cast::<T>();
                let val = std::ptr::read(ptr);
                std::ptr::write(a[start + i].get(), MaybeUninit::uninit());
                out.push(val);
            }
        }
        for i in 0..(n - first) {
            unsafe {
                // SAFETY: 回绕段仍位于 `[r, r + n)` 内，已初始化。
                let ptr = a[i].get().cast::<T>();
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
        let Some((a, alloc)) = self.read_buffer() else {
            return 0;
        };
        let start = r % alloc;
        let first = (alloc - start).min(n);
        for i in 0..first {
            unsafe {
                // SAFETY: 槽位位于 `[r, r + n)` 内，已初始化。
                let ptr = a[start + i].get().cast::<T>();
                let val = std::ptr::read(ptr);
                std::ptr::write(a[start + i].get(), MaybeUninit::uninit());
                drop(val);
            }
        }
        for i in 0..(n - first) {
            unsafe {
                // SAFETY: 回绕段仍位于 `[r, r + n)` 内，已初始化。
                let ptr = a[i].get().cast::<T>();
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
        //AI=== 发布方侧屏障：write.pos 的 Release 发布必须先于“是否有等待者”的读取，
        // 否则可能读到 0 而跳过唤醒，同时等待者又读到旧的 write.pos 而睡下 —— 双方互盲后
        // 再无人唤醒（x86 允许 StoreLoad 重排），表现为阻塞测试偶发永久卡死。
        // Publisher-side barrier: the Release store of write.pos must be visible before the
        // waiter-count read; otherwise the notify is skipped while the waiter still observes the
        // old position (x86 StoreLoad reordering) and sleeps forever with no further wakeup.
        fence(Ordering::SeqCst);
        //AI_END===
        if self.read.has_waiters() {
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.read.notify_one_locked();
        }
    }

    fn notify_writer_after_read(&self) {
        //AI=== 同上，方向相反：read.pos 的发布先于写者等待计数读取。
        // Same as above, mirrored: publish read.pos before reading the writer-waiter count.
        fence(Ordering::SeqCst);
        //AI_END===
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
    /// 调整逻辑容量；超过物理分配时重新分配并复制存量数据。
    ///
    /// - 新容量大于物理分配：分配新的 A/B，阻止新写端，等待在途写端退出；
    ///   读端继续运行。复制 `[read, write)` 到新 A（逻辑位置不变），交换 A/B，
    ///   更新容量，旧 A 进入延迟释放队列。
    /// - 新容量不大于物理分配：只原子更新逻辑容量。写端若已写入超过新容量的
    ///   数据不会丢失，但会等待读端排空后才能继续写。
    ///
    /// Adjust the logical capacity; grow past the allocation by reallocating and copying.
    ///
    /// - Larger than the physical allocation: allocate new A/B, block new writers, wait for
    ///   in-flight writers (readers keep running), copy `[read, write)` into the new A at the
    ///   same logical positions, swap A/B, update the capacity and retire the old A.
    /// - Otherwise only the logical capacity is updated atomically. Data already written past the
    ///   new capacity is kept; writers wait until the reader drains below it.
    pub(crate) fn resize(&self, new_size: usize) {
        if new_size == 0 {
            return;
        }
        let _m = lock_ignore_poison(&self.mutation_mutex);
        if self.clear_pending.load(Ordering::Acquire)
            || self.truncate_pending.load(Ordering::Acquire)
        {
            return;
        }

        let alloc = self.alloc_cap.load(Ordering::Acquire);
        if new_size <= alloc {
            self.cap.store(new_size, Ordering::Release);
            // 容量可能变大，唤醒等待空位的写端 / the capacity may have grown; wake waiting writers
            self.wake_writers();
            return;
        }

        if self.resize_pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let _resize = ResizePendingGuard::new(self);

        // 先等写端退出；读端可以继续推进 read.pos
        // Wait for writers first; readers may keep advancing read.pos.
        self.wait_writers_zero();

        // 先读 w 再读 r：r 只会增大，保证 used 不超过旧物理分配
        // Load w before r: r only grows, so used cannot exceed the old allocation.
        let w = self.write.pos.load(Ordering::Acquire);
        let r = self.read.pos.load(Ordering::Acquire);
        let used = w.wrapping_sub(r).min(alloc);

        let new_a = make_cells(new_size);
        let new_b = make_cells(new_size);
        let old_ptr = self.a_ptr.load(Ordering::Acquire);

        unsafe {
            // SAFETY: 写端已退出；读端只读旧 A。复制 `[r, w)` 到新 A 的同一逻辑位置。
            // SAFETY: writers have exited; readers only read the old A. Copy `[r, w)` into the
            // new A at the same logical positions.
            let old_a = std::slice::from_raw_parts(old_ptr, alloc);
            for i in 0..used {
                let logical = r.wrapping_add(i);
                let src = logical % alloc;
                let dst = logical % new_size;
                let value = std::ptr::read(old_a[src].get()).assume_init();
                std::ptr::write(new_a[dst].get(), MaybeUninit::new(value));
            }
        }

        // seqlock：复制期间读端继续输出旧数据；只有下面这段交换窗口内读到“暂无数据”。
        // Seqlock: readers keep delivering old data during the copy; only this swap window
        // reports "no data".
        self.resize_seq.fetch_add(1, Ordering::AcqRel);
        let mut old_a = new_a;
        let mut old_b = new_b;
        unsafe {
            // SAFETY: 写端已退出；读端不会访问两个 Box 头本身，旧 A 交换后立即进入
            // `retired` 队列（在途读者退出前不释放），旧 B 全部是未初始化槽位。
            // SAFETY: writers have exited; readers never touch the Box headers. The old A is
            // retired (not freed until in-flight readers exit) and the old B is all uninitialized.
            std::ptr::swap(self.a.get(), &mut old_a);
            std::ptr::swap(self.b.get(), &mut old_b);
            self.a_ptr
                .store(cells_data_ptr(&*self.a.get()), Ordering::Release);
        }
        self.alloc_cap.store(new_size, Ordering::Release);
        self.cap.store(new_size, Ordering::Release);
        self.resize_seq.fetch_add(1, Ordering::AcqRel);

        // 旧 A 可能仍被在途读者使用，延迟释放；旧 B 全是未初始化槽位
        // The old A may still be read; retire it. The old B is all uninitialized slots.
        self.retire(old_a);
        drop(old_b);
    }

    pub(crate) fn read_slice(&self, dst: &mut [T]) -> usize {
        if dst.is_empty() {
            return 0;
        }
        let Some(_guard) = self.enter_read() else {
            return 0;
        };
        let Some((a, alloc)) = self.read_buffer() else {
            return 0;
        };
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let avail = w.wrapping_sub(r);
        if avail == 0 || avail > alloc {
            return 0;
        }
        let n = dst.len().min(avail);
        let start = r % alloc;
        let first = (alloc - start).min(n);
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
        let Some(_guard) = self.enter_write() else {
            return 0;
        };
        let Some((a, alloc)) = self.read_buffer() else {
            return 0;
        };
        let cap = self.cap.load(Ordering::Acquire);
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let used = w.wrapping_sub(r);
        if used >= cap {
            return 0;
        }
        let free = cap - used;
        let n = src.len().min(free);
        if n == 0 {
            return 0;
        }
        let start = w % alloc;
        let first = (alloc - start).min(n);
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
    writer: bool,
}

impl<T> Drop for InFlightGuard<'_, T> {
    fn drop(&mut self) {
        if self.writer && self.inner.writers_in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.wake_clear_waiter();
        }
        if self.inner.in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.wake_clear_waiter();
            self.inner.drain_retired();
        }
    }
}

/// 扩容期间的写端闸门：复位 `resize_pending` 并唤醒等待的写端。
///
/// Writer gate during a grow: clears `resize_pending` and wakes waiting writers.
struct ResizePendingGuard<'a, T> {
    inner: &'a Inner<T>,
}

impl<'a, T> ResizePendingGuard<'a, T> {
    fn new(inner: &'a Inner<T>) -> Self {
        Self { inner }
    }
}

impl<T> Drop for ResizePendingGuard<'_, T> {
    fn drop(&mut self) {
        self.inner.resize_pending.store(false, Ordering::Release);
        self.inner.wake_writers();
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
                    let alloc = self.inner.alloc_cap.load(Ordering::Acquire);
                    let mut idx = self.old_r;
                    while idx != self.old_w {
                        let slot = idx % alloc;
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
        self.drain_retired();
        let r = self.read.pos.load(Ordering::Relaxed);
        let w = self.write.pos.load(Ordering::Relaxed);
        let a = unsafe {
            // SAFETY: `&mut self` 独占，`Inner` 正在被 drop，无并发访问；
            // `a` 与 `b` 两个 `Box` 在 `Inner` 完全 drop 后才释放。
            &*self.a.get()
        };
        let mut idx = r;
        let alloc = self.alloc_cap.load(Ordering::Relaxed);
        while idx != w {
            let slot = idx % alloc;
            unsafe {
                // SAFETY: 独占 `&mut self`，无并发；`idx` 位于 `[r, w)`，槽位
                // 已初始化。`drop_in_place` 把 `*mut MaybeUninit<T>` cast 成
                // `*mut T`（布局相同），就地 drop 元素，不读取值。
                std::ptr::drop_in_place(a[slot].get().cast::<T>());
            }
            idx = idx.wrapping_add(1);
        }
        // b 由 clear / truncate_back / reject_pending_writers 保证全 uninit；
        // 若从未执行，b 保持初始 uninit。故不遍历 b。
    }
}
