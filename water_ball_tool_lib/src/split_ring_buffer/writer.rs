use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::Arc;
//AI=== 写端兜底超时需要 Duration / Duration for the writer-side fallback timeout
use std::time::Duration;
//AI_END===

use crate::split_ring_buffer::error::{Error, Kind, WriteAllError};
//AI=== 写端等待改为带超时版本 / writer waits use the timed variant
use crate::split_ring_buffer::inner::{Inner, lock_ignore_poison, wait_timeout_ignore_poison};
//AI_END===
use crate::split_ring_buffer::wait_loop;

//AI=== 写端等待空位的兜底超时：读端通知不再持 `wait_mutex`，极端竞态下可能丢一次唤醒；
// 超时保证阻塞写端最多多等这么久而不是永久挂起。
// Fallback timeout for a blocked writer: the reader notifies without `wait_mutex`, so a rare
// race can lose one wakeup; the timeout bounds the wait instead of hanging.
const WRITE_WAIT_TIMEOUT: Duration = Duration::from_millis(10);
//AI_END===

fn kind_to_error<T>(k: Kind<T>) -> Error<T> {
    match k {
        Kind::Full(v) => Error::full(v),
        Kind::Clearing(v) => Error::clearing(v),
        Kind::Truncating(v) => Error::truncating(v),
        Kind::Resizing(v) => Error::resizing(v),
        Kind::WriteRejectedValue(v) => Error::write_rejected_value(v),
        Kind::WriteRejected => Error::write_rejected(),
        _ => unreachable!("push_one only returns Full / Clearing / Truncating / Resizing"),
    }
}

/// 有锁写端。
///
/// `Send + Sync + Clone`（`T: Send` 时）；方法使用 `&self`，内部通过写端锁
/// 串行化同端多个 clone 的写指针推进。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
///
/// let (reader, writer) = locked_ring_buffer::<u8>(4);
/// assert!(writer.push(1).is_ok());
/// assert_eq!(reader.pop(), Some(1));
/// ```
pub struct LockedWriter<T> {
    inner: Arc<Inner<T>>,
}

impl<T> LockedWriter<T> {
    pub(crate) fn new(inner: Arc<Inner<T>>) -> Self {
        inner.write.inc_count();
        Self { inner }
    }

    /// 非阻塞写入单个元素。
    ///
    /// 满返回 `Err(Full(value))`；`clear` 中返回 `Err(Clearing(value))`；
    /// `truncate_back` 中返回 `Err(Truncating(value))`。不受
    /// `reject_pending_writers` 影响。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(2);
    /// assert!(writer.push(1).is_ok());
    /// assert!(writer.push(2).is_ok());
    /// assert!(writer.push(3).is_err());
    /// assert_eq!(reader.pop(), Some(1));
    /// ```
    pub fn push(&self, value: T) -> Result<(), Error<T>> {
        let _g = lock_ignore_poison(&self.inner.write.lock);
        self.inner.push_one(value).map_err(kind_to_error)
    }

    /// 等待至少一个空位后写入。
    ///
    /// 若在等待期间发生 `reject_pending_writers`，返回
    /// `Err(WriteRejectedValue(value))`，归还元素。`reject` 之后进入的写端
    /// 不受影响。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert!(writer.push_wait(1).is_ok());
    /// assert_eq!(reader.pop(), Some(1));
    /// ```
    pub fn push_wait(&self, value: T) -> Result<(), Error<T>> {
        let my_epoch = self.inner.current_rejection_epoch();
        let mut value = value;
        let mut is_wait_loop = false;
        loop {
            if self.inner.rejection_changed(my_epoch) {
                return Err(Error::write_rejected_value(value));
            }
            {
                let _g = lock_ignore_poison(&self.inner.write.lock);
                match self.inner.push_one(value) {
                    Ok(()) => return Ok(()),
                    Err(
                        Kind::Full(v) | Kind::Clearing(v) | Kind::Truncating(v) | Kind::Resizing(v),
                    ) => {
                        value = v;
                    }
                    Err(_) => unreachable!(),
                }
            }
            if self.inner.read.is_closed() {
                return Err(Error::cannot_wait());
            }
            if wait_loop(&mut is_wait_loop) {
                continue;
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            if self.inner.rejection_changed(my_epoch) {
                drop(g);
                return Err(Error::write_rejected_value(value));
            }
            self.inner.write.enter_wait();
            if self.inner.read.is_closed() {
                self.inner.write.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.writer_should_wait() {
                self.inner.write.leave_wait();
                drop(g);
                continue;
            }
            //AI=== 带超时等待兜底丢唤醒 / timed wait bounds a lost wakeup
            g = wait_timeout_ignore_poison(&self.inner.write.cvar, g, WRITE_WAIT_TIMEOUT);
            //AI_END===
            self.inner.write.leave_wait();
            drop(g);
        }
    }

    /// 清空缓冲区：交换 A/B，重置指针，drop 旧 B 中未读元素。
    ///
    /// 与 `truncate_back` / `reject_pending_writers` 互斥；进行期间阻塞所有
    /// 读写。并发 `clear` 直接返回。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<String>(4);
    /// assert!(writer.push(String::from("a")).is_ok());
    /// assert_eq!(reader.len(), 1);
    /// writer.clear();
    /// assert_eq!(reader.len(), 0);
    /// ```
    pub fn clear(&self) {
        self.inner.clear();
    }

    /// 保留前 `keep` 个元素，丢弃尾部。与 `clear` / `reject_pending_writers`
    /// 互斥，进行期间阻塞所有读写。
    ///
    /// `keep >= len` 时不丢弃任何元素。`clear` 或另一个 `truncate_back` 正在
    /// 进行时直接返回（无操作）。
    ///
    /// 不获取写端锁：`write_all` 会持写端锁等待空位，若本方法也抢写端锁，
    /// 两者将互相等待（本方法本应释放空间）。互斥由内部的 `mutation_mutex` +
    /// `truncate_pending` 保证。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<String>(4);
    /// writer.push(String::from("a")).expect("push");
    /// writer.push(String::from("b")).expect("push");
    /// writer.truncate_back(1);
    /// assert_eq!(reader.len(), 1);
    /// assert_eq!(reader.pop().as_deref(), Some("a"));
    /// ```
    pub fn truncate_back(&self, keep: usize) {
        //AI=== 不再获取写端锁，避免与持锁等待空位的 write_all 互相等待。
        self.inner.truncate_back(keep);
        //AI_END===
    }

    /// 清空缓冲区并拒绝当前所有 pending 的阻塞写端，让它们返回
    /// `Err(WriteRejected)` / `Err(WriteRejectedValue(value))`。
    ///
    /// 之后进入的阻塞写端不受影响，正常等待 / 写入。非阻塞 `push` /
    /// `try_push` / `write` / `try_write` 不受影响。与 `clear` /
    /// `truncate_back` 互斥。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    /// use water_ball_tool_lib::split_ring_buffer::Kind;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert!(writer.push(1).is_ok());
    /// writer.reject_pending_writers();
    /// assert_eq!(reader.len(), 0);
    ///
    /// // reject 之后的新写入不受影响
    /// assert!(writer.push_wait(2).is_ok());
    /// assert_eq!(reader.pop(), Some(2));
    ///
    /// let _ = Kind::<u8>::WriteRejected;
    /// ```
    pub fn reject_pending_writers(&self) {
        self.inner.reject_pending_writers();
    }

    //AI=== 非破坏性唤醒阻塞写端：不清空数据 / non-destructive wake for blocked writers: no data is cleared
    /// 主动唤醒阻塞写端，但**不清空缓冲区**。
    ///
    /// `write_all` 会返回 [`WriteAllErrorKind::Interrupted`](crate::split_ring_buffer::WriteAllErrorKind::Interrupted)，
    /// 并携带已写入元素数；缓冲区内容保持不变，可从 `written` 处续写。
    /// `push_wait` / `write_wait` 不受此方法影响。
    ///
    /// Wake blocked writers without clearing the buffer. `write_all` returns
    /// `Interrupted` with the written count; queued data stays intact and the
    /// caller may resume from `written`. `push_wait` / `write_wait` are unaffected.
    pub fn wake_blocked_writers(&self) {
        self.inner.wake_blocked_writers();
    }
    //AI_END===

    /// 尝试获取写端锁并写入。获取锁失败返回 `Err(Locked)`。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (_reader, writer) = locked_ring_buffer::<u8>(2);
    /// assert!(writer.try_push(1).is_ok());
    /// assert!(writer.try_push(2).is_ok());
    /// assert!(writer.try_push(3).is_err());
    /// ```
    pub fn try_push(&self, value: T) -> Result<(), Error<T>> {
        let _g = self
            .inner
            .write
            .lock
            .try_lock()
            .map_err(|_| Error::locked())?;
        self.inner.push_one(value).map_err(kind_to_error)
    }

    /// 当前已写元素数。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (_reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert_eq!(writer.len(), 0);
    /// assert!(writer.push(1).is_ok());
    /// assert_eq!(writer.len(), 1);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// 是否为空。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (_reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert!(writer.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// 是否已满。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (_reader, writer) = locked_ring_buffer::<u8>(2);
    /// assert!(writer.push(1).is_ok());
    /// assert!(writer.push(2).is_ok());
    /// assert!(writer.is_full());
    /// ```
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }

    //AI=== 当前逻辑容量（resize 后可变化）/ current logical capacity (may change after resize)
    /// 当前逻辑容量；`resize` 后可能变化。
    ///
    /// Current logical capacity; may change after [`LockedWriter::resize`].
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (_reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert_eq!(writer.capacity(), 4);
    /// ```
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }
    //AI_END===
}

impl<T: Copy> LockedWriter<T> {
    /// 从 `src` 复制数据到缓冲区，返回实际写入数量。仅 `T: Copy` 可用。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert_eq!(writer.write(&[1, 2, 3]), 3);
    /// assert_eq!(writer.write(&[4, 5]), 1);
    /// assert_eq!(reader.len(), 4);
    /// ```
    pub fn write(&self, src: &[T]) -> usize {
        if src.is_empty() {
            return 0;
        }
        let _g = lock_ignore_poison(&self.inner.write.lock);
        self.inner.write_slice(src)
    }

    /// 等待可写空间后从 `src` 复制数据。仅 `T: Copy` 可用。
    ///
    /// 若在等待期间发生 `reject_pending_writers`，返回
    /// `Err(WriteRejected)`；调用方可通过 `src` 获取剩余数据。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(8);
    /// match writer.write_wait(&[1, 2, 3]) {
    ///     Ok(n) => assert_eq!(n, 3),
    ///     Err(_) => panic!("unexpected error"),
    /// }
    /// assert_eq!(reader.len(), 3);
    /// ```
    pub fn write_wait(&self, src: &[T]) -> Result<usize, Error<T>> {
        if src.is_empty() {
            return Ok(0);
        }
        let my_epoch = self.inner.current_rejection_epoch();
        let mut is_wait_loop = false;
        loop {
            if self.inner.rejection_changed(my_epoch) {
                return Err(Error::write_rejected());
            }
            {
                let _g = lock_ignore_poison(&self.inner.write.lock);
                let n = self.inner.write_slice(src);
                if n > 0 {
                    return Ok(n);
                }
            }
            if self.inner.read.is_closed() {
                return Err(Error::cannot_wait());
            }
            if wait_loop(&mut is_wait_loop) {
                continue;
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            if self.inner.rejection_changed(my_epoch) {
                drop(g);
                return Err(Error::write_rejected());
            }
            self.inner.write.enter_wait();
            if self.inner.read.is_closed() {
                self.inner.write.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.writer_should_wait() {
                self.inner.write.leave_wait();
                drop(g);
                continue;
            }
            //AI=== 带超时等待兜底丢唤醒 / timed wait bounds a lost wakeup
            g = wait_timeout_ignore_poison(&self.inner.write.cvar, g, WRITE_WAIT_TIMEOUT);
            //AI_END===
            self.inner.write.leave_wait();
            drop(g);
        }
    }

    /// 阻塞直到 `src` 全部写入。仅 `T: Copy` 可用。
    ///
    /// 失败时返回 [`WriteAllError`]，其中 `written` 是本次调用已成功写入的
    /// 元素数：
    /// - `Interrupted`：被 `wake_blocked_writers` 唤醒，缓冲内容不变，可从
    ///   `written` 处续写；
    /// - `Rejected`：被 `reject_pending_writers` 清空，已写前缀失效，不能续写；
    /// - `PeerClosed`：读端消失，无法继续。
    ///
    /// On failure, [`WriteAllError`] carries the number of elements written by
    /// this call. `Interrupted` may be resumed from `written`; `Rejected` means
    /// the buffer was cleared and the prefix is gone; `PeerClosed` cannot continue.
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(8);
    /// assert!(writer.write_all(&[1, 2, 3, 4, 5]).is_ok());
    /// assert_eq!(reader.len(), 5);
    /// ```
    pub fn write_all(&self, src: &[T]) -> Result<(), WriteAllError> {
        if src.is_empty() {
            return Ok(());
        }
        let my_rejection = self.inner.current_rejection_epoch();
        let my_wake = self.inner.current_wake_epoch();
        let _g = lock_ignore_poison(&self.inner.write.lock);
        let mut written = 0;
        let mut is_wait_loop = false;
        loop {
            if self.inner.rejection_changed(my_rejection) {
                return Err(WriteAllError::rejected(written));
            }
            if self.inner.wake_changed(my_wake) {
                return Err(WriteAllError::interrupted(written));
            }
            let n = self.inner.write_slice(&src[written..]);
            if n > 0 {
                written += n;
                if written == src.len() {
                    return Ok(());
                }
                continue;
            }
            if self.inner.read.is_closed() {
                return Err(WriteAllError::peer_closed(written));
            }
            if wait_loop(&mut is_wait_loop) {
                continue;
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            if self.inner.rejection_changed(my_rejection) {
                drop(g);
                return Err(WriteAllError::rejected(written));
            }
            if self.inner.wake_changed(my_wake) {
                drop(g);
                return Err(WriteAllError::interrupted(written));
            }
            self.inner.write.enter_wait();
            if self.inner.read.is_closed() {
                self.inner.write.leave_wait();
                return Err(WriteAllError::peer_closed(written));
            }
            if !self.inner.writer_should_wait() {
                self.inner.write.leave_wait();
                drop(g);
                continue;
            }
            //AI=== 带超时等待兜底丢唤醒 / timed wait bounds a lost wakeup
            g = wait_timeout_ignore_poison(&self.inner.write.cvar, g, WRITE_WAIT_TIMEOUT);
            //AI_END===
            self.inner.write.leave_wait();
            drop(g);
            is_wait_loop = false;
        }
    }

    /// 尝试获取写端锁并写入切片。仅 `T: Copy` 可用。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (_reader, writer) = locked_ring_buffer::<u8>(4);
    /// match writer.try_write(&[1, 2, 3]) {
    ///     Ok(n) => assert_eq!(n, 3),
    ///     Err(_) => panic!("unexpected Locked"),
    /// }
    /// ```
    pub fn try_write(&self, src: &[T]) -> Result<usize, Error<T>> {
        if src.is_empty() {
            return Ok(0);
        }
        let _g = self
            .inner
            .write
            .lock
            .try_lock()
            .map_err(|_| Error::locked())?;
        Ok(self.inner.write_slice(src))
    }

    //AI=== 调整缓冲区大小 / resize the buffer
    /// 调整逻辑容量；超过物理分配时重新分配并复制存量数据。仅 `T: Copy` 可用。
    ///
    /// - 扩容：阻止新写端并等待在途写端退出，读端在复制期间可继续读取，只在极短的交换窗口内
    ///   读到“暂无数据”；分配新 A/B、复制存量、交换缓冲区，旧缓冲区延迟释放。
    /// - 缩小或在物理分配内调整：只更新逻辑容量。已写入超过新容量的数据会保留，
    ///   写端等待读端排空后继续。
    ///
    /// Resize the logical capacity; grow past the allocation by reallocating and copying the
    /// live data. Only `T: Copy` is supported.
    ///
    /// - Grow: block new writers, wait for in-flight writers (readers keep running), allocate
    ///   new A/B, copy live data, swap the buffers and retire the old one.
    /// - Shrink / grow within the allocation: only the logical capacity changes. Data already
    ///   written past the new capacity is kept; writers wait until the reader drains.
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// writer.write(&[1, 2, 3]);
    /// writer.resize(8);
    /// assert_eq!(writer.capacity(), 8);
    /// let mut buf = [0u8; 3];
    /// assert_eq!(reader.read(&mut buf), 3);
    /// assert_eq!(buf, [1, 2, 3]);
    /// ```
    pub fn resize(&self, size: usize) {
        self.inner.resize(size);
    }
    //AI_END===
}

impl<T> Clone for LockedWriter<T> {
    fn clone(&self) -> Self {
        self.inner.write.inc_count();
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Drop for LockedWriter<T> {
    fn drop(&mut self) {
        self.inner.close_writer();
    }
}

/// 无锁写端。
///
/// `Send`、`!Sync`、`!Clone`；方法使用 `&mut self`。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
///
/// let (mut reader, mut writer) = unlocked_ring_buffer::<String>(4);
/// assert!(writer.push(String::from("a")).is_ok());
/// assert_eq!(reader.pop().as_deref(), Some("a"));
/// ```
pub struct UnlockedWriter<T> {
    inner: Arc<Inner<T>>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T> UnlockedWriter<T> {
    pub(crate) fn new(inner: Arc<Inner<T>>) -> Self {
        inner.write.inc_count();
        Self {
            inner,
            _not_sync: PhantomData,
        }
    }

    /// 非阻塞写入单个元素。不受 `reject_pending_writers` 影响。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (_reader, mut writer) = unlocked_ring_buffer::<u8>(2);
    /// assert!(writer.push(1).is_ok());
    /// assert!(writer.push(2).is_ok());
    /// assert!(writer.push(3).is_err());
    /// ```
    pub fn push(&mut self, value: T) -> Result<(), Error<T>> {
        self.inner.push_one(value).map_err(kind_to_error)
    }

    /// 等待至少一个空位后写入。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert!(writer.push_wait(1).is_ok());
    /// assert_eq!(reader.pop(), Some(1));
    /// ```
    pub fn push_wait(&mut self, value: T) -> Result<(), Error<T>> {
        let my_epoch = self.inner.current_rejection_epoch();
        let mut value = value;
        let mut is_wait_loop = false;
        loop {
            if self.inner.rejection_changed(my_epoch) {
                return Err(Error::write_rejected_value(value));
            }
            match self.inner.push_one(value) {
                Ok(()) => return Ok(()),
                Err(
                    Kind::Full(v) | Kind::Clearing(v) | Kind::Truncating(v) | Kind::Resizing(v),
                ) => value = v,
                Err(_) => unreachable!(),
            }
            if self.inner.read.is_closed() {
                return Err(Error::cannot_wait());
            }
            if wait_loop(&mut is_wait_loop) {
                continue;
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            if self.inner.rejection_changed(my_epoch) {
                drop(g);
                return Err(Error::write_rejected_value(value));
            }
            self.inner.write.enter_wait();
            if self.inner.read.is_closed() {
                self.inner.write.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.writer_should_wait() {
                self.inner.write.leave_wait();
                drop(g);
                continue;
            }
            //AI=== 带超时等待兜底丢唤醒 / timed wait bounds a lost wakeup
            g = wait_timeout_ignore_poison(&self.inner.write.cvar, g, WRITE_WAIT_TIMEOUT);
            //AI_END===
            self.inner.write.leave_wait();
            drop(g);
        }
    }

    /// 清空缓冲区。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<String>(4);
    /// assert!(writer.push(String::from("a")).is_ok());
    /// writer.clear();
    /// assert_eq!(reader.len(), 0);
    /// ```
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// 保留前 `keep` 个元素，丢弃尾部。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<String>(4);
    /// writer.push(String::from("a")).expect("push");
    /// writer.push(String::from("b")).expect("push");
    /// writer.truncate_back(1);
    /// assert_eq!(reader.len(), 1);
    /// ```
    pub fn truncate_back(&mut self, keep: usize) {
        self.inner.truncate_back(keep);
    }

    /// 清空缓冲区并拒绝当前所有 pending 的阻塞写端。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert!(writer.push(1).is_ok());
    /// writer.reject_pending_writers();
    /// assert_eq!(reader.len(), 0);
    /// ```
    pub fn reject_pending_writers(&self) {
        self.inner.reject_pending_writers();
    }

    //AI=== 非破坏性唤醒阻塞写端：不清空数据 / non-destructive wake for blocked writers: no data is cleared
    /// 主动唤醒阻塞写端，但**不清空缓冲区**；语义与 [`LockedWriter::wake_blocked_writers`] 相同。
    ///
    /// Wake blocked writers without clearing the buffer; semantics match
    /// [`LockedWriter::wake_blocked_writers`].
    pub fn wake_blocked_writers(&self) {
        self.inner.wake_blocked_writers();
    }
    //AI_END===

    /// 当前已写元素数。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (_reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert_eq!(writer.len(), 0);
    /// assert!(writer.push(1).is_ok());
    /// assert_eq!(writer.len(), 1);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// 是否为空。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (_reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert!(writer.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// 是否已满。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (_reader, mut writer) = unlocked_ring_buffer::<u8>(2);
    /// assert!(writer.push(1).is_ok());
    /// assert!(writer.push(2).is_ok());
    /// assert!(writer.is_full());
    /// ```
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }

    //AI=== 当前逻辑容量（resize 后可变化）/ current logical capacity (may change after resize)
    /// 当前逻辑容量；`resize` 后可能变化。
    ///
    /// Current logical capacity; may change after [`UnlockedWriter::resize`].
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (_reader, writer) = unlocked_ring_buffer::<u8>(4);
    /// assert_eq!(writer.capacity(), 4);
    /// ```
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }
    //AI_END===
}

impl<T: Copy> UnlockedWriter<T> {
    /// 从 `src` 复制数据到缓冲区。仅 `T: Copy` 可用。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert_eq!(writer.write(&[1, 2, 3]), 3);
    /// assert_eq!(reader.len(), 3);
    /// ```
    pub fn write(&mut self, src: &[T]) -> usize {
        if src.is_empty() {
            return 0;
        }
        self.inner.write_slice(src)
    }

    /// 等待可写空间后从 `src` 复制数据。仅 `T: Copy` 可用。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(8);
    /// match writer.write_wait(&[1, 2, 3]) {
    ///     Ok(n) => assert_eq!(n, 3),
    ///     Err(_) => panic!("unexpected error"),
    /// }
    /// assert_eq!(reader.len(), 3);
    /// ```
    pub fn write_wait(&mut self, src: &[T]) -> Result<usize, Error<T>> {
        if src.is_empty() {
            return Ok(0);
        }
        let my_epoch = self.inner.current_rejection_epoch();
        let mut is_wait_loop = false;
        loop {
            if self.inner.rejection_changed(my_epoch) {
                return Err(Error::write_rejected());
            }
            let n = self.inner.write_slice(src);
            if n > 0 {
                return Ok(n);
            }
            if self.inner.read.is_closed() {
                return Err(Error::cannot_wait());
            }
            if wait_loop(&mut is_wait_loop) {
                continue;
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            if self.inner.rejection_changed(my_epoch) {
                drop(g);
                return Err(Error::write_rejected());
            }
            self.inner.write.enter_wait();
            if self.inner.read.is_closed() {
                self.inner.write.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.writer_should_wait() {
                self.inner.write.leave_wait();
                drop(g);
                continue;
            }
            //AI=== 带超时等待兜底丢唤醒 / timed wait bounds a lost wakeup
            g = wait_timeout_ignore_poison(&self.inner.write.cvar, g, WRITE_WAIT_TIMEOUT);
            //AI_END===
            self.inner.write.leave_wait();
            drop(g);
        }
    }

    /// 阻塞直到 `src` 全部写入。仅 `T: Copy` 可用。
    ///
    /// 失败时返回 [`WriteAllError`]，语义与 [`LockedWriter::write_all`] 相同。
    ///
    /// On failure [`WriteAllError`] carries the written count; semantics match
    /// [`LockedWriter::write_all`].
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(8);
    /// assert!(writer.write_all(&[1, 2, 3, 4, 5]).is_ok());
    /// assert_eq!(reader.len(), 5);
    /// ```
    pub fn write_all(&mut self, src: &[T]) -> Result<(), WriteAllError> {
        if src.is_empty() {
            return Ok(());
        }
        let my_rejection = self.inner.current_rejection_epoch();
        let my_wake = self.inner.current_wake_epoch();
        let mut written = 0;
        let mut is_wait_loop = false;
        loop {
            if self.inner.rejection_changed(my_rejection) {
                return Err(WriteAllError::rejected(written));
            }
            if self.inner.wake_changed(my_wake) {
                return Err(WriteAllError::interrupted(written));
            }
            let n = self.inner.write_slice(&src[written..]);
            if n > 0 {
                written += n;
                if written == src.len() {
                    return Ok(());
                }
                continue;
            }
            if self.inner.read.is_closed() {
                return Err(WriteAllError::peer_closed(written));
            }
            if wait_loop(&mut is_wait_loop) {
                continue;
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            if self.inner.rejection_changed(my_rejection) {
                drop(g);
                return Err(WriteAllError::rejected(written));
            }
            if self.inner.wake_changed(my_wake) {
                drop(g);
                return Err(WriteAllError::interrupted(written));
            }
            self.inner.write.enter_wait();
            if self.inner.read.is_closed() {
                self.inner.write.leave_wait();
                return Err(WriteAllError::peer_closed(written));
            }
            if !self.inner.writer_should_wait() {
                self.inner.write.leave_wait();
                drop(g);
                continue;
            }
            //AI=== 带超时等待兜底丢唤醒 / timed wait bounds a lost wakeup
            g = wait_timeout_ignore_poison(&self.inner.write.cvar, g, WRITE_WAIT_TIMEOUT);
            //AI_END===
            self.inner.write.leave_wait();
            drop(g);
            is_wait_loop = false;
        }
    }

    //AI=== 调整缓冲区大小 / resize the buffer
    /// 调整逻辑容量；超过物理分配时重新分配并复制存量数据。仅 `T: Copy` 可用。
    ///
    /// 语义与 [`LockedWriter::resize`] 相同。
    ///
    /// Resize the logical capacity; grow past the allocation by reallocating and copying the
    /// live data. Only `T: Copy` is supported; semantics match [`LockedWriter::resize`].
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// writer.write(&[1, 2, 3]);
    /// writer.resize(8);
    /// assert_eq!(writer.capacity(), 8);
    /// let mut buf = [0u8; 3];
    /// assert_eq!(reader.read(&mut buf), 3);
    /// assert_eq!(buf, [1, 2, 3]);
    /// ```
    pub fn resize(&mut self, size: usize) {
        self.inner.resize(size);
    }
    //AI_END===
}

impl<T> Drop for UnlockedWriter<T> {
    fn drop(&mut self) {
        self.inner.close_writer();
    }
}

/// 写端枚举包装，仅承载有锁/无锁实例，不代理任何操作。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
///
/// let (_reader, writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Locked);
/// assert!(writer.into_locked().is_ok());
/// ```
pub enum Writer<T> {
    /// 有锁写端实例。
    Locked(LockedWriter<T>),
    /// 无锁写端实例。
    Unlocked(UnlockedWriter<T>),
}

impl<T> Writer<T> {
    /// 若为 `Locked`，返回其具体类型；否则返回原包装。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
    ///
    /// let (_reader, writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Locked);
    /// assert!(writer.into_locked().is_ok());
    /// ```
    pub fn into_locked(self) -> Result<LockedWriter<T>, Self> {
        match self {
            Writer::Locked(w) => Ok(w),
            other => Err(other),
        }
    }

    /// 若为 `Unlocked`，返回其具体类型；否则返回原包装。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
    ///
    /// let (_reader, writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Unlocked);
    /// assert!(writer.into_unlocked().is_ok());
    /// ```
    pub fn into_unlocked(self) -> Result<UnlockedWriter<T>, Self> {
        match self {
            Writer::Unlocked(w) => Ok(w),
            other => Err(other),
        }
    }

    /// 是否承载有锁写端。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
    ///
    /// let (_reader, writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Locked);
    /// assert!(writer.is_locked());
    /// ```
    #[must_use]
    pub fn is_locked(&self) -> bool {
        matches!(self, Writer::Locked(_))
    }

    /// 是否承载无锁写端。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
    ///
    /// let (_reader, writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Unlocked);
    /// assert!(writer.is_unlocked());
    /// ```
    #[must_use]
    pub fn is_unlocked(&self) -> bool {
        matches!(self, Writer::Unlocked(_))
    }
}
