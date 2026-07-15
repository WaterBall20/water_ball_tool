use std::fmt;

/// WBFP 模块错误类型 / WBFP module error type
#[derive(Debug)]
pub enum PackFileError {
    /// I/O 错误 / I/O error
    Io(std::io::Error),
    /// 锁错误（互斥锁、文件锁）/ Lock error (mutex, file lock)
    Lock(String),
    /// 数据完整性错误（哈希验证等）/ Integrity error (hash verification, etc.)
    Integrity(String),
    /// 数据格式错误 / Data format error
    Format(String),
    /// 未找到 / Not found
    NotFound(String),
    /// 不是目录 / Not a directory
    NotADirectory(String),
    /// 版本不兼容 / Version incompatibility
    Version(String),
    /// 状态错误（未加载、已锁定等）/ State error (not loaded, locked, etc.)
    State(String),
    /// 其他错误 / Other error
    Other(String),
}

/// WBFP 模块结果类型 / WBFP module result type
pub type Result<T> = std::result::Result<T, PackFileError>;

impl fmt::Display for PackFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackFileError::Io(e) => write!(f, "{}", e),
            PackFileError::Lock(msg) => write!(f, "锁错误 / Lock error: {}", msg),
            PackFileError::Integrity(msg) => write!(f, "完整性错误 / Integrity error: {}", msg),
            PackFileError::Format(msg) => write!(f, "格式错误 / Format error: {}", msg),
            PackFileError::NotFound(msg) => write!(f, "未找到 / Not found: {}", msg),
            PackFileError::NotADirectory(msg) => write!(f, "不是目录 / Not a directory: {}", msg),
            PackFileError::Version(msg) => write!(f, "版本错误 / Version error: {}", msg),
            PackFileError::State(msg) => write!(f, "状态错误 / State error: {}", msg),
            PackFileError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for PackFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PackFileError::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// 允许 `?` 操作符将 `std::io::Error` 自动转换为 `PackFileError`
impl From<std::io::Error> for PackFileError {
    fn from(e: std::io::Error) -> Self {
        PackFileError::Io(e)
    }
}

/// 允许 `?` 操作符将 `PackFileError` 自动转换为 `std::io::Error`
/// Used in std::io trait implementations (Read/Write/Seek) that must return `io::Result`.
impl From<PackFileError> for std::io::Error {
    fn from(e: PackFileError) -> Self {
        std::io::Error::other(e.to_string())
    }
}
