#[cfg(test)]
pub mod test;

use std::cell::UnsafeCell;
use std::fmt::{Debug, Display, Formatter};
use std::mem::MaybeUninit;
use std::ops::{Index, IndexMut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

struct Slot<T>(UnsafeCell<MaybeUninit<T>>);

impl<T> Slot<T> {
    fn new() -> Self {
        Self(UnsafeCell::new(MaybeUninit::uninit()))
    }

    /// # Safety
    /// 调用方保证该槽位当前处于未初始化状态（不属于 [r, w)）。
    unsafe fn write(&self, value: T) {
        // SAFETY: UnsafeCell 允许 &self 下写入；MaybeUninit 允许覆盖未初始化内存。
        unsafe {
            (*self.0.get()).write(value);
        }
    }

    /// # Safety
    /// 调用方保证该槽位当前处于已初始化状态（属于 [r, w)）。
    unsafe fn take(&self) -> T {
        // SAFETY: 槽位已初始化，assume_init_read 移出值；
        // 之后槽位逻辑上回到未初始化状态，调用方不得重复 take。
        unsafe { (*self.0.get()).assume_init_read() }
    }

    /// # Safety
    /// 调用方保证该槽位当前处于已初始化状态，且在引用存续期间不会被 write/take。
    unsafe fn get_ref(&self) -> &T {
        // SAFETY: 槽位已初始化。
        unsafe { (*self.0.get()).assume_init_ref() }
    }

    /// # Safety
    /// - 槽位必须处于已初始化状态；
    /// - 调用方必须保证：在解引用返回指针的整个存续期内，该槽位被独占
    ///   （无并发读写，且不得与其他 `&mut T` 引用重叠）。
    ///
    /// 返回裸指针而非 `&mut T`：`&self` 无法提供 `&mut T` 所需的独占
    /// 生命周期；由调用方在自己的 `&mut self` 上下文中转换为 `&mut T`。
    unsafe fn get_mut_ptr(&self) -> *mut T {
        // SAFETY: `Slot<T> = UnsafeCell<MaybeUninit<T>>`；`MaybeUninit<T>` 与 `T`
        // 具有相同的大小、对齐和起始地址（标准库布局保证），故指针 cast 合法。
        // 指针所指内存已初始化（由调用方 SAFETY 条件保证）。独占性亦由调用方保证。
        self.0.get().cast::<T>()
    }
}

impl<T: Copy> Slot<T> {
    /// # Safety
    /// 槽位必须处于已初始化状态。调用后槽位保持已初始化（复制而非移动）。
    unsafe fn copy_out(&self) -> T {
        // SAFETY: 槽位已初始化，取引用后按位复制；源槽位不变。
        unsafe { *(*self.0.get()).assume_init_ref() }
    }
}

pub struct ConcurrentRing<T> {
    slots: Box<[Slot<T>]>,
    cap: AtomicUsize,
    write_pos: AtomicUsize,
    read_pos: AtomicUsize,
    lock_content: LockContent,
    max_cap: AtomicUsize,
    is_max_cap: AtomicBool,
}

struct LockContent {
    write_lock: Mutex<()>,
    read_gate: RwLock<()>,

    // —— 阻塞路径的锁 ——
    wait_lock: Mutex<()>, // Condvar 关联锁，同时保护“检查 + wait”
    not_full: Condvar,    // 写者在此等待“有空间”
    not_empty: Condvar,   // 读者在此等待“有数据”

    // —— 唤醒优化 ——
    read_waiters: AtomicUsize,  // 正在 not_empty 上等待的读者数
    write_waiters: AtomicUsize, // 正在 not_full 上等待的写者数
}

impl<T> Drop for ConcurrentRing<T> {
    fn drop(&mut self) {
        // 此时已无并发访问（&mut self），无需锁。
        let r = *self.read_pos.get_mut();
        let w = *self.write_pos.get_mut();
        let mut i = r;
        while i != w {
            let idx = i % self.cap.load(Ordering::SeqCst);
            // SAFETY: [r, w) 区间内的槽位处于已初始化状态（由 push/write 写入且未 pop/read）。
            unsafe {
                self.slots[idx].take();
            }
            i = i.wrapping_add(1);
        }
        // Box<[Slot<T>]> 随后释放内存；MaybeUninit 不会 drop 内容，正确。
    }
}

// SAFETY:
// 只有 T: Send 才能跨线程移动槽位内容；UnsafeCell<MaybeUninit<T>>
// 本身不提供 Sync，需由外部同步原语（write_lock / read_gate / wait_lock）保证互斥访问。
unsafe impl<T: Send> Send for ConcurrentRing<T> {}

// SAFETY:
// 所有对 Slot 的写/取均被 write_lock、read_gate、wait_lock 保护，
// 且 T: Send 足以支持跨线程转移所有权（pop/take）。
unsafe impl<T: Send> Sync for ConcurrentRing<T> {}

impl<T> ConcurrentRing<T> {
    // ---------- 锁获取辅助：毒化解毒 ----------

    /// 获取 `write_lock`；若检测到 poisoning，取回 guard 并清除毒化标志。
    fn lock_write_lock(&self) -> MutexGuard<'_, ()> {
        match self.lock_content.write_lock.lock() {
            Ok(g) => g,
            Err(e) => {
                let g = e.into_inner();
                self.lock_content.write_lock.clear_poison();
                g
            }
        }
    }

    /// 以写方式获取 `read_gate`；毒化解毒。
    fn lock_read_gate_write(&self) -> RwLockWriteGuard<'_, ()> {
        match self.lock_content.read_gate.write() {
            Ok(g) => g,
            Err(e) => {
                let g = e.into_inner();
                self.lock_content.read_gate.clear_poison();
                g
            }
        }
    }

    /// 以读方式获取 `read_gate`；毒化解毒。
    fn lock_read_gate_read(&self) -> RwLockReadGuard<'_, ()> {
        match self.lock_content.read_gate.read() {
            Ok(g) => g,
            Err(e) => {
                let g = e.into_inner();
                self.lock_content.read_gate.clear_poison();
                g
            }
        }
    }

    /// 获取 `wait_lock`；毒化解毒。
    fn lock_wait_lock(&self) -> MutexGuard<'_, ()> {
        match self.lock_content.wait_lock.lock() {
            Ok(g) => g,
            Err(e) => {
                let g = e.into_inner();
                self.lock_content.wait_lock.clear_poison();
                g
            }
        }
    }

    /// 在 `not_full` 上等待；关联 `wait_lock` 被毒化时取回 guard 并解毒。
    fn wait_not_full<'a>(&self, guard: MutexGuard<'a, ()>) -> MutexGuard<'a, ()> {
        match self.lock_content.not_full.wait(guard) {
            Ok(g) => g,
            Err(e) => {
                let g = e.into_inner();
                self.lock_content.wait_lock.clear_poison();
                g
            }
        }
    }

    /// 在 `not_empty` 上等待；关联 `wait_lock` 被毒化时取回 guard 并解毒。
    fn wait_not_empty<'a>(&self, guard: MutexGuard<'a, ()>) -> MutexGuard<'a, ()> {
        match self.lock_content.not_empty.wait(guard) {
            Ok(g) => g,
            Err(e) => {
                let g = e.into_inner();
                self.lock_content.wait_lock.clear_poison();
                g
            }
        }
    }
}

impl<T> ConcurrentRing<T> {
    ///
    /// # Panics
    /// cap == 0, panic: capacity must be non-zero
    pub fn new_max_capacity(cap: usize) -> Self {
        assert!(cap > 0, "capacity must be non-zero");
        let slots = (0..cap).map(|_| Slot::new()).collect();
        let lock_content = LockContent {
            write_lock: Mutex::new(()),
            read_gate: RwLock::new(()),
            wait_lock: Mutex::new(()),
            not_empty: Condvar::new(),
            not_full: Condvar::new(),
            read_waiters: AtomicUsize::new(0),
            write_waiters: AtomicUsize::new(0),
        };
        Self {
            slots,
            cap: AtomicUsize::new(cap),
            write_pos: AtomicUsize::new(0),
            read_pos: AtomicUsize::new(0),
            lock_content,
            max_cap: AtomicUsize::new(cap),
            is_max_cap: AtomicBool::new(true),
        }
    }

    // ---------- 内部辅助：写入 / 取出的“核心逻辑” ----------
    // 调用方必须持有 write_lock
    fn push_inner(&self, value: T) -> Result<(), T> {
        let w = self.write_pos.load(Ordering::SeqCst);
        let r = self.read_pos.load(Ordering::SeqCst);
        let cap = self.cap.load(Ordering::SeqCst);
        if w.wrapping_sub(r) >= cap {
            return Err(value);
        }
        // SAFETY: w 处槽位不在 [r, w) 内（未初始化），调用方持有 write_lock 独占写指针。
        unsafe { self.slots[w % cap].write(value) };
        self.write_pos.store(w.wrapping_add(1), Ordering::SeqCst);
        Ok(())
    }

    // 调用方必须持有 read_gate.write
    fn pop_inner(&self) -> Option<T> {
        let r = self.read_pos.load(Ordering::Relaxed);
        let w = self.write_pos.load(Ordering::Acquire);
        if r == w {
            return None;
        }
        let cap = self.cap.load(Ordering::Acquire);
        // SAFETY: r 处槽位属于 [r, w)（已初始化），调用方持有 read_gate 独占。
        let v = unsafe { self.slots[r % cap].take() };
        self.read_pos.store(r.wrapping_add(1), Ordering::Release);
        Some(v)
    }

    // ---------- 唤醒辅助 ----------
    // 注意：即使无等待者也必须获取 wait_lock，以建立 happens-before，
    // 否则“刚准备进入等待的线程”可能看不到刚才的写入。
    fn wake_readers(&self) {
        let _wl = self.lock_wait_lock();
        if self.lock_content.read_waiters.load(Ordering::SeqCst) > 0 {
            self.lock_content.not_empty.notify_all();
        }
    }

    fn wake_writers(&self) {
        let _wl = self.lock_wait_lock();
        if self.lock_content.write_waiters.load(Ordering::SeqCst) > 0 {
            self.lock_content.not_full.notify_all();
        }
    }

    // ---------- 非阻塞版本（语义不变，仅追加唤醒） ----------
    pub fn push(&self, value: T) -> ConcurrentRingResult<(), T> {
        let result = {
            let _g = self.lock_write_lock();
            self.push_inner(value)
        };
        if result.is_ok() {
            self.wake_readers();
        }
        match result {
            Ok(()) => Ok(()),
            Err(v) => Err(ConcurrentRingError::new(
                "Full",
                ConcurrentRingErrorKind::Full(v),
            )),
        }
    }

    pub fn pop(&self) -> Option<T> {
        let result = {
            let _rg = self.lock_read_gate_write();
            self.pop_inner()
        };
        if result.is_some() {
            self.wake_writers();
        }
        result
    }

    pub fn capacity(&self) -> usize {
        self.cap.load(Ordering::SeqCst)
    }

    pub fn max_capacity(&self) -> Option<usize> {
        if self.is_max_cap.load(Ordering::Acquire) {
            Some(self.max_cap.load(Ordering::Acquire))
        } else {
            None
        }
    }

    pub fn is_empty(&self) -> bool {
        self.read_pos.load(Ordering::SeqCst) == self.write_pos.load(Ordering::SeqCst)
    }

    pub fn is_full(&self) -> bool {
        self.is_max_cap.load(Ordering::SeqCst) && self.len() >= self.max_cap.load(Ordering::SeqCst)
    }

    pub fn len(&self) -> usize {
        self.write_pos
            .load(Ordering::SeqCst)
            .wrapping_sub(self.read_pos.load(Ordering::SeqCst))
    }

    // ======================================================================
    //  ★ 新增：阻塞版本 push_wait / pop_wait
    //  设计要点：
    //    1. 检查/等待一律在 wait_lock 下进行；
    //    2. 实际读写锁（write_lock / read_gate）只短暂持有，且都放在
    //       wait_lock 内部获取并立刻释放，顺序为 wait_lock → 数据锁；
    //    3. 唤醒动作也在 wait_lock 内完成（本线程已持有，直接 notify）。
    // ======================================================================
    pub fn push_wait(&self, value: T) {
        let mut value = value;
        let mut guard = self.lock_wait_lock();
        loop {
            // 先尝试写入（短暂获取 write_lock）
            let result = {
                let _wg = self.lock_write_lock();
                self.push_inner(value)
            };
            match result {
                Ok(()) => {
                    // 已经在 wait_lock 下，直接唤醒读者，无需再次加锁
                    if self.lock_content.read_waiters.load(Ordering::SeqCst) > 0 {
                        self.lock_content.not_empty.notify_all();
                    }
                    return;
                }
                Err(v) => {
                    // 缓冲区满，继续等待
                    value = v;
                    self.lock_content
                        .write_waiters
                        .fetch_add(1, Ordering::SeqCst);
                    guard = self.wait_not_full(guard);
                    self.lock_content
                        .write_waiters
                        .fetch_sub(1, Ordering::SeqCst);
                }
            }
        }
    }

    pub fn pop_wait(&self) -> T {
        let mut guard = self.lock_wait_lock();
        loop {
            // 先尝试读出（短暂获取 read_gate.write）
            let result = {
                let _rg = self.lock_read_gate_write();
                self.pop_inner()
            };
            if let Some(v) = result {
                // 已经在 wait_lock 下，直接唤醒写者
                if self.lock_content.write_waiters.load(Ordering::SeqCst) > 0 {
                    self.lock_content.not_full.notify_all();
                }
                return v;
            }
            // 缓冲区空，继续等待
            self.lock_content
                .read_waiters
                .fetch_add(1, Ordering::SeqCst);
            guard = self.wait_not_empty(guard);
            self.lock_content
                .read_waiters
                .fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// 清空缓冲区：移出并丢弃 `[r, w)` 区间内的全部元素，使 `len() == 0`，
    /// 随后唤醒所有正在 `not_full` 上等待的写入者。
    ///
    /// **锁策略**：
    /// - 同时获取 `write_lock`（阻止新写入）和 `read_gate.write`（阻止读取），
    ///   保证「快照 r/w → 清空槽位 → 重置读指针」期间状态不被并发修改。
    /// - 加锁顺序固定为 `write_lock → read_gate.write`，与 `write()` / `read()`
    ///   的单一数据锁顺序兼容，不会与任何现有路径形成锁顺序反转。
    /// - **数据锁必须在调用 `wake_writers` 前释放**：`wake_writers` 内部会获取
    ///   `wait_lock`，而 `push_wait` / `write_wait` / `write_all` 持有 `wait_lock`
    ///   时可能正请求 `write_lock`；若在 `write_lock` 内调用 `wake_writers`，
    ///   将与这些路径形成锁顺序反转死锁。
    pub fn clear(&self) {
        // —— 阶段 1：双数据锁下清空 [r, w) ——
        {
            let _wg = self.lock_write_lock();
            let _rg = self.lock_read_gate_write();

            let r = self.read_pos.load(Ordering::Acquire);
            let w = self.write_pos.load(Ordering::Acquire);
            let cap = self.cap.load(Ordering::SeqCst);

            let mut i = r;
            while i != w {
                let idx = i % cap;
                // SAFETY: 已持有 write_lock 阻止任何新写入、read_gate.write 阻止任何读取，
                // 故本线程独占访问 slots；[r, w) 区间内槽位均已由写者初始化且未被取走，
                // take 后槽位逻辑上回到未初始化状态，符合 Slot::take 的安全前提。
                // take 返回的值立即丢弃（drop），完成资源释放。
                unsafe {
                    self.slots[idx].take();
                }
                i = i.wrapping_add(1);
            }

            // 读指针推进到写指针：len() = w - w = 0；写指针不动，
            // 不影响后续 push/write 的逻辑位置语义。
            self.read_pos.store(w, Ordering::Release);
        } // 依次释放 read_gate.write、write_lock（与获取顺序相反）

        // —— 阶段 2：数据锁已释放，唤醒等待的写入者 ——
        // 整个缓冲区被腾空，所有在 not_full 上等待的写者都可能立刻成功，
        // 因此使用 notify_all 而非 notify_one。
        self.wake_writers();
    }
}

impl<T: Copy> ConcurrentRing<T> {
    // 调用方必须已持有 wait_lock
    fn try_push_locked(&self, value: T) -> Result<(), T> {
        let _wg = self.lock_write_lock();
        self.push_inner(value)
    }

    // 调用方必须已持有 wait_lock
    fn try_read_locked(&self, out: &mut [T]) -> usize {
        let _rg = self.lock_read_gate_write();
        let r = self.read_pos.load(Ordering::Relaxed);
        let w = self.write_pos.load(Ordering::Acquire);
        let n = out.len().min(w.wrapping_sub(r));
        if n == 0 {
            return 0;
        }
        let cap = self.cap.load(Ordering::Acquire);
        let first_phys = r % cap;
        let first_len = n.min(cap - first_phys);
        for i in 0..first_len {
            // SAFETY: 第一段 [r, r+first_len) 均属已初始化区间。
            out[i] = unsafe { self.slots[first_phys + i].copy_out() };
        }
        for i in first_len..n {
            // SAFETY: 第二段物理下标从 0 开始，逻辑上仍属 [r, r+n) 已初始化区间。
            out[i] = unsafe { self.slots[i - first_len].copy_out() };
        }
        self.read_pos.store(r.wrapping_add(n), Ordering::Release);
        n
    }

    // 调用方必须持有 write_lock
    fn write_inner(&self, values: &[T]) -> usize {
        let w = self.write_pos.load(Ordering::Relaxed);
        let r = self.read_pos.load(Ordering::Acquire);
        let used = w.wrapping_sub(r);
        let cap = self.cap.load(Ordering::SeqCst);
        let space = cap - used;
        let n = values.len().min(space);
        if n == 0 {
            return 0;
        }

        let first_phys = w % cap;
        let first_len = n.min(cap - first_phys);

        for i in 0..first_len {
            // SAFETY: 槽位 (w+i)%cap 落在当前已用区间 [r, w) 之外，处于未初始化状态；
            // 调用方持有 write_lock 独占写指针，故可安全写入。
            unsafe { self.slots[first_phys + i].write(values[i]) };
        }
        for i in first_len..n {
            // SAFETY: 同上；第二段物理下标从 0 开始，逻辑上仍属 [w, w+n)。
            unsafe { self.slots[i - first_len].write(values[i]) };
        }

        self.write_pos.store(w.wrapping_add(n), Ordering::Release);
        n
    }

    // 调用方必须持有 read_gate.write
    fn read_inner(&self, out: &mut [T]) -> usize {
        let r = self.read_pos.load(Ordering::Acquire);
        let w = self.write_pos.load(Ordering::Acquire);
        let n = out.len().min(w.wrapping_sub(r));
        if n == 0 {
            return 0;
        }

        let cap = self.cap.load(Ordering::SeqCst);
        let first_phys = r % cap;
        let first_len = n.min(cap - first_phys);

        for i in 0..first_len {
            // SAFETY: 槽位 (r+i)%cap 属于已初始化区间 [r, w)；持有 read_gate 独占读，
            // 且 T: Copy 保证 copy_out 只做按位复制、不动源槽位。
            out[i] = unsafe { self.slots[first_phys + i].copy_out() };
        }
        for i in first_len..n {
            // SAFETY: 同上；第二段物理下标从 0 开始。
            out[i] = unsafe { self.slots[i - first_len].copy_out() };
        }

        self.read_pos.store(r.wrapping_add(n), Ordering::Release);
        n
    }
}

impl<T: Copy> ConcurrentRing<T> {
    /// 消费读：拷贝至多 out.len() 个元素，推进读指针。
    pub fn read(&self, out: &mut [T]) -> usize {
        let n = {
            let _gate = self.lock_read_gate_write();
            self.read_inner(out)
        };
        if n > 0 {
            self.wake_writers();
        }
        n
    }

    /// 借读：拷贝至多 out.len() 个元素，不推进读指针。
    pub fn borrow(&self, out: &mut [T]) -> usize {
        let _gate = self.lock_read_gate_read();

        let r = self.read_pos.load(Ordering::Acquire);
        let w = self.write_pos.load(Ordering::Acquire);
        let n = out.len().min(w.wrapping_sub(r));
        if n == 0 {
            return 0;
        }

        let cap = self.cap.load(Ordering::SeqCst);
        let first_phys = r % cap;
        let first_len = n.min(cap - first_phys);

        for i in 0..first_len {
            // SAFETY: 同上，第二段物理下标从 0 开始。
            out[i] = unsafe { self.slots[first_phys + i].copy_out() };
        }
        for i in first_len..n {
            // SAFETY: 同上，第二段物理下标从 0 开始。
            out[i] = unsafe { self.slots[i - first_len].copy_out() };
        }

        n // 不推进，不需要唤醒写者
    }
}

impl<T: Copy> ConcurrentRing<T> {
    /// 批量写：从 values 复制至多 values.len() 个元素，推进写指针。
    pub fn write(&self, values: &[T]) -> usize {
        let n = {
            let _g = self.lock_write_lock();
            self.write_inner(values)
        };
        if n > 0 {
            self.wake_readers();
        }
        n
    }
}

impl<T: Copy> ConcurrentRing<T> {
    /// 阻塞批量写：至少等到有 1 个空位，然后尽可能多地写入 values，
    /// 返回本次实际写入数量（`>= 1`）。
    ///
    /// 语义要点：只保证「写一点」，不保证写完全部；需要「写完全部」请用 [`write_all`]。
    pub fn write_wait(&self, values: &[T]) -> usize {
        if values.is_empty() {
            return 0;
        }
        // 进入等待-检查循环：整个循环期间持有 wait_lock，
        // 保证「检查空间 → wait」的原子性，避免丢失唤醒。
        let mut guard = self.lock_wait_lock();
        loop {
            // 短暂获取 write_lock 完成实际写入（持锁顺序：wait_lock → write_lock）
            let n = {
                let _wg = self.lock_write_lock();
                self.write_inner(values)
            };

            if n > 0 {
                // 已持有 wait_lock，不能调用 wake_readers（会二次获取 wait_lock 死锁），
                // 因此在此内联通知读者。
                if self.lock_content.read_waiters.load(Ordering::SeqCst) > 0 {
                    self.lock_content.not_empty.notify_all();
                }
                return n;
            }

            // 缓冲区为空位 0，进入 not_full 等待。
            // 先记账再 wait，避免「增加计数后、真正 wait 前」丢失唤醒。
            self.lock_content
                .write_waiters
                .fetch_add(1, Ordering::SeqCst);
            guard = self.wait_not_full(guard);
            self.lock_content
                .write_waiters
                .fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// 阻塞批量写：确保 `values` 全部写入。
    ///
    /// * 若 `values.len() > capacity`：返回 kind 为 `TooLarge` 的错误（**不携带数据**）。
    ///   这是**参数非法**，与 `Full(T)`（缓冲区当前满、可等待）语义不同，
    ///   调用方不应重试，应改用 `write_wait` 分批写入。
    /// * 否则：循环「尝试写 → 满则 wait」直到全部写完。
    pub fn write_all(&self, values: &[T]) -> ConcurrentRingResult<(), T> {
        if values.is_empty() {
            return Ok(());
        }

        // 尺寸检查：超过容量则不可能一次调用完成，直接报错（不进入 wait 循环）。
        let cap = self.cap.load(Ordering::SeqCst);
        if values.len() > cap {
            return Err(ConcurrentRingError::new(
                "write_all: values length exceeds capacity",
                // 使用新增的 TooLarge 变体——不携带数据，与 Full(T) 在 match 上区分。
                ConcurrentRingErrorKind::TooLarge,
            ));
        }

        let mut guard = self.lock_wait_lock();
        let mut written = 0usize;
        loop {
            // 持锁顺序：wait_lock → write_lock
            let n = {
                let _wg = self.lock_write_lock();
                self.write_inner(&values[written..])
            };

            if n > 0 {
                written += n;
                // 已持有 wait_lock，内联唤醒读者（勿调用 wake_readers，否则二次加锁死锁）。
                if self.lock_content.read_waiters.load(Ordering::SeqCst) > 0 {
                    self.lock_content.not_empty.notify_all();
                }
            }

            if written == values.len() {
                return Ok(());
            }

            // 还有剩余没写完，等待读者腾出空间。
            self.lock_content
                .write_waiters
                .fetch_add(1, Ordering::SeqCst);
            guard = self.wait_not_full(guard);
            self.lock_content
                .write_waiters
                .fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// 阻塞批量读：至少等到有 1 个元素，然后尽可能多地读出（至多 out.len()），
    /// 返回本次实际读取数量（`>= 1`）。
    pub fn read_wait(&self, out: &mut [T]) -> usize {
        if out.is_empty() {
            return 0;
        }
        let mut guard = self.lock_wait_lock();
        loop {
            // 持锁顺序：wait_lock → read_gate.write
            let n = {
                let _rg = self.lock_read_gate_write();
                self.read_inner(out)
            };

            if n > 0 {
                // 已持有 wait_lock，内联唤醒写者（勿调用 wake_writers）。
                if self.lock_content.write_waiters.load(Ordering::SeqCst) > 0 {
                    self.lock_content.not_full.notify_all();
                }
                return n;
            }

            // 缓冲区无数据，进入 not_empty 等待。
            self.lock_content
                .read_waiters
                .fetch_add(1, Ordering::SeqCst);
            guard = self.wait_not_empty(guard);
            self.lock_content
                .read_waiters
                .fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// 阻塞批量读：确保 `out` 被填满（即读完 `out.len()` 个元素）。
    ///
    /// * 若 `out.len() > capacity`：返回 kind 为 `TooLarge` 的错误（**不携带数据**）。
    ///   这是**参数非法**，与「缓冲区当前空、可等待」不同，调用方不应重试，
    ///   应改用 `read_wait` 分批读取。
    /// * 否则：循环「尝试读 → 空则 wait」直到 `out` 填满。
    pub fn read_exact(&self, out: &mut [T]) -> ConcurrentRingResult<(), T> {
        if out.is_empty() {
            return Ok(());
        }

        // 尺寸检查：单次 read_exact 请求的元素数超过容量上限，判定为参数错误。
        let cap = self.cap.load(Ordering::SeqCst);
        if out.len() > cap {
            return Err(ConcurrentRingError::new(
                "read_exact: out length exceeds capacity",
                ConcurrentRingErrorKind::TooLarge,
            ));
        }

        let mut guard = self.lock_wait_lock();
        let mut filled = 0usize;
        loop {
            // 持锁顺序：wait_lock → read_gate.write
            let n = {
                let _rg = self.lock_read_gate_write();
                self.read_inner(&mut out[filled..])
            };

            if n > 0 {
                filled += n;
                // 已持有 wait_lock，内联唤醒写者。
                if self.lock_content.write_waiters.load(Ordering::SeqCst) > 0 {
                    self.lock_content.not_full.notify_all();
                }
            }

            if filled == out.len() {
                return Ok(());
            }

            // 还没读满，等待写者填充更多数据。
            self.lock_content
                .read_waiters
                .fetch_add(1, Ordering::SeqCst);
            guard = self.wait_not_empty(guard);
            self.lock_content
                .read_waiters
                .fetch_sub(1, Ordering::SeqCst);
        }
    }
}

// ---------- 错误类型（原样保留） ----------
type ConcurrentRingResult<R, T> = Result<R, ConcurrentRingError<T>>;
#[derive(Clone)]
pub struct ConcurrentRingError<T> {
    message: String,
    kind: ConcurrentRingErrorKind<T>,
}

impl<T> ConcurrentRingError<T> {
    fn new(message: &str, kind: ConcurrentRingErrorKind<T>) -> Self {
        Self {
            message: message.to_string(),
            kind,
        }
    }
}

impl<T> Debug for ConcurrentRingError<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.kind {
            ConcurrentRingErrorKind::Full(_) => "Full",
            ConcurrentRingErrorKind::Locked(_) => "Locked",
            ConcurrentRingErrorKind::TooLarge => "TooLarge",
        };
        write!(
            f,
            "ConcurrentRingError {{ kind: {:?}, message: {} }}",
            kind, self.message
        )
    }
}

impl<T> Display for ConcurrentRingError<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "ConcurrentRingError: {}", self.message)
    }
}

impl<T> std::error::Error for ConcurrentRingError<T> {}

#[derive(Clone)]
pub enum ConcurrentRingErrorKind<T> {
    Full(T),
    Locked(Option<T>),
    /// 单次批量操作的请求量超过环形缓冲区容量上限。
    ///
    /// 与 `Full(T)` 语义不同：这是**参数非法**（数量本身就超出上限），
    /// 任何时刻都不可能在一次调用内完成，调用方不应重试，而应改用
    /// `write_wait` / `read_wait` 分批处理。
    ///
    /// **不携带任何数据**：错误原因只与「数量」有关，与具体元素无关；
    /// 输入 slice 仍在调用方手中，无需回传。
    TooLarge,
}

impl<T> ConcurrentRing<T> {
    /// 进入全锁模式：同时持有 `write_lock` 与 `read_gate.write`。
    ///
    /// Guard 存活期间，任何 `push/pop/write/read/borrow/*_wait/*` 及
    /// `clear` 都会被阻塞（因为它们需要同两把锁之一）；Guard drop 后恢复。
    ///
    /// 加锁顺序固定为 `write_lock → read_gate.write`，与 `clear` 一致，
    /// 不会与任何现有路径形成顺序反转。
    pub fn lock(&self) -> ConcurrentRingGuard<'_, T> {
        let write_guard = self.lock_write_lock();
        let read_guard = self.lock_read_gate_write();
        ConcurrentRingGuard {
            ring: self,
            _read_guard: read_guard,
            _write_guard: write_guard,
        }
    }
}

/// 全锁模式的 RAII Guard，提供 Vec / VecDeque 风格的读写接口。
///
/// **字段声明顺序**：`_read_guard` 在前，`_write_guard` 在后。
/// Rust 按声明顺序 drop 字段，故 `_read_guard` 先释放、`_write_guard` 后释放，
/// 与获取顺序（write_lock → read_gate.write）相反 —— 正确的解锁顺序。
pub struct ConcurrentRingGuard<'a, T> {
    ring: &'a ConcurrentRing<T>,
    _read_guard: RwLockWriteGuard<'a, ()>,
    _write_guard: MutexGuard<'a, ()>,
}
impl<'a, T> ConcurrentRingGuard<'a, T> {
    // ---------- 基本尺寸 ----------
    pub fn len(&self) -> usize {
        let r = self.ring.read_pos.load(Ordering::Relaxed);
        let w = self.ring.write_pos.load(Ordering::Relaxed);
        w.wrapping_sub(r)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn is_full(&self) -> bool {
        self.len() >= self.capacity()
    }
    pub fn capacity(&self) -> usize {
        self.ring.cap.load(Ordering::Relaxed)
    }

    // ---------- 索引访问 ----------
    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len() {
            return None;
        }
        let r = self.ring.read_pos.load(Ordering::Acquire);
        let cap = self.capacity();
        let phys = (r.wrapping_add(index)) % cap;
        // SAFETY: 持有排他锁（write_lock + read_gate.write），无人能改写槽位；
        // index < len 保证逻辑位置在 [r, w) 内、槽位已初始化。
        unsafe { Some(self.ring.slots[phys].get_ref()) }
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index >= self.len() {
            return None;
        }
        let r = self.ring.read_pos.load(Ordering::Acquire);
        let cap = self.capacity();
        let phys = r.wrapping_add(index) % cap;

        // SAFETY:
        // - 槽位位于 [r, w) 内，已初始化（写者写入、未被取走）；
        // - Guard 持有排他锁（write_lock + read_gate.write），无并发写者/读者；
        // - `&mut self` 保证本函数调用独占 Guard，返回的 `&mut T` 生命周期绑定
        //   到 `&mut self`，不与其他引用重叠。
        unsafe { Some(&mut *self.ring.slots[phys].get_mut_ptr()) }
    }

    pub fn front(&self) -> Option<&T> {
        self.get(0)
    }
    pub fn front_mut(&mut self) -> Option<&mut T> {
        self.get_mut(0)
    }

    pub fn back(&self) -> Option<&T> {
        let len = self.len();
        if len == 0 { None } else { self.get(len - 1) }
    }
    pub fn back_mut(&mut self) -> Option<&mut T> {
        let len = self.len();
        if len == 0 {
            None
        } else {
            self.get_mut(len - 1)
        }
    }

    // ---------- Vec / VecDeque 风格修改 ----------
    pub fn push_back(&mut self, value: T) -> Result<(), T> {
        // 复用内部 push_inner：Guard 持有 write_lock，符合其前置条件。
        self.ring.push_inner(value)
    }

    pub fn push_front(&mut self, value: T) -> Result<(), T> {
        let cap = self.capacity();
        let w = self.ring.write_pos.load(Ordering::Relaxed);
        let r = self.ring.read_pos.load(Ordering::Relaxed);
        if w.wrapping_sub(r) >= cap {
            return Err(value);
        }
        let new_r = r.wrapping_sub(1);
        let phys = new_r % cap;
        // SAFETY: len < cap 说明 (r-1)%cap 位置不在 [r, w) 内，即该槽位
        // 当前未初始化；Guard 持有排他锁，独占访问。
        unsafe {
            self.ring.slots[phys].write(value);
        }
        self.ring.read_pos.store(new_r, Ordering::Release);
        Ok(())
    }

    pub fn pop_front(&mut self) -> Option<T> {
        // 复用内部 pop_inner：Guard 持有 read_gate.write，符合其前置条件。
        self.ring.pop_inner()
    }

    pub fn pop_back(&mut self) -> Option<T> {
        let w = self.ring.write_pos.load(Ordering::Relaxed);
        let r = self.ring.read_pos.load(Ordering::Relaxed);
        if r == w {
            return None;
        }
        let cap = self.capacity();
        let new_w = w.wrapping_sub(1);
        let phys = new_w % cap;
        // SAFETY: (w-1)%cap 位于 [r, w) 内，已初始化；Guard 独占。
        let v = unsafe { self.ring.slots[phys].take() };
        self.ring.write_pos.store(new_w, Ordering::Release);
        Some(v)
    }

    pub fn clear(&mut self) {
        // 逐个 pop_front，每个 take 出的 T 立即 drop。
        while self.pop_front().is_some() {}
    }

    // ---------- 切片访问 ----------
    pub fn as_slices(&self) -> (&[T], &[T]) {
        let r = self.ring.read_pos.load(Ordering::Acquire);
        let w = self.ring.write_pos.load(Ordering::Acquire);
        let cap = self.capacity();
        let len = w.wrapping_sub(r);
        if len == 0 {
            return (&[], &[]);
        }
        let r_phys = r % cap;
        let first_len = (cap - r_phys).min(len);
        let second_len = len - first_len;
        // SAFETY:
        // - `Slot<T> = UnsafeCell<MaybeUninit<T>>`；`UnsafeCell` 与 `MaybeUninit`
        //   都是 `#[repr(transparent)]`，`MaybeUninit<T>` 与 `T` 布局一致，
        //   因此 `*const Slot<T>` 与 `*const T` 指向相同大小/对齐的内存。
        // - [r, w) 内槽位全部已初始化（push/write 写入、未 pop），切片内元素合法。
        // - Guard 持有排他锁，切片在借用期内不会与任何写者竞争。
        unsafe {
            let base = self.ring.slots.as_ptr() as *const T;
            let first = std::slice::from_raw_parts(base.add(r_phys), first_len);
            let second = std::slice::from_raw_parts(base, second_len);
            (first, second)
        }
    }

    pub fn as_mut_slices(&mut self) -> (&mut [T], &mut [T]) {
        let r = self.ring.read_pos.load(Ordering::Acquire);
        let w = self.ring.write_pos.load(Ordering::Acquire);
        let cap = self.capacity();
        let len = w.wrapping_sub(r);
        if len == 0 {
            return (&mut [], &mut []);
        }
        let r_phys = r % cap;
        let first_len = (cap - r_phys).min(len);
        let second_len = len - first_len;
        // SAFETY: 同上，且 `&mut self` 保证本轮借用独占两个切片，
        // 两个切片在物理上不重叠（first 在 [r_phys, cap)，second 在 [0, second_len)）。
        unsafe {
            let base = self.ring.slots.as_ptr() as *mut T;
            let first = std::slice::from_raw_parts_mut(base.add(r_phys), first_len);
            let second = std::slice::from_raw_parts_mut(base, second_len);
            (first, second)
        }
    }

    // ---------- 迭代器 ----------
    pub fn iter(&self) -> Iter<'_, T> {
        let r = self.ring.read_pos.load(Ordering::Acquire);
        let w = self.ring.write_pos.load(Ordering::Acquire);
        Iter {
            ring: self.ring,
            pos: r,
            remaining: w.wrapping_sub(r),
        }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        let r = self.ring.read_pos.load(Ordering::Acquire);
        let w = self.ring.write_pos.load(Ordering::Acquire);
        IterMut {
            ring: self.ring,
            pos: r,
            remaining: w.wrapping_sub(r),
        }
    }
}

impl<'a, T> Index<usize> for ConcurrentRingGuard<'a, T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        self.get(index)
            .expect("ConcurrentRingGuard index out of bounds")
    }
}

impl<'a, T> IndexMut<usize> for ConcurrentRingGuard<'a, T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        self.get_mut(index)
            .expect("ConcurrentRingGuard index out of bounds")
    }
}

/// 只读迭代器：从逻辑起点 r 前进 `len` 步。
pub struct Iter<'a, T> {
    ring: &'a ConcurrentRing<T>,
    pos: usize,
    remaining: usize,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        if self.remaining == 0 {
            return None;
        }
        let cap = self.ring.cap.load(Ordering::Relaxed);
        let phys = self.pos % cap;
        self.pos = self.pos.wrapping_add(1);
        self.remaining -= 1;
        // SAFETY: 迭代区间 [r, r+len) 全部已初始化；Guard 持有排他锁保证稳定。
        unsafe { Some(self.ring.slots[phys].get_ref()) }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl<'a, T> ExactSizeIterator for Iter<'a, T> {}

/// 可变迭代器：每个 next 返回不同物理槽位的 &mut T，因此无别名。
pub struct IterMut<'a, T> {
    ring: &'a ConcurrentRing<T>,
    pos: usize,
    remaining: usize,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;
    fn next(&mut self) -> Option<&'a mut T> {
        if self.remaining == 0 {
            return None;
        }
        let cap = self.ring.cap.load(Ordering::Relaxed);
        let phys = self.pos % cap;
        self.pos = self.pos.wrapping_add(1);
        self.remaining -= 1;

        // SAFETY:
        // - 槽位位于迭代区间 [r, r+len) 内，已初始化；
        // - `IterMut` 由 `&mut ConcurrentRingGuard` 创建，故本轮只有一个
        //   `IterMut`，整体访问独占；
        // - 每次 `next` 的物理下标 `phys` 互不相同（pos 单调递增且最多走 len 步），
        //   所以返回的 `&mut T` 互不重叠。
        // 生命周期：返回类型 `&'a mut T` 绑定到 `self.ring: &'a ConcurrentRing<T>`，
        // `&mut *raw` 被赋予 `'a`，由调用方保证在此期间无其他 `&mut`。
        unsafe { Some(&mut *self.ring.slots[phys].get_mut_ptr()) }
    }
}
impl<T> ExactSizeIterator for IterMut<'_, T> {}

// 让 Guard 能直接用于 `for x in &guard` / `for x in &mut guard`
impl<'b, T> IntoIterator for &'b ConcurrentRingGuard<'_, T> {
    type Item = &'b T;
    type IntoIter = Iter<'b, T>;
    fn into_iter(self) -> Iter<'b, T> {
        self.iter()
    }
}

impl<'b, T> IntoIterator for &'b mut ConcurrentRingGuard<'_, T> {
    type Item = &'b mut T;
    type IntoIter = IterMut<'b, T>;
    fn into_iter(self) -> IterMut<'b, T> {
        self.iter_mut()
    }
}
