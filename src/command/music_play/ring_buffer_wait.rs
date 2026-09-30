use std::collections::VecDeque;
use std::error::Error;
use std::fmt::Debug;
use std::io;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

// 等待版本的环形缓冲区
pub struct RingBufferWait<T> {
    buffer: Arc<Mutex<VecDeque<T>>>,
    push_condvar: Arc<Condvar>,
    pop_condvar: Arc<Condvar>,
    max_len: Arc<AtomicUsize>,
    is_max_len: Arc<AtomicBool>,
    object_size: Arc<AtomicUsize>,
}
impl<T> Clone for RingBufferWait<T> {
    fn clone(&self) -> Self {
        self.object_size.fetch_add(1, Ordering::Relaxed);
        Self {
            buffer: self.buffer.clone(),
            push_condvar: self.push_condvar.clone(),
            pop_condvar: self.pop_condvar.clone(),
            max_len: self.max_len.clone(),
            is_max_len: self.is_max_len.clone(),
            object_size: self.object_size.clone(),
        }
    }
}
impl<T> Drop for RingBufferWait<T> {
    fn drop(&mut self) {
        let old = self.object_size.fetch_sub(1, Ordering::Relaxed);
        if old > 2 {
            return;
        }
        self.pop_condvar.notify_one();
        self.push_condvar.notify_one();
    }
}

impl<T> RingBufferWait<T> {
    fn new_inner(vec_deque: VecDeque<T>, is_max_len: bool, max_len: usize) -> Self {
        Self {
            object_size: Arc::new(AtomicUsize::new(1)),
            buffer: Arc::new(Mutex::new(vec_deque)),
            push_condvar: Arc::new(Condvar::new()),
            pop_condvar: Arc::new(Condvar::new()),
            max_len: Arc::new(AtomicUsize::new(max_len)),
            is_max_len: Arc::new(AtomicBool::new(is_max_len)),
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
        if self.is_max_len.load(Ordering::SeqCst) {
            loop {
                if buffer.len() > self.max_len.load(Ordering::Relaxed) {
                    //可挂起性检查
                    if self.object_size.load(Ordering::Relaxed) == 1 {
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
                    if buffer.len() > self.max_len.load(Ordering::Relaxed) {
                        continue;
                    }
                }
                break;
            }
        }
        inner_fn(&mut buffer, value);
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
        loop {
            if buffer.is_empty() {
                //可挂起性检查
                if self.object_size.load(Ordering::Relaxed) == 1 {
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
                if buffer.is_empty() {
                    continue;
                }
            }
            match inner_fn(&mut buffer) {
                Some(x) => {
                    self.push_condvar.notify_one();
                    break Ok(x);
                }
                None => unreachable!("逻辑错误"),
            }
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
        if self.is_max_len.load(Ordering::SeqCst) {
            let this_len = buffer.len();
            if this_len > self.max_len.load(Ordering::SeqCst) {
                //报错
                return Err(RingBufferWaitError::new(
                    RingBufferWaitErrorKind::MaxLen,
                    "环形缓冲区已满",
                    Some(value),
                ));
            }
        }
        inner_fn(&mut buffer, value);
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
        //缓冲区数量检查
        self.push_condvar.notify_one();
        inner_fn(&mut buffer)
    }
}

impl<T> RingBufferWait<T> {
    pub fn new() -> Self {
        Self::new_inner(VecDeque::new(), false, 0)
    }

    pub fn new_max_len(len: usize) -> Self {
        Self::new_inner(VecDeque::with_capacity(len), true, len)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::new_inner(VecDeque::with_capacity(capacity), false, 0)
    }

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
}

impl<T: Copy> RingBufferWait<T> /*Write*/ {
    pub fn write(&mut self, buf: &[T]) -> RingBufferWaitResult<usize, T> {
        let mut buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.is_max_len.load(Ordering::Relaxed)
            && buffer.len() + buf.len() > self.max_len.load(Ordering::Relaxed)
        {
            //占满挂起
            loop {
                if buffer.len() >= self.max_len.load(Ordering::Relaxed) {
                    // 可挂起检查
                    if self.object_size.load(Ordering::Relaxed) == 1 {
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
                }
                let write_len = if buffer.len() + buf.len() > self.max_len.load(Ordering::Relaxed) {
                    let len = self
                        .max_len
                        .load(Ordering::Relaxed)
                        .saturating_sub(buffer.len());
                    if len == 0 {
                        continue;
                    }
                    len
                } else {
                    buf.len()
                };
                for v in &buf[..write_len] {
                    buffer.push_back(*v);
                }
                return Ok(write_len);
            }
        }
        for v in buf {
            buffer.push_back(*v);
        }
        self.pop_condvar.notify_one();
        Ok(buf.len())
    }

    fn write_all(&mut self, buf: &[T]) -> RingBufferWaitResult<(), T> {
        let mut buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.is_max_len.load(Ordering::Relaxed) {
            loop {
                if buffer.len() + buf.len() > self.max_len.load(Ordering::Relaxed) {
                    // 可挂起检查
                    if self.object_size.load(Ordering::Relaxed) == 1 {
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
        }
        for v in buf {
            buffer.push_back(*v);
        }
        self.pop_condvar.notify_one();
        Ok(())
    }
}

impl<T: Copy> RingBufferWait<T> {
    pub(crate) fn read(&mut self, buf: &mut [T]) -> RingBufferWaitResult<usize, ()> {
        let mut buffer = self
            .buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            // 为空挂起
            if buffer.is_empty() {
                // 可挂起检查
                if self.object_size.load(Ordering::Relaxed) == 1 {
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
        let mut this_read_len = 0;
        for _ in 0..read_len {
            match buffer.pop_front() {
                Some(v) => {
                    buf[this_read_len] = v;
                    this_read_len += 1;
                }
                None => return Ok(this_read_len),
            }
        }
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
                if self.object_size.load(Ordering::Relaxed) == 1 {
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
                None => unreachable!("逻辑错误"),
            }
        }
        self.push_condvar.notify_one();
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
        Self::new_inner(value, false, 0)
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

impl<T: std::fmt::Debug + Sync + Send + 'static> From<RingBufferWaitError<T>> for io::Error {
    fn from(value: RingBufferWaitError<T>) -> Self {
        Self::other(value)
    }
}

impl<T> std::fmt::Display for RingBufferWaitError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.error_kind {
            RingBufferWaitErrorKind::MaxLen => "MaxLen",
            RingBufferWaitErrorKind::OneObjectWait => "OneObjectWait",
            RingBufferWaitErrorKind::ReadSizeTooLarge => "ReadSizeTooLarge",
        };
        write!(
            f,
            "RingBufferError: Kind: {kind}, message: {}",
            self.message
        )
    }
}

#[derive(Debug)]
pub enum RingBufferWaitErrorKind {
    MaxLen,
    OneObjectWait,
    ReadSizeTooLarge,
}
