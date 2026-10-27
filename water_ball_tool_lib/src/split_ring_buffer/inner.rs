use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};

// ============================================================
// 锁中毒辅助函数
//
// `Mutex::lock()` / `Condvar::wait()` 返回 `LockResult<_>`。一旦持锁线程
// panic，锁会被标记为中毒，后续 `lock()` / `wait()` 返回 `Err(PoisonError)`。
//
// 本模块约定：中毒时不向外传播、不 panic；使用 `PoisonError::into_inner()`
// 取回内部守卫继续使用。理由：
// - 读/写锁只保护“指针推进 + 槽位写入”这一段，若 panics 时完成了一半，
//   指针与槽位可能不一致——但 panic 本身已导致调用方无法继续按约定使用
//   缓冲区，此处选择“尽力恢复”而非“再次 panic”。
// - `wait_mutex` 只用于阻塞/唤醒协调，数据正确性由原子指针与 `Acquire` /
//   `Release` 保证；中毒对其无害。
// ============================================================

#[inline]
pub(crate) fn lock_ignore_poison<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // 不使用 `unwrap()`：中毒时取回内部守卫继续使用。
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[inline]
pub(crate) fn wait_ignore_poison<'a, T>(c: &Condvar, g: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
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

pub(crate) struct Inner<T: Copy> {
    cells: Box<[UnsafeCell<MaybeUninit<T>>]>,
    cap: usize,
    pub(crate) read: Endpoint,
    pub(crate) write: Endpoint,
    pub(crate) wait_mutex: Mutex<()>,
}

// SAFETY: `Inner<T>` 手写 `Send`。
// - `cells` 中 `UnsafeCell` 抑制自动 `Sync` 推导，`MaybeUninit` 按 `T` 传播，
//   故须手工声明。
// - `T: Send` 足够：元素由写端写入、读端读出，跨线程只发生一次转移；同一
//   槽位同一时刻只被一方访问（读 `[r, w)`、写 `[w, w + free)`，不重叠）。
// - 同端多 clone 由 `Endpoint::lock` 串行化；`wait_mutex` 仅协调等待。
// - 读写指针 `Acquire` / `Release` 配对发布槽位内容。
unsafe impl<T: Copy + Send> Send for Inner<T> {}

// SAFETY: `Inner<T>` 手写 `Sync`。
// - 同上，`UnsafeCell` / `MaybeUninit` 抑制自动 `Sync`。
// - `T: Send` 足够（无需 `T: Sync`）：同一槽位同一时刻只被一方读或写，数据
//   是“移动/复制”而非双向共享读。
// - 同端锁串行化；读写端区间分离；等待由 `wait_mutex` 协调。
unsafe impl<T: Copy + Send> Sync for Inner<T> {}

impl<T: Copy> Inner<T> {
    pub(crate) fn with_capacity(cap: usize) -> Self {
        assert!(cap > 0, "ring buffer capacity must be > 0");
        let cells = (0..cap)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            cells,
            cap,
            read: Endpoint::new(),
            write: Endpoint::new(),
            wait_mutex: Mutex::new(()),
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

    pub(crate) fn read_one(&self) -> Option<T> {
        loop {
            let r = self.read.pos.load(Ordering::Acquire);
            let w = self.write.pos.load(Ordering::Acquire);
            let avail = w.wrapping_sub(r);
            if avail == 0 || avail > self.cap {
                return None;
            }
            let idx = r % self.cap;

            // SAFETY: 解引用 `*mut MaybeUninit<T>` 并 `assume_init()`。
            // - `avail > 0` 说明 `[r, w)` 非空，`idx = r % cap` 已初始化；
            //   该槽位由 `write_one` / `write_slice` 写入并经 `write.pos.store(Release)` 发布。
            // - `w = write.pos.load(Acquire)` 与写端 `Release` 配对，保证可见性。
            // - 调用方保证单读端（`read.lock` 或 `&mut self`）；与 `clear` 并发时
            //   下方 CAS 会失败重试，不会读到废弃数据。
            // - `T: Copy` ⇒ `MaybeUninit<T>: Copy`，`*ptr` 为复制而非 move，
            //   `assume_init()` 消耗副本后槽位保持未初始化状态，可被复用。
            let val = unsafe { (*self.cells[idx].get()).assume_init() };

            match self.read.pos.compare_exchange(
                r,
                r.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.notify_writer_after_read();
                    return Some(val);
                }
                Err(_) => continue,
            }
        }
    }

    pub(crate) fn write_one(&self, value: T) -> Result<(), T> {
        let r = self.read.pos.load(Ordering::Acquire);
        let w = self.write.pos.load(Ordering::Acquire);
        let used = w.wrapping_sub(r);
        if used >= self.cap || used > self.cap {
            return Err(value);
        }
        let idx = w % self.cap;

        // SAFETY: 解引用 `*mut MaybeUninit<T>` 并 `write`。
        // - `used < cap` 说明 `[w, w + free)` 未初始化，`idx = w % cap` 不在
        //   `[r, w)` 内；槽位空闲。
        // - 调用方保证单写端（`write.lock` 或 `&mut self`）；`clear` 只改
        //   `read.pos`，不推进 `write.pos`。
        // - 读端只读 `[r, w)`，不触及 `idx`。
        // - 写入后 `write.pos.store(Release)` 发布；读端 `Acquire` 观测到新 `w`
        //   后才可能读该槽位，形成 happens-before。
        unsafe {
            (*self.cells[idx].get()).write(value);
        }

        self.write.pos.store(w.wrapping_add(1), Ordering::Release);
        self.notify_reader_after_write();
        Ok(())
    }

    pub(crate) fn read_slice(&self, dst: &mut [T]) -> usize {
        if dst.is_empty() {
            return 0;
        }
        loop {
            let r = self.read.pos.load(Ordering::Acquire);
            let w = self.write.pos.load(Ordering::Acquire);
            let avail = w.wrapping_sub(r);
            if avail == 0 || avail > self.cap {
                return 0;
            }
            let n = dst.len().min(avail);
            let start = r % self.cap;
            let first = (self.cap - start).min(n);

            for i in 0..first {
                // SAFETY: 第一段，从 `start` 到环尾。
                // - `[r, r + n)` 全在 `[r, w)` 内（`n <= avail`），槽位已初始化；
                // - 调用方保证单读端；写端只写 `[w, w + free)`，不触及本区间；
                // - `Acquire` / `Release` 配对保证可见性；
                // - `T: Copy` 下 `assume_init()` 复制值，槽位保持未初始化语义。
                // - 与 `clear` 并发时下方 CAS 失败重试，读到值会被丢弃，无安全后果。
                dst[i] = unsafe { (*self.cells[start + i].get()).assume_init() };
            }

            for i in 0..(n - first) {
                // SAFETY: 第二段，回绕到环头。
                // - 剩余 `n - first` 槽位从 `0` 开始，仍位于 `[r, r + n)` 已初始化范围内；
                // - 其余同第一段：单读端、写端不触及、`Acquire` / `Release`、`T: Copy`。
                dst[first + i] = unsafe { (*self.cells[i].get()).assume_init() };
            }

            match self.read.pos.compare_exchange(
                r,
                r.wrapping_add(n),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.notify_writer_after_read();
                    return n;
                }
                Err(_) => continue,
            }
        }
    }

    pub(crate) fn write_slice(&self, src: &[T]) -> usize {
        if src.is_empty() {
            return 0;
        }
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

        for i in 0..first {
            // SAFETY: 第一段，从 `start` 到环尾。
            // - `[w, w + n)` 位于未初始化区间（`n <= free`），槽位空闲；
            // - 调用方保证单写端（`write.lock` 或 `&mut self`）；`clear` 不推进 `write.pos`；
            // - 读端只读 `[r, w)`，不触及本区间；
            // - 写入后统一 `write.pos.store(Release)` 发布。
            unsafe {
                (*self.cells[start + i].get()).write(src[i]);
            }
        }

        for i in 0..(n - first) {
            // SAFETY: 第二段，回绕到环头。
            // - 剩余 `n - first` 槽位从 `0` 开始，仍位于 `[w, w + n)` 空闲区间；
            // - 其余同第一段：单写端、读端不触及、写入后统一 `Release` 发布。
            unsafe {
                (*self.cells[i].get()).write(src[first + i]);
            }
        }

        self.write.pos.store(w.wrapping_add(n), Ordering::Release);
        self.notify_reader_after_write();
        n
    }

    pub(crate) fn clear_internal(&self) {
        let w = self.write.pos.load(Ordering::Acquire);
        self.read.pos.store(w, Ordering::Release);
        // 不使用 `unwrap()`：中毒时取回内部守卫继续使用。
        let _g = lock_ignore_poison(&self.wait_mutex);
        self.write.notify_all_locked();
    }

    fn notify_reader_after_write(&self) {
        if self.read.has_waiters() {
            // 不使用 `unwrap()`：中毒时取回内部守卫继续使用。
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.read.notify_one_locked();
        }
    }

    fn notify_writer_after_read(&self) {
        if self.write.has_waiters() {
            // 不使用 `unwrap()`：中毒时取回内部守卫继续使用。
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.write.notify_one_locked();
        }
    }

    pub(crate) fn close_reader(&self) {
        if self.read.dec_count_and_close() {
            // 不使用 `unwrap()`：中毒时取回内部守卫继续使用。
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.write.notify_all_locked();
        }
    }

    pub(crate) fn close_writer(&self) {
        if self.write.dec_count_and_close() {
            // 不使用 `unwrap()`：中毒时取回内部守卫继续使用。
            let _g = lock_ignore_poison(&self.wait_mutex);
            self.read.notify_all_locked();
        }
    }
}
