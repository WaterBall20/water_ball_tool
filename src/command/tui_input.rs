//! 两个 TUI 共用的终端输入 / terminal input shared by both TUIs.
//!
//! 鼠标捕获、Windows VT 报文解析、列表双击状态与行命中。
//!
//! Mouse capture, Windows VT report parsing, list double-click state and row hit-testing.

//AI=== 公共 TUI 输入：鼠标捕获、VT 解析与列表命中 / shared TUI input: mouse capture, VT parsing and list hit-testing
use ratatui::crossterm::Command;
use ratatui::crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyCode, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use std::error::Error;
use std::io::{self, Read, Write};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use tracing::warn;
use vtparse::{CsiParam, VTActor, VTParser};

/// 滚轮一次滚动的行数 / rows per wheel notch.
pub(crate) const WHEEL_STEP: isize = 3;

/// 单独的 ESC 等待后续字节的超时 / timeout for a lone ESC to wait for the rest of a sequence.
pub(crate) const ESC_PENDING_TIMEOUT: Duration = Duration::from_millis(50);

/// 双击判定窗口 / double-click window.
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(500);

/// 列表双击状态 / list double-click state.
#[derive(Default)]
pub(crate) struct MouseState {
    last_click: Option<(Instant, usize)>,
}

impl MouseState {
    /// 记录一次左键点击并返回是否构成双击。
    ///
    /// Record a left click and report whether it completes a double click.
    pub(crate) fn register_click(&mut self, index: usize) -> bool {
        let double = self
            .last_click
            .take()
            .is_some_and(|(at, previous)| previous == index && at.elapsed() < DOUBLE_CLICK_WINDOW);
        if !double {
            self.last_click = Some((Instant::now(), index));
        }
        double
    }

    #[cfg(test)]
    pub(crate) fn set_last_click(&mut self, at: Instant, index: usize) {
        self.last_click = Some((at, index));
    }
}

/// 带边框列表中鼠标位置对应的行索引；边框与视口外返回 `None`。
///
/// Row index under a mouse position in a bordered list; `None` on the border or outside the viewport.
pub(crate) fn list_index_at(
    area: Rect,
    offset: usize,
    len: usize,
    column: u16,
    row: u16,
) -> Option<usize> {
    if !area.contains(Position::new(column, row)) {
        return None;
    }
    let inner_row = row.checked_sub(area.y.saturating_add(1))?;
    if inner_row >= area.height.saturating_sub(2) {
        return None;
    }
    let index = offset + usize::from(inner_row);
    (index < len).then_some(index)
}

//AI=== 鼠标捕获：TUI 中滚轮与点击列表需要；Drop 释放，panic 展开时也能恢复
// Mouse capture for wheel scrolling and list clicks; Drop releases it, including during unwinding.
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

//AI=== Windows VT 输入解析：ConPTY 把鼠标/方向键作为 ANSI 字节流交付，用 vtparse 解析。
// Windows VT input parsing: ConPTY delivers mouse/arrow keys as an ANSI byte stream; parse it with vtparse.
pub(crate) struct VtInput {
    parser: VTParser,
    events: Vec<VtEvent>,
    x10: X10State,
    //AI=== 单独的 ESC 可能是按键，也可能是序列前缀 / a lone ESC may be a key or the prefix of a sequence
    pending_esc: Option<Instant>,
    //AI_END===
}

#[derive(Default)]
struct X10State {
    active: bool,
    filled: usize,
    payload: [u8; 3],
}

impl X10State {
    fn start(&mut self) {
        self.active = true;
        self.filled = 0;
    }

    fn push(&mut self, byte: u8) -> Option<[u8; 3]> {
        self.payload[self.filled] = byte;
        self.filled += 1;
        if self.filled < self.payload.len() {
            return None;
        }
        self.active = false;
        Some(self.payload)
    }
}

pub(crate) enum VtEvent {
    Key(KeyCode, KeyModifiers),
    Mouse(MouseEventKind, u16, u16),
}

impl VtInput {
    pub(crate) fn new() -> Self {
        Self {
            parser: VTParser::new(),
            events: Vec::new(),
            x10: X10State::default(),
            //AI=== 默认没有挂起的 ESC / no pending ESC by default
            pending_esc: None,
            //AI_END===
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        //AI=== ESC 与后续字节拆成两次读取时：以 [ / O 开头的仍是转义序列，否则前一个 ESC 是独立按键
        // When ESC and the rest arrive in separate reads: a chunk starting with [ / O is still an escape
        // sequence; otherwise the pending ESC was a standalone key.
        if self.pending_esc.take().is_some() && !matches!(bytes.first(), Some(b'[' | b'O')) {
            self.parser = VTParser::new();
            self.x10 = X10State::default();
            self.events
                .push(VtEvent::Key(KeyCode::Esc, KeyModifiers::empty()));
        }
        self.pending_esc = (bytes.len() == 1 && bytes[0] == 0x1b).then(Instant::now);
        //AI_END===
        let Self {
            parser,
            events,
            x10,
            ..
        } = self;
        let mut actor = VtActor { events, x10 };
        parser.parse(bytes, &mut actor);
    }

    //AI=== 单独的 ESC 超时未等到后续字节，才作为 Esc 键（同时复位解析器）
    // A lone ESC that has waited past the timeout becomes the Esc key (and the parser is reset).
    pub(crate) fn take_lone_esc(&mut self, timeout: Duration) -> bool {
        if let Some(at) = self.pending_esc
            && at.elapsed() >= timeout
        {
            self.pending_esc = None;
            self.parser = VTParser::new();
            self.x10 = X10State::default();
            self.events
                .push(VtEvent::Key(KeyCode::Esc, KeyModifiers::empty()));
            return true;
        }
        false
    }
    //AI_END===

    pub(crate) fn drain(&mut self) -> Vec<VtEvent> {
        std::mem::take(&mut self.events)
    }
}

struct VtActor<'a> {
    events: &'a mut Vec<VtEvent>,
    x10: &'a mut X10State,
}

impl VTActor for VtActor<'_> {
    fn print(&mut self, b: char) {
        if self.x10.active {
            let byte = u8::try_from(b as u32).unwrap_or(b' ');
            if let Some(payload) = self.x10.push(byte)
                && let Some((kind, column, row)) = x10_mouse(payload)
            {
                self.events.push(VtEvent::Mouse(kind, column, row));
            }
            return;
        }

        let code = if b == '\u{7f}' {
            KeyCode::Backspace
        } else {
            KeyCode::Char(b)
        };
        self.events.push(VtEvent::Key(code, KeyModifiers::empty()));
    }

    fn execute_c0_or_c1(&mut self, control: u8) {
        let event = match control {
            0x03 => Some((KeyCode::Char('c'), KeyModifiers::CONTROL)),
            0x08 | 0x7f => Some((KeyCode::Backspace, KeyModifiers::empty())),
            0x0a | 0x0d => Some((KeyCode::Enter, KeyModifiers::empty())),
            0x09 => Some((KeyCode::Tab, KeyModifiers::empty())),
            _ => None,
        };
        if let Some((code, modifiers)) = event {
            self.events.push(VtEvent::Key(code, modifiers));
        }
    }

    fn dcs_hook(&mut self, _: u8, _: &[i64], _: &[u8], _: bool) {}

    fn dcs_put(&mut self, _: u8) {}

    fn dcs_unhook(&mut self) {}

    fn esc_dispatch(&mut self, _: &[i64], _: &[u8], _: bool, _: u8) {}

    fn csi_dispatch(&mut self, params: &[CsiParam], _: bool, byte: u8) {
        if byte == b'M' && params.is_empty() {
            // X10 鼠标：CSI M + 3 个原始字节；不支持 SGR 的终端会走这里
            // X10 mouse: CSI M plus 3 raw bytes; terminals without SGR fall back here.
            self.x10.start();
            return;
        }
        if let Some((kind, column, row)) = sgr_mouse(params, byte) {
            self.events.push(VtEvent::Mouse(kind, column, row));
            return;
        }

        let code = match byte {
            b'A' => Some(KeyCode::Up),
            b'B' => Some(KeyCode::Down),
            b'C' => Some(KeyCode::Right),
            b'D' => Some(KeyCode::Left),
            b'H' => Some(KeyCode::Home),
            b'F' => Some(KeyCode::End),
            b'Z' => Some(KeyCode::BackTab),
            b'~' => match params.first().and_then(csi_integer) {
                Some(5) => Some(KeyCode::PageUp),
                Some(6) => Some(KeyCode::PageDown),
                Some(1 | 7) => Some(KeyCode::Home),
                Some(4 | 8) => Some(KeyCode::End),
                Some(3) => Some(KeyCode::Delete),
                Some(n @ 11..=15) => Some(KeyCode::F(u8::try_from(n - 10).unwrap_or(1))),
                Some(n @ 17..=21) => Some(KeyCode::F(u8::try_from(n - 16).unwrap_or(6))),
                Some(23) => Some(KeyCode::F(11)),
                Some(24) => Some(KeyCode::F(12)),
                _ => None,
            },
            _ => None,
        };
        if let Some(code) = code {
            self.events.push(VtEvent::Key(code, KeyModifiers::empty()));
        }
    }

    fn osc_dispatch(&mut self, _: &[&[u8]]) {}

    fn apc_dispatch(&mut self, _: Vec<u8>) {}
}

fn csi_integer(param: &CsiParam) -> Option<u16> {
    match param {
        CsiParam::Integer(value) => u16::try_from(*value).ok(),
        CsiParam::P(_) => None,
    }
}

/// SGR 鼠标报文：`CSI < b ; x ; y M/m`，坐标是 1-based。
///
/// SGR mouse report: `CSI < b ; x ; y M/m`; coordinates are 1-based.
fn sgr_mouse(params: &[CsiParam], byte: u8) -> Option<(MouseEventKind, u16, u16)> {
    if !matches!(byte, b'M' | b'm') {
        return None;
    }
    let numbers: Vec<u16> = params.iter().filter_map(csi_integer).collect();
    if numbers.len() < 3 {
        return None;
    }
    let button = numbers[0];
    let column = numbers[1].saturating_sub(1);
    let row = numbers[2].saturating_sub(1);

    let kind = match (byte, button & 0b110_0011) {
        (b'M', 0) => MouseEventKind::Down(MouseButton::Left),
        (b'M', 64) => MouseEventKind::ScrollUp,
        (b'M', 65) => MouseEventKind::ScrollDown,
        _ => return None,
    };
    Some((kind, column, row))
}

fn x10_mouse(payload: [u8; 3]) -> Option<(MouseEventKind, u16, u16)> {
    let button = payload[0].saturating_sub(32);
    let column = u16::from(payload[1].saturating_sub(33));
    let row = u16::from(payload[2].saturating_sub(33));
    let kind = if button & 0x40 != 0 {
        if button & 1 == 0 {
            MouseEventKind::ScrollUp
        } else {
            MouseEventKind::ScrollDown
        }
    } else if button & 0x20 != 0 || button & 0x03 != 0 {
        return None;
    } else {
        MouseEventKind::Down(MouseButton::Left)
    };
    Some((kind, column, row))
}

//AI=== 原始字节读取线程：VT 模式不经过 crossterm，避免 ESC/CR 被丢弃
// Raw-byte reader thread: VT mode bypasses crossterm so ESC/CR are not dropped.
pub(crate) fn spawn_stdin_reader() -> Result<Receiver<Vec<u8>>, Box<dyn Error>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("tui-stdin".to_string())
        .spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 1024];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })
        .map_err(|e| format!("创建 stdin 读取线程失败 / failed to spawn stdin reader: {e}"))?;
    Ok(rx)
}
//AI_END===
