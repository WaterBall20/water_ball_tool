//! `split_ring_buffer` —— 读写分离、可搭配有锁/无锁、可等待的环形缓冲区。
//!
//! 读端与写端各自独立可选有锁或无锁，共享同一个内部环形缓冲区。当前仅实现
//! `T: Copy` 类型。
//!
//! # 模式
//!
//! - 有锁端：`Send + Sync + Clone`，方法 `&self`，内部加锁，提供 `try_*`。
//! - 无锁端：`Send`、`!Sync`、`!Clone`，方法 `&mut self`，仅等待时加锁。
//!
//! # 示例
//!
//! ```
//! use water_ball_tool_lib::split_ring_buffer::locked_ring_buffer;
//!
//! let (reader, writer) = locked_ring_buffer::<u8>(4);
//! assert!(writer.push(1).is_ok());
//! assert_eq!(reader.pop(), Some(1));
//! ```

pub mod error;
mod inner;
pub mod mode;
mod reader;
#[cfg(test)]
mod test;
mod writer;

pub use error::{Error, Kind};
pub use mode::{ReadMode, WriteMode};
pub use reader::{LockedReader, Reader, UnlockedReader};
pub use writer::{LockedWriter, UnlockedWriter, Writer};

use inner::Inner;
use std::sync::Arc;

fn make_inner<T: Copy>(capacity: usize) -> Arc<Inner<T>> {
    Arc::new(Inner::with_capacity(capacity))
}

/// 按运行时模式构造环形缓冲区，返回 [`Reader`] / [`Writer`] 枚举包装。
///
/// 调用方需通过 [`Reader::into_locked`] / [`Reader::into_unlocked`] 与
/// [`Writer::into_locked`] / [`Writer::into_unlocked`] 解包后使用具体类型。
///
/// # 参数
///
/// - `capacity`：缓冲区容量，必须大于 `0`。
/// - `read_mode`：读取端模式。
/// - `write_mode`：写入端模式。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
///
/// let (reader, writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Unlocked);
/// assert!(reader.is_locked());
/// assert!(writer.is_unlocked());
/// ```
pub fn ring_buffer<T: Copy>(
    capacity: usize,
    read_mode: ReadMode,
    write_mode: WriteMode,
) -> (Reader<T>, Writer<T>) {
    let inner = make_inner::<T>(capacity);
    let reader = match read_mode {
        ReadMode::Locked => Reader::Locked(LockedReader::new(Arc::clone(&inner))),
        ReadMode::Unlocked => Reader::Unlocked(UnlockedReader::new(Arc::clone(&inner))),
    };
    let writer = match write_mode {
        WriteMode::Locked => Writer::Locked(LockedWriter::new(Arc::clone(&inner))),
        WriteMode::Unlocked => Writer::Unlocked(UnlockedWriter::new(inner)),
    };
    (reader, writer)
}

/// 构造有锁读端 + 有锁写端的环形缓冲区。
///
/// # 参数
///
/// - `capacity`：容量，必须大于 `0`。
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
pub fn locked_ring_buffer<T: Copy>(capacity: usize) -> (LockedReader<T>, LockedWriter<T>) {
    let inner = make_inner::<T>(capacity);
    (
        LockedReader::new(Arc::clone(&inner)),
        LockedWriter::new(inner),
    )
}

/// 构造无锁读端 + 无锁写端的环形缓冲区。
///
/// # 参数
///
/// - `capacity`：容量，必须大于 `0`。
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
pub fn unlocked_ring_buffer<T: Copy>(capacity: usize) -> (UnlockedReader<T>, UnlockedWriter<T>) {
    let inner = make_inner::<T>(capacity);
    (
        UnlockedReader::new(Arc::clone(&inner)),
        UnlockedWriter::new(inner),
    )
}

/// 构造有锁读端 + 无锁写端的混合环形缓冲区。
///
/// # 参数
///
/// - `capacity`：容量，必须大于 `0`。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::locked_read_unlocked_write_ring_buffer;
///
/// let (reader, mut writer) = locked_read_unlocked_write_ring_buffer::<u8>(4);
/// assert!(writer.push(1).is_ok());
/// assert_eq!(reader.pop(), Some(1));
/// ```
pub fn locked_read_unlocked_write_ring_buffer<T: Copy>(
    capacity: usize,
) -> (LockedReader<T>, UnlockedWriter<T>) {
    let inner = make_inner::<T>(capacity);
    (
        LockedReader::new(Arc::clone(&inner)),
        UnlockedWriter::new(inner),
    )
}

/// 构造无锁读端 + 有锁写端的混合环形缓冲区。
///
/// # 参数
///
/// - `capacity`：容量，必须大于 `0`。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::unlocked_read_locked_write_ring_buffer;
///
/// let (mut reader, writer) = unlocked_read_locked_write_ring_buffer::<u8>(4);
/// assert!(writer.push(1).is_ok());
/// assert_eq!(reader.pop(), Some(1));
/// ```
pub fn unlocked_read_locked_write_ring_buffer<T: Copy>(
    capacity: usize,
) -> (UnlockedReader<T>, LockedWriter<T>) {
    let inner = make_inner::<T>(capacity);
    (
        UnlockedReader::new(Arc::clone(&inner)),
        LockedWriter::new(inner),
    )
}
