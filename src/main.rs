//! 水球工具 CLI 入口点 / Water Ball Tool CLI entry point
//!
//! 解析命令行参数并通过 `command::cli()` 路由到对应的子命令。
//! 初始化全局日志系统（tracing），通过 `MultiProgressWriter` 适配器
//! 将日志输出与 indicatif 进度条共存。
//!
//! Parses CLI arguments and routes them to subcommands via `command::cli()`.
//! Initializes the global logging system (tracing), routing log output
//! through a `MultiProgressWriter` adapter to coexist with the indicatif progress bar.

use crate::command::Cli;
use clap::Parser;
use indicatif::MultiProgress;
use std::io;
use std::io::Write;
use tracing::error;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

mod command;

fn main() {
    let cli = Cli::parse();

    let mp = MultiProgress::new();
    init_global_logging(&mp);

    let result = command::cli(cli, Option::from(&mp));
    if let Err(err) = result {
        error!("运行时发生错误， err: {}", err);
    }
}

/// 创建一个包装类，让 MultiProgress 兼容 Write trait。
///
/// tracing 日志系统通过此适配器输出到 indicatif 的进度条区域，
/// 避免日志和进度条互相覆盖。
///
/// A wrapper that adapts MultiProgress to the Write trait so tracing
/// log output coexists with the progress bar without visual glitches.
struct MultiProgressWriter(MultiProgress);

impl Write for MultiProgressWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        self.0.println(s)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

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

    let mp_writer = mp.clone();

    _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(false)
                .with_writer(move || MultiProgressWriter(mp_writer.clone())),
        )
        .try_init();
}
