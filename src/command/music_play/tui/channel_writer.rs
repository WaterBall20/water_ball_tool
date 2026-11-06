use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver, SyncSender};
use tracing_subscriber::{Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

// 与终端日志一致的容量：队列满时丢弃，绝不阻塞调用线程
// Same capacity as the terminal log path: drop when full instead of parking the caller thread.
const LOG_CHANNEL_CAP: usize = 1024;

/// 日志级别，用于面板着色。
///
/// Log level, used to color the panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// 级别在格式化行中的文本 / the level token in a formatted line
    pub fn token(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        match token {
            "TRACE" => Some(Self::Trace),
            "DEBUG" => Some(Self::Debug),
            "INFO" => Some(Self::Info),
            "WARN" => Some(Self::Warn),
            "ERROR" => Some(Self::Error),
            _ => None,
        }
    }
}

/// 一行日志：保留级别以便 UI 分色。
///
/// One log line; the level is kept so the UI can color it.
#[derive(Clone)]
pub struct LogLine {
    pub level: Option<LogLevel>,
    pub text: String,
}

impl LogLine {
    // 默认 fmt 布局为 "<时间戳> <LEVEL> <消息>"，级别只可能出现在行首附近
    // The default fmt layout is "<timestamp> <LEVEL> <message>"; the level only appears near the start.
    const LEVEL_SCAN_LIMIT: usize = 40;

    fn parse(text: String) -> Self {
        let head = text.get(..text.len().min(Self::LEVEL_SCAN_LIMIT));
        let level = head.and_then(|head| {
            ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"]
                .into_iter()
                .filter_map(|token| head.find(token).map(|pos| (pos, token)))
                .min_by_key(|(pos, _)| *pos)
                .and_then(|(_, token)| LogLevel::from_token(token))
        });
        Self { level, text }
    }
}

/// 一个将写入内容按行转换为 `LogLine` 并通过有界通道发送的 Writer。
///
/// A Writer that turns writes into `LogLine`s and sends them through a bounded channel.
struct ChannelWriter {
    tx: SyncSender<LogLine>,
    buffer: Vec<u8>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        // 一次 write 可能包含多行：切出全部完整行，剩余部分留到下次
        // A single write may contain several lines: drain all complete lines and keep the remainder.
        let mut start = 0;
        for (i, &b) in self.buffer.iter().enumerate() {
            if b == b'\n' {
                let line = String::from_utf8_lossy(&self.buffer[start..i]).into_owned();
                // shortcut: 面板消费不及（1024 行）时丢弃该行，避免日志阻塞音频/播放线程
                // shortcut: drop the line when the panel falls 1024 lines behind, so logging never blocks audio/playback threads
                let _ = self.tx.try_send(LogLine::parse(line));
                start = i + 1;
            }
        }
        if start > 0 {
            self.buffer.drain(..start);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct ChannelMakeWriter(SyncSender<LogLine>);

impl<'a> fmt::MakeWriter<'a> for ChannelMakeWriter {
    type Writer = ChannelWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ChannelWriter {
            tx: self.0.clone(),
            buffer: Vec::with_capacity(256),
        }
    }
}

/// 初始化 TUI 日志系统，返回日志接收端。
///
/// Debug 构建把 `debug` 级别同时输出到面板和日志文件，文件位于项目 `temp/logs/`；
/// Release 构建面板为 `info`，日志文件位于用户数据目录。与 `main.rs` 的终端日志不同，
/// 这里不经过 `MultiProgress::suspend()`，避免撕坏 TUI。
///
/// Initialize the TUI logging system and return the receiving end.
/// Debug builds send `debug` records to the panel and to a log file under the project `temp/logs/`; release builds
/// show `info` in the panel and keep the `debug` log file in the user data directory. Unlike the `main.rs` terminal
/// path this never goes through `MultiProgress::suspend()`.
pub fn init_logging(module: &str) -> Receiver<LogLine> {
    let (tx, rx) = mpsc::sync_channel(LOG_CHANNEL_CAP);

    let panel_level = if cfg!(debug_assertions) {
        "debug"
    } else {
        "info"
    };

    let registry = tracing_subscriber::registry().with(
        fmt::layer()
            .with_target(false)
            .with_ansi(false)
            .with_writer(ChannelMakeWriter(tx))
            .with_filter(crate::log_filter(panel_level)),
    );

    use std::fs::OpenOptions;
    match log_file_path(module)
        .and_then(|path| OpenOptions::new().create(true).append(true).open(path))
    {
        Ok(file) => {
            let file_writer = crate::FileWriter::new(file);
            if let Err(e) = registry
                .with(
                    fmt::layer()
                        .event_format(crate::BracketFormat)
                        .with_ansi(false)
                        .with_writer(move || file_writer.clone())
                        .with_filter(crate::log_filter("debug")),
                )
                .try_init()
            {
                eprintln!("[logging] 日志系统注册失败: {e}");
            }
        }
        Err(e) => {
            eprintln!("[logging] 创建日志文件失败，仅输出到 TUI 面板: {e}");
            if let Err(e) = registry.try_init() {
                eprintln!("[logging] 日志系统注册失败: {e}");
            }
        }
    }

    rx
}

//AI=== Debug 构建写项目 temp 目录，Release 构建写用户数据目录 / debug builds use the project temp dir, release uses user data
#[cfg(debug_assertions)]
fn log_file_path(module: &str) -> io::Result<std::path::PathBuf> {
    crate::debug_log_file_path(module)
}

#[cfg(not(debug_assertions))]
fn log_file_path(module: &str) -> io::Result<std::path::PathBuf> {
    crate::log_file_path(module)
}
//AI_END===

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn splits_every_line_and_keeps_the_remainder() -> Result<(), Box<dyn std::error::Error>> {
        let (tx, rx) = mpsc::sync_channel(8);
        let mut writer = ChannelWriter {
            tx,
            buffer: Vec::new(),
        };

        writer.write_all(b"first\nsecond\npart")?;
        assert_eq!(rx.try_recv()?.text, "first");
        assert_eq!(rx.try_recv()?.text, "second");
        assert!(rx.try_recv().is_err());

        writer.write_all(b"ial\n")?;
        assert_eq!(rx.try_recv()?.text, "partial");
        Ok(())
    }

    #[test]
    fn parses_the_level_from_the_line_head() {
        let line = LogLine::parse("2026-01-01T00:00:00Z  WARN 磁盘将满".to_string());
        assert_eq!(line.level, Some(LogLevel::Warn));

        // 消息里出现的级别词不能覆盖行首的真实级别
        // A level word inside the message must not override the real level at the line head.
        let line = LogLine::parse("2026-01-01T00:00:00Z  INFO DEBUG is not the level".to_string());
        assert_eq!(line.level, Some(LogLevel::Info));
    }
}
