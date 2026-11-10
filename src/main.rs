//! 水球工具 CLI 入口点 / Water Ball Tool CLI entry point
//!
//! 解析命令行参数并通过 `command::cli()` 路由到对应的子命令。
//! 初始化全局日志系统（tracing），通过 `MultiProgressWriter` 适配器
//! 将日志输出与 indicatif 进度条共存。
//!
//! Parses CLI arguments and routes them to subcommands via `command::cli()`.
//! Initializes the global logging system (tracing), routing log output
//! through a `MultiProgressWriter` adapter to coexist with the indicatif progress bar.
#![deny(clippy::unwrap_used)]

use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::exit;
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use crate::command::Cli;
use chrono::Utc;
use clap::Parser;
//AI=== ProjectDirs 只用于 Release 日志路径 / ProjectDirs is only needed for the release log path
#[cfg(not(debug_assertions))]
use directories::ProjectDirs;
//AI_END===
use indicatif::MultiProgress;
use tracing::{Event, Subscriber};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{FmtContext, FormatEvent, FormatFields, MakeWriter, format::Writer},
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
};

mod command;

fn main() {
    let cli = Cli::parse();

    let result = command::cli(cli);
    if let Err(err) = result {
        eprintln!("运行时发生错误， err: {err}");
        exit(-1)
    }
}

// ===========================================================================
// 自定义 FormatEvent：仅用于文件
// ===========================================================================
//AI=== TUI 日志复用文件层：只提升可见性并加构造器 / TUI logging reuses the file layer: visibility + constructor only
pub(crate) struct BracketFormat;
//AI_END===

impl<S, N> FormatEvent<S, N> for BracketFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();

        // [时间]
        write!(writer, "[{}] ", Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ"))?;

        // [级别]
        write!(writer, "[{}] ", meta.level())?;

        // [线程名 ThreadId(N)] —— 有名输出名，无名只出 ID
        let t = std::thread::current();
        match t.name() {
            Some(name) => write!(writer, "[{} {:?}] ", name, t.id())?,
            None => write!(writer, "[{:?}] ", t.id())?,
        }

        // [文件:行]
        match (meta.file(), meta.line()) {
            (Some(f), Some(l)) => write!(writer, "[{}:{}] ", f, l)?,
            (Some(f), None) => write!(writer, "[{}] ", f)?,
            _ => {}
        }

        // 消息 + 结构化字段
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

// ===========================================================================
// 终端 writer
//
// 调用线程（含搜索、UI、音频事件线程）不直接做终端 I/O：整行交给有界通道，
// 由 `log-writer` 线程统一走 MultiProgress::suspend() 打印。
// 调用线程只做一次 try_send，因此调试器停在断点上、或控制台写被卡住时，
// 都不会有线程阻塞在日志里（原来的共享互斥锁 + MultiProgress 写锁 + 阻塞控制台写）。
//
// Caller threads (search, UI, audio event dispatch) never touch the terminal
// themselves: whole lines go into a bounded channel and the `log-writer` thread
// prints them through MultiProgress::suspend(). Callers only do a try_send, so a
// paused debugger or a stalled console can no longer park a thread inside logging
// (the old path held a shared mutex + the MultiProgress write lock across blocking I/O).
// ===========================================================================
const LOG_CHANNEL_CAP: usize = 1024;

struct TerminalWriter {
    // None: 后台线程创建失败，退化为同步直写 / None: writer thread failed to spawn, fall back to direct writes
    tx: Option<SyncSender<String>>,
    mp: MultiProgress,
    buf: Vec<u8>,
}

impl TerminalWriter {
    fn send(&self, line: String) {
        match &self.tx {
            // shortcut: 队列满（1024 行，通常是调试器暂停或控制台卡住期间）直接丢弃终端日志，绝不阻塞调用线程；文件日志走 FileWriter，不受影响
            // shortcut: a full queue (1024 lines, usually while the debugger is paused or the console is stalled) drops terminal lines instead of blocking the caller; file logging is unaffected.
            Some(tx) => {
                let _ = tx.try_send(line);
            }
            None => self.mp.suspend(|| print!("{line}")),
        }
    }
}

impl Write for TerminalWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        while let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            self.send(String::from_utf8_lossy(&line).into_owned());
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&self.buf).into_owned();
            self.buf.clear();
            self.send(line);
        }
        Ok(())
    }
}

impl Drop for TerminalWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

#[derive(Clone)]
struct TerminalMakeWriter {
    tx: Option<SyncSender<String>>,
    mp: MultiProgress,
}

impl<'a> MakeWriter<'a> for TerminalMakeWriter {
    type Writer = TerminalWriter;
    fn make_writer(&'a self) -> Self::Writer {
        TerminalWriter {
            tx: self.tx.clone(),
            mp: self.mp.clone(),
            buf: Vec::with_capacity(256),
        }
    }
}

/// 启动后台终端写出线程；创建失败时返回 `None`，由调用方退化为同步直写（不 panic）。
///
/// Spawn the background terminal writer thread; returns `None` on spawn failure so
/// the caller can fall back to direct writes instead of panicking.
fn spawn_terminal_writer(mp: &MultiProgress) -> Option<SyncSender<String>> {
    let (tx, rx) = mpsc::sync_channel::<String>(LOG_CHANNEL_CAP);
    let writer_mp = mp.clone();
    match thread::Builder::new()
        .name("log-writer".to_string())
        .spawn(move || {
            for line in rx {
                writer_mp.suspend(|| print!("{line}"));
            }
        }) {
        Ok(_) => Some(tx),
        Err(e) => {
            eprintln!("[logging] 日志写出线程创建失败，回退同步输出: {e}");
            None
        }
    }
}

// ===========================================================================
// 文件 writer
// ===========================================================================
struct FileInner {
    file: File,
    buf: Vec<u8>,
}

impl FileInner {
    fn lock(shared: &Mutex<Self>) -> MutexGuard<'_, Self> {
        match shared.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                let mut g = poisoned.into_inner();
                g.buf.clear();
                shared.clear_poison();
                g
            }
        }
    }
}

impl Write for FileInner {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        let mut start = 0;
        for (i, &b) in self.buf.iter().enumerate() {
            if b == b'\n' {
                self.file.write_all(&self.buf[start..=i])?;
                start = i + 1;
            }
        }
        if start > 0 {
            self.buf.drain(..start);
            self.file.flush()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            self.file.write_all(&self.buf)?;
            self.buf.clear();
        }
        self.file.flush()
    }
}

#[derive(Clone)]
pub(crate) struct FileWriter(Arc<Mutex<FileInner>>);

impl FileWriter {
    //AI=== 供 TUI 日志层构造文件 writer / build a file writer for the TUI logging layer
    pub(crate) fn new(file: File) -> Self {
        Self(Arc::new(Mutex::new(FileInner {
            file,
            buf: Vec::with_capacity(256),
        })))
    }
    //AI_END===
}

impl Write for FileWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        FileInner::lock(&self.0).write(data)
    }
    fn flush(&mut self) -> io::Result<()> {
        FileInner::lock(&self.0).flush()
    }
}

// ===========================================================================
// 日志文件名生成
//   <用户数据目录>/<Cargo包名>/logs/<模块名>/<模块名>-<时间戳>.log
// ===========================================================================
//AI=== Release 构建的日志路径；Debug 构建改用 debug_log_file_path
// Release log path; debug builds use debug_log_file_path instead.
#[cfg(not(debug_assertions))]
pub(crate) fn log_file_path(module: &str) -> io::Result<PathBuf> {
    let module = sanitize_module(module);
    let app = sanitize_module(env!("CARGO_PKG_NAME"));

    let base_dir = ProjectDirs::from("", "", &app)
        .map(|p| p.data_local_dir().to_path_buf())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|dir| dir.join(&app)))
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "无法确定用户数据目录，也无法回退到可执行文件目录",
            )
        })?;

    let log_dir = base_dir.join("logs").join(&module);
    std::fs::create_dir_all(&log_dir)?;

    let now = chrono::Local::now();
    let filename = format!("{}-{}.log", module, now.format("%Y-%m-%d_%H-%M-%S"));
    Ok(log_dir.join(filename))
}

//AI=== Debug 构建的日志文件写到项目 temp 目录：TUI 异常退出后仍可取证
// Debug-build log files go to the project temp directory so a crashed TUI still leaves evidence.
#[cfg(debug_assertions)]
pub(crate) fn debug_log_file_path(module: &str) -> io::Result<PathBuf> {
    let module = sanitize_module(module);
    let log_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("temp")
        .join("logs")
        .join(&module);
    std::fs::create_dir_all(&log_dir)?;
    let now = chrono::Local::now();
    Ok(log_dir.join(format!(
        "{}-{}.log",
        module,
        now.format("%Y-%m-%d_%H-%M-%S")
    )))
}
//AI_END===

fn sanitize_module(module: &str) -> String {
    let s: String = module
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = s.trim_matches('.').trim();
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

// ===========================================================================
// 日志过滤器：把第三方库（lofty 等）经 tracing-log 桥接进来的记录压到 warn
//
// 每条日志都要走「TermInner 互斥锁 → MultiProgress 全局写锁 → clear + print + 全部重绘」
// 这条串行阻塞路径，逐块/逐帧的 debug 日志会把事件循环和播放线程一起拖住。
// ===========================================================================
//AI=== 第三方 log 记录（tracing-subscriber 默认启用 tracing-log 桥接）只保留 warn，避免每条 debug 日志都清屏重绘进度条
// third-party `log` records (bridged by tracing-subscriber's default `tracing-log` feature) are capped at warn so a debug line no longer triggers a full clear+redraw of every bar.
pub(crate) fn log_filter(level: &str) -> EnvFilter {
    EnvFilter::new(format!("{level},lofty=warn"))
}
//AI_END===

// ===========================================================================
// 内部初始化：终端恢复默认格式
// ===========================================================================
fn init_terminal_only(mp: &MultiProgress, level: &str) {
    let writer = TerminalMakeWriter {
        tx: spawn_terminal_writer(mp),
        mp: mp.clone(),
    };

    if let Err(e) = tracing_subscriber::registry()
        .with(
            // 默认 fmt::layer，样式与原实现一致
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(writer)
                .with_filter(log_filter(level)),
        )
        .try_init()
    {
        eprintln!("[logging] 日志系统注册失败: {e}");
    }
}

#[cfg(not(debug_assertions))]
fn init_split(mp: &MultiProgress, file: File) {
    let writer = TerminalMakeWriter {
        tx: spawn_terminal_writer(mp),
        mp: mp.clone(),
    };
    let file_writer = FileWriter(Arc::new(Mutex::new(FileInner {
        file,
        buf: Vec::with_capacity(256),
    })));

    if let Err(e) = tracing_subscriber::registry()
        // ---------- 终端：info+，默认样式 ----------
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(writer)
                .with_filter(log_filter("info")),
        )
        // ---------- 文件：debug+，自定义方括号样式 ----------
        .with(
            tracing_subscriber::fmt::layer()
                .event_format(BracketFormat)
                .with_ansi(false)
                .with_writer(move || file_writer.clone())
                .with_filter(log_filter("debug")),
        )
        .try_init()
    {
        eprintln!("[logging] 日志系统注册失败: {e}");
    }
}

// ===========================================================================
// 公开入口
// ===========================================================================
#[must_use]
pub fn multi_progress_init(module: &str) -> MultiProgress {
    let mp = MultiProgress::new();
    let _ = init_global_logging(&mp, module);
    mp
}

#[must_use]
pub fn init_global_logging(mp: &MultiProgress, module: &str) -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        let _ = module;
        init_terminal_only(mp, "debug");
        None
    }

    #[cfg(not(debug_assertions))]
    {
        use std::fs::OpenOptions;
        let result = log_file_path(module).and_then(|path| {
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            Ok((path, file))
        });

        match result {
            Ok((path, file)) => {
                init_split(mp, file);
                Some(path)
            }
            Err(e) => {
                eprintln!("[logging] 无法创建日志文件: {e}，回退到终端 debug 级别输出");
                init_terminal_only(mp, "debug");
                None
            }
        }
    }
}
