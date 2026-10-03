use std::collections::{TryReserveError, VecDeque};
use std::error::Error;
use std::fmt::Debug;
use std::io;
use std::io::{Read, Write};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

// 等待版本的环形缓冲区
pub struct RingBufferWait<T> {
    buffer: Arc<Mutex<VecDeque<T>>>,
    push_condvar: Arc<Condvar>,
    pop_condvar: Arc<Condvar>,
    max_len: Arc<AtomicUsize>,
    is_max_len: Arc<AtomicBool>,
    object_size: Arc<AtomicUsize>,
    len: Arc<AtomicUsize>,
}
impl<T> Clone for RingBufferWait<T> {
    fn clone(&self) -> Self {
        self.object_size.fetch_add(1, Ordering::Release);
        Self {
            buffer: self.buffer.clone(),
            push_condvar: self.push_condvar.clone(),
            pop_condvar: self.pop_condvar.clone(),
            max_len: self.max_len.clone(),
            is_max_len: self.is_max_len.clone(),
            object_size: self.object_size.clone(),
            len: self.len.clone(),
        }
    }
}
impl<T> Drop for RingBufferWait<T> {
    fn drop(&mut self) {
        if self.object_size.fetch_sub(1, Ordering::AcqRel) != 2 {
            return;
        }
        self.pop_condvar.notify_all();
        self.push_condvar.notify_all();
    }
}

impl<T> RingBufferWait<T> {
    fn new_inner(vec_deque: VecDeque<T>, len: usize) -> Self {
        Self {
            object_size: Arc::new(AtomicUsize::new(1)),
            buffer: Arc::new(Mutex::new(vec_deque)),
            push_condvar: Arc::new(Condvar::new()),
            pop_condvar: Arc::new(Condvar::new()),
            max_len: Arc::new(AtomicUsize::new(0)),
            is_max_len: Arc::new(AtomicBool::new(false)),
            len: Arc::new(AtomicUsize::new(len)),
        }
    }

    // 占满挂起实现
    fn push_inner_fn<F>(&mut self, value: T, inner_fn: F) -> RingBufferWaitResult<(), T>
    where
        F: FnOnce(&mut VecDeque<T>, T),
    {
        let mut buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        //缓冲区数量检查
        while self.is_max_len.load(Ordering::Relaxed) {
            if buffer.len() >= self.max_len.load(Ordering::Relaxed) {
                //可挂起性检查
                if self.object_size.load(Ordering::Acquire) == 1 {
                    return Err(RingBufferWaitError::new(
                        RingBufferWaitErrorKind::OneObjectWait,
                        "环形缓冲区只有一个共享对象，无法等待",
                        Some(value),
                    ));
                }
                //挂起
                buffer = self
                    .push_condvar
                    .wait(buffer)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            }
            break;
        }
        inner_fn(&mut buffer, value);
        self.len.store(buffer.len(), Ordering::Release);
        self.pop_condvar.notify_one(); //唤醒
        Ok(())
    }

    // 为空挂起实现
    fn pop_inner_fn<F>(&mut self, inner_fn: F) -> RingBufferWaitResult<T, ()>
    where
        F: FnOnce(&mut VecDeque<T>) -> Option<T>,
    {
        let mut buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        //缓冲区数量检查
        while buffer.is_empty() {
            //可挂起性检查
            if self.object_size.load(Ordering::Acquire) == 1 {
                Err(RingBufferWaitError::new(
                    RingBufferWaitErrorKind::OneObjectWait,
                    "环形缓冲区只有一个共享对象，无法等待",
                    None,
                ))?;
            }
            // 挂起
            buffer = self
                .pop_condvar
                .wait(buffer)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        match inner_fn(&mut buffer) {
            Some(x) => {
                self.push_condvar.notify_one();
                self.len.store(buffer.len(), Ordering::Release);
                Ok(x)
            }
            None => unreachable!("逻辑错误"),
        }
    }

    //尝试增加内部实现
    fn try_push_inner_fn<F>(&mut self, value: T, inner_fn: F) -> RingBufferWaitResult<(), T>
    where
        F: FnOnce(&mut VecDeque<T>, T),
    {
        // 数量检查
        let mut buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        //缓冲区数量检查
        if self.is_max_len.load(Ordering::Relaxed)
            && buffer.len() >= self.max_len.load(Ordering::Relaxed)
        {
            //报错
            return Err(RingBufferWaitError::new(
                RingBufferWaitErrorKind::MaxLen,
                "环形缓冲区已满",
                Some(value),
            ));
        }
        inner_fn(&mut buffer, value);
        self.len.store(buffer.len(), Ordering::Release);
        self.pop_condvar.notify_one(); //唤醒
        Ok(())
    }

    // 尝试获取实现
    fn try_pop_inner_fn<F>(&mut self, inner_fn: F) -> Option<T>
    where
        F: FnOnce(&mut VecDeque<T>) -> Option<T>,
    {
        let mut buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let r = inner_fn(&mut buffer);
        self.len.store(buffer.len(), Ordering::Release);
        if r.is_some() {
            self.push_condvar.notify_one();
        }
        r
    }
}
impl<T> RingBufferWait<T> /*VecDeque*/ {
    pub fn pop_front(&mut self) -> RingBufferWaitResult<T, ()> {
        self.pop_inner_fn(VecDeque::pop_front)
    }

    pub fn pop_back(&mut self) -> RingBufferWaitResult<T, ()> {
        self.pop_inner_fn(VecDeque::pop_back)
    }
    pub fn push_back(&mut self, value: T) -> RingBufferWaitResult<(), T> {
        self.push_inner_fn(value, VecDeque::push_back)
    }

    pub fn push_front(&mut self, value: T) -> RingBufferWaitResult<(), T> {
        self.push_inner_fn(value, VecDeque::push_front)
    }

    pub fn get_item(&self, index: usize) -> Option<RingBufferWaitItemGuard<'_, T>> {
        let buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if index >= buffer.len() {
            None
        } else {
            Some(RingBufferWaitItemGuard {
                guard: buffer,
                index,
            })
        }
    }
}

impl<T> RingBufferWait<T> {
    pub fn new() -> Self {
        Self::new_inner(VecDeque::new(), 0)
    }

    pub fn new_max_len(len: usize) -> RingBufferWaitResult<Self, ()> {
        let mut s = Self::new_inner(VecDeque::new(), 0);
        s.set_max_len(len)?.on_max_len()?;
        Ok(s)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::new_inner(VecDeque::with_capacity(capacity), 0)
    }

    pub fn is_empty(&self) -> bool {
        self.len.load(Ordering::Acquire) == 0
    }

    pub fn is_full(&self) -> bool {
        self.is_max_len.load(Ordering::Relaxed)
            && self.len.load(Ordering::Acquire) >= self.max_len.load(Ordering::Relaxed)
    }

    pub fn set_max_len(&mut self, max_len: usize) -> RingBufferWaitResult<&mut Self, ()> {
        if max_len == 0 {
            return Err(RingBufferWaitError::new(
                RingBufferWaitErrorKind::MaxLenZero,
                "最大长度不能为零",
                None,
            ));
        }
        let mut buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let buffer_len = buffer.len();
        if max_len > buffer_len {
            buffer.try_reserve_exact(max_len - buffer_len)?;
        }
        self.max_len.store(max_len, Ordering::Release);
        if max_len > buffer_len {
            //唤醒挂起的生产者线程
            self.push_condvar.notify_all();
        }
        drop(buffer);
        Ok(self)
    }

    pub fn on_max_len(&mut self) -> RingBufferWaitResult<(), ()> {
        self.set_max_len_enabled(true)
    }

    pub fn off_max_len(&mut self) {
        let _ = self.set_max_len_enabled(false);
        self.push_condvar.notify_all();
    }

    pub fn set_max_len_enabled(&mut self, enabled: bool) -> RingBufferWaitResult<(), ()> {
        if enabled && self.max_len.load(Ordering::Relaxed) == 0 {
            return Err(RingBufferWaitError::new(
                RingBufferWaitErrorKind::MaxLenZero,
                "尚未设置有效的最大长度，请先调用 set_max_len",
                None,
            ));
        }
        self.is_max_len.store(enabled, Ordering::Release);
        Ok(())
    }
}

impl<T: Copy> RingBufferWait<T> /*Write*/ {
    pub fn write(&mut self, buf: &[T]) -> RingBufferWaitResult<usize, T> {
        let mut buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while self.is_max_len.load(Ordering::Relaxed) {
            if buffer.len() >= self.max_len.load(Ordering::Relaxed) {
                // 可挂起检查
                if self.object_size.load(Ordering::Acquire) == 1 {
                    return Err(RingBufferWaitError::new(
                        RingBufferWaitErrorKind::OneObjectWait,
                        "环形缓冲区只有一个共享对象，无法等待",
                        None,
                    ));
                }
                //挂起
                buffer = self
                    .push_condvar
                    .wait(buffer)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            }
            break;
        }

        let write_len = if self.is_max_len.load(Ordering::Relaxed) {
            self.max_len
                .load(Ordering::Relaxed)
                .checked_sub(buffer.len())
                .map(|n| n.min(buf.len()))
        } else {
            Some(buf.len())
        };
        let Some(write_len) = write_len else {
            unreachable!("逻辑错误，或发生竟态")
        };
        for v in &buf[..write_len] {
            buffer.push_back(*v);
        }
        self.pop_condvar.notify_one();
        self.len.store(buffer.len(), Ordering::Release);
        Ok(write_len)
    }

    pub(crate) fn write_all(&mut self, buf: &[T]) -> RingBufferWaitResult<(), T> {
        let mut buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while self.is_max_len.load(Ordering::Relaxed) {
            if buf.len() > self.max_len.load(Ordering::Relaxed) {
                return Err(RingBufferWaitError::new(
                    RingBufferWaitErrorKind::WriteSizeTooLarge,
                    &format!(
                        "写入的大小过大, buffer max len: {}, buf len: {}",
                        self.max_len.load(Ordering::Relaxed),
                        buf.len()
                    ),
                    None,
                ));
            }
            let total = buffer.len().checked_add(buf.len()).ok_or_else(|| {
                RingBufferWaitError::new(
                    RingBufferWaitErrorKind::LenOverflow {
                        buffer_len: buffer.len(),
                        buf_len: buf.len(),
                    },
                    "buffer.len() + buf.len() 溢出 usize",
                    None,
                )
            })?;
            if total > self.max_len.load(Ordering::Relaxed) {
                // 可挂起检查
                if self.object_size.load(Ordering::Acquire) == 1 {
                    Err(RingBufferWaitError::new(
                        RingBufferWaitErrorKind::OneObjectWait,
                        "环形缓冲区只有一个共享对象，无法等待",
                        None,
                    ))?;
                }
                //挂起
                buffer = self
                    .push_condvar
                    .wait(buffer)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            }
            break;
        }
        for v in buf {
            buffer.push_back(*v);
        }
        self.len.store(buffer.len(), Ordering::Release);
        self.pop_condvar.notify_all();
        Ok(())
    }
}

impl<T: Copy> RingBufferWait<T> /*Read*/ {
    pub(crate) fn read(&mut self, buf: &mut [T]) -> RingBufferWaitResult<usize, ()> {
        let mut buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            // 为空挂起
            if buffer.is_empty() {
                // 可挂起检查
                if self.object_size.load(Ordering::Acquire) == 1 {
                    Err(RingBufferWaitError::new(
                        RingBufferWaitErrorKind::OneObjectWait,
                        "环形缓冲区只有一个共享对象，无法等待",
                        None,
                    ))?;
                }
                //挂起
                buffer = self
                    .pop_condvar
                    .wait(buffer)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            }
            break;
        }
        let read_len = buf.len().min(buffer.len());
        for i in buf.iter_mut().take(read_len) {
            if let Some(v) = buffer.pop_front() {
                *i = v;
            } else {
                unreachable!("逻辑错误，或发生不当的竟态");
            }
        }
        self.len.store(buffer.len(), Ordering::Release);
        self.push_condvar.notify_one();
        Ok(read_len)
    }

    pub fn read_exact(&mut self, buf: &mut [T]) -> RingBufferWaitResult<(), ()> {
        let mut buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.is_max_len.load(Ordering::Relaxed)
            && buf.len() > self.max_len.load(Ordering::Relaxed)
        {
            return Err(RingBufferWaitError::new(
                RingBufferWaitErrorKind::ReadSizeTooLarge,
                &format!(
                    "读取的大小过大, buffer max len: {}, buf len: {}",
                    self.max_len.load(Ordering::Relaxed),
                    buf.len()
                ),
                None,
            ));
        }
        //不满足挂起直到满足
        loop {
            if buffer.len() < buf.len() {
                // 可挂起检查
                if self.object_size.load(Ordering::Acquire) == 1 {
                    return Err(RingBufferWaitError::new(
                        RingBufferWaitErrorKind::OneObjectWait,
                        "环形缓冲区只有一个共享对象，无法等待",
                        None,
                    ));
                }
                //挂起
                buffer = self
                    .pop_condvar
                    .wait(buffer)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            }
            break;
        }
        for i in buf {
            match buffer.pop_front() {
                Some(v) => {
                    *i = v;
                }
                None => unreachable!("逻辑错误，或发生不当的竟态"),
            }
        }
        self.len.store(buffer.len(), Ordering::Release);
        self.push_condvar.notify_all();
        Ok(())
    }
}

impl Write for RingBufferWait<u8> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(self.write(buf)?)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        Ok(self.write_all(buf)?)
    }
}

impl Read for RingBufferWait<u8> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Ok(self.read(buf)?)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        Ok(self.read_exact(buf)?)
    }
}

impl<T> From<VecDeque<T>> for RingBufferWait<T> {
    fn from(value: VecDeque<T>) -> Self {
        let len = value.len();
        Self::new_inner(value, len)
    }
}

type RingBufferWaitResult<T, O> = Result<T, RingBufferWaitError<O>>;

#[derive(Debug)]
pub struct RingBufferWaitError<T> {
    error_kind: RingBufferWaitErrorKind,
    message: String,
    value: Option<T>,
}
impl<T> RingBufferWaitError<T> {
    fn new(error_kind: RingBufferWaitErrorKind, message: &str, value: Option<T>) -> Self {
        Self {
            error_kind,
            message: message.to_string(),
            value,
        }
    }
}
impl<T> RingBufferWaitError<T> {
    pub fn kind(&self) -> &RingBufferWaitErrorKind {
        &self.error_kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn get_value(&mut self) -> Option<T> {
        self.value.take()
    }
}

impl<T: std::fmt::Debug> Error for RingBufferWaitError<T> {}

impl<T> std::fmt::Display for RingBufferWaitError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.error_kind {
            RingBufferWaitErrorKind::MaxLen => "MaxLen",
            RingBufferWaitErrorKind::MaxLenZero => "MaxLenZero",
            RingBufferWaitErrorKind::OneObjectWait => "OneObjectWait",
            RingBufferWaitErrorKind::ReadSizeTooLarge => "ReadSizeTooLarge",
            RingBufferWaitErrorKind::WriteSizeTooLarge => "WriteSizeTooLarge",
            RingBufferWaitErrorKind::TryReserveError(_) => "TryReserveError",
            RingBufferWaitErrorKind::LenOverflow {
                buffer_len,
                buf_len,
            } => &format!("LenOverflow {{ buffer_len : {buffer_len}, buf_len : {buf_len} }} "),
        };
        write!(
            f,
            "RingBufferError: Kind: {kind}, message: {}",
            self.message
        )
    }
}

impl<T: std::fmt::Debug + Sync + Send + 'static> From<RingBufferWaitError<T>> for io::Error {
    fn from(value: RingBufferWaitError<T>) -> Self {
        Self::other(value)
    }
}

impl<T> From<TryReserveError> for RingBufferWaitError<T> {
    fn from(value: TryReserveError) -> Self {
        let message = &format!("{value:?}");
        Self::new(
            RingBufferWaitErrorKind::TryReserveError(value),
            message,
            None,
        )
    }
}

#[derive(Debug)]
pub enum RingBufferWaitErrorKind {
    MaxLen,
    MaxLenZero,
    OneObjectWait,
    ReadSizeTooLarge,
    WriteSizeTooLarge,
    TryReserveError(TryReserveError),
    LenOverflow { buffer_len: usize, buf_len: usize },
}

pub struct RingBufferWaitItemGuard<'a, T> {
    guard: MutexGuard<'a, VecDeque<T>>,
    index: usize,
}

impl<T> Deref for RingBufferWaitItemGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        if let Some(value) = self.guard.get(self.index) {
            value
        } else {
            unreachable!("逻辑错误：创建了下标不存在对象的RingBufferWaitItemGuard")
        }
    }
}

impl<'a, T> DerefMut for RingBufferWaitItemGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        if let Some(value) = self.guard.get_mut(self.index) {
            value
        } else {
            unreachable!("逻辑错误：创建了下标不存在对象的RingBufferWaitItemGuard")
        }
    }
}
