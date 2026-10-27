/// 读取端模式，用于 [`crate::ring_buffer`] 运行时选择读取端类型。
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    /// 有锁读端：`Send + Sync + Clone`，方法使用 `&self`，内部加锁。
    Locked,
    /// 无锁读端：`Send`、`!Sync`、`!Clone`，方法使用 `&mut self`，仅等待时加锁。
    Unlocked,
}

/// 写入端模式，用于 [`crate::ring_buffer`] 运行时选择写入端类型。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::{ring_buffer, ReadMode, WriteMode};
///
/// let (_reader, writer) = ring_buffer::<u8>(4, ReadMode::Unlocked, WriteMode::Locked);
/// assert!(writer.is_locked());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// 有锁写端：`Send + Sync + Clone`，方法使用 `&self`，内部加锁。
    Locked,
    /// 无锁写端：`Send`、`!Sync`、`!Clone`，方法使用 `&mut self`，仅等待时加锁。
    Unlocked,
}