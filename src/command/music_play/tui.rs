pub mod app;
pub mod channel_writer;
#[cfg(test)]
mod test;
pub mod ui;

use crate::command::music_play::runtime::Runtime;
use crate::command::music_play::scanner::ScanMessage;
//AI=== seek 手势状态 / seek gesture state
use crate::command::music_play::tui::app::{App, FooterAction, SeekGesture, Window};
//AI_END===
use crate::command::tui_input::{
    ESC_PENDING_TIMEOUT, VtEvent, VtInput, WHEEL_STEP, spawn_stdin_reader,
};
use ratatui::DefaultTerminal;
//AI=== 鼠标事件用于滚轮与点击播放列表 / mouse events drive wheel scrolling and playlist clicks
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
//AI_END===
use std::error::Error;
//AI=== 命令模式解析循环模式 / command mode parses the loop mode
use std::str::FromStr;
//AI_END===
use std::sync::mpsc::Receiver;
//AI=== 方向键长按检测 / arrow-key hold detection
use std::time::{Duration, Instant};
//AI_END===
use tracing::{debug, error};
//AI_END===
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
        app.apply(runtime.snapshot(app.window == Window::Effects));

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
                app.playlist.tracks.push(display);
            }
            ScanMessage::Progress { files, dirs } => app.scan.discovery = (files, dirs),
            ScanMessage::Done => app.scan.done = true,
        }

        if should_start(*started, shuffle, app.playlist.tracks.len(), app.scan.done) {
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
    //AI=== 命令输入优先；其余按键按当前窗口路由，窗口内不落到主界面 / command input first; other keys route by active window, never fall through
    if app.command.input.is_some() {
        handle_command_key(code, modifiers, runtime, app);
        return;
    }
    if code == KeyCode::Esc && app.window != Window::None {
        app.close_window();
        return;
    }
    match app.window {
        Window::None => handle_main_key(code, modifiers, runtime, app),
        Window::Help => handle_help_key(code, app),
        Window::Debug => handle_debug_key(code, app),
        Window::Effects => handle_effects_key(code, runtime, app),
    }
    //AI_END===
}

//AI=== 主界面按键 / main-view keys
fn handle_main_key(code: KeyCode, modifiers: KeyModifiers, runtime: &Runtime, app: &mut App) {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => app.should_quit = true,
        //AI=== `:` 进入命令模式 / `:` opens command mode
        KeyCode::Char(':') => {
            app.command.input = Some(String::new());
            app.command.error = None;
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
        //AI=== Debug/效果器改用功能键与 e / debug uses an F-key, effects uses e
        KeyCode::F(12) => app.toggle_debug(),
        KeyCode::Char('e') => app.toggle_effects(),
        //AI_END===
        KeyCode::Char('+') | KeyCode::Char('=') => runtime.add_volume(0.05),
        KeyCode::Char('-') => runtime.add_volume(-0.05),
        _ => {}
    }
}
//AI_END===

//AI=== 窗口专属按键：与主界面完全隔离 / window-only keys, fully isolated from the main view
fn handle_help_key(code: KeyCode, app: &mut App) {
    if code == KeyCode::Char('?') {
        app.close_window();
    }
}

fn handle_debug_key(code: KeyCode, app: &mut App) {
    if code == KeyCode::F(12) {
        app.close_window();
    }
}

fn handle_effects_key(code: KeyCode, runtime: &Runtime, app: &mut App) {
    match code {
        KeyCode::Char('e') => app.close_window(),
        KeyCode::Up => app.select_effect(-1),
        KeyCode::Down => app.select_effect(1),
        KeyCode::PageUp => app.select_effect(-5),
        KeyCode::PageDown => app.select_effect(5),
        KeyCode::Home => app.effects.selected = 0,
        KeyCode::End => app.effects.selected = app.effects.list.len().saturating_sub(1),
        //AI=== 空格/回车切换选中效果器的启用状态 / Space/Enter toggles the selected effect
        KeyCode::Char(' ') | KeyCode::Enter => {
            let Some(effect) = app.effects.list.get(app.effects.selected) else {
                return;
            };
            if !effect.can_disable {
                return;
            }
            let chain = effect.chain;
            let index = effect.index;
            let enabled = !effect.enabled;
            //AI=== 命令异步应用，最终状态由快照确认，不做本地乐观更新 / the command is async; wait for the snapshot
            if !runtime.set_effect_enabled(chain, index, enabled) {
                debug!(
                    "效果器开关请求被拒绝 / effect toggle request rejected: chain={chain:?} index={index}"
                );
            }
            //AI_END===
        }
        //AI_END===
        _ => {}
    }
}
//AI_END===

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
    let duration = app.playback.duration;

    if let Some(gesture) = app.seek.gesture.as_mut()
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
    let target = seek_target(app.playback.pos, direction, duration);
    report(runtime.seek(target));
    app.seek.gesture = Some(SeekGesture {
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
    let Some(gesture) = app.seek.gesture.as_ref() else {
        return;
    };
    let idle = gesture.last.elapsed();
    if gesture.pending {
        if idle < SEEK_RELEASE_DELAY {
            return;
        }
        let target = gesture.target;
        app.seek.gesture = None;
        report(runtime.seek(target));
    } else if idle >= SEEK_HOLD_WINDOW {
        app.seek.gesture = None;
    }
}
//AI_END===

//AI=== 鼠标：列表内滚轮滚动，左键点击曲目播放 / mouse: wheel scrolls inside the list, left click plays a track
fn handle_mouse(mouse: MouseEvent, runtime: &Runtime, app: &mut App) {
    //AI=== 命令输入或任何窗口显示时忽略鼠标 / ignore the mouse while typing a command or while a window is open
    if app.command.input.is_some() || app.window != Window::None {
        return;
    }
    //AI_END===
    debug!(
        "鼠标 / mouse: {:?} at ({}, {})",
        mouse.kind, mouse.column, mouse.row
    );
    let position = ratatui::layout::Position::new(mouse.column, mouse.row);
    //AI=== 底栏按钮优先：提示文字本身就是点击目标 / footer buttons first: the hint labels are the targets
    if app.footer.area.contains(position) {
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(action) = app.footer_action_at(mouse.column)
        {
            debug!("底栏按钮 / footer button: {action:?}");
            handle_footer_action(action, runtime, app);
        }
        return;
    }
    //AI_END===
    if !app.playlist.area.contains(position) {
        return;
    }
    match mouse.kind {
        MouseEventKind::ScrollUp => app.scroll_playlist(-WHEEL_STEP),
        MouseEventKind::ScrollDown => app.scroll_playlist(WHEEL_STEP),
        MouseEventKind::Down(MouseButton::Left) => {
            let index = app.playlist_index_at(mouse.column, mouse.row);
            //AI=== 点击诊断：行命中/未命中与列表状态 / click diagnostics: hit/miss, area and list state
            debug!(
                "播放列表点击 / playlist click: index={index:?} area={:?} offset={} len={}",
                app.playlist.area,
                app.playlist.offset,
                app.playlist.tracks.len()
            );
            //AI_END===
            //AI=== 双击才切换曲目，单击避免误触 / only a double click switches tracks so a single click cannot misfire
            if let Some(index) = index
                && app.mouse.register_click(index)
            {
                report(runtime.play_at(index));
            }
            //AI_END===
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
        FooterAction::Toggle => {
            if runtime.is_playing() {
                report(runtime.pause());
            } else {
                report(runtime.play());
            }
        }
        FooterAction::Debug => app.toggle_debug(),
        //AI=== 底栏效果器按钮 / footer effects button
        FooterAction::Effects => app.toggle_effects(),
        //AI_END===
        FooterAction::Command => {
            app.command.input = Some(String::new());
            app.command.error = None;
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
    //AI=== 效果器窗口命令 / effects-window command
    ("effects", "effects", "开关效果器窗口"),
    //AI_END===
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
            app.seek.gesture = None;
            let target = if let Some(relative) = arg.strip_prefix('+') {
                app.playback
                    .pos
                    .checked_add(parse_time(relative)?)
                    .ok_or_else(|| "目标时间超出范围".to_string())?
            } else if let Some(relative) = arg.strip_prefix('-') {
                app.playback.pos.saturating_sub(parse_time(relative)?)
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
            let volume = parse_volume(arg, app.playback.volume)?;
            runtime.add_volume(volume - app.playback.volume);
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
            if index == 0 || index > app.playlist.tracks.len() {
                return Err(format!("序号超出范围：1-{}", app.playlist.tracks.len()));
            }
            command_result(runtime.play_at(index - 1))?;
        }
        "debug" => app.toggle_debug(),
        //AI=== 效果器窗口命令 / effects-window command
        "effects" => app.toggle_effects(),
        //AI_END===
        "help" => app.open_help(),
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
        KeyCode::Esc => app.command.input = None,
        KeyCode::Enter => {
            let line = app.command.input.take().unwrap_or_default();
            match execute_command(&line, runtime, app) {
                Ok(()) => app.command.error = None,
                Err(message) => app.command.error = Some(message),
            }
        }
        KeyCode::Backspace => {
            if let Some(buffer) = app.command.input.as_mut() {
                buffer.pop();
            }
        }
        KeyCode::Char(c) if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            if let Some(buffer) = app.command.input.as_mut() {
                buffer.push(c);
            }
        }
        _ => {}
    }
}
//AI_END===
