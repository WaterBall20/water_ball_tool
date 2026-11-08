//! wbfp 子命令模块 / wbfp subcommand module
//!
//! 水球包文件（WBFP）的打包、解包、哈希校验、只读浏览与文件管理。
//! 终端默认进入 TUI；`--no-tui` 或非 TTY 环境回退到经典进度条输出。
//! 省略子命令时进入文件管理器：左侧文件系统、右侧包视图、最右预览；包路径可选。
//!
//! WaterBall Files Pack (WBFP) pack, unpack, hash-verify, read-only browsing and file management.
//! A terminal defaults to the TUI; `--no-tui` or a non-TTY environment falls back
//! to the classic progress-bar output. Without a subcommand the file manager opens:
//! filesystem on the left, pack in the middle, preview on the right; the pack path is optional.

//AI=== 模块根：参数、路由与 TUI/经典路径选择 / module root: arguments, routing and TUI/classic selection
use clap::{Args, Subcommand};
use std::io::IsTerminal;
use tracing::debug;

mod classic;
mod common;
mod events;
mod ops;
mod tui;

/// 水球包文件操作 / WaterBall Files Pack operations.
#[derive(Args, Debug)]
#[command(subcommand_negates_reqs = true)]
pub(crate) struct WaterBallFilePackCommand {
    #[command(subcommand)]
    pub(crate) commands: Option<WaterBallFilePackCommands>,
    /// 包文件路径；省略子命令时可选，提供则直接打开该包 / Pack file path; optional without a subcommand, and opens the pack when provided
    pub(crate) pack_path: Option<String>,
    /// 禁用 TUI，使用经典进度条输出 / Disable the TUI and use the classic progress-bar output
    #[arg(long, global = true)]
    pub(crate) no_tui: bool,
}

impl WaterBallFilePackCommand {
    #[cfg(test)]
    pub(crate) fn new(commands: WaterBallFilePackCommands) -> Self {
        // 测试默认走经典路径，避免依赖终端 / tests default to the classic path, never a terminal
        Self {
            commands: Some(commands),
            pack_path: None,
            no_tui: true,
        }
    }
}

/// wbfp 子命令枚举 / wbfp subcommand enum
#[derive(Subcommand, Debug)]
pub(crate) enum WaterBallFilePackCommands {
    /// 解包 / Unpack
    #[command(visible_alias = "u")]
    Unpack(WaterBallFilePackCommandsUnpack),
    /// 打包 / Pack
    #[command(visible_alias = "p")]
    Pack(WaterBallFilePackCommandsPack),
    /// 对整个包文件哈希校验 / Hash verify the entire pack file
    #[command(visible_alias = "h")]
    HashVerify(WaterBallFilePackCommandsHashVerify),
    /// 只读浏览包内文件 / Browse the pack contents read-only
    #[command(visible_alias = "b")]
    Browse(WaterBallFilePackCommandsBrowse),
}

/// 解包子命令参数 / Unpack subcommand arguments
#[derive(Args, Debug)]
pub(crate) struct WaterBallFilePackCommandsUnpack {
    /// 水球包包文件路径 / WaterBall pack file path
    pub(crate) pack_path: String,
    /// 输出目录路径（缺省则去掉 .wbfp 后缀，生成在同级目录）/ Output directory path (defaults to stripping .wbfp suffix, placed next to pack file)
    pub(crate) out_dir: Option<String>,
    /// 解包前进行哈希校验 / Hash verify before unpacking
    #[arg(short = 'H', long)]
    pub(crate) hash_verify: bool,
    /// 解包时所用的线程数，缺省使用CPU线程数量 / Thread count for unpacking, defaults to CPU count
    #[arg(short, long)]
    pub(crate) thread_count: Option<usize>,
}

impl WaterBallFilePackCommandsUnpack {
    #[cfg(test)]
    pub(crate) fn new(
        pack_path: String,
        out_dir: Option<String>,
        hash_verify: bool,
        thread_count: Option<usize>,
    ) -> Self {
        Self {
            pack_path,
            out_dir,
            hash_verify,
            thread_count,
        }
    }
}

/// 打包子命令参数 / Pack subcommand arguments
#[derive(Args, Debug)]
pub(crate) struct WaterBallFilePackCommandsPack {
    /// 打包的文件或目录路径 / Input file or directory path
    pub(crate) in_path: String,
    /// 输出的包文件路径（省略则在当前目录生成同名文件）/ Output pack file path (generates same-name file in CWD if omitted)
    pub(crate) out_pack_path: Option<String>,
    /// 不分离清单到 .wbm 文件 / Do not separate manifest to .wbm file
    #[arg(short, long)]
    pub(crate) no_separation: bool,
    /// 复制时所用的线程数，缺省使用CPU线程数量
    #[arg(short, long)]
    pub(crate) thread_count: Option<usize>,
    /// 不启用写入优化
    #[arg(short = 'w', long)]
    pub(crate) no_write_optimization: bool,
}

impl WaterBallFilePackCommandsPack {
    #[cfg(test)]
    pub(crate) fn new(
        in_path: String,
        out_pack_path: Option<String>,
        no_separation: bool,
        thread_count: Option<usize>,
        no_write_optimization: bool,
    ) -> Self {
        Self {
            in_path,
            out_pack_path,
            no_separation,
            thread_count,
            no_write_optimization,
        }
    }
}

/// 哈希校验子命令参数 / Hash verify subcommand arguments
#[derive(Args, Debug)]
pub(crate) struct WaterBallFilePackCommandsHashVerify {
    /// 包文件路径 / Pack file path
    pub(crate) pack_path: String,
    /// 校验时所用的线程数，缺省使用CPU线程数量 / Thread count for verification, defaults to CPU count
    #[arg(short, long)]
    pub(crate) thread_count: Option<usize>,
    #[arg(short, long)]
    pub(crate) verbose: bool,
}

impl WaterBallFilePackCommandsHashVerify {
    #[cfg(test)]
    pub(crate) fn new(pack_path: String, verbose: bool) -> Self {
        Self {
            pack_path,
            thread_count: None,
            verbose,
        }
    }
}

/// 只读浏览子命令参数 / Read-only browse subcommand arguments
#[derive(Args, Debug)]
pub(crate) struct WaterBallFilePackCommandsBrowse {
    /// 包文件路径 / Pack file path
    pub(crate) pack_path: String,
}

/// wbfp 命令路由入口：终端默认 TUI，非终端或 `--no-tui` 走经典路径；
/// 无子命令时进入文件管理器，包路径可选。
///
/// wbfp entry point: a terminal defaults to the TUI; a non-terminal or `--no-tui`
/// uses the classic path. Without a subcommand the file manager opens, with an
/// optional pack path.
pub fn run(args: WaterBallFilePackCommand) -> Result<(), Box<dyn std::error::Error>> {
    // 无子命令直接进入文件管理器；包路径可选，由 TUI 决定是否打开。
    // No subcommand opens the file manager directly; the optional pack path is handled by the TUI.
    let manager = args.commands.is_none();
    let browse = matches!(&args.commands, Some(WaterBallFilePackCommands::Browse(_)));
    let stdout_tty = std::io::stdout().is_terminal();
    let stdin_tty = std::io::stdin().is_terminal();
    debug!(
        "wbfp route: no_tui={}, stdout_tty={stdout_tty}, stdin_tty={stdin_tty}, browse={browse}, manager={manager}",
        args.no_tui
    );
    let use_tui = !args.no_tui && stdout_tty && stdin_tty;
    if (browse || manager) && !use_tui {
        return Err(
            "浏览/文件管理需要交互式终端 / browse and file management require an interactive terminal"
                .into(),
        );
    }
    if use_tui {
        tui::run(args)
    } else {
        commands(args)
    }
}

/// 经典进度条路径入口（测试与 `--no-tui` 使用）。
///
/// Classic progress-bar entry point (used by tests and `--no-tui`).
pub fn commands(args: WaterBallFilePackCommand) -> Result<(), Box<dyn std::error::Error>> {
    classic::commands(args)
}

#[cfg(test)]
mod test;
//AI_END===
