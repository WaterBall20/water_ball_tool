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
use crate::command::ram_probe::RamProbeArgs;
use crate::command::wbfp::WaterBallFilePackCommand;
use clap::error::Result;
use clap::{Parser, Subcommand};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
//AI=== 共享八分块进度条：mp 与 wbfp 的 TUI 共用 / shared eighth-block bar for the mp and wbfp TUIs
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
};
//AI_END===
use std::time::Duration;
use tracing::error;
use unicode_width::UnicodeWidthStr;

mod ff;
pub mod music_play;
pub mod ram_probe;
#[cfg(test)]
mod test;
mod tui_input;
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
    MP(MusicPlayArgs),
    RP(RamProbeArgs),
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
fn create_pb(mp: &MultiProgress) -> ProgressBar {
    let pb = mp.add(ProgressBar::new_spinner());
    pb.enable_steady_tick(Duration::from_millis(100)); // 让转标自己动起来
    pb
}

//AI=== 新增进度条样式构造辅助：模板非法时记录错误并回退默认样式，替代各处 .expect
/// 根据模板构造进度条样式。
///
/// 模板非法（占位符写错等）时不会 panic：记录一条错误日志并退回默认进度条样式，
/// 保证进度条功能降级而不中断任务。
///
/// Build a progress style from a template.
///
/// An invalid template (for example a malformed placeholder) never panics: an error is
/// logged and the default bar style is returned, so the task degrades instead of aborting.
pub(crate) fn progress_style(template: &str) -> ProgressStyle {
    ProgressStyle::default_bar().template(template).map_or_else(
        |e| {
            error!("进度条模板非法，回退到默认样式 / invalid progress template: {e}");
            ProgressStyle::default_bar().progress_chars(EIGHTH_BLOCK_CHARS)
        },
        |style| style.progress_chars(EIGHTH_BLOCK_CHARS),
    )
}
//AI_END===

//AI=== 八分块进度：indicatif 与两套 TUI 共用同一组字符 / eighth blocks shared by indicatif and both TUIs
/// indicatif 进度条字符：满格 `█`、七级细分、空格背景 / indicatif bar chars: `█`, seven partials, blank background
const EIGHTH_BLOCK_CHARS: &str = "█▉▊▋▌▍▎▏ ";

/// 八分块条：`width` 个终端格，每格可再分 8 级。宽度按字符数计。
///
/// Eighth-block bar: `width` terminal cells, each resolved to 1/8. Width counts characters.
pub(crate) fn eighth_block_bar(width: usize, ratio: f64) -> String {
    const PARTIALS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    const FULL: char = '█';
    let filled = ratio.clamp(0.0, 1.0) * width as f64;
    let mut full = filled.floor() as usize;
    let mut eighths = ((filled - full as f64) * 8.0).round() as usize;
    if eighths == 8 {
        full += 1;
        eighths = 0;
    }
    let full = full.min(width);
    let mut bar = String::with_capacity(width);
    bar.extend(std::iter::repeat_n(FULL, full));
    if full < width && eighths > 0 {
        bar.push(PARTIALS[eighths]);
    }
    bar.extend(std::iter::repeat_n(
        ' ',
        width.saturating_sub(bar.chars().count()),
    ));
    bar
}

/// 在 `area` 首行绘制八分块进度条，标签放在进度条右侧。
///
/// 进度条始终是连续的一段，文字不覆盖进度条；标签过长时让出至少
/// `MIN_BAR_CELLS` 格给进度条，超出部分截断。
///
/// Draw the eighth-block bar on the first row of `area` with the label to its right.
/// The bar is always contiguous because no text is drawn over it; a long label leaves at
/// least [`MIN_BAR_CELLS`] cells for the bar and is truncated.
pub(crate) fn render_eighth_block_bar(
    frame: &mut Frame,
    area: Rect,
    ratio: f64,
    color: Color,
    label: &str,
    label_style: Style,
) {
    /// 标签再长也给进度条留出的最小格数 / minimum cells reserved for the bar
    const MIN_BAR_CELLS: usize = 8;
    let width = usize::from(area.width);
    let label_width = label.width().min(width.saturating_sub(MIN_BAR_CELLS));
    let bar_width = width.saturating_sub(label_width + 1);
    let bar = eighth_block_bar(bar_width, ratio);
    let buf = frame.buffer_mut();
    for (i, ch) in bar.chars().enumerate() {
        buf[(area.x + i as u16, area.y)].set_char(ch).set_fg(color);
    }
    let label_x = area.x.saturating_add(bar_width as u16 + 1);
    buf.set_stringn(
        label_x,
        area.y,
        label,
        usize::from(area.right().saturating_sub(label_x)),
        label_style,
    );
}
//AI_END===

pub(crate) fn cli(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Commands::Ff(args) => ff::args(args),
        //AI=== wbfp 终端默认 TUI，非终端/--no-tui 回退经典路径 / wbfp defaults to the TUI on a terminal
        Commands::Wbfp(commands) => wbfp::run(commands),
        //AI_END===
        Commands::MP(args) => music_play::args(args),
        Commands::RP(args) => ram_probe::args(args),
    }
}
