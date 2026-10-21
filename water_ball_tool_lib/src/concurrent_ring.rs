#[cfg(test)]
pub mod test;

use std::cell::UnsafeCell;
use std::fmt::{Debug, Display, Formatter};
use std::mem::MaybeUninit;
use std::ops::{Index, IndexMut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{
    Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError,
};

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
    slots: UnsafeCell<Box<[Slot<T>]>>,
    cap: AtomicUsize,
    write_pos: AtomicUsize,
    read_pos: AtomicUsize,
    lock_content: LockContent,
    max_cap: AtomicUsize,
    is_max_cap: AtomicBool,
}

#[derive(Default)]
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

impl LockContent {
    fn new() -> Self {
        Self::default()
    }
}

impl<T> Drop for ConcurrentRing<T> {
    fn drop(&mut self) {
        let r = *self.read_pos.get_mut();
        let w = *self.write_pos.get_mut();
        let cap = *self.cap.get_mut();
        // SAFETY: `&mut self` 表明本线程对 ring 独占访问，无其他线程可读写
        // `slots`；`UnsafeCell::get()` 返回指向内部 `Box<[Slot<T>]>` 的裸指针，
        // 在此独占上下文中解引用为 `&[Slot<T>]` 是安全的。
        let slots = unsafe { &*self.slots.get() };
        let mut i = r;
        while i != w {
            let idx = i % cap;
            // SAFETY: &mut self 独占；[r, w) 内槽位已初始化，take 后立即 drop。
            unsafe {
                slots[idx].take();
            }
            i = i.wrapping_add(1);
        }
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
    /// 不定长模式：初始 cap = 0，首次写入时扩容。
    pub fn new() -> Self {
        Self {
            slots: UnsafeCell::new(Vec::new().into_boxed_slice()),
            cap: AtomicUsize::new(0),
            write_pos: AtomicUsize::new(0),
            read_pos: AtomicUsize::new(0),
            lock_content: LockContent::default(),
            max_cap: AtomicUsize::new(0),
            is_max_cap: AtomicBool::new(false),
        }
    }

    ///
    /// # Error
    /// cap == 0, panic: capacity must be non-zero
    pub fn new_max_capacity(cap: usize) -> ConcurrentRingResult<Self, T> {
        if cap == 0 {
            return Err(ConcurrentRingError::new(
                "capacity must be non-zero",
                ConcurrentRingErrorKind::MaxCapacityZero,
            ));
        }
        Ok(Self {
            slots: UnsafeCell::new((0..cap).map(|_| Slot::new()).collect()),
            cap: AtomicUsize::new(cap),
            write_pos: AtomicUsize::new(0),
            read_pos: AtomicUsize::new(0),
            lock_content: LockContent::new(),
            max_cap: AtomicUsize::new(cap),
            is_max_cap: AtomicBool::new(true),
        })
    }
}

impl<T> Default for ConcurrentRing<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------- 锁 helper ----------

impl<T> ConcurrentRing<T> {
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

    // try 版本：None 表示锁被占用，调用方转 Locked
    fn try_lock_write_lock(&self) -> Option<MutexGuard<'_, ()>> {
        match self.lock_content.write_lock.try_lock() {
            Ok(g) => Some(g),
            Err(TryLockError::Poisoned(e)) => {
                let g = e.into_inner();
                self.lock_content.write_lock.clear_poison();
                Some(g)
            }
            Err(TryLockError::WouldBlock) => None,
        }
    }
    fn try_lock_read_gate_write(&self) -> Option<RwLockWriteGuard<'_, ()>> {
        match self.lock_content.read_gate.try_write() {
            Ok(g) => Some(g),
            Err(TryLockError::Poisoned(e)) => {
                let g = e.into_inner();
                self.lock_content.read_gate.clear_poison();
                Some(g)
            }
            Err(TryLockError::WouldBlock) => None,
        }
    }
}

// ---------- 内部辅助 ----------

impl<T> ConcurrentRing<T> {
    #[inline]
    fn len_relaxed(&self) -> usize {
        self.write_pos
            .load(Ordering::Relaxed)
            .wrapping_sub(self.read_pos.load(Ordering::Relaxed))
    }

    #[inline]
    fn effective_cap(&self) -> usize {
        let cap = self.cap.load(Ordering::Acquire);
        if self.is_max_cap.load(Ordering::Acquire) {
            let max_cap = self.max_cap.load(Ordering::Acquire);
            if max_cap == 0 {
                // 观察到 (is_max_cap=true, max_cap=0)：disable/enable 的中间态，
                // 语义等价于「未设置上限」，返回 cap 避免 min(cap, 0)=0 误判满。
                cap
            } else {
                cap.min(max_cap)
            }
        } else {
            cap
        }
    }

    /// # Safety
    /// 调用方必须持有 `write_lock` 或 `read_gate` 之一，且在整个引用存续期间保持持有。
    #[inline]
    unsafe fn slots_ref(&self) -> &[Slot<T>] {
        // SAFETY: 由调用方持锁保证 slots 不会被替换。
        unsafe { &*self.slots.get() }
    }

    /// # Safety 前置条件
    /// - 调用方持 `write_lock` + `read_gate.write`
    /// - `new_cap > cap`（非 no-op）
    /// - `new_cap >= len`
    fn grow_locked(&self, new_cap: usize) {
        debug_assert!(new_cap > self.cap.load(Ordering::Acquire));
        let cap = self.cap.load(Ordering::Acquire);
        let r = self.read_pos.load(Ordering::Acquire);
        let w = self.write_pos.load(Ordering::Acquire);
        let len = w.wrapping_sub(r);
        debug_assert!(new_cap >= len);

        let mut new_slots: Vec<Slot<T>> = Vec::with_capacity(new_cap);
        for _ in 0..new_cap {
            new_slots.push(Slot::new());
        }
        // SAFETY: 调用方按前置条件持有 `write_lock` + `read_gate.write`，
        // 在本次函数调用期间无人能替换 `slots` 内部的 `Box<[Slot<T>]>`；
        // 解引用得到覆盖旧数组的 `&[Slot<T>]` 合法。
        let old = unsafe { self.slots_ref() };
        for i in 0..len {
            //for (i, <Item>) in in new_slots.iter().enumerate().take(len)
            let idx = r.wrapping_add(i) % cap;
            // SAFETY: idx 位于 [r, w) 内，槽位已初始化；持全锁独占。
            let item = unsafe { old[idx].take() };
            // SAFETY: new_slots[i] 未初始化；持全锁独占。
            unsafe {
                new_slots[i].write(item);
            }
        }
        // SAFETY: 持全锁，独占 slots；旧 Box drop 时 Slot 无 drop（MaybeUninit）。
        unsafe {
            *self.slots.get() = new_slots.into_boxed_slice();
        }
        self.cap.store(new_cap, Ordering::Release);
        self.read_pos.store(0, Ordering::Release);
        self.write_pos.store(len, Ordering::Release);
    }

    // 调用方须持 write_lock
    fn push_inner(&self, value: T) -> Result<(), T> {
        let w = self.write_pos.load(Ordering::Acquire);
        let r = self.read_pos.load(Ordering::Acquire);
        let cap = self.cap.load(Ordering::Acquire);
        if w.wrapping_sub(r) >= self.effective_cap() {
            return Err(value);
        }
        // SAFETY: w % cap 不在 [r, w) 内（未初始化）；持 write_lock 独占写指针。
        unsafe {
            self.slots_ref()[w % cap].write(value);
        }
        self.write_pos.store(w.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    // 调用方须持 read_gate.write
    fn pop_inner(&self) -> Option<T> {
        let r = self.read_pos.load(Ordering::Relaxed);
        let w = self.write_pos.load(Ordering::Acquire);
        if r == w {
            return None;
        }
        let cap = self.cap.load(Ordering::Acquire);
        // SAFETY: r % cap 属于 [r, w)，已初始化；持 read_gate.write 独占。
        let v = unsafe { self.slots_ref()[r % cap].take() };
        self.read_pos.store(r.wrapping_add(1), Ordering::Release);
        Some(v)
    }
}

// ---------- 尽量无锁路径 ----------

impl<T> ConcurrentRing<T> {
    pub fn push(&self, value: T) -> ConcurrentRingResult<(), T> {
        let fast = {
            let _g = self.lock_write_lock();
            self.push_inner(value)
        };
        match fast {
            Ok(()) => {
                self.wake_readers();
                Ok(())
            }
            Err(v) => self.push_slow(v),
        }
    }

    fn push_slow(&self, value: T) -> ConcurrentRingResult<(), T> {
        // 已满：若已启用 max_cap 则是硬上限 → Full
        if self.is_max_cap.load(Ordering::Acquire) {
            return Err(ConcurrentRingError::new(
                "Full",
                ConcurrentRingErrorKind::Full(value),
            ));
        }
        // 不定长模式：阻塞获取全锁扩容（与 clear / lock() 同序）
        let _wg = self.lock_write_lock();
        let _rg = self.lock_read_gate_write();
        let cap = self.cap.load(Ordering::Acquire);
        let len = self.len_relaxed();
        if len >= cap {
            let target = cap.saturating_mul(2).max(len + 1).max(1);
            self.grow_locked(target);
        }
        let r = self.push_inner(value);
        drop(_rg);
        drop(_wg);
        self.wake_writers();
        self.wake_readers();
        match r {
            Ok(()) => Ok(()),
            Err(v) => Err(ConcurrentRingError::new(
                "Full",
                ConcurrentRingErrorKind::Full(v),
            )),
        }
    }

    pub fn pop(&self) -> Option<T> {
        let r = {
            let _rg = self.lock_read_gate_write();
            self.pop_inner()
        };
        if r.is_some() {
            self.wake_writers();
        }
        r
    }
}

impl<T: Copy> ConcurrentRing<T> {
    fn write_inner(&self, values: &[T]) -> usize {
        let w = self.write_pos.load(Ordering::Relaxed);
        let r = self.read_pos.load(Ordering::Acquire);
        let cap = self.cap.load(Ordering::Acquire);
        let space = self.effective_cap().saturating_sub(w.wrapping_sub(r));
        let n = values.len().min(space);
        if n == 0 {
            return 0;
        }
        let first_phys = w % cap;
        let first_len = n.min(cap - first_phys);
        // SAFETY: 调用方按契约持有 `write_lock`；期间无人能替换 `slots` 的
        // `Box<[Slot<T>]>`（替换只在 `grow_locked` 中发生，其也需 `write_lock`）。
        let slots = unsafe { self.slots_ref() };
        for i in 0..first_len {
            // SAFETY: (w+i)%cap 在 [r, w) 之外，未初始化；持 write_lock 独占。
            unsafe {
                slots[first_phys + i].write(values[i]);
            }
        }
        for i in first_len..n {
            // SAFETY: 第二段物理下标从 0 开始。
            unsafe {
                slots[i - first_len].write(values[i]);
            }
        }
        self.write_pos.store(w.wrapping_add(n), Ordering::Release);
        n
    }

    fn read_inner(&self, out: &mut [T]) -> usize {
        let r = self.read_pos.load(Ordering::Acquire);
        let w = self.write_pos.load(Ordering::Acquire);
        let n = out.len().min(w.wrapping_sub(r));
        if n == 0 {
            return 0;
        }
        let cap = self.cap.load(Ordering::Acquire);
        let first_phys = r % cap;
        let first_len = n.min(cap - first_phys);
        // SAFETY: 调用方按契约持有 `read_gate.write`；期间无人能替换 `slots` 的
        // `Box<[Slot<T>]>`（替换只在 `grow_locked` 中发生，其也需 `read_gate.write`）。
        let slots = unsafe { self.slots_ref() };
        for i in 0..first_len {
            // SAFETY: (r+i)%cap 在 [r, w) 内已初始化；持 read_gate.write 独占。
            unsafe {
                out[i] = slots[first_phys + i].copy_out();
            }
        }
        for i in first_len..n {
            // SAFETY: 第二段物理下标从 0 开始。
            unsafe {
                out[i] = slots[i - first_len].copy_out();
            }
        }
        self.read_pos.store(r.wrapping_add(n), Ordering::Release);
        n
    }

    pub fn write(&self, values: &[T]) -> usize {
        let n = {
            let _g = self.lock_write_lock();
            self.write_inner(values)
        };
        if n > 0 {
            self.wake_readers();
        }
        if n < values.len() && !values.is_empty() && !self.is_max_cap.load(Ordering::Acquire) {
            return self.write_slow(values);
        }
        n
    }

    fn write_slow(&self, values: &[T]) -> usize {
        let _wg = self.lock_write_lock();
        let _rg = self.lock_read_gate_write();
        let cap = self.cap.load(Ordering::Acquire);
        let len = self.len_relaxed();
        if len >= cap {
            let target = cap.saturating_mul(2).max(len + values.len()).max(1);
            self.grow_locked(target);
        }
        let n = self.write_inner(values);
        drop(_rg);
        drop(_wg);
        self.wake_writers();
        self.wake_readers();
        n
    }

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

    pub fn borrow(&self, out: &mut [T]) -> usize {
        let _gate = self.lock_read_gate_read();
        let r = self.read_pos.load(Ordering::Acquire);
        let w = self.write_pos.load(Ordering::Acquire);
        let n = out.len().min(w.wrapping_sub(r));
        if n == 0 {
            return 0;
        }
        let cap = self.cap.load(Ordering::Acquire);
        let first_phys = r % cap;
        let first_len = n.min(cap - first_phys);
        // SAFETY: 本函数持有 `read_gate.read` 共享锁；`grow_locked` 需要
        // `read_gate.write`，与本共享锁互斥，故 `slots` 的 `Box<[Slot<T>]>`
        let slots = unsafe { self.slots_ref() };
        for i in 0..first_len {
            // SAFETY: [r, r+n) 内已初始化；持 read_gate.read 共享锁，copy_out 只读。
            unsafe {
                out[i] = slots[first_phys + i].copy_out();
            }
        }
        for i in first_len..n {
            // SAFETY: 第二段物理下标从 0 开始。
            unsafe {
                out[i] = slots[i - first_len].copy_out();
            }
        }
        n
    }
}

// ---------- 完全无锁路径 ----------

impl<T> ConcurrentRing<T> {
    /// 完全无锁：任何锁冲突都返回 `Locked`，不阻塞。
    pub fn try_push(&self, value: T) -> ConcurrentRingResult<(), T> {
        let Some(_g) = self.try_lock_write_lock() else {
            return Err(ConcurrentRingError::new(
                "Locked",
                ConcurrentRingErrorKind::Locked(Some(value)),
            ));
        };
        match self.push_inner(value) {
            Ok(()) => {
                drop(_g);
                self.wake_readers();
                Ok(())
            }
            Err(v) => {
                if self.is_max_cap.load(Ordering::Acquire) {
                    drop(_g);
                    return Err(ConcurrentRingError::new(
                        "Full",
                        ConcurrentRingErrorKind::Full(v),
                    ));
                }
                let Some(_rg) = self.try_lock_read_gate_write() else {
                    drop(_g);
                    return Err(ConcurrentRingError::new(
                        "Locked",
                        ConcurrentRingErrorKind::Locked(Some(v)),
                    ));
                };
                let cap = self.cap.load(Ordering::Acquire);
                let len = self.len_relaxed();
                if len >= cap {
                    let target = cap.saturating_mul(2).max(len + 1).max(1);
                    self.grow_locked(target);
                }
                let r = self.push_inner(v);
                drop(_rg);
                drop(_g);
                self.wake_writers();
                self.wake_readers();
                match r {
                    Ok(()) => Ok(()),
                    Err(v) => Err(ConcurrentRingError::new(
                        "Full",
                        ConcurrentRingErrorKind::Full(v),
                    )),
                }
            }
        }
    }

    /// 完全无锁单元素取出：冲突返回 `Locked(None)`。
    pub fn try_pop(&self) -> ConcurrentRingResult<Option<T>, T> {
        let v = {
            let Some(_rg) = self.try_lock_read_gate_write() else {
                return Err(ConcurrentRingError::new(
                    "Locked",
                    ConcurrentRingErrorKind::Locked(None),
                ));
            };
            self.pop_inner()
        };
        if v.is_some() {
            self.wake_writers();
        }
        Ok(v)
    }
}

impl<T: Copy> ConcurrentRing<T> {
    /// 完全无锁：任何锁冲突都返回 `Locked(None)`。
    pub fn try_write(&self, values: &[T]) -> ConcurrentRingResult<usize, T> {
        if values.is_empty() {
            return Ok(0);
        }
        let Some(_g) = self.try_lock_write_lock() else {
            return Err(ConcurrentRingError::new(
                "Locked",
                ConcurrentRingErrorKind::Locked(None),
            ));
        };
        let n = self.write_inner(values);
        if n > 0 {
            drop(_g);
            self.wake_readers();
            return Ok(n);
        }
        if self.is_max_cap.load(Ordering::Acquire) {
            drop(_g);
            return Err(ConcurrentRingError::new(
                "Full",
                ConcurrentRingErrorKind::Full(values[0]),
            ));
        }
        let Some(_rg) = self.try_lock_read_gate_write() else {
            drop(_g);
            return Err(ConcurrentRingError::new(
                "Locked",
                ConcurrentRingErrorKind::Locked(None),
            ));
        };
        let cap = self.cap.load(Ordering::Acquire);
        let len = self.len_relaxed();
        if len >= cap {
            let target = cap.saturating_mul(2).max(len + values.len()).max(1);
            self.grow_locked(target);
        }
        let n = self.write_inner(values);
        drop(_rg);
        drop(_g);
        self.wake_writers();
        self.wake_readers();
        if n > 0 {
            Ok(n)
        } else {
            Err(ConcurrentRingError::new(
                "Full",
                ConcurrentRingErrorKind::Full(values[0]),
            ))
        }
    }

    /// 完全无锁批量读：仅尝试获取 `read_gate.write`，冲突立即返回 `Locked(None)`。
    ///
    /// 语义与 [`read`] 一致（消耗数据、推进读指针、唤醒写者），但不阻塞：
    /// - 锁被占用（例如另一个 `read` / `pop` / `clear` 或 `borrow` 正在运行）
    ///   → `Locked(None)`，调用方可稍后重试。
    /// - 缓冲区空 → `Ok(0)`（与 `read` 一致，不视为错误）。
    /// - `out` 为空 → `Ok(0)`，完全不碰锁。
    ///
    /// 读不涉及扩容，因此只需 `read_gate.write` 一把锁，不触碰 `write_lock`。
    pub fn try_read(&self, out: &mut [T]) -> ConcurrentRingResult<usize, T> {
        if out.is_empty() {
            return Ok(0);
        }
        // 只在 read_gate.write 上 try_lock；冲突即 Locked(None)。
        // `out` 的数据仍属于调用方，锁失败时没有任何数据被消耗，故 `Locked` 不携带值。
        let n = {
            let Some(_rg) = self.try_lock_read_gate_write() else {
                return Err(ConcurrentRingError::new(
                    "Locked",
                    ConcurrentRingErrorKind::Locked(None),
                ));
            };
            self.read_inner(out)
        }; // ← read_gate.write 在此释放，之后再唤醒写者（锁顺序 wait_lock → read_gate.write）

        if n > 0 {
            self.wake_writers();
        }
        Ok(n)
    }
}

// ---------- 阻塞路径 ----------

impl<T> ConcurrentRing<T> {
    pub fn push_wait(&self, value: T) {
        let mut value = value;
        let mut guard = self.lock_wait_lock();
        loop {
            let result = {
                let _wg = self.lock_write_lock();
                self.push_inner(value)
            };
            match result {
                Ok(()) => {
                    if self.lock_content.read_waiters.load(Ordering::Acquire) > 0 {
                        self.lock_content.not_empty.notify_all();
                    }
                    return;
                }
                Err(v) => {
                    value = v;
                    if self.is_max_cap.load(Ordering::Acquire) {
                        // 固定模式：等待
                        self.lock_content
                            .write_waiters
                            .fetch_add(1, Ordering::Release);
                        guard = self.wait_not_full(guard);
                        self.lock_content
                            .write_waiters
                            .fetch_sub(1, Ordering::Release);
                    } else {
                        // 不定长模式：持 wait_lock 下全锁扩容
                        let _wg = self.lock_write_lock();
                        let _rg = self.lock_read_gate_write();
                        let cap = self.cap.load(Ordering::Acquire);
                        let len = self.len_relaxed();
                        if len >= cap {
                            let target = cap.saturating_mul(2).max(len + 1).max(1);
                            self.grow_locked(target);
                        }
                        drop(_rg);
                        drop(_wg);
                        if self.lock_content.write_waiters.load(Ordering::Acquire) > 0 {
                            self.lock_content.not_full.notify_all();
                        }
                    }
                }
            }
        }
    }

    pub fn pop_wait(&self) -> T {
        let mut guard = self.lock_wait_lock();
        loop {
            let result = {
                let _rg = self.lock_read_gate_write();
                self.pop_inner()
            };
            if let Some(v) = result {
                if self.lock_content.write_waiters.load(Ordering::Acquire) > 0 {
                    self.lock_content.not_full.notify_all();
                }
                return v;
            }
            self.lock_content
                .read_waiters
                .fetch_add(1, Ordering::Release);
            guard = self.wait_not_empty(guard);
            self.lock_content
                .read_waiters
                .fetch_sub(1, Ordering::Release);
        }
    }
}

impl<T: Copy> ConcurrentRing<T> {
    pub fn write_wait(&self, values: &[T]) -> usize {
        if values.is_empty() {
            return 0;
        }
        let mut guard = self.lock_wait_lock();
        loop {
            let n = {
                let _wg = self.lock_write_lock();
                self.write_inner(values)
            };
            if n > 0 {
                if self.lock_content.read_waiters.load(Ordering::Acquire) > 0 {
                    self.lock_content.not_empty.notify_all();
                }
                return n;
            }
            if self.is_max_cap.load(Ordering::Acquire) {
                self.lock_content
                    .write_waiters
                    .fetch_add(1, Ordering::Release);
                guard = self.wait_not_full(guard);
                self.lock_content
                    .write_waiters
                    .fetch_sub(1, Ordering::Release);
            } else {
                let _wg = self.lock_write_lock();
                let _rg = self.lock_read_gate_write();
                let cap = self.cap.load(Ordering::Acquire);
                let len = self.len_relaxed();
                if len >= cap {
                    let target = cap.saturating_mul(2).max(len + values.len()).max(1);
                    self.grow_locked(target);
                }
                drop(_rg);
                drop(_wg);
                if self.lock_content.write_waiters.load(Ordering::Acquire) > 0 {
                    self.lock_content.not_full.notify_all();
                }
            }
        }
    }

    pub fn write_all(&self, values: &[T]) -> ConcurrentRingResult<(), T> {
        if values.is_empty() {
            return Ok(());
        }
        // 固定模式下 `out.len() > max_cap` 会永久阻塞（无写者时无法填满），
        // 在获取 wait_lock 之前直接拒绝，避免测试/调用方挂起。
        if self.is_max_cap.load(Ordering::Acquire) {
            let max_cap = self.max_cap.load(Ordering::Acquire);
            if max_cap > 0 && values.len() > max_cap {
                return Err(ConcurrentRingError::new(
                    "read_exact: out length exceeds max_cap",
                    ConcurrentRingErrorKind::TooLarge,
                ));
            }
        }
        let mut guard = self.lock_wait_lock();
        let mut written = 0;
        loop {
            let n = {
                let _wg = self.lock_write_lock();
                self.write_inner(&values[written..])
            };
            if n > 0 {
                written += n;
                if self.lock_content.read_waiters.load(Ordering::Acquire) > 0 {
                    self.lock_content.not_empty.notify_all();
                }
            }
            if written == values.len() {
                return Ok(());
            }
            if self.is_max_cap.load(Ordering::Acquire) {
                self.lock_content
                    .write_waiters
                    .fetch_add(1, Ordering::Release);
                guard = self.wait_not_full(guard);
                self.lock_content
                    .write_waiters
                    .fetch_sub(1, Ordering::Release);
            } else {
                let _wg = self.lock_write_lock();
                let _rg = self.lock_read_gate_write();
                let cap = self.cap.load(Ordering::Acquire);
                let len = self.len_relaxed();
                if len >= cap {
                    let target = cap
                        .saturating_mul(2)
                        .max(len + (values.len() - written))
                        .max(1);
                    self.grow_locked(target);
                }
                drop(_rg);
                drop(_wg);
                if self.lock_content.write_waiters.load(Ordering::Acquire) > 0 {
                    self.lock_content.not_full.notify_all();
                }
            }
        }
    }

    pub fn read_wait(&self, out: &mut [T]) -> usize {
        if out.is_empty() {
            return 0;
        }
        let mut guard = self.lock_wait_lock();
        loop {
            let n = {
                let _rg = self.lock_read_gate_write();
                self.read_inner(out)
            };
            if n > 0 {
                if self.lock_content.write_waiters.load(Ordering::Acquire) > 0 {
                    self.lock_content.not_full.notify_all();
                }
                return n;
            }
            self.lock_content
                .read_waiters
                .fetch_add(1, Ordering::Release);
            guard = self.wait_not_empty(guard);
            self.lock_content
                .read_waiters
                .fetch_sub(1, Ordering::Release);
        }
    }

    /// 阻塞读满 `out`。
    ///
    /// # Error
    /// - 固定模式（`is_max_cap = true`）下，`out.len() > max_cap` 时返回 `TooLarge`：
    ///   物理 `cap` 上限就是 `max_cap`，ring 满时也无法一次填满 `out`，
    ///   剩余元素必须靠新数据持续流入才能补齐 —— 这属于参数误用。
    /// - 不定长模式（`is_max_cap = false`）不设检查：`cap` 会按需扩容，
    ///   写者持续写入即可填满。
    pub fn read_exact(&self, out: &mut [T]) -> ConcurrentRingResult<(), T> {
        if out.is_empty() {
            return Ok(());
        }

        // 固定模式下 `out.len() > max_cap` 会永久阻塞（无写者时无法填满），
        // 在获取 wait_lock 之前直接拒绝，避免测试/调用方挂起。
        if self.is_max_cap.load(Ordering::Acquire) {
            let max_cap = self.max_cap.load(Ordering::Acquire);
            if max_cap > 0 && out.len() > max_cap {
                return Err(ConcurrentRingError::new(
                    "read_exact: out length exceeds max_cap",
                    ConcurrentRingErrorKind::TooLarge,
                ));
            }
        }

        let mut guard = self.lock_wait_lock();
        let mut filled = 0;
        loop {
            let n = {
                let _rg = self.lock_read_gate_write();
                self.read_inner(&mut out[filled..])
            };
            if n > 0 {
                filled += n;
                if self.lock_content.write_waiters.load(Ordering::Acquire) > 0 {
                    self.lock_content.not_full.notify_all();
                }
            }
            if filled == out.len() {
                return Ok(());
            }
            self.lock_content
                .read_waiters
                .fetch_add(1, Ordering::Release);
            guard = self.wait_not_empty(guard);
            self.lock_content
                .read_waiters
                .fetch_sub(1, Ordering::Release);
        }
    }
}

// ---------- 显式扩缩容与模式开关 ----------

impl<T> ConcurrentRing<T> {
    /// 显式保证 `cap >= len + additional`（受 max_cap 约束）。
    pub fn reserve(&self, additional: usize) -> ConcurrentRingResult<(), T> {
        let _wg = self.lock_write_lock();
        let _rg = self.lock_read_gate_write();
        let len = self.len_relaxed();
        let target = len + additional;
        if self.is_max_cap.load(Ordering::Acquire) {
            let max_cap = self.max_cap.load(Ordering::Acquire);
            if target > max_cap {
                return Err(ConcurrentRingError::new(
                    "reserve: target exceeds max_cap",
                    ConcurrentRingErrorKind::TooLarge,
                ));
            }
        }
        let cap = self.cap.load(Ordering::Acquire);
        if target > cap {
            self.grow_locked(target);
        }
        drop(_rg);
        drop(_wg);
        self.wake_writers();
        Ok(())
    }

    /// 启用/更新硬上限。`new_max == 0` 报错。
    pub fn enable_max_capacity(&self, new_max: usize) -> ConcurrentRingResult<(), T> {
        if new_max == 0 {
            return Err(ConcurrentRingError::new(
                "enable_max_capacity: max_cap must be non-zero",
                ConcurrentRingErrorKind::TooLarge,
            ));
        }
        let cap = self.cap.load(Ordering::Acquire);
        if new_max > cap {
            // ① 扩容（全锁）
            let _wg = self.lock_write_lock();
            let _rg = self.lock_read_gate_write();
            let cap_now = self.cap.load(Ordering::Acquire);
            if new_max > cap_now {
                self.grow_locked(new_max);
            }
            self.max_cap.store(new_max, Ordering::Release);
            self.is_max_cap.store(true, Ordering::Release);
            drop(_rg);
            drop(_wg);
            self.wake_writers();
        } else {
            // ② / ③ 减小或相等：只写 max_cap（write_lock 保护）
            let _wg = self.lock_write_lock();
            let len = self.len_relaxed();
            if new_max < len {
                return Err(ConcurrentRingError::new(
                    "enable_max_capacity: max_cap < current len",
                    ConcurrentRingErrorKind::TooLarge,
                ));
            }
            self.max_cap.store(new_max, Ordering::Release);
            self.is_max_cap.store(true, Ordering::Release);
        }
        Ok(())
    }

    /// 关闭上限：不加锁，仅以 store 顺序保证读者不会看到不一致中间态。
    pub fn disable_max_capacity(&self) {
        // 先关标志，再置 0：读到 is_max_cap=false 的线程不使用 max_cap。
        self.is_max_cap.store(false, Ordering::Release);
        self.max_cap.store(0, Ordering::Release);
    }
    pub fn clear(&self) {
        {
            let _wg = self.lock_write_lock();
            let _rg = self.lock_read_gate_write();
            let r = self.read_pos.load(Ordering::Acquire);
            let w = self.write_pos.load(Ordering::Acquire);

            if std::mem::needs_drop::<T>() {
                let cap = self.cap.load(Ordering::Acquire);
                // SAFETY: 持全锁独占 slots；[r, w) 内已初始化，take 后立即 drop。
                let slots = unsafe { self.slots_ref() };
                let mut i = r;
                while i != w {
                    let idx = i % cap;
                    // SAFETY: idx 位于 [r, w) 内，槽位已初始化。
                    unsafe {
                        slots[idx].take();
                    }
                    i = i.wrapping_add(1);
                }
                self.read_pos.store(w, Ordering::Release);
            } else {
                // T 无 drop glue：无需逐槽 take，O(1)。
                // 槽位内的旧字节仍在，但逻辑上视为未初始化；
                // 后续 push/write 会用 MaybeUninit::write 覆盖（不读旧值）。
                self.read_pos.store(w, Ordering::Release);
            }
        }
        self.wake_writers();
    }
}

// ---------- 状态查询 ----------

impl<T> ConcurrentRing<T> {
    pub fn capacity(&self) -> usize {
        self.cap.load(Ordering::Acquire)
    }
    pub fn max_capacity(&self) -> Option<usize> {
        if self.is_max_cap.load(Ordering::Acquire) {
            let m = self.max_cap.load(Ordering::Acquire);
            if m == 0 { None } else { Some(m) }
        } else {
            None
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len_relaxed() == 0
    }
    pub fn is_full(&self) -> bool {
        self.len_relaxed() >= self.effective_cap()
    }
    pub fn len(&self) -> usize {
        self.len_relaxed()
    }
}

impl<T> ConcurrentRing<T> {
    // ---------- 唤醒辅助 ----------
    // 注意：即使无等待者也必须获取 wait_lock，以建立 happens-before，
    // 否则“刚准备进入等待的线程”可能看不到刚才的写入。
    fn wake_readers(&self) {
        let _wl = self.lock_wait_lock();
        if self.lock_content.read_waiters.load(Ordering::Acquire) > 0 {
            self.lock_content.not_empty.notify_all();
        }
    }

    fn wake_writers(&self) {
        let _wl = self.lock_wait_lock();
        if self.lock_content.write_waiters.load(Ordering::Acquire) > 0 {
            self.lock_content.not_full.notify_all();
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
            ConcurrentRingErrorKind::MaxCapacityZero => "MaxCapacityZero",
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
    MaxCapacityZero,
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
    // ---------- 内部辅助 ----------

    /// 获取 slots 切片。Guard 持有全锁，独占访问。
    #[inline]
    fn slots(&self) -> &[Slot<T>] {
        // SAFETY: Guard 持有 write_lock + read_gate.write，
        // 在 Guard 生命周期内无人能替换 slots（grow_locked 需全锁）。
        unsafe { &*self.ring.slots.get() }
    }

    /// 满时自动扩容（仅不定长模式生效），返回扩容后的「在满时 push 是否还需要扩容」。
    /// 已持全锁，可直接调用 `grow_locked`。
    fn grow_if_needed(&mut self, min_extra: usize) {
        // 固定模式不扩容。
        if self.ring.is_max_cap.load(Ordering::Acquire) {
            return;
        }
        let cap = self.ring.cap.load(Ordering::Acquire);
        let len = self.len();
        // 目标容量：至少能容纳当前元素 + 额外 min_extra。
        let target = cap.saturating_mul(2).max(len + min_extra).max(1);
        // 若目标大于物理容量，执行重分配。
        if target > cap {
            self.ring.grow_locked(target);
        }
    }

    // ---------- 基本尺寸 ----------

    pub fn len(&self) -> usize {
        let r = self.ring.read_pos.load(Ordering::Relaxed);
        let w = self.ring.write_pos.load(Ordering::Relaxed);
        w.wrapping_sub(r)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 是否「无法再写入」：固定模式 / 已启用 max_cap 时，`len >= effective_cap`；
    /// 不定长模式下 `effective_cap == cap`，语义为「物理已满、需要扩容」。
    pub fn is_full(&self) -> bool {
        self.len() >= self.ring.effective_cap()
    }

    /// 物理数组容量。
    pub fn capacity(&self) -> usize {
        self.ring.cap.load(Ordering::Relaxed)
    }

    /// 逻辑容量上限（0 = 未设置）。
    pub fn max_capacity(&self) -> usize {
        self.ring.max_cap.load(Ordering::Relaxed)
    }

    // ---------- 索引访问 ----------

    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len() {
            return None;
        }
        let r = self.ring.read_pos.load(Ordering::Acquire);
        let cap = self.capacity();
        let phys = r.wrapping_add(index) % cap;
        // SAFETY: 持有排他锁（write_lock + read_gate.write），无人能改写槽位；
        // index < len 保证逻辑位置在 [r, w) 内、槽位已初始化。
        unsafe { Some(self.slots()[phys].get_ref()) }
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
        // - Guard 持有排他锁，无并发写者/读者；
        // - `&mut self` 保证本函数调用独占 Guard，返回的 `&mut T` 生命周期绑定
        //   到 `&mut self`，不与其他引用重叠。
        unsafe { Some(&mut *self.slots()[phys].get_mut_ptr()) }
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

    /// 尾部插入。满时若处于不定长模式则自动扩容。
    pub fn push_back(&mut self, value: T) -> Result<(), T> {
        match self.ring.push_inner(value) {
            Ok(()) => Ok(()),
            Err(v) => {
                // 固定模式：真正满，返回 Err。
                if self.ring.is_max_cap.load(Ordering::Acquire) {
                    return Err(v);
                }
                // 不定长模式：扩容（Guard 已持全锁，可直接 grow_locked）。
                self.grow_if_needed(1);
                self.ring.push_inner(v)
            }
        }
    }

    /// 头部插入。满时若处于不定长模式则自动扩容。
    pub fn push_front(&mut self, value: T) -> Result<(), T> {
        let effective = self.ring.effective_cap();
        let w = self.ring.write_pos.load(Ordering::Relaxed);
        let r = self.ring.read_pos.load(Ordering::Relaxed);
        let len = w.wrapping_sub(r);

        if len >= effective {
            // 固定模式：真正满，返回 Err。
            if self.ring.is_max_cap.load(Ordering::Acquire) {
                return Err(value);
            }
            // 不定长模式：先扩容。grow_locked 后 r=0, w=len, cap=target。
            self.grow_if_needed(1);
            let cap = self.capacity();
            // 扩容后新元素的逻辑读位置为 0-1（回绕）。
            let new_r = 0usize.wrapping_sub(1);
            let phys = new_r % cap;
            // SAFETY: 新位置 `target-1` 未初始化（因为 len < target），Guard 持全锁独占。
            unsafe {
                self.slots()[phys].write(value);
            }
            self.ring.read_pos.store(new_r, Ordering::Release);
            return Ok(());
        }

        // 非满：正常 push_front。
        let cap = self.capacity();
        let new_r = r.wrapping_sub(1);
        let phys = new_r % cap;
        // SAFETY: len < cap 说明 (r-1)%cap 位置不在 [r, w) 内，即该槽位
        // 当前未初始化；Guard 持有排他锁，独占访问。
        unsafe {
            self.slots()[phys].write(value);
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
        let v = unsafe { self.slots()[phys].take() };
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
        // - Guard 持有排他锁，切片在借用期内不会与任何写者竞争，
        //   也不会被 grow_locked 替换（grow_locked 需全锁）。
        unsafe {
            let base = self.slots().as_ptr() as *const T;
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
            let base = self.slots().as_ptr() as *mut T;
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
        // SAFETY: 迭代区间 [r, r+len) 全部已初始化；`Iter<'a, T>` 由
        // `ConcurrentRingGuard<'_, T>` 的 `iter(&self)` 创建，其生命周期 'a 绑定到
        // Guard 借用，Guard 在 'a 内持有全锁，保证 `slots` 不被 `grow_locked` 替换。
        // `&*self.ring.slots.get()` 得到 `&'a Box<[Slot<T>]>`（由 `UnsafeCell` 与
        // 上述锁保证），auto-deref 到 `&'a [Slot<T>]`。
        let slots: &'a [Slot<T>] = unsafe { &*self.ring.slots.get() };
        // SAFETY: phys 位置属于迭代区间，已初始化。
        unsafe { Some(slots[phys].get_ref()) }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl<T> ExactSizeIterator for Iter<'_, T> {}

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
        //   `IterMut`，整体访问独占；Guard 在 'a 内持有全锁保证 slots 不被替换；
        // - 每次 `next` 的物理下标 `phys` 互不相同（pos 单调递增且最多走 len 步），
        //   所以返回的 `&mut T` 互不重叠。
        let slots: &'a [Slot<T>] = unsafe { &*self.ring.slots.get() };
        // SAFETY: `Iter<'a, T>` 只能由 `&'a ConcurrentRingGuard` 的 `iter()`
        // 构造，因此 Guard 的生命周期覆盖 `'a`，Guard 在 `'a` 内持有全锁，
        // 期间无人能替换 `slots`；`&*self.ring.slots.get()` 得到
        // `&'a Box<[Slot<T>]>`，自动 deref 为 `&'a [Slot<T>]` 合法。
        unsafe { Some(&mut *slots[phys].get_mut_ptr()) }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl<T> ExactSizeIterator for IterMut<'_, T> {}

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
