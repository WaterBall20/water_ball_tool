use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::Arc;

use super::error::Error;
use super::inner::{lock_ignore_poison, wait_ignore_poison, Inner};

/// 有锁读端。
///
/// `Send + Sync + Clone`；方法使用 `&self`，内部通过读端锁串行化同端多个
/// clone 的读指针推进。读取成功后可 `Clone` 共享同一内部缓冲区。
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
pub struct LockedReader<T: Copy> {
    inner: Arc<Inner<T>>,
}

impl<T: Copy> LockedReader<T> {
    pub(crate) fn new(inner: Arc<Inner<T>>) -> Self {
        inner.read.inc_count();
        Self { inner }
    }

    /// 非阻塞弹出单个元素。
    ///
    /// 空时返回 `None`。可阻塞获取读端锁，但不等待数据。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert_eq!(reader.pop(), None);
    /// assert!(writer.push(1).is_ok());
    /// assert_eq!(reader.pop(), Some(1));
    /// ```
    pub fn pop(&self) -> Option<T> {
        let _g = lock_ignore_poison(&self.inner.read.lock);
        self.inner.read_one()
    }

    /// 等待至少一个元素后弹出。
    ///
    /// 返回 `Err(CannotWait)` 表示等待期间对端关闭且无数据。若对端关闭前已
    /// 存在数据，直接弹出；对端已关闭且缓冲区空才返回错误。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert!(writer.push(7).is_ok());
    /// match reader.pop_wait() {
    ///     Ok(v) => assert_eq!(v, 7),
    ///     Err(_) => panic!("unexpected error"),
    /// }
    /// ```
    pub fn pop_wait(&self) -> Result<T, Error<T>> {
        loop {
            {
                let _g = lock_ignore_poison(&self.inner.read.lock);
                if let Some(v) = self.inner.read_one() { return Ok(v); }
            }
            if self.inner.write.is_closed() { return Err(Error::cannot_wait()); }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.enter_wait();
            if self.inner.write.is_closed() {
                self.inner.read.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.is_empty() {
                self.inner.read.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.read.cvar, g);
            self.inner.read.leave_wait();
            drop(g);
        }
    }

    /// 从缓冲区复制数据到 `dst`，返回实际复制的元素数。
    ///
    /// 无可读数据时返回 `0`。可阻塞获取读端锁，但不等待数据。`dst` 为空时
    /// 直接返回 `0`。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// let mut buf = [0u8; 4];
    /// assert_eq!(reader.read(&mut buf), 0);
    /// assert_eq!(writer.write(&[1, 2, 3]), 3);
    /// assert_eq!(reader.read(&mut buf), 3);
    /// assert_eq!(&buf[..3], &[1, 2, 3]);
    /// ```
    pub fn read(&self, dst: &mut [T]) -> usize {
        if dst.is_empty() { return 0; }
        let _g = lock_ignore_poison(&self.inner.read.lock);
        self.inner.read_slice(dst)
    }

    /// 等待可读数据后复制到 `dst`，返回实际复制的元素数。
    ///
    /// 仅当前可读数量为 `0` 时等待；一旦可读，能读多少读多少，不要求读满。
    /// `dst` 为空时直接返回 `Ok(0)`。对端关闭且无可读数据返回 `CannotWait`。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(8);
    /// assert!(writer.write(&[1, 2, 3]) > 0);
    /// let mut buf = [0u8; 8];
    /// match reader.read_wait(&mut buf) {
    ///     Ok(n) => assert_eq!(n, 3),
    ///     Err(_) => panic!("unexpected error"),
    /// }
    /// ```
    pub fn read_wait(&self, dst: &mut [T]) -> Result<usize, Error<T>> {
        if dst.is_empty() { return Ok(0); }
        loop {
            {
                let _g = lock_ignore_poison(&self.inner.read.lock);
                let n = self.inner.read_slice(dst);
                if n > 0 { return Ok(n); }
            }
            if self.inner.write.is_closed() { return Err(Error::cannot_wait()); }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.enter_wait();
            if self.inner.write.is_closed() {
                self.inner.read.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.is_empty() {
                self.inner.read.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.read.cvar, g);
            self.inner.read.leave_wait();
            drop(g);
        }
    }

    /// 阻塞直到 `dst` 填满。
    ///
    /// 允许 `dst.len()` 大于缓冲区容量，内部循环完成，不截断。`dst` 为空时
    /// 直接返回 `Ok(())`。部分完成后对端关闭返回 `CannotWait`，已完成部分
    /// 保持已读状态。持读端锁直到完成，期间其他 `Reader` clone 会被阻塞。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert!(writer.write_all(&[1, 2, 3]).is_ok());
    /// let mut buf = [0u8; 3];
    /// assert!(reader.read_exact(&mut buf).is_ok());
    /// assert_eq!(buf, [1, 2, 3]);
    /// ```
    pub fn read_exact(&self, dst: &mut [T]) -> Result<(), Error<T>> {
        if dst.is_empty() { return Ok(()); }
        let _g = lock_ignore_poison(&self.inner.read.lock);
        let mut filled = 0;
        while filled < dst.len() {
            let n = self.inner.read_slice(&mut dst[filled..]);
            if n > 0 { filled += n; continue; }
            if self.inner.write.is_closed() { return Err(Error::cannot_wait()); }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.enter_wait();
            if self.inner.write.is_closed() {
                self.inner.read.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.is_empty() {
                self.inner.read.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.read.cvar, g);
            self.inner.read.leave_wait();
            drop(g);
        }
        Ok(())
    }

    /// 尝试获取读端锁并弹出。仅存在于有锁端。
    ///
    /// 获取锁失败返回 `Err(Locked)`；拿到锁后空返回 `Ok(None)`，与 [`pop`]
    /// 一致。
    ///
    /// [`pop`]: LockedReader::pop
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert!(writer.push(1).is_ok());
    /// match reader.try_pop() {
    ///     Ok(Some(v)) => assert_eq!(v, 1),
    ///     Ok(None) => panic!("expected Some"),
    ///     Err(_) => panic!("unexpected Locked"),
    /// }
    /// ```
    pub fn try_pop(&self) -> Result<Option<T>, Error<T>> {
        let _g = self.inner.read.lock.try_lock().map_err(|_| Error::locked())?;
        Ok(self.inner.read_one())
    }

    /// 尝试获取读端锁并读取。仅存在于有锁端。
    ///
    /// 获取锁失败返回 `Err(Locked)`；拿到锁后无可读返回 `Ok(0)`，与 [`read`]
    /// 一致。
    ///
    /// [`read`]: LockedReader::read
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert!(writer.write(&[1, 2]) > 0);
    /// let mut buf = [0u8; 4];
    /// match reader.try_read(&mut buf) {
    ///     Ok(n) => assert_eq!(n, 2),
    ///     Err(_) => panic!("unexpected Locked"),
    /// }
    /// ```
    pub fn try_read(&self, dst: &mut [T]) -> Result<usize, Error<T>> {
        if dst.is_empty() { return Ok(0); }
        let _g = self.inner.read.lock.try_lock().map_err(|_| Error::locked())?;
        Ok(self.inner.read_slice(dst))
    }

    /// 当前可读元素数，原子加载指针计算，不加锁。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert_eq!(reader.len(), 0);
    /// assert!(writer.write(&[1, 2, 3]) > 0);
    /// assert_eq!(reader.len(), 3);
    /// ```
    pub fn len(&self) -> usize { self.inner.len() }

    /// 是否为空。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(4);
    /// assert!(reader.is_empty());
    /// assert!(writer.push(1).is_ok());
    /// assert!(!reader.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool { self.inner.is_empty() }

    /// 是否已满。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<u8>(2);
    /// assert!(writer.write_all(&[1, 2]).is_ok());
    /// assert!(reader.is_full());
    /// ```
    pub fn is_full(&self) -> bool { self.inner.is_full() }
}

impl<T: Copy> Clone for LockedReader<T> {
    fn clone(&self) -> Self {
        self.inner.read.inc_count();
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl<T: Copy> Drop for LockedReader<T> {
    fn drop(&mut self) { self.inner.close_reader(); }
}

/// 无锁读端。
///
/// `Send`、`!Sync`、`!Clone`；方法使用 `&mut self`，涉及内部缓冲区操作时
/// 不加端锁，仅阻塞等待时使用内部等待锁。适用于单线程独占读取。
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
pub struct UnlockedReader<T: Copy> {
    inner: Arc<Inner<T>>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T: Copy> UnlockedReader<T> {
    pub(crate) fn new(inner: Arc<Inner<T>>) -> Self {
        inner.read.inc_count();
        Self { inner, _not_sync: PhantomData }
    }

    /// 非阻塞弹出单个元素。空返回 `None`。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert_eq!(reader.pop(), None);
    /// assert!(writer.push(1).is_ok());
    /// assert_eq!(reader.pop(), Some(1));
    /// ```
    pub fn pop(&mut self) -> Option<T> { self.inner.read_one() }

    /// 等待至少一个元素后弹出。
    ///
    /// 对端关闭且无数据返回 `Err(CannotWait)`。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert!(writer.push(7).is_ok());
    /// match reader.pop_wait() {
    ///     Ok(v) => assert_eq!(v, 7),
    ///     Err(_) => panic!("unexpected error"),
    /// }
    /// ```
    pub fn pop_wait(&mut self) -> Result<T, Error<T>> {
        loop {
            if let Some(v) = self.inner.read_one() { return Ok(v); }
            if self.inner.write.is_closed() { return Err(Error::cannot_wait()); }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.enter_wait();
            if self.inner.write.is_closed() {
                self.inner.read.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.is_empty() {
                self.inner.read.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.read.cvar, g);
            self.inner.read.leave_wait();
            drop(g);
        }
    }

    /// 从缓冲区复制数据到 `dst`，返回实际复制的元素数。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert!(writer.write(&[1, 2, 3]) > 0);
    /// let mut buf = [0u8; 4];
    /// assert_eq!(reader.read(&mut buf), 3);
    /// assert_eq!(&buf[..3], &[1, 2, 3]);
    /// ```
    pub fn read(&mut self, dst: &mut [T]) -> usize {
        if dst.is_empty() { return 0; }
        self.inner.read_slice(dst)
    }

    /// 等待可读数据后复制到 `dst`，返回实际复制的元素数。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(8);
    /// assert!(writer.write(&[1, 2]) > 0);
    /// let mut buf = [0u8; 8];
    /// match reader.read_wait(&mut buf) {
    ///     Ok(n) => assert_eq!(n, 2),
    ///     Err(_) => panic!("unexpected error"),
    /// }
    /// ```
    pub fn read_wait(&mut self, dst: &mut [T]) -> Result<usize, Error<T>> {
        if dst.is_empty() { return Ok(0); }
        loop {
            let n = self.inner.read_slice(dst);
            if n > 0 { return Ok(n); }
            if self.inner.write.is_closed() { return Err(Error::cannot_wait()); }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.enter_wait();
            if self.inner.write.is_closed() {
                self.inner.read.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.is_empty() {
                self.inner.read.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.read.cvar, g);
            self.inner.read.leave_wait();
            drop(g);
        }
    }

    /// 阻塞直到 `dst` 填满。
    ///
    /// 允许 `dst.len()` 大于缓冲区容量，内部循环完成。部分完成后对端关闭
    /// 返回 `CannotWait`。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert!(writer.write_all(&[1, 2, 3]).is_ok());
    /// let mut buf = [0u8; 3];
    /// assert!(reader.read_exact(&mut buf).is_ok());
    /// assert_eq!(buf, [1, 2, 3]);
    /// ```
    pub fn read_exact(&mut self, dst: &mut [T]) -> Result<(), Error<T>> {
        if dst.is_empty() { return Ok(()); }
        let mut filled = 0;
        while filled < dst.len() {
            let n = self.inner.read_slice(&mut dst[filled..]);
            if n > 0 { filled += n; continue; }
            if self.inner.write.is_closed() { return Err(Error::cannot_wait()); }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.enter_wait();
            if self.inner.write.is_closed() {
                self.inner.read.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.is_empty() {
                self.inner.read.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.read.cvar, g);
            self.inner.read.leave_wait();
            drop(g);
        }
        Ok(())
    }

    /// 当前可读元素数，原子加载，不加锁。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert_eq!(reader.len(), 0);
    /// assert!(writer.write(&[1, 2]) > 0);
    /// assert_eq!(reader.len(), 2);
    /// ```
    pub fn len(&self) -> usize { self.inner.len() }

    /// 是否为空。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert!(reader.is_empty());
    /// assert!(writer.push(1).is_ok());
    /// assert!(!reader.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool { self.inner.is_empty() }

    /// 是否已满。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(2);
    /// assert!(writer.write_all(&[1, 2]).is_ok());
    /// assert!(reader.is_full());
    /// ```
    pub fn is_full(&self) -> bool { self.inner.is_full() }
}

impl<T: Copy> Drop for UnlockedReader<T> {
    fn drop(&mut self) { self.inner.close_reader(); }
}

/// 读端枚举包装，仅承载有锁/无锁实例，不代理任何操作。
///
/// 由 [`crate::ring_buffer`] 在运行时按 [`ReadMode`](crate::ReadMode) 返回。
/// 调用方需通过 [`Reader::into_locked`] / [`Reader::into_unlocked`] 解包后
/// 使用具体类型的方法。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
///
/// let (reader, writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Locked);
/// let reader = match reader.into_locked() {
///     Ok(r) => r,
///     Err(_) => panic!("expected Locked variant"),
/// };
/// assert!(writer.into_locked().is_ok());
/// assert!(reader.is_empty());
/// ```
pub enum Reader<T: Copy> {
    /// 有锁读端实例。
    Locked(LockedReader<T>),
    /// 无锁读端实例。
    Unlocked(UnlockedReader<T>),
}

impl<T: Copy> Reader<T> {
    /// 若为 [`Reader::Locked`]，返回其具体类型；否则返回原包装。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
    ///
    /// let (reader, _writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Locked);
    /// assert!(reader.into_locked().is_ok());
    /// ```
    pub fn into_locked(self) -> Result<LockedReader<T>, Self> {
        match self { Reader::Locked(r) => Ok(r), other => Err(other) }
    }

    /// 若为 [`Reader::Unlocked`]，返回其具体类型；否则返回原包装。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
    ///
    /// let (reader, _writer) = ring_buffer::<u8>(4, ReadMode::Unlocked, WriteMode::Locked);
    /// assert!(reader.into_unlocked().is_ok());
    /// ```
    pub fn into_unlocked(self) -> Result<UnlockedReader<T>, Self> {
        match self { Reader::Unlocked(r) => Ok(r), other => Err(other) }
    }

    /// 是否承载有锁读端。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
    ///
    /// let (reader, _writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Locked);
    /// assert!(reader.is_locked());
    /// ```
    pub fn is_locked(&self) -> bool { matches!(self, Reader::Locked(_)) }

    /// 是否承载无锁读端。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
    ///
    /// let (reader, _writer) = ring_buffer::<u8>(4, ReadMode::Unlocked, WriteMode::Locked);
    /// assert!(reader.is_unlocked());
    /// ```
    pub fn is_unlocked(&self) -> bool { matches!(self, Reader::Unlocked(_)) }
}