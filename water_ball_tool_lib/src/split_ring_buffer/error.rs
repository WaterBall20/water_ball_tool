use std::borrow::Cow;
use std::fmt;

/// 错误种类，元素类型与缓冲区一致。
///
/// # 变体
///
/// - [`Kind::Locked`]：`try_*` 方法尝试获取端锁失败。
/// - [`Kind::CannotWait`]：等待期间对端 drop / 关闭，无法继续等待。
/// - [`Kind::WriteRejected`]：保留未实现。
/// - [`Kind::Full`]：写入满时归还被拒绝的元素。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::Kind;
///
/// let k: Kind<u8> = Kind::Full(42);
/// match k {
///     Kind::Full(v) => assert_eq!(v, 42),
///     _ => panic!("expected Full"),
/// }
/// ```
pub enum Kind<T> {
    /// `try_*` 方法尝试获取端锁失败。
    Locked,
    /// 等待期间对端 drop / 关闭，无法继续等待。
    CannotWait,
    /// 写入被拒绝，保留未实现。
    WriteRejected,
    /// 缓冲区满，归还未写入的元素。
    Full(T),
}

/// 环形缓冲区统一错误结构。
///
/// # 字段
///
/// - [`Error::message`]：人类可读的错误描述，静态或借用生命周期。
/// - [`Error::kind`]：错误种类，携带与缓冲区同类型的泛型参数。
///
/// # 示例
///
/// ```
/// use water_ball_tool_lib::split_ring_buffer::{Error, Kind};
///
/// let err = Error::full(7u8);
/// assert_eq!(&*err.message, "buffer is full");
/// assert!(matches!(err.kind, Kind::Full(7)));
/// ```
pub struct Error<T> {
    /// 人类可读的错误描述。
    pub message: Cow<'static, str>,
    /// 错误种类。
    pub kind: Kind<T>,
}

impl<T> Error<T> {
    /// 构造一个新的错误。
    ///
    /// # 参数
    ///
    /// - `message`：错误描述，`&'static str` 或 `Cow<'static, str>`。
    /// - `kind`：错误种类。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::{Error, Kind};
    ///
    /// let err = Error::<u8>::new("custom", Kind::Locked);
    /// assert_eq!(&*err.message, "custom");
    /// ```
    pub fn new(message: impl Into<Cow<'static, str>>, kind: Kind<T>) -> Self {
        Self { message: message.into(), kind }
    }

    /// 构造“锁未获取”错误，`try_*` 方法返回。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::{Error, Kind};
    ///
    /// let err = Error::<u8>::locked();
    /// assert!(matches!(err.kind, Kind::Locked));
    /// ```
    pub fn locked() -> Self {
        Self::new("lock not acquired", Kind::Locked)
    }

    /// 构造“无法等待”错误，对端关闭时返回。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::{Error, Kind};
    ///
    /// let err = Error::<u8>::cannot_wait();
    /// assert!(matches!(err.kind, Kind::CannotWait));
    /// ```
    pub fn cannot_wait() -> Self {
        Self::new("peer closed while waiting", Kind::CannotWait)
    }

    /// 构造“满”错误，归还被拒绝的元素。
    ///
    /// # 示例
    ///
    /// ```
    /// use water_ball_tool_lib::split_ring_buffer::{Error, Kind};
    ///
    /// let err = Error::full(9u8);
    /// match err.kind {
    ///     Kind::Full(v) => assert_eq!(v, 9),
    ///     _ => panic!("expected Full"),
    /// }
    /// ```
    pub fn full(value: T) -> Self {
        Self::new("buffer is full", Kind::Full(value))
    }
}

impl<T: fmt::Debug> fmt::Debug for Kind<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Locked => f.write_str("Locked"),
            Kind::CannotWait => f.write_str("CannotWait"),
            Kind::WriteRejected => f.write_str("WriteRejected"),
            Kind::Full(v) => f.debug_tuple("Full").field(v).finish(),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Error<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("message", &self.message)
            .field("kind", &self.kind)
            .finish()
    }
}

impl<T> fmt::Display for Error<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl<T: fmt::Debug> std::error::Error for Error<T> {}