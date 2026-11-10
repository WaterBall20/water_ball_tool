//! wbfp TUI 主循环 / wbfp TUI main loop
//!
//! 50ms 一帧，期间排空日志与任务事件、渲染、处理键盘/鼠标。
//! 任务完成前按 `q` 会等任务结束后退出，避免中途丢弃写入。
//!
//! One frame every 50ms: drain logs and job events, render, then handle
//! keyboard/mouse input. Pressing `q` while a job runs quits after it finishes,
//! so writes are never abandoned mid-flight.

//AI=== TUI 主循环与终端生命周期 / TUI main loop and terminal lifecycle
mod app;
mod browse;
mod ui;

use crate::command::music_play::tui::channel_writer;
use crate::command::tui_input::{
    ESC_PENDING_TIMEOUT, MouseCapture, VtEvent, VtInput, WHEEL_STEP, spawn_stdin_reader,
};
use crate::command::wbfp::WaterBallFilePackCommand;
use app::{App, Focus, Window};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::error::Error;
use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;
use tracing::{debug, error, info, warn};

const TICK: Duration = Duration::from_millis(50);

pub(super) fn run(args: WaterBallFilePackCommand) -> Result<(), Box<dyn Error>> {
    let logs = channel_writer::init_logging("wbfp");
    info!("wbfp TUI: 初始化日志 / logging initialized");
    let mut app = match App::from_args(args) {
        Ok(app) => app,
        Err(e) => {
            error!("wbfp TUI: 初始化失败 / initialization failed: {e}");
            return Err(e);
        }
    };
    info!(
        "wbfp TUI: 打开 {} / opening, mode={:?}, entries={}",
        app.ui.title,
        app.ui.mode,
        app.nav.entries.len()
    );
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(e) => {
            error!("wbfp TUI: 终端初始化失败 / terminal init failed: {e}");
            return Err(e.into());
        }
    };
    // 宽屏默认显示预览，窄屏默认只显示列表；`p` 随时切换 / wide screens show the preview, narrow ones the list
    if let Ok(size) = terminal.size() {
        app.nav.show_preview = size.width >= 80;
    }
    // 与 mp 共用鼠标捕获：Windows 走 VT 输入，避免 crossterm 把 SGR 报文拆成普通按键。
    // Shared with mp: Windows uses VT input so crossterm cannot split SGR reports into literal keys.
    let mouse = MouseCapture::enable().unwrap_or_else(|e| {
        warn!(
                "wbfp TUI: 鼠标初始化失败，继续无鼠标运行 / mouse setup failed, continuing without mouse: {e}"
            );
        MouseCapture::disabled()
    });
    let vt_input = mouse.vt_input();
    // VT 模式直接读原始 stdin 字节；crossterm 会丢弃 ESC/CR 等控制字符。
    // VT mode reads raw stdin bytes; crossterm drops control characters such as ESC/CR.
    let stdin_rx = if vt_input {
        Some(spawn_stdin_reader()?)
    } else {
        None
    };
    info!(
        "wbfp TUI: ready, width={}, vt_input={vt_input}",
        terminal.size().map_or(0, |s| s.width)
    );

    // 任务事件队列不阻塞 IO：工作线程只做入队，进度事件按 4 MiB 限流；UI 侧对任务列表设上限。
    // shortcut: unbounded queue + rate-limited progress; switch to a bounded lock-free queue if a job can outpace the UI by millions of events.
    let (tx, rx) = mpsc::channel();
    app.set_job_sender(tx);
    if let Err(e) = app.start_pending_job() {
        error!("wbfp TUI: 后台任务启动失败 / job spawn failed: {e}");
        drop(mouse);
        if let Err(restore) = ratatui::try_restore() {
            eprintln!("恢复终端失败 / failed to restore terminal: {restore}");
        }
        return Err(e.into());
    }

    let mut result = event_loop(&mut terminal, &mut app, &logs, &rx, stdin_rx.as_ref());
    if let Err(e) = &result {
        error!("wbfp TUI: 主循环退出 / main loop failed: {e}");
    }
    drop(mouse);
    let restore = ratatui::try_restore();

    for handle in app.take_job_handles() {
        if handle.join().is_err() && result.is_ok() {
            error!("wbfp TUI: 任务线程 panic / job thread panicked");
            result =
                Err(io::Error::other("wbfp 任务线程异常退出 / wbfp job thread panicked").into());
        }
    }
    if let Err(e) = restore {
        if result.is_ok() {
            result = Err(e.into());
        } else {
            eprintln!("恢复终端失败 / failed to restore terminal: {e}");
        }
    }
    info!("wbfp TUI: exit");
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    logs: &Receiver<channel_writer::LogLine>,
    events: &Receiver<super::events::JobEvent>,
    stdin_rx: Option<&Receiver<Vec<u8>>>,
) -> Result<(), Box<dyn Error>> {
    let mut vt = VtInput::new();
    loop {
        while let Ok(line) = logs.try_recv() {
            app.push_log(line);
        }
        drain_events(app, events);

        terminal.draw(|frame| ui::render(app, frame))?;

        if let Some(stdin) = stdin_rx {
            match stdin.recv_timeout(TICK) {
                Ok(bytes) => {
                    vt.feed(&bytes);
                    while let Ok(more) = stdin.try_recv() {
                        vt.feed(&more);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            dispatch_vt(&mut vt, app);
            if vt.take_lone_esc(ESC_PENDING_TIMEOUT) {
                dispatch_vt(&mut vt, app);
            }
        } else if event::poll(TICK)? {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    handle_key(key.code, key.modifiers, app);
                }
                Event::Mouse(mouse) => handle_mouse(mouse, app),
                _ => {}
            }
        }

        if app.ui.should_quit && !app.jobs.running {
            break;
        }
    }
    Ok(())
}

fn drain_events(app: &mut App, events: &Receiver<super::events::JobEvent>) {
    loop {
        match events.try_recv() {
            Ok(event) => app.apply_event(event),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                if app.jobs.running {
                    error!("wbfp TUI: 任务事件通道断开 / job event channel disconnected");
                    app.jobs.running = false;
                    if let Some(job) = app.jobs.state.as_mut() {
                        job.finished = true;
                        job.error =
                            Some("任务线程异常退出 / job thread exited unexpectedly".to_string());
                    }
                }
                break;
            }
        }
    }
}

fn request_quit(app: &mut App) {
    debug!(
        "quit requested: job_running={}, selected={}/{}",
        app.jobs.running,
        app.active_nav().selected,
        app.active_nav().entries.len()
    );
    app.ui.should_quit = true;
    if app.jobs.running {
        app.ui.quit_after_job = true;
    }
}

fn handle_key(code: KeyCode, modifiers: KeyModifiers, app: &mut App) {
    debug!(
        "key: {code:?} mods={modifiers:?}, window={:?}, selected={}/{}, cwd={}",
        app.ui.window,
        app.active_nav().selected,
        app.active_nav().entries.len(),
        app.display_cwd()
    );
    // 上一轮的错误只保留到下一次按键 / keep the previous error until the next key press
    app.ui.error = None;

    // 重命名输入框优先消费所有按键 / the rename editor consumes every key
    if app.rename_key(code, modifiers) {
        return;
    }
    // Esc 与退出键对所有窗口通用；其余按键按窗口枚举分发，避免互相串键。
    // Esc and the quit keys are common to every window; other keys dispatch by the window enum.
    if code == KeyCode::Esc && app.cancel_transient() {
        return;
    }
    if code == KeyCode::Char('q')
        || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL))
    {
        request_quit(app);
        return;
    }

    match app.ui.window {
        Window::Help => {
            if code == KeyCode::Char('?') {
                app.ui.window = Window::None;
            }
        }
        Window::Debug => {
            if code == KeyCode::F(12) {
                app.ui.window = Window::None;
            }
        }
        Window::Confirm => match code {
            KeyCode::Char('y') | KeyCode::Enter => app.confirm_delete(),
            KeyCode::Char('n') => app.cancel_delete(),
            _ => {}
        },
        Window::None => match code {
            KeyCode::Tab | KeyCode::BackTab => app.switch_focus(),
            KeyCode::Char('w') if modifiers.contains(KeyModifiers::CONTROL) => app.close_pack(),
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => app.back(),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => app.enter(),
            KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
            KeyCode::PageUp => app.page_selection(-1),
            KeyCode::PageDown => app.page_selection(1),
            KeyCode::Home => app.select_first(),
            KeyCode::End => app.select_last(),
            KeyCode::Char('p') => app.nav.show_preview = !app.nav.show_preview,
            //AI=== 跨系统标记确认与同系统操作 / cross-pane confirmation and same-namespace actions
            KeyCode::Char('v') => {
                if app.picker.is_some() {
                    app.confirm_picker();
                } else if app.pending.is_some() {
                    app.confirm_pending();
                }
            }
            KeyCode::Char('y') if app.is_manager() => app.copy_cross_namespace(),
            KeyCode::Char('x') if app.is_manager() => app.move_cross_namespace(),
            KeyCode::Char('c') if app.is_manager() => app.copy_same_namespace(),
            KeyCode::Char('m') if app.is_manager() => app.move_same_namespace(),
            KeyCode::Char('r') if app.is_manager() => app.start_rename(),
            KeyCode::Delete if app.is_manager() => app.request_delete(),
            //AI_END===
            KeyCode::F(12) => app.ui.window = Window::Debug,
            KeyCode::Char('?') => app.ui.window = Window::Help,
            _ => {}
        },
    }
}

fn focus_pane_at(app: &mut App, column: u16, row: u16) -> bool {
    if !app.is_manager() {
        return true;
    }
    let position = ratatui::layout::Position::new(column, row);
    // 目标选择器/待执行传输激活时只允许操作目标列表 / only the target pane is interactive during a picker or pending transfer
    if app.picker.is_some() || app.pending.is_some() {
        return app.active_nav().list_area.contains(position);
    }
    if app.local_nav.list_area.contains(position) {
        app.focus = Focus::Local;
        return true;
    }
    if app.pack.is_some() && app.nav.list_area.contains(position) {
        app.focus = Focus::Pack;
        return true;
    }
    false
}

fn handle_mouse(mouse: MouseEvent, app: &mut App) {
    handle_mouse_kind(mouse.kind, mouse.column, mouse.row, app);
}

fn handle_mouse_kind(kind: MouseEventKind, column: u16, row: u16, app: &mut App) {
    // 弹层是模态的：打开时鼠标不再操作背后的列表 / popups are modal; the list behind them ignores the mouse
    if app.ui.window != Window::None || app.rename.is_some() {
        return;
    }
    debug!(
        "mouse: {kind:?} at ({column},{row}), active_area={:?}",
        app.active_nav().list_area
    );
    if !focus_pane_at(app, column, row) {
        debug!("mouse ignored: outside navigable panes");
        return;
    }
    if !app
        .active_nav()
        .list_area
        .contains(ratatui::layout::Position::new(column, row))
    {
        debug!("mouse ignored: outside list");
        return;
    }
    match kind {
        MouseEventKind::ScrollUp => app.scroll(-WHEEL_STEP),
        MouseEventKind::ScrollDown => app.scroll(WHEEL_STEP),
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(index) = app.list_index_at(column, row) {
                app.select_index(index);
                // 双击进入目录 / double-click enters the directory
                let double = app.active_nav_mut().mouse.register_click(index);
                if double {
                    app.enter();
                }
            }
        }
        _ => {}
    }
}

fn dispatch_vt(vt: &mut VtInput, app: &mut App) {
    for event in vt.drain() {
        match event {
            VtEvent::Key(code, modifiers) => handle_key(code, modifiers, app),
            VtEvent::Mouse(kind, column, row) => handle_mouse_kind(kind, column, row, app),
        }
    }
}
//AI_END===

#[cfg(test)]
mod test;
