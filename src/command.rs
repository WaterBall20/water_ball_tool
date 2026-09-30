//! CLI 命令模块 / CLI command module
//!
//! 处理 `ff`（文件搜索）和 `wbfp`（水球包文件）两个子命令的路由与参数解析。
//! 提供进度条创建、样式设置和进度更新的公共辅助函数。
//!
//! Handles routing and argument parsing for two subcommands: `ff` (file finder)
//! and `wbfp` (WaterBall File Pack). Provides shared helpers for creating and
//! updating progress bars with consistent styling.

use crate::command::ff::FileFinderArgs;
use crate::command::music_play::MusicPlayArgs;
use crate::command::wbfp::WaterBallFilePackCommand;
use clap::error::Result;
use clap::{Parser, Subcommand};
use indicatif::{MultiProgress, ProgressBar};
use std::time::Duration;

mod ff;
mod music_play;
#[cfg(test)]
mod test;
mod wbfp;
pub mod music_play_test;

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
    MP(MusicPlayArgs),
    Mpt(MusicPlayArgs),
}

static BUF_LEN: usize = 10 * 1024 * 1024;

/// 打包进度条样式模板 / Pack progress bar style template
///
/// 颜色方案 / Color scheme:
///   prefix     -> 绿色加粗   (任务标识)
///   wide_bar   -> 青底蓝条   (保留原有,进度条主体)
///   elapsed    -> dim        (次要信息,弱化)
///   eta        -> 黄色       (估算值,醒目)
///   percent    -> 品红       (核心数值)
///   bytes      -> 默认       (字节数)
///   msg        -> 黄色       (提示语,易读)
const PACK_PROGRESS_STYLE_TEMPLATE: &str = "{prefix:<8.bold.green} [{wide_bar:.cyan/blue}] \
[{elapsed_precise:.dim}(ETA:{eta:>4.yellow})] \
{percent_precise:>7.magenta}% {bytes:>11} / {total_bytes:>11} \n{msg:.yellow}";

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

pub(crate) fn cli(cli: Cli, mp: Option<MultiProgress>) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Commands::Ff(args) => ff::args(args, mp),
        Commands::Wbfp(commands) => wbfp::commands(commands, mp),
        Commands::MP(args) => music_play::args(args, mp),
        Commands::Mpt(args) => music_play_test::args(args, mp),
    }
}
