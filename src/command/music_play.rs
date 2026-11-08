use crate::command::music_play::runtime::Runtime;
use crate::command::music_play::tui::app::App;
use crate::command::tui_input::MouseCapture;
use clap::Args;
use std::error;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::{error, info, warn};
use water_ball_tool_lib::music_play::LoopMode;

//AI=== 引擎在库包，终端前端由 TUI 主循环驱动 / the engine lives in the library; the TUI loop drives the front-end
mod runtime;
mod scanner;
pub mod tui;
//AI_END===

#[derive(Args, Debug)]
pub(crate) struct MusicPlayArgs {
    ///播放文件或目录 / Files or directories to play
    files: Vec<PathBuf>,
    //AI=== 音量范围收敛为 0.0–1.0，越界返回参数错误 / volume is restricted to 0.0–1.0, out-of-range is rejected
    /// 初始音量（0.0–1.0）/ Initial volume (0.0–1.0)
    #[arg(short, long, default_value_t = 0.75)]
    volume: f32,
    //AI_END===
    ///详细输出 / Verbose output
    #[arg(long)]
    verbose: bool,
    //AI=== 默认只加载第一首、不自动播放 / load the first track but do not start playing by default
    /// 找到第一首后自动开始播放 / Start playing automatically once the first track is found
    #[arg(short, long)]
    autoplay: bool,
    //AI_END===
    //AI=== 不在二进制包实现 clap 的 ValueEnum（orphan rule），改用 FromStr 解析 / ValueEnum cannot be implemented here, parse via FromStr
    /// 播放模式（sequential-loop|sequential|repeat-one|shuffle|periodic-shuffle）
    /// Playback mode (sequential-loop|sequential|repeat-one|shuffle|periodic-shuffle)
    #[arg(
        short,
        long,
        default_value = "sequential-loop",
        value_parser = LoopMode::from_str
    )]
    mode: LoopMode,
    //AI_END===
}

//AI=== 把 panic 写进日志文件：TUI 崩溃后不保留界面，文件是唯一证据
// Log panics to the file: a crashed TUI leaves no screen, so the file is the only evidence.
fn install_panic_logging() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        error!("TUI panic: {info}");
        previous(info);
    }));
}
//AI_END===

pub fn args(args: MusicPlayArgs) -> Result<(), Box<dyn error::Error>> {
    //AI=== 音量越界属于参数错误：返回错误而不是 panic / out-of-range volume is an argument error, not a panic
    if !(0.0..=1.0).contains(&args.volume) {
        let value = args.volume;
        return Err(format!(
            "参数错误：音量必须在 0.0 到 1.0 之间，实际为 {value} \
             / argument error: volume must be within 0.0..=1.0, got {value}"
        )
        .into());
    }
    //AI_END===

    let logs = tui::channel_writer::init_logging("music_play");
    install_panic_logging();
    info!("启动 TUI 音乐播放器 / starting the TUI music player");

    let mut runtime = match Runtime::new(args.mode, args.volume) {
        Ok(runtime) => runtime,
        Err(e) => {
            return Err(format!("创建运行时失败 / failed to create runtime: {e:?}").into());
        }
    };
    let autoplay = args.autoplay;
    let scans = scanner::spawn(args.files)?;

    let mut terminal = ratatui::try_init()?;
    //AI=== 启用鼠标捕获；失败时降级为无鼠标运行，而不是退出 TUI
    // Enable mouse capture; on failure keep running without the mouse instead of exiting the TUI.
    let mouse = match MouseCapture::enable() {
        Ok(mouse) => mouse,
        Err(e) => {
            warn!("启用鼠标捕获失败，继续无鼠标运行 / failed to enable mouse capture: {e}");
            MouseCapture::disabled()
        }
    };
    let vt_input = mouse.vt_input();
    //AI_END===
    let mut app = App::new();
    let result = tui::run(
        &mut terminal,
        &mut runtime,
        &mut app,
        &logs,
        &scans,
        autoplay,
        vt_input,
    );
    //AI=== TUI 异常退出也要留下日志 / log an abnormal TUI exit as well
    if let Err(e) = &result {
        error!("TUI 主循环异常退出 / TUI loop failed: {e:?}");
    }
    //AI_END===

    runtime.stop();
    info!("退出 TUI 音乐播放器 / leaving the TUI music player");
    //AI=== 恢复终端前释放鼠标捕获 / release mouse capture before restoring the terminal
    drop(mouse);
    //AI_END===
    if let Err(e) = ratatui::try_restore() {
        eprintln!("恢复终端失败 / failed to restore terminal: {e}");
    }
    result?;
    Ok(())
}
