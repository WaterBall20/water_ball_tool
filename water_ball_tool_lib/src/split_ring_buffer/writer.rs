use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::split_ring_buffer::error::{Error, Kind};
use crate::split_ring_buffer::inner::{Inner, lock_ignore_poison, wait_ignore_poison};

fn kind_to_error<T>(k: Kind<T>) -> Error<T> {
    match k {
        Kind::Full(v) => Error::full(v),
        Kind::Clearing(v) => Error::clearing(v),
        Kind::Truncating(v) => Error::truncating(v),
        _ => unreachable!("push_one only returns Full / Clearing / Truncating"),
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
    /// `truncate_back` 中返回 `Err(Truncating(value))`。
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
                match self.inner.push_one(value) {
                    Ok(()) => return Ok(()),
                    Err(Kind::Full(v)) | Err(Kind::Clearing(v)) | Err(Kind::Truncating(v)) => {
                        value = v
                    }
                    Err(_) => unreachable!(),
                }
            }
            if self.inner.read.is_closed() {
                return Err(Error::cannot_wait());
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
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
            g = wait_ignore_poison(&self.inner.write.cvar, g);
            self.inner.write.leave_wait();
            drop(g);
        }
    }

    /// 清空缓冲区：交换 A/B，重置指针，drop 旧 B 中未读元素。
    ///
    /// 与 `truncate_back` 互斥；进行期间阻塞所有读写。并发 `clear` 直接返回。
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

    /// 保留前 `keep` 个元素，丢弃尾部。与 `clear` 互斥，进行期间阻塞所有读写。
    ///
    /// `keep >= len` 时不丢弃任何元素。`clear` 或另一个 `truncate_back` 正在
    /// 进行时直接返回（无操作）。拿写端锁直至完成。
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
        let _w = lock_ignore_poison(&self.inner.write.lock);
        self.inner.truncate_back(keep);
    }

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
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }
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
        loop {
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
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
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
            g = wait_ignore_poison(&self.inner.write.cvar, g);
            self.inner.write.leave_wait();
            drop(g);
        }
    }

    /// 阻塞直到 `src` 全部写入。仅 `T: Copy` 可用。
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
        if src.is_empty() {
            return Ok(());
        }
        let _g = lock_ignore_poison(&self.inner.write.lock);
        let mut written = 0;
        while written < src.len() {
            let n = self.inner.write_slice(&src[written..]);
            if n > 0 {
                written += n;
                continue;
            }
            if self.inner.read.is_closed() {
                return Err(Error::cannot_wait());
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
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
            g = wait_ignore_poison(&self.inner.write.cvar, g);
            self.inner.write.leave_wait();
            drop(g);
        }
        Ok(())
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

    /// 非阻塞写入单个元素。
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
        let mut value = value;
        loop {
            match self.inner.push_one(value) {
                Ok(()) => return Ok(()),
                Err(Kind::Full(v)) | Err(Kind::Clearing(v)) | Err(Kind::Truncating(v)) => value = v,
                Err(_) => unreachable!(),
            }
            if self.inner.read.is_closed() {
                return Err(Error::cannot_wait());
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
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
            g = wait_ignore_poison(&self.inner.write.cvar, g);
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
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }
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
        loop {
            let n = self.inner.write_slice(src);
            if n > 0 {
                return Ok(n);
            }
            if self.inner.read.is_closed() {
                return Err(Error::cannot_wait());
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
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
            g = wait_ignore_poison(&self.inner.write.cvar, g);
            self.inner.write.leave_wait();
            drop(g);
        }
    }

    /// 阻塞直到 `src` 全部写入。仅 `T: Copy` 可用。
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
        if src.is_empty() {
            return Ok(());
        }
        let mut written = 0;
        while written < src.len() {
            let n = self.inner.write_slice(&src[written..]);
            if n > 0 {
                written += n;
                continue;
            }
            if self.inner.read.is_closed() {
                return Err(Error::cannot_wait());
            }
            let mut g = lock_ignore_poison(&self.inner.wait_mutex);
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
            g = wait_ignore_poison(&self.inner.write.cvar, g);
            self.inner.write.leave_wait();
            drop(g);
        }
        Ok(())
    }
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
    pub fn is_unlocked(&self) -> bool {
        matches!(self, Writer::Unlocked(_))
    }
}
