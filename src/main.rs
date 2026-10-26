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
#[cfg(not(debug_assertions))]
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::exit;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::command::Cli;
use chrono::Utc;
use clap::Parser;
use directories::ProjectDirs;
use indicatif::MultiProgress;
use tracing::{Event, Subscriber};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{FmtContext, FormatEvent, FormatFields, format::Writer},
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
struct BracketFormat;

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
// ===========================================================================
struct TermInner {
    mp: MultiProgress,
    buf: Vec<u8>,
}

impl TermInner {
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

impl Write for TermInner {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        while let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            let s = String::from_utf8_lossy(&line);
            self.mp.suspend(|| print!("{s}"));
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            let s = String::from_utf8_lossy(&self.buf).into_owned();
            self.mp.suspend(|| println!("{s}"));
            self.buf.clear();
        }
        Ok(())
    }
}

#[derive(Clone)]
struct TerminalWriter(Arc<Mutex<TermInner>>);

impl Write for TerminalWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        TermInner::lock(&self.0).write(data)
    }
    fn flush(&mut self) -> io::Result<()> {
        TermInner::lock(&self.0).flush()
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
struct FileWriter(Arc<Mutex<FileInner>>);

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
fn log_file_path(module: &str) -> io::Result<PathBuf> {
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
// 内部初始化：终端恢复默认格式
// ===========================================================================
fn init_terminal_only(mp: &MultiProgress, level: &str) {
    let terminal = TerminalWriter(Arc::new(Mutex::new(TermInner {
        mp: mp.clone(),
        buf: Vec::with_capacity(256),
    })));

    if let Err(e) = tracing_subscriber::registry()
        .with(
            // 默认 fmt::layer，样式与原实现一致
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(move || terminal.clone())
                .with_filter(EnvFilter::new(level)),
        )
        .try_init()
    {
        eprintln!("[logging] 日志系统注册失败: {e}");
    }
}

#[cfg(not(debug_assertions))]
fn init_split(mp: &MultiProgress, file: File) {
    let terminal = TerminalWriter(Arc::new(Mutex::new(TermInner {
        mp: mp.clone(),
        buf: Vec::with_capacity(256),
    })));
    let file_writer = FileWriter(Arc::new(Mutex::new(FileInner {
        file,
        buf: Vec::with_capacity(256),
    })));

    if let Err(e) = tracing_subscriber::registry()
        // ---------- 终端：info+，默认样式 ----------
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(move || terminal.clone())
                .with_filter(EnvFilter::new("info")),
        )
        // ---------- 文件：debug+，自定义方括号样式 ----------
        .with(
            tracing_subscriber::fmt::layer()
                .event_format(BracketFormat)
                .with_ansi(false)
                .with_writer(move || file_writer.clone())
                .with_filter(EnvFilter::new("debug")),
        )
        .try_init()
    {
        eprintln!("[logging] 日志系统注册失败: {e}");
    }
}

// ===========================================================================
// 公开入口
// ===========================================================================
pub fn multi_progress_init(module: &str) -> MultiProgress {
    let mp = MultiProgress::new();
    let _ = init_global_logging(&mp, module);
    mp
}

pub fn init_global_logging(mp: &MultiProgress, module: &str) -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        let _ = module;
        init_terminal_only(mp, "debug");
        return None;
    }

    #[cfg(not(debug_assertions))]
    {
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
