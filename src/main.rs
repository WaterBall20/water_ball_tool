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

use crate::command::Cli;
use clap::Parser;
use indicatif::MultiProgress;
use std::io;
use std::io::Write;
use std::process::exit;
use std::sync::{Arc, Mutex, MutexGuard};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

mod command;

fn main() {
    let cli = Cli::parse();

    let result = command::cli(cli);
    if let Err(err) = result {
        eprintln!("运行时发生错误， err: {err}");
        exit(-1)
    }
}

// ---------------------------------------------------------------------------
// 共享 Writer：内部持有 MultiProgress 与按行缓冲
// ---------------------------------------------------------------------------

/// 内部状态：仅在锁内访问。
struct Inner {
    mp: MultiProgress,
    buf: Vec<u8>,
}

impl Inner {
    /// 获取锁；若被毒化，执行真正的解毒。
    ///
    /// 毒化意味着上一次持锁的写线程 panic 了。`Inner` 中唯一可能处于
    /// 半写状态的是 `buf: Vec<u8>`（`extend_from_slice` 或 `drain`
    /// 到一半）。`MultiProgress` 内部有它自己的锁，我们无法置为一致
    /// 状态，交给 indicatif 内部处理。
    ///
    /// 解毒操作：清空 `buf`，丢掉可能残留的半行；然后 `clear_poison()`
    /// 让后续锁恢复快速路径。丢掉一行远好过把半行脏数据喂给终端。
    fn lock(shared: &Mutex<Self>) -> MutexGuard<'_, Self> {
        match shared.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                let mut g = poisoned.into_inner();
                g.buf.clear(); // 修复半写状态
                shared.clear_poison(); // 毒已处理，恢复快速路径
                g
            }
        }
    }
}

impl Write for Inner {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);

        // 只在真正的行边界（\n）输出，且原样保留 \n
        while let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            let s = String::from_utf8_lossy(&line);
            // 关键：绕过 MultiProgress::println，用 suspend + print!
            // s 已自带 \n，print! 原样写出，不额外补换行。
            self.mp.suspend(|| {
                print!("{s}");
            });
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            let s = String::from_utf8_lossy(&self.buf).into_owned();
            self.mp.suspend(|| {
                println!("{s}"); // 残留半行，补 \n 后输出
            });
            self.buf.clear();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 对外暴露的 MakeWriter 适配器
// ---------------------------------------------------------------------------

/// 克隆后仍共享同一个 `Inner`，确保一次 tracing 事件中的多次 `write`
/// 落到同一个按行缓冲区，而不是每次新建空 buffer。
#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Inner>>);

impl Write for SharedWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        Inner::lock(&self.0).write(data)
    }
    fn flush(&mut self) -> io::Result<()> {
        Inner::lock(&self.0).flush()
    }
}

// ---------------------------------------------------------------------------
// 初始化全局日志（保持你原来的函数签名）
// ---------------------------------------------------------------------------

/// 初始化全局日志系统，将 tracing 输出重定向到 indicatif 进度条。
///
/// 在 debug 模式下日志级别为 `debug`，release 模式下为 `info`。
/// 日志通过 `MultiProgressWriter` 适配器输出，与进度条共存而不互相干扰。
///
/// Initialize global logging, redirecting tracing output to the indicatif progress bar.
///
/// Log level is `debug` in debug mode and `info` in release mode.
/// Logs are routed through a `MultiProgressWriter` adapter to coexist with the progress bar.
pub fn init_global_logging(mp: &MultiProgress) {
    #[cfg(debug_assertions)]
    let filter = EnvFilter::new("debug");
    #[cfg(not(debug_assertions))]
    let filter = EnvFilter::new("info");

    let writer = SharedWriter(Arc::new(Mutex::new(Inner {
        mp: mp.clone(),
        buf: Vec::with_capacity(256),
    })));

    _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(false)
                .with_writer(move || writer.clone()),
        )
        .try_init();
}

pub fn multi_progress_init() -> MultiProgress {
    let mp = MultiProgress::new();
    init_global_logging(&mp);
    mp
}
