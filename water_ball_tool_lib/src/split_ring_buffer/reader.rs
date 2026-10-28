use std::cell::Cell;
use std::marker::PhantomData;
use std::ops::Range;
use std::sync::Arc;

use crate::split_ring_buffer::error::Error;
use crate::split_ring_buffer::inner::{Inner, lock_ignore_poison, wait_ignore_poison};

/// 有锁读端。
///
/// `Send + Sync + Clone`（`T: Send` 时）；方法使用 `&self`，内部通过读端锁
/// 串行化同端多个 clone 的读指针推进。
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
pub struct LockedReader<T> {
    inner: Arc<Inner<T>>,
}

impl<T> LockedReader<T> {
    pub(crate) fn new(inner: Arc<Inner<T>>) -> Self {
        inner.read.inc_count();
        Self { inner }
    }

    /// 非阻塞弹出单个元素。空或 `clear` / `truncate` 进行中返回 `None`。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<String>(4);
    /// assert_eq!(reader.pop(), None);
    /// assert!(writer.push(String::from("a")).is_ok());
    /// assert_eq!(reader.pop().as_deref(), Some("a"));
    /// ```
    pub fn pop(&self) -> Option<T> {
        let _g = lock_ignore_poison(&self.inner.read.lock);
        self.inner.pop_one()
    }

    /// 等待至少一个元素后弹出。
    ///
    /// 对端关闭且无数据返回 `CannotWait`；`clear` / `truncate` 进行中会等待
    /// 其完成。
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
                if let Some(v) = self.inner.pop_one() {
                    return Ok(v);
                }
            }
            if self.inner.write.is_closed() {
                return Err(Error::cannot_wait());
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.enter_wait();
            if self.inner.write.is_closed() {
                self.inner.read.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.reader_should_wait() {
                self.inner.read.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.read.cvar, g);
            self.inner.read.leave_wait();
            drop(g);
        }
    }

    /// 尝试获取读端锁并弹出。获取锁失败返回 `Err(Locked)`；空或 `clear` /
    /// `truncate` 中返回 `Ok(None)`。
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
        let _g = self
            .inner
            .read
            .lock
            .try_lock()
            .map_err(|_| Error::locked())?;
        Ok(self.inner.pop_one())
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
    /// assert!(writer.push(1).is_ok());
    /// assert_eq!(reader.len(), 1);
    /// ```
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
    /// let (reader, _writer) = locked_ring_buffer::<u8>(4);
    /// assert!(reader.is_empty());
    /// ```
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
    /// let (reader, writer) = locked_ring_buffer::<u8>(2);
    /// assert!(writer.push(1).is_ok());
    /// assert!(writer.push(2).is_ok());
    /// assert!(reader.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }

    /// 取走 `range` 内元素，返回 `Vec<T>`。
    ///
    /// 只接受前缀 `0..k` 与全量 `0..len`；`start != 0` 返回 `InvalidRange`。
    /// 后缀截断请用 [`crate::split_ring_buffer::LockedWriter::truncate_back`]。
    /// `clear` / `truncate` 进行中返回空 `Vec`。持读端锁直到完成。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<String>(4);
    /// writer.push(String::from("a")).expect("push");
    /// writer.push(String::from("b")).expect("push");
    /// let v = reader.drain(0..1).expect("drain");
    /// assert_eq!(v, vec![String::from("a")]);
    /// assert_eq!(reader.len(), 1);
    /// ```
    pub fn drain(&self, range: Range<usize>) -> Result<Vec<T>, Error<T>> {
        if range.start != 0 {
            return Err(Error::invalid_range());
        }
        let _r = lock_ignore_poison(&self.inner.read.lock);
        let Some(_flight) = self.inner.enter() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        self.inner.drain_prefix(&mut out, range.end);
        Ok(out)
    }

    /// 释放 `range` 内元素，不返回。区间约束与 [`drain`](Self::drain) 相同。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
    ///
    /// let (reader, writer) = locked_ring_buffer::<String>(4);
    /// writer.push(String::from("a")).expect("push");
    /// reader.release(0..1).expect("release");
    /// assert_eq!(reader.len(), 0);
    /// ```
    pub fn release(&self, range: Range<usize>) -> Result<(), Error<T>> {
        if range.start != 0 {
            return Err(Error::invalid_range());
        }
        let _r = lock_ignore_poison(&self.inner.read.lock);
        let Some(_flight) = self.inner.enter() else {
            return Ok(());
        };
        self.inner.release_prefix(range.end);
        Ok(())
    }
}

impl<T: Copy> LockedReader<T> {
    /// 从缓冲区复制数据到 `dst`，返回实际复制的元素数。仅 `T: Copy` 可用。
    ///
    /// 无可读数据或 `clear` / `truncate` 中返回 `0`；`dst` 为空返回 `0`。
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
        if dst.is_empty() {
            return 0;
        }
        let _g = lock_ignore_poison(&self.inner.read.lock);
        self.inner.read_slice(dst)
    }

    /// 等待可读数据后复制到 `dst`。仅 `T: Copy` 可用。
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
        if dst.is_empty() {
            return Ok(0);
        }
        loop {
            {
                let _g = lock_ignore_poison(&self.inner.read.lock);
                let n = self.inner.read_slice(dst);
                if n > 0 {
                    return Ok(n);
                }
            }
            if self.inner.write.is_closed() {
                return Err(Error::cannot_wait());
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.enter_wait();
            if self.inner.write.is_closed() {
                self.inner.read.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.reader_should_wait() {
                self.inner.read.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.read.cvar, g);
            self.inner.read.leave_wait();
            drop(g);
        }
    }

    /// 阻塞直到 `dst` 填满。仅 `T: Copy` 可用。
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
        if dst.is_empty() {
            return Ok(());
        }
        let _g = lock_ignore_poison(&self.inner.read.lock);
        let mut filled = 0;
        while filled < dst.len() {
            let n = self.inner.read_slice(&mut dst[filled..]);
            if n > 0 {
                filled += n;
                continue;
            }
            if self.inner.write.is_closed() {
                return Err(Error::cannot_wait());
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.enter_wait();
            if self.inner.write.is_closed() {
                self.inner.read.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.reader_should_wait() {
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

    /// 尝试获取读端锁并读取。仅 `T: Copy` 可用。
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
        if dst.is_empty() {
            return Ok(0);
        }
        let _g = self
            .inner
            .read
            .lock
            .try_lock()
            .map_err(|_| Error::locked())?;
        Ok(self.inner.read_slice(dst))
    }
}

impl<T> Clone for LockedReader<T> {
    fn clone(&self) -> Self {
        self.inner.read.inc_count();
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Drop for LockedReader<T> {
    fn drop(&mut self) {
        self.inner.close_reader();
    }
}

/// 无锁读端。
///
/// `Send`、`!Sync`、`!Clone`（`T: Send` 时 `Send`）；方法使用 `&mut self`。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
///
/// let (mut reader, mut writer) = unlocked_ring_buffer::<String>(4);
/// assert!(writer.push(String::from("x")).is_ok());
/// assert_eq!(reader.pop().as_deref(), Some("x"));
/// ```
pub struct UnlockedReader<T> {
    inner: Arc<Inner<T>>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T> UnlockedReader<T> {
    pub(crate) fn new(inner: Arc<Inner<T>>) -> Self {
        inner.read.inc_count();
        Self {
            inner,
            _not_sync: PhantomData,
        }
    }

    /// 非阻塞弹出单个元素。空或 `clear` / `truncate` 中返回 `None`。
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
    pub fn pop(&mut self) -> Option<T> {
        self.inner.pop_one()
    }

    /// 等待至少一个元素后弹出。
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
            if let Some(v) = self.inner.pop_one() {
                return Ok(v);
            }
            if self.inner.write.is_closed() {
                return Err(Error::cannot_wait());
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.enter_wait();
            if self.inner.write.is_closed() {
                self.inner.read.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.reader_should_wait() {
                self.inner.read.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.read.cvar, g);
            self.inner.read.leave_wait();
            drop(g);
        }
    }

    /// 当前可读元素数。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    /// assert_eq!(reader.len(), 0);
    /// assert!(writer.push(1).is_ok());
    /// assert_eq!(reader.len(), 1);
    /// ```
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
    /// let (mut reader, _writer) = unlocked_ring_buffer::<u8>(4);
    /// assert!(reader.is_empty());
    /// ```
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
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(2);
    /// assert!(writer.push(1).is_ok());
    /// assert!(writer.push(2).is_ok());
    /// assert!(reader.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }

    /// 取走 `range` 内元素。区间约束同 [`LockedReader::drain`]。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<String>(4);
    /// writer.push(String::from("a")).expect("push");
    /// let v = reader.drain(0..1).expect("drain");
    /// assert_eq!(v, vec![String::from("a")]);
    /// ```
    pub fn drain(&mut self, range: Range<usize>) -> Result<Vec<T>, Error<T>> {
        if range.start != 0 {
            return Err(Error::invalid_range());
        }
        let Some(_flight) = self.inner.enter() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        self.inner.drain_prefix(&mut out, range.end);
        Ok(out)
    }

    /// 释放 `range` 内元素。区间约束同 [`LockedReader::release`]。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::unlocked_ring_buffer;
    ///
    /// let (mut reader, mut writer) = unlocked_ring_buffer::<String>(4);
    /// writer.push(String::from("a")).expect("push");
    /// reader.release(0..1).expect("release");
    /// assert_eq!(reader.len(), 0);
    /// ```
    pub fn release(&mut self, range: Range<usize>) -> Result<(), Error<T>> {
        if range.start != 0 {
            return Err(Error::invalid_range());
        }
        let Some(_flight) = self.inner.enter() else {
            return Ok(());
        };
        self.inner.release_prefix(range.end);
        Ok(())
    }
}

impl<T: Copy> UnlockedReader<T> {
    /// 从缓冲区复制数据到 `dst`。仅 `T: Copy` 可用。
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
        if dst.is_empty() {
            return 0;
        }
        self.inner.read_slice(dst)
    }

    /// 等待可读数据后复制到 `dst`。仅 `T: Copy` 可用。
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
        if dst.is_empty() {
            return Ok(0);
        }
        loop {
            let n = self.inner.read_slice(dst);
            if n > 0 {
                return Ok(n);
            }
            if self.inner.write.is_closed() {
                return Err(Error::cannot_wait());
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.enter_wait();
            if self.inner.write.is_closed() {
                self.inner.read.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.reader_should_wait() {
                self.inner.read.leave_wait();
                drop(g);
                continue;
            }
            g = wait_ignore_poison(&self.inner.read.cvar, g);
            self.inner.read.leave_wait();
            drop(g);
        }
    }

    /// 阻塞直到 `dst` 填满。仅 `T: Copy` 可用。
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
        if dst.is_empty() {
            return Ok(());
        }
        let mut filled = 0;
        while filled < dst.len() {
            let n = self.inner.read_slice(&mut dst[filled..]);
            if n > 0 {
                filled += n;
                continue;
            }
            if self.inner.write.is_closed() {
                return Err(Error::cannot_wait());
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
            self.inner.read.enter_wait();
            if self.inner.write.is_closed() {
                self.inner.read.leave_wait();
                return Err(Error::cannot_wait());
            }
            if !self.inner.reader_should_wait() {
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
}

impl<T> Drop for UnlockedReader<T> {
    fn drop(&mut self) {
        self.inner.close_reader();
    }
}

/// 读端枚举包装，仅承载有锁/无锁实例，不代理任何操作。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
///
/// let (reader, _writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Locked);
/// let reader = match reader.into_locked() {
///     Ok(r) => r,
///     Err(_) => panic!("expected Locked variant"),
/// };
/// assert!(reader.is_empty());
/// ```
pub enum Reader<T> {
    /// 有锁读端实例。
    Locked(LockedReader<T>),
    /// 无锁读端实例。
    Unlocked(UnlockedReader<T>),
}

impl<T> Reader<T> {
    /// 若为 `Locked`，返回其具体类型；否则返回原包装。
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
        match self {
            Reader::Locked(r) => Ok(r),
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
    /// let (reader, _writer) = ring_buffer::<u8>(4, ReadMode::Unlocked, WriteMode::Locked);
    /// assert!(reader.into_unlocked().is_ok());
    /// ```
    pub fn into_unlocked(self) -> Result<UnlockedReader<T>, Self> {
        match self {
            Reader::Unlocked(r) => Ok(r),
            other => Err(other),
        }
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
    pub fn is_locked(&self) -> bool {
        matches!(self, Reader::Locked(_))
    }

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
    pub fn is_unlocked(&self) -> bool {
        matches!(self, Reader::Unlocked(_))
    }
}
