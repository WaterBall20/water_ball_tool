use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::Arc;

use super::error::Error;
use super::inner::{lock_ignore_poison, wait_ignore_poison, Inner};

/// 有锁写端。
///
/// `Send + Sync + Clone`；方法使用 `&self`，内部通过写端锁串行化同端多个
/// clone 的写指针推进。
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
pub struct LockedWriter<T: Copy> {
    inner: Arc<Inner<T>>,
}

impl<T: Copy> LockedWriter<T> {
    pub(crate) fn new(inner: Arc<Inner<T>>) -> Self {
        inner.write.inc_count();
        Self { inner }
    }

    /// 非阻塞写入单个元素。
    ///
    /// 满时返回 `Err(Kind::Full(value))` 归还元素。可阻塞获取写端锁，但不
    /// 等待空间。
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
        self.inner.write_one(value).map_err(Error::full)
    }

    /// 等待至少一个空位后写入。
    ///
    /// 对端关闭且满返回 `Err(CannotWait)`。对端关闭前有空位则立即写入。
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
        let mut value = value;
        loop {
            {
                let _g = lock_ignore_poison(&self.inner.write.lock);
                match self.inner.write_one(value) {
                    Ok(()) => return Ok(()),
                    Err(v) => value = v,
                }
            }
            if self.inner.read.is_closed() { return Err(Error::cannot_wait()); }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.write.enter_wait();
            if self.inner.read.is_closed() {
                self.inner.write.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.is_full() {
                self.inner.write.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.write.cvar, g);
            self.inner.write.leave_wait();
            drop(g);
        }
    }

    /// 从 `src` 复制数据到缓冲区，返回实际写入的元素数。
    ///
    /// 无空间返回 `0`。可阻塞获取写端锁，但不等待空间。`src` 为空返回 `0`。
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
        if src.is_empty() { return 0; }
        let _g = lock_ignore_poison(&self.inner.write.lock);
        self.inner.write_slice(src)
    }

    /// 等待可写空间后从 `src` 复制数据，返回实际写入的元素数。
    ///
    /// 仅当前可写空间为 `0` 时等待；一旦有空间，能写多少写多少，不要求写满。
    /// `src` 为空返回 `Ok(0)`。对端关闭且满返回 `CannotWait`。
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
        if src.is_empty() { return Ok(0); }
        loop {
            {
                let _g = lock_ignore_poison(&self.inner.write.lock);
                let n = self.inner.write_slice(src);
                if n > 0 { return Ok(n); }
            }
            if self.inner.read.is_closed() { return Err(Error::cannot_wait()); }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.write.enter_wait();
            if self.inner.read.is_closed() {
                self.inner.write.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.is_full() {
                self.inner.write.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.write.cvar, g);
            self.inner.write.leave_wait();
            drop(g);
        }
    }

    /// 阻塞直到 `src` 全部写入。
    ///
    /// 允许 `src.len()` 大于缓冲区容量，内部循环完成，不截断。`src` 为空
    /// 直接返回 `Ok(())`。部分完成后对端关闭返回 `CannotWait`。持写端锁
    /// 直到完成，期间其他 `Writer` clone 会被阻塞。
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
    pub fn write_all(&self, src: &[T]) -> Result<(), Error<T>> {
        if src.is_empty() { return Ok(()); }
        let _g = lock_ignore_poison(&self.inner.write.lock);
        let mut written = 0;
        while written < src.len() {
            let n = self.inner.write_slice(&src[written..]);
            if n > 0 { written += n; continue; }
            if self.inner.read.is_closed() { return Err(Error::cannot_wait()); }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.write.enter_wait();
            if self.inner.read.is_closed() {
                self.inner.write.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.is_full() {
                self.inner.write.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.write.cvar, g);
            self.inner.write.leave_wait();
            drop(g);
        }
        Ok(())
    }

    /// 清空缓冲区。
    ///
    /// 推进读指针追上写指针，逻辑清空；不重置为 0，避免与并发写竞争。不获取
    /// 读/写端锁。只唤醒等待写空间的写端。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert!(writer.write(&[1, 2, 3]) > 0);
    /// assert_eq!(reader.len(), 3);
    /// writer.clear();
    /// assert_eq!(reader.len(), 0);
    /// ```
    pub fn clear(&self) { self.inner.clear_internal(); }

    /// 尝试获取写端锁并写入单个元素。仅存在于有锁端。
    ///
    /// 获取锁失败返回 `Err(Locked)`；满返回 `Err(Full(value))`，与 [`push`]
    /// 一致。
    ///
    /// [`push`]: LockedWriter::push
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
        let _g = self.inner.write.lock.try_lock().map_err(|_| Error::locked())?;
        self.inner.write_one(value).map_err(Error::full)
    }

    /// 尝试获取写端锁并写入切片。仅存在于有锁端。
    ///
    /// 获取锁失败返回 `Err(Locked)`；无空间返回 `Ok(0)`，与 [`write`] 一致。
    ///
    /// [`write`]: LockedWriter::write
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
        if src.is_empty() { return Ok(0); }
        let _g = self.inner.write.lock.try_lock().map_err(|_| Error::locked())?;
        Ok(self.inner.write_slice(src))
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
    /// assert!(writer.write(&[1, 2]) > 0);
    /// assert_eq!(writer.len(), 2);
    /// ```
    pub fn len(&self) -> usize { self.inner.len() }

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
    pub fn is_empty(&self) -> bool { self.inner.is_empty() }

    /// 是否已满。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (_reader, writer) = locked_ring_buffer::<u8>(2);
    /// assert!(writer.write_all(&[1, 2]).is_ok());
    /// assert!(writer.is_full());
    /// ```
    pub fn is_full(&self) -> bool { self.inner.is_full() }
}

impl<T: Copy> Clone for LockedWriter<T> {
    fn clone(&self) -> Self {
        self.inner.write.inc_count();
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl<T: Copy> Drop for LockedWriter<T> {
    fn drop(&mut self) { self.inner.close_writer(); }
}

/// 无锁写端。
///
/// `Send`、`!Sync`、`!Clone`；方法使用 `&mut self`，涉及内部缓冲区操作时不
/// 加端锁，仅阻塞等待时使用内部等待锁。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
///
/// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
/// assert!(writer.push(1).is_ok());
/// assert_eq!(reader.pop(), Some(1));
/// ```
pub struct UnlockedWriter<T: Copy> {
    inner: Arc<Inner<T>>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T: Copy> UnlockedWriter<T> {
    pub(crate) fn new(inner: Arc<Inner<T>>) -> Self {
        inner.write.inc_count();
        Self { inner, _not_sync: PhantomData }
    }

    /// 非阻塞写入单个元素。满返回 `Err(Full(value))`。
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
        self.inner.write_one(value).map_err(Error::full)
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
        let mut value = value;
        loop {
            match self.inner.write_one(value) {
                Ok(()) => return Ok(()),
                Err(v) => value = v,
            }
            if self.inner.read.is_closed() { return Err(Error::cannot_wait()); }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.write.enter_wait();
            if self.inner.read.is_closed() {
                self.inner.write.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.is_full() {
                self.inner.write.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.write.cvar, g);
            self.inner.write.leave_wait();
            drop(g);
        }
    }

    /// 从 `src` 复制数据到缓冲区，返回实际写入的元素数。
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
        if src.is_empty() { return 0; }
        self.inner.write_slice(src)
    }

    /// 等待可写空间后从 `src` 复制数据，返回实际写入的元素数。
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
        if src.is_empty() { return Ok(0); }
        loop {
            let n = self.inner.write_slice(src);
            if n > 0 { return Ok(n); }
            if self.inner.read.is_closed() { return Err(Error::cannot_wait()); }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.write.enter_wait();
            if self.inner.read.is_closed() {
                self.inner.write.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.is_full() {
                self.inner.write.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.write.cvar, g);
            self.inner.write.leave_wait();
            drop(g);
        }
    }

    /// 阻塞直到 `src` 全部写入。
    ///
    /// 允许 `src.len()` 大于缓冲区容量，内部循环完成。
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
    pub fn write_all(&mut self, src: &[T]) -> Result<(), Error<T>> {
        if src.is_empty() { return Ok(()); }
        let mut written = 0;
        while written < src.len() {
            let n = self.inner.write_slice(&src[written..]);
            if n > 0 { written += n; continue; }
            if self.inner.read.is_closed() { return Err(Error::cannot_wait()); }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.write.enter_wait();
            if self.inner.read.is_closed() {
                self.inner.write.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.is_full() {
                self.inner.write.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.write.cvar, g);
            self.inner.write.leave_wait();
            drop(g);
        }
        Ok(())
    }

    /// 清空缓冲区，只唤醒等待写空间的写端。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert!(writer.write(&[1, 2, 3]) > 0);
    /// writer.clear();
    /// assert_eq!(reader.len(), 0);
    /// ```
    pub fn clear(&mut self) { self.inner.clear_internal(); }

    /// 当前已写元素数。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (_reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert_eq!(writer.len(), 0);
    /// assert!(writer.write(&[1, 2]) > 0);
    /// assert_eq!(writer.len(), 2);
    /// ```
    pub fn len(&self) -> usize { self.inner.len() }

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
    pub fn is_empty(&self) -> bool { self.inner.is_empty() }

    /// 是否已满。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (_reader, mut writer) = unlocked_ring_buffer::<u8>(2);
    /// assert!(writer.write_all(&[1, 2]).is_ok());
    /// assert!(writer.is_full());
    /// ```
    pub fn is_full(&self) -> bool { self.inner.is_full() }
}

impl<T: Copy> Drop for UnlockedWriter<T> {
    fn drop(&mut self) { self.inner.close_writer(); }
}

/// 写端枚举包装，仅承载有锁/无锁实例，不代理任何操作。
///
/// 由 [`crate::ring_buffer`] 在运行时按 [`WriteMode`](crate::WriteMode) 返回。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
///
/// let (_reader, writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Locked);
/// assert!(writer.into_locked().is_ok());
/// ```
pub enum Writer<T: Copy> {
    /// 有锁写端实例。
    Locked(LockedWriter<T>),
    /// 无锁写端实例。
    Unlocked(UnlockedWriter<T>),
}

impl<T: Copy> Writer<T> {
    /// 若为 [`Writer::Locked`]，返回其具体类型；否则返回原包装。
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
        match self { Writer::Locked(w) => Ok(w), other => Err(other) }
    }

    /// 若为 [`Writer::Unlocked`]，返回其具体类型；否则返回原包装。
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
        match self { Writer::Unlocked(w) => Ok(w), other => Err(other) }
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
    pub fn is_locked(&self) -> bool { matches!(self, Writer::Locked(_)) }

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
    pub fn is_unlocked(&self) -> bool { matches!(self, Writer::Unlocked(_)) }
}