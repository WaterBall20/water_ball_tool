//! CLI 命令模块 / CLI command module
//!
//! 处理 `ff`（文件搜索）和 `wbfp`（水球包文件）两个子命令的路由与参数解析。
//! 提供进度条创建、样式设置和进度更新的公共辅助函数。
//!
//! Handles routing and argument parsing for two subcommands: `ff` (file finder)
//! and `wbfp` (WaterBall File Pack). Provides shared helpers for creating and
//! updating progress bars with consistent styling.

use crate::command::ff::FileFinderArgs;
use crate::command::wbfp::WaterBallFilePackArgs;
use clap::error::Result;
use clap::{Parser, Subcommand};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::path::Path;
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
    Wbfp(WaterBallFilePackArgs),
}

static BUF_LEN: usize = 1024 * 1024;

/// 搜索进度条样式模板 / Search progress bar style template
const PROGRESS_STYLE_TEMPLATE: &str =
    "{spinner:.green} [{bar:40.cyan/blue}] [{elapsed_precise}] {msg}";

/// 打包进度条样式模板 / Pack progress bar style template
const PACK_PROGRESS_STYLE_TEMPLATE: &str = "{prefix:<8} [{bar:40.cyan/blue}] [{elapsed_precise}(ETA:{eta:>4})] {percent_precise:>7}% {bytes:>11}/{total_bytes:>11} \n {msg}";

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
/// 将进度条设置为"进度条"模式（有确定长度）。
///
/// 一般用于知道总长度的场景（如解包、哈希校验时知道总数据量）。
/// 进度条字符为 `=>-`，显示百分比、已用时间和预计剩余时间。
///
/// Switch the progress bar to "determinate bar" mode (known total length).
///
/// Used when the total length is known (e.g., unpacking, hash verification).
/// Bar characters are `=>-`, showing percentage, elapsed and ETA.
fn set_pb_style2(pb: Option<&ProgressBar>, data_len: u64) {
    if let Some(pb) = pb {
        pb.set_length(data_len);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(PROGRESS_STYLE_TEMPLATE)
                .unwrap()
                .progress_chars("=>-"),
        );
        pb.set_message("0.00%");
    }
}

/// 向进度回调发送更新（限频：每超过 10×BUF_LEN 字节才触发）。
///
/// 用于解包和哈希校验场景中的进度更新，避免过于频繁的 UI 刷新。
/// 回调参数：(已写字节增量, 文件计数增量, 包路径, 文件路径)
///
/// Send progress updates to a callback (rate-limited: triggers every 10×BUF_LEN bytes).
///
/// Used in unpack and hash-verify scenarios to avoid excessive UI refreshes.
/// Callback params: (write_delta, file_count_delta, pack_path, file_path)
fn update_pb(
    pb_c: &mut Option<&mut dyn FnMut(u64, u64, String, String)>,
    path1: &Path,
    path2: &Path,
    lase_up_pb_c_write_len: &mut u64,
    write_len: &mut u64,
) {
    if let Some(pb_c) = pb_c {
        let l_len = *write_len - *lase_up_pb_c_write_len;
        if l_len > 10 * (BUF_LEN as u64) {
            pb_c(
                l_len,
                0,
                path1.display().to_string(),
                path2.display().to_string(),
            );
            *lase_up_pb_c_write_len = *write_len;
        }
    }
}

pub(crate) fn cli(cli: Cli, mp: Option<&MultiProgress>) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Commands::Ff(ff) => ff::ff(ff, mp),
        Commands::Wbfp(wbfp) => wbfp::wbfp(wbfp, mp),
    }
}
