use crate::command::music_play::runtime::Runtime;
use crate::command::music_play::tui::app::App;
use clap::Args;
//AI=== 鼠标捕获：Windows 使用 VT 输入 + ANSI 报文，Unix 使用 crossterm 命令
// Mouse capture: Windows uses VT input + ANSI reports, Unix uses the crossterm command.
use ratatui::crossterm::Command;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use std::io::{self, Write};
//AI_END===
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

//AI=== 鼠标捕获：TUI 中滚轮与点击播放列表需要；Drop 释放，panic 展开时也能恢复
// Mouse capture for wheel scrolling and playlist clicks; Drop releases it, including during unwinding.
pub(crate) struct MouseCapture {
    /// 是否启用了 VT 输入（鼠标报文由 TUI 的 ANSI 解析器处理）
    /// Whether VT input is enabled (the TUI parses mouse reports from the ANSI stream).
    vt_input: bool,
    #[cfg(windows)]
    saved_input_mode: Option<u32>,
}

impl MouseCapture {
    pub(crate) fn enable() -> io::Result<Self> {
        #[cfg(windows)]
        {
            match enable_vt_input() {
                Ok(mode) => {
                    if let Err(e) = write_command(EnableMouseCapture) {
                        restore_input_mode(mode);
                        return Err(e);
                    }
                    Ok(Self {
                        vt_input: true,
                        saved_input_mode: Some(mode),
                    })
                }
                Err(e) => {
                    // 没有 VT 输入时，Windows 控制台会把鼠标报文拆成裸字节按键，直接禁用鼠标避免误触快捷键。
                    // Without VT input Windows delivers mouse reports as raw key bytes; disable the mouse instead
                    // of letting those bytes trigger hotkeys.
                    warn!("VT 输入不可用，鼠标已禁用 / VT input unavailable, mouse disabled: {e}");
                    Ok(Self::disabled())
                }
            }
        }
        #[cfg(not(windows))]
        {
            write_command(EnableMouseCapture)?;
            Ok(Self { vt_input: false })
        }
    }

    pub(crate) fn disabled() -> Self {
        Self {
            vt_input: false,
            #[cfg(windows)]
            saved_input_mode: None,
        }
    }

    /// 键盘专用模式：关闭鼠标上报并清掉旧进程可能留下的 SGR 鼠标模式。
    ///
    /// Keyboard-only mode: disable mouse reporting and clear SGR mouse mode left by a previous process.
    pub(crate) fn keyboard_only() -> Self {
        #[cfg(windows)]
        if let Err(e) = write_command(DisableMouseCapture) {
            warn!("关闭鼠标上报失败 / failed to disable mouse reporting: {e}");
        }
        Self::disabled()
    }

    pub(crate) fn vt_input(&self) -> bool {
        self.vt_input
    }
}

impl Drop for MouseCapture {
    fn drop(&mut self) {
        #[cfg(windows)]
        let enabled = self.vt_input;
        #[cfg(not(windows))]
        let enabled = true;

        if enabled && let Err(e) = write_command(DisableMouseCapture) {
            eprintln!("释放鼠标捕获失败 / failed to release mouse capture: {e}");
        }

        #[cfg(windows)]
        if let Some(mode) = self.saved_input_mode {
            restore_input_mode(mode);
        }
    }
}

/// 把 crossterm 终端命令的 ANSI 表示写到 stdout；Windows 上执行命令默认走 WinAPI，不会写这些序列。
///
/// Write the ANSI representation of a crossterm command to stdout; on Windows executing the command uses WinAPI
/// and does not emit these sequences.
fn write_command(command: impl Command) -> io::Result<()> {
    let mut sequence = String::new();
    command
        .write_ansi(&mut sequence)
        .map_err(|_| io::Error::other("无法编码终端命令 / failed to encode terminal command"))?;
    let mut stdout = io::stdout();
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()
}

//AI=== Windows：打开 ENABLE_VIRTUAL_TERMINAL_INPUT，让 ConPTY 交付完整 VT 序列而不是裸鼠标字节
// Windows: enable ENABLE_VIRTUAL_TERMINAL_INPUT so ConPTY delivers full VT sequences instead of raw mouse bytes.
#[cfg(windows)]
fn enable_vt_input() -> io::Result<u32> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE,
        SetConsoleMode,
    };

    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(io::Error::other(
            "stdin 不是控制台句柄 / stdin is not a console handle",
        ));
    }

    let mut mode = 0u32;
    if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_INPUT) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(mode)
}

#[cfg(windows)]
fn restore_input_mode(mode: u32) {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode};

    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return;
    }
    if unsafe { SetConsoleMode(handle, mode) } == 0 {
        warn!(
            "恢复控制台输入模式失败 / failed to restore console input mode: {}",
            io::Error::last_os_error()
        );
    }
}
//AI_END===

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
