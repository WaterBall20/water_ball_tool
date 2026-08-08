//! CLI 命令模块 / CLI command module
//!
//! 处理 `ff`（文件搜索）和 `wbfp`（水球包文件）两个子命令的路由与参数解析。
//! 提供进度条创建、样式设置和进度更新的公共辅助函数。
//!
//! Handles routing and argument parsing for two subcommands: `ff` (file finder)
//! and `wbfp` (WaterBall File Pack). Provides shared helpers for creating and
//! updating progress bars with consistent styling.

use crate::command::ff::FileFinderArgs;
use crate::command::wbfp::WaterBallFilePackCommand;
use clap::error::Result;
use clap::{Parser, Subcommand};
use indicatif::{MultiProgress, ProgressBar};
use std::time::Duration;

mod ff;
#[cfg(test)]
mod test;
mod wbfp;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
#[command(propagate_version = true)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Ff(FileFinderArgs),
    Wbfp(WaterBallFilePackCommand),
}

static BUF_LEN: usize = 1024 * 1024;


/// 打包进度条样式模板 / Pack progress bar style template
const PACK_PROGRESS_STYLE_TEMPLATE: &str = "{prefix:<8} [{bar:40.cyan/blue}] [{elapsed_precise}(ETA:{eta:>4})] {percent_precise:>7}% {bytes:>11}/{total_bytes:>11} \n{msg}";

// 挂起时 spinner 模板 / Spinner template when suspended
const SPINNER_TEMPLATE: &str = "{spinner:.blue} {prefix:<8} {msg}";

/// 创建并注册一个进度条实例到 `MultiProgress` 中。
///
/// 如果 `MultiProgress` 为 `None` 则返回 `None`（无进度条模式）。
/// 进度条启用每 100ms 的自动旋转动画（spinner）。
///
/// Create and register a progress bar instance in `MultiProgress`.
///
/// Returns `None` if `MultiProgress` is `None` (no-progress mode).
/// The spinner auto-rotates every 100ms.
fn create_pb(mp: Option<&MultiProgress>) -> Option<ProgressBar> {
    if let Some(mp) = mp {
        let pb = mp.add(ProgressBar::new_spinner());
        pb.enable_steady_tick(Duration::from_millis(100)); // 让转标自己动起来
        Some(pb)
    } else {
        None
    }
}

pub(crate) fn cli(cli: Cli, mp: Option<&MultiProgress>) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Commands::Ff(ff) => ff::ff(ff, mp),
        Commands::Wbfp(wbfp) => wbfp::wbfp(&wbfp, mp),
    }
}
