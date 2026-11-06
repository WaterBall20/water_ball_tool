pub mod app;
pub mod channel_writer;
#[cfg(test)]
mod test;
pub mod ui;

use crate::command::music_play::runtime::Runtime;
use crate::command::music_play::scanner::ScanMessage;
//AI=== seek 手势状态 / seek gesture state
use crate::command::music_play::tui::app::{App, FooterAction, SeekGesture};
//AI_END===
use ratatui::DefaultTerminal;
//AI=== 鼠标事件用于滚轮与点击播放列表 / mouse events drive wheel scrolling and playlist clicks
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
//AI_END===
use std::error::Error;
//AI=== Windows VT 输入改为直接读 stdin 字节 / Windows VT input reads raw stdin bytes
use std::io::Read;
//AI_END===
//AI=== 命令模式解析循环模式 / command mode parses the loop mode
use std::str::FromStr;
//AI_END===
use std::sync::mpsc::Receiver;
//AI=== 方向键长按检测 / arrow-key hold detection
use std::time::{Duration, Instant};
//AI_END===
use tracing::{debug, error};
//AI_END===
//AI=== Windows VT 输入解析器 / Windows VT input parser
use vtparse::{CsiParam, VTActor, VTParser};
use water_ball_tool_lib::music_play::LoopMode;

// 50ms 一帧：进度条同时以这个频率刷新 / one frame every 50ms; the progress bar refreshes at the same rate
const TICK: Duration = Duration::from_millis(50);

//AI=== 左右方向键：单次 5s；700ms 内视为同一次长按；停手 150ms 后提交预览
// Left/right arrows: 5s per press; events within 700ms belong to one hold; commit the preview 150ms after it stops.
const SEEK_STEP: Duration = Duration::from_secs(5);
/// 长按每秒推进的预览时长 / preview seconds advanced per second of holding
const SEEK_HOLD_RATE: f64 = 10.0;
const SEEK_HOLD_WINDOW: Duration = Duration::from_millis(700);
const SEEK_RELEASE_DELAY: Duration = Duration::from_millis(150);
//AI_END===

//AI=== 单独的 ESC 等待后续字节的时长；超时才当作 Esc 键 / how long a lone ESC waits for the rest of a sequence
const ESC_PENDING_TIMEOUT: Duration = Duration::from_millis(50);
//AI_END===

// 随机模式先攒够 5 首再初始化；扫描结束时不足 5 首也会兜底初始化
// Shuffle modes collect 5 tracks before initializing; at scan end fewer tracks still initialize.
const SHUFFLE_INIT_TRACKS: usize = 5;

//AI=== TUI 主循环：日志/扫描/播放快照 → 渲染 → 按键
// TUI main loop: logs/scan/playback snapshot → render → keys.
pub(super) fn run(
    terminal: &mut DefaultTerminal,
    runtime: &mut Runtime,
    app: &mut App,
    logs: &Receiver<channel_writer::LogLine>,
    scans: &Receiver<ScanMessage>,
    autoplay: bool,
    vt_input: bool,
) -> Result<(), Box<dyn Error>> {
    let shuffle = matches!(
        runtime.loop_mode(),
        LoopMode::Shuffle | LoopMode::PeriodicShuffle
    );
    let mut started = false;
    //AI=== Windows VT 输入：方向键/鼠标等转义序列交给 ANSI 解析器
    // Windows VT input: escape sequences (arrows/mouse) go through the ANSI parser.
    let mut vt = VtInput::new();
    // crossterm 0.29 在 VT 输入模式下会丢弃 ESC/CR 等控制字符，键盘改读原始字节
    // crossterm 0.29 drops control chars such as ESC/CR under VT input, so read raw bytes instead.
    let stdin_rx = if vt_input {
        Some(spawn_stdin_reader()?)
    } else {
        None
    };
    //AI_END===
    loop {
        drain_logs(logs, app);
        drain_scans(scans, runtime, app, autoplay, shuffle, &mut started);
        app.apply(runtime.snapshot());

        terminal.draw(|frame| ui::render(app, frame))?;

        //AI=== VT 输入：收原始字节 → vtparse；单独 ESC 超时才作为按键
        // VT input: collect raw bytes into vtparse; a lone ESC becomes a key only after the timeout.
        if let Some(stdin) = &stdin_rx {
            match stdin.recv_timeout(TICK) {
                Ok(bytes) => {
                    vt.feed(&bytes);
                    while let Ok(more) = stdin.try_recv() {
                        vt.feed(&more);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            dispatch_vt(&mut vt, runtime, app);
            if vt.take_lone_esc(ESC_PENDING_TIMEOUT) {
                dispatch_vt(&mut vt, runtime, app);
            }
        } else if event::poll(TICK)? {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    debug!(
                        "按键 / key: {:?} modifiers={:?} kind={:?}",
                        key.code, key.modifiers, key.kind
                    );
                    handle_key(key.code, key.modifiers, runtime, app);
                }
                Event::Mouse(mouse) => handle_mouse(mouse, runtime, app),
                _ => {}
            }
        }
        //AI_END===

        if app.should_quit {
            break;
        }
        //AI=== 长按停手后提交预览跳转 / commit the preview jump once the hold goes quiet
        commit_seek_gesture(runtime, app);
        //AI_END===
    }
    Ok(())
}

fn drain_logs(logs: &Receiver<channel_writer::LogLine>, app: &mut App) {
    while let Ok(line) = logs.try_recv() {
        app.push_log(line);
    }
}

fn drain_scans(
    scans: &Receiver<ScanMessage>,
    runtime: &mut Runtime,
    app: &mut App,
    autoplay: bool,
    shuffle: bool,
    started: &mut bool,
) {
    // 用户已经手动加载了曲目（n / 播放），不再自动初始化
    // The user already loaded a track manually (next / play), so skip automatic initialization.
    if !*started && runtime.has_current_track() {
        *started = true;
    }

    while let Ok(message) = scans.try_recv() {
        match message {
            ScanMessage::Track(track) => {
                let display = format!("{} — {}", track.title(), track.artist());
                runtime.add_track(track);
                app.playlist.push(display);
            }
            ScanMessage::Progress { files, dirs } => app.discovery = (files, dirs),
            ScanMessage::Done => app.scan_done = true,
        }

        if should_start(*started, shuffle, app.playlist.len(), app.scan_done) {
            start(runtime, autoplay);
            *started = true;
        }
    }
}

/// 初始化 = 加载第一首；只有 `--autoplay` 才真正开始播放
///
/// Initialization = load the first track; only `--autoplay` actually starts playback.
fn start(runtime: &Runtime, autoplay: bool) {
    report(runtime.next_track());
    if autoplay {
        report(runtime.play());
    }
}

/// 是否已经可以初始化：随机模式等 5 首，扫描结束不足 5 首时兜底；其他模式第一首即可
///
/// Whether initialization can run: shuffle waits for 5 tracks with a scan-end fallback; other modes start with the
/// first track.
fn should_start(started: bool, shuffle: bool, loaded: usize, scan_done: bool) -> bool {
    if started || loaded == 0 {
        return false;
    }
    if scan_done {
        return true;
    }
    !shuffle || loaded >= SHUFFLE_INIT_TRACKS
}

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
fn spawn_stdin_reader() -> Result<Receiver<Vec<u8>>, Box<dyn Error>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("mp-stdin".to_string())
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

fn dispatch_vt(vt: &mut VtInput, runtime: &Runtime, app: &mut App) {
    for event in vt.drain() {
        match event {
            VtEvent::Key(code, modifiers) => {
                debug!("VT 按键 / vt key: {:?} modifiers={:?}", code, modifiers);
                handle_key(code, modifiers, runtime, app);
            }
            VtEvent::Mouse(kind, column, row) => handle_mouse(
                MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::empty(),
                },
                runtime,
                app,
            ),
        }
    }
}
//AI_END===

fn handle_key(code: KeyCode, modifiers: KeyModifiers, runtime: &Runtime, app: &mut App) {
    //AI=== 命令输入优先；帮助弹窗用 Esc 关闭 / command input takes priority; Esc closes the help popup
    if app.command.is_some() {
        handle_command_key(code, modifiers, runtime, app);
        return;
    }
    if app.show_help && code == KeyCode::Esc {
        app.show_help = false;
        return;
    }
    //AI_END===
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => app.should_quit = true,
        //AI=== `:` 进入命令模式 / `:` opens command mode
        KeyCode::Char(':') => {
            app.command = Some(String::new());
            app.command_error = None;
            app.show_help = false;
        }
        //AI_END===
        //AI=== `n` 下一曲；左右方向键跳转进度 / `n` next track; left/right arrows seek
        KeyCode::Char('n') => report(runtime.next_track()),
        KeyCode::Left => handle_seek_key(-1, runtime, app),
        KeyCode::Right => handle_seek_key(1, runtime, app),
        //AI_END===
        //AI=== 播放列表滚动键：上下、翻页、首尾 / playlist scroll keys: line, page and ends
        KeyCode::Up => app.scroll_playlist(-1),
        KeyCode::Down => app.scroll_playlist(1),
        KeyCode::PageUp => app.scroll_playlist_page(-1),
        KeyCode::PageDown => app.scroll_playlist_page(1),
        KeyCode::Home => app.scroll_playlist_to(0),
        KeyCode::End => app.scroll_playlist_to(usize::MAX),
        //AI_END===
        KeyCode::Char(' ') => {
            if runtime.is_playing() {
                report(runtime.pause());
            } else {
                report(runtime.play());
            }
        }
        KeyCode::Char('p') => report(runtime.pause()),
        KeyCode::Char('P') => report(runtime.play()),
        KeyCode::Char('d') | KeyCode::F(12) => app.show_debug = !app.show_debug,
        KeyCode::Char('+') | KeyCode::Char('=') => runtime.add_volume(0.05),
        KeyCode::Char('-') => runtime.add_volume(-0.05),
        _ => {}
    }
}

//AI=== 左右方向键 seek：点按立刻 ±5s；重复事件只移动预览，停手后一次提交
// Left/right seek: a tap steps ±5s immediately; repeats only move the preview and are committed once, on release.
fn seek_target(base: Duration, direction: i8, duration: Duration) -> Duration {
    seek_by(base, direction, SEEK_STEP, duration)
}

/// 从起点按给定偏移计算目标，并夹到曲目时长 / compute a target from a base and offset, clamped to the duration
fn seek_by(base: Duration, direction: i8, offset: Duration, duration: Duration) -> Duration {
    let target = if direction < 0 {
        base.saturating_sub(offset)
    } else {
        base.saturating_add(offset)
    };
    if duration.is_zero() {
        target
    } else {
        target.min(duration)
    }
}

fn handle_seek_key(direction: i8, runtime: &Runtime, app: &mut App) {
    let now = Instant::now();
    let duration = app.duration;

    if let Some(gesture) = app.seek_gesture.as_mut()
        && gesture.direction == direction
        && now.duration_since(gesture.last) <= SEEK_HOLD_WINDOW
    {
        // 长按预览按持有时间推进，避免跟随终端重复速率 / advance by held time so the terminal repeat rate cannot
        // make a hold jump hundreds of seconds.
        let held = now.duration_since(gesture.started);
        let offset = Duration::from_secs_f64((held.as_secs_f64() * SEEK_HOLD_RATE).min(1.0e9));
        gesture.last = now;
        gesture.target = seek_by(gesture.start, direction, offset, duration);
        gesture.pending = true;
        return;
    }

    // 新手势：第一下立即生效（点按语义），后续重复只改预览
    // A new gesture applies its first step immediately (the tap case); later repeats only move the preview.
    let target = seek_target(app.pos, direction, duration);
    report(runtime.seek(target));
    app.seek_gesture = Some(SeekGesture {
        target,
        pending: false,
        last: now,
        direction,
        started: now,
        start: target,
    });
}

/// 停手后提交长按预览；点按已即时提交，过重复窗口后只需清除状态
/// Commit the hold preview once the keys go quiet; a tap was already applied and only needs its state cleared.
fn commit_seek_gesture(runtime: &Runtime, app: &mut App) {
    let Some(gesture) = app.seek_gesture.as_ref() else {
        return;
    };
    let idle = gesture.last.elapsed();
    if gesture.pending {
        if idle < SEEK_RELEASE_DELAY {
            return;
        }
        let target = gesture.target;
        app.seek_gesture = None;
        report(runtime.seek(target));
    } else if idle >= SEEK_HOLD_WINDOW {
        app.seek_gesture = None;
    }
}
//AI_END===

//AI=== 鼠标：列表内滚轮滚动，左键点击曲目播放 / mouse: wheel scrolls inside the list, left click plays a track
fn handle_mouse(mouse: MouseEvent, runtime: &Runtime, app: &mut App) {
    //AI=== 命令输入与帮助弹窗期间忽略鼠标 / ignore the mouse while typing a command or reading help
    if app.command.is_some() || app.show_help {
        return;
    }
    //AI_END===
    debug!(
        "鼠标 / mouse: {:?} at ({}, {})",
        mouse.kind, mouse.column, mouse.row
    );
    let position = ratatui::layout::Position::new(mouse.column, mouse.row);
    //AI=== 底栏按钮优先：提示文字本身就是点击目标 / footer buttons first: the hint labels are the targets
    if app.footer_area.contains(position) {
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(action) = app.footer_action_at(mouse.column)
        {
            debug!("底栏按钮 / footer button: {action:?}");
            handle_footer_action(action, runtime, app);
        }
        return;
    }
    //AI_END===
    if !app.playlist_area.contains(position) {
        return;
    }
    match mouse.kind {
        MouseEventKind::ScrollUp => app.scroll_playlist(-3),
        MouseEventKind::ScrollDown => app.scroll_playlist(3),
        MouseEventKind::Down(MouseButton::Left) => {
            let index = app.playlist_index_at(mouse.column, mouse.row);
            //AI=== 点击诊断：行命中/未命中与列表状态 / click diagnostics: hit/miss, area and list state
            debug!(
                "播放列表点击 / playlist click: index={index:?} area={:?} offset={} len={}",
                app.playlist_area,
                app.playlist_offset,
                app.playlist.len()
            );
            //AI_END===
            if let Some(index) = index {
                report(runtime.play_at(index));
            }
        }
        _ => {}
    }
}
//AI_END===

//AI=== 底栏按钮动作 / footer button actions
fn handle_footer_action(action: FooterAction, runtime: &Runtime, app: &mut App) {
    match action {
        FooterAction::Quit => app.should_quit = true,
        FooterAction::Next => report(runtime.next_track()),
        FooterAction::SeekBack | FooterAction::SeekForward => {
            let direction = if action == FooterAction::SeekBack {
                -1
            } else {
                1
            };
            app.seek_gesture = None;
            report(runtime.seek(seek_target(app.pos, direction, app.duration)));
        }
        FooterAction::Toggle => {
            if runtime.is_playing() {
                report(runtime.pause());
            } else {
                report(runtime.play());
            }
        }
        FooterAction::ScrollUp => app.scroll_playlist(-1),
        FooterAction::ScrollDown => app.scroll_playlist(1),
        FooterAction::VolumeDown => runtime.add_volume(-0.05),
        FooterAction::VolumeUp => runtime.add_volume(0.05),
        FooterAction::Debug => app.show_debug = !app.show_debug,
        FooterAction::Command => {
            app.command = Some(String::new());
            app.command_error = None;
            app.show_help = false;
        }
    }
}
//AI_END===

fn report(result: Result<(), Box<dyn Error>>) {
    if let Err(e) = result {
        error!("播放操作失败 / playback command failed: {e}");
    }
}
//AI_END===

//AI=== 命令模式：`:` 前缀 + 唯一前缀匹配 / command mode: `:` prefix with unique-prefix matching
/// 命令表：名称、用法、说明；`?` 是 help 的别名。
///
/// Command table: name, usage, description; `?` is an alias for `help`.
pub(super) const COMMANDS: &[(&str, &str, &str)] = &[
    ("quit", "quit", "退出播放器"),
    ("play", "play", "开始播放"),
    ("pause", "pause", "暂停"),
    ("toggle", "toggle", "播放 / 暂停"),
    ("next", "next", "下一曲"),
    ("prev", "prev", "上一曲"),
    (
        "seek",
        "seek <mm:ss|+N|-N>",
        "跳转进度（秒或 mm:ss，+/- 为相对）",
    ),
    ("vol", "vol <0-100|+N|-N>", "设置或增减音量"),
    (
        "mode",
        "mode <name>",
        "循环模式：sequential-loop|sequential|repeat-one|shuffle|periodic-shuffle",
    ),
    ("goto", "goto <n>", "跳到第 n 首并播放"),
    ("debug", "debug", "开关 Debug 弹窗"),
    ("help", "help", "显示帮助（? 同义）"),
];

/// 唯一前缀匹配；无匹配或歧义都返回面向用户的错误 / unique-prefix match; no match or ambiguity is a user-facing error
fn resolve_command(input: &str) -> Result<&'static str, String> {
    let input = if input == "?" { "help" } else { input };
    let mut names = COMMANDS
        .iter()
        .map(|(name, _, _)| *name)
        .filter(|name| name.starts_with(input));
    let first = names
        .next()
        .ok_or_else(|| format!("未知命令：{input}（:help 查看）"))?;
    match names.next() {
        Some(second) => Err(format!("命令不唯一：{input}（{first} / {second}）")),
        None => Ok(first),
    }
}

/// 解析音量参数：绝对百分比或 `+N`/`-N` 相对增减，结果夹到 0–1
/// Parse a volume argument: absolute percent or a `+N`/`-N` delta, clamped to 0–1
fn parse_volume(arg: &str, current: f32) -> Result<f32, String> {
    let parse = |text: &str| text.parse::<f32>().map_err(|_| format!("音量无效：{arg}"));
    let percent = if let Some(relative) = arg.strip_prefix('+') {
        current * 100.0 + parse(relative)?
    } else if let Some(relative) = arg.strip_prefix('-') {
        current * 100.0 - parse(relative)?
    } else {
        parse(arg)?
    };
    Ok((percent / 100.0).clamp(0.0, 1.0))
}

/// 解析时间参数：`ss`、`mm:ss` 或 `hh:mm:ss`，上限 1e9 秒避免 `from_secs_f64` panic
/// Parse a time argument: `ss`, `mm:ss` or `hh:mm:ss`; capped at 1e9 seconds so `from_secs_f64` cannot panic
fn parse_time(arg: &str) -> Result<Duration, String> {
    let mut seconds = 0.0f64;
    let mut parts = 0usize;
    for part in arg.split(':') {
        let value = part
            .parse::<f64>()
            .map_err(|_| format!("时间无效：{arg}"))?;
        if !value.is_finite() || value < 0.0 {
            return Err(format!("时间无效：{arg}"));
        }
        seconds = seconds * 60.0 + value;
        parts += 1;
    }
    if parts == 0 || parts > 3 || !(0.0..=1.0e9).contains(&seconds) {
        return Err(format!("时间无效：{arg}"));
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn execute_command(line: &str, runtime: &Runtime, app: &mut App) -> Result<(), String> {
    let mut parts = line.split_whitespace();
    let Some(name) = parts.next() else {
        return Ok(());
    };
    let args: Vec<&str> = parts.collect();

    match resolve_command(name)? {
        "quit" => app.should_quit = true,
        "play" => command_result(runtime.play())?,
        "pause" => command_result(runtime.pause())?,
        "toggle" => {
            if runtime.is_playing() {
                command_result(runtime.pause())?;
            } else {
                command_result(runtime.play())?;
            }
        }
        "next" => command_result(runtime.next_track())?,
        "prev" => command_result(runtime.prev_track())?,
        //AI=== seek：绝对时间或相对 ±N；时长夹取在 Runtime 内完成
        // seek: absolute time or a relative ±N; Runtime clamps it to the track duration.
        "seek" => {
            let Some(arg) = args.first() else {
                return Err("用法：:seek <mm:ss|+N|-N>".to_string());
            };
            // 命令行跳转取消未提交的方向键预览 / an explicit seek cancels any pending arrow preview
            app.seek_gesture = None;
            let target = if let Some(relative) = arg.strip_prefix('+') {
                app.pos
                    .checked_add(parse_time(relative)?)
                    .ok_or_else(|| "目标时间超出范围".to_string())?
            } else if let Some(relative) = arg.strip_prefix('-') {
                app.pos.saturating_sub(parse_time(relative)?)
            } else {
                parse_time(arg)?
            };
            command_result(runtime.seek(target))?;
        }
        //AI_END===
        "vol" => {
            let Some(arg) = args.first() else {
                return Err("用法：:vol <0-100|+N|-N>".to_string());
            };
            let volume = parse_volume(arg, app.volume)?;
            runtime.add_volume(volume - app.volume);
        }
        "mode" => {
            let Some(arg) = args.first() else {
                return Err(
                    "用法：:mode <sequential-loop|sequential|repeat-one|shuffle|periodic-shuffle>"
                        .to_string(),
                );
            };
            let mode = LoopMode::from_str(arg).map_err(|e| format!("模式无效：{e}"))?;
            runtime.set_loop_mode(mode);
        }
        "goto" => {
            let Some(arg) = args.first() else {
                return Err("用法：:goto <n>".to_string());
            };
            let index = arg
                .parse::<usize>()
                .map_err(|_| format!("曲目序号无效：{arg}"))?;
            if index == 0 || index > app.playlist.len() {
                return Err(format!("序号超出范围：1-{}", app.playlist.len()));
            }
            command_result(runtime.play_at(index - 1))?;
        }
        "debug" => app.show_debug = !app.show_debug,
        "help" => app.show_help = true,
        _ => unreachable!("未处理的命令 / unhandled command"),
    }
    Ok(())
}

/// 命令失败：写日志并返回可直接显示的错误文本 / command failure: log it and return displayable text
fn command_result(result: Result<(), Box<dyn Error>>) -> Result<(), String> {
    result.map_err(|e| {
        error!("命令执行失败 / command failed: {e}");
        e.to_string()
    })
}

fn handle_command_key(code: KeyCode, modifiers: KeyModifiers, runtime: &Runtime, app: &mut App) {
    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return;
    }
    match code {
        KeyCode::Esc => app.command = None,
        KeyCode::Enter => {
            let line = app.command.take().unwrap_or_default();
            match execute_command(&line, runtime, app) {
                Ok(()) => app.command_error = None,
                Err(message) => app.command_error = Some(message),
            }
        }
        KeyCode::Backspace => {
            if let Some(buffer) = app.command.as_mut() {
                buffer.pop();
            }
        }
        KeyCode::Char(c) if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            if let Some(buffer) = app.command.as_mut() {
                buffer.push(c);
            }
        }
        _ => {}
    }
}
//AI_END===
