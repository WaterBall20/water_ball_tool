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

use crate::command::music_play::MouseCapture;
use crate::command::music_play::tui::channel_writer;
use crate::command::music_play::tui::{VtEvent, VtInput};
use crate::command::wbfp::WaterBallFilePackCommand;
use app::App;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::error::Error;
use std::io;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

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
        app.title,
        app.mode,
        app.entries.len()
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
        app.show_preview = size.width >= 80;
    }
    // Windows/ConPTY 下 crossterm 会把 SGR 鼠标序列拆成普通字符，导致列表被鼠标洪流淹没；
    // 暂时只启用键盘，并在启动时清除旧进程残留的鼠标上报。非 Windows 仍启用鼠标。
    // On Windows/ConPTY crossterm splits SGR mouse reports into literal keys and floods the list;
    // keyboard-only for now, clearing stale mouse reporting left by an older process. Other platforms keep the mouse.
    #[cfg(windows)]
    let mouse = MouseCapture::keyboard_only();
    #[cfg(not(windows))]
    let mouse = match MouseCapture::enable() {
        Ok(mouse) => mouse,
        Err(e) => {
            error!("wbfp TUI: 鼠标初始化失败 / mouse setup failed: {e}");
            if let Err(restore) = ratatui::try_restore() {
                eprintln!("恢复终端失败 / failed to restore terminal: {restore}");
            }
            return Err(e.into());
        }
    };
    let vt_input = mouse.vt_input();
    info!(
        "wbfp TUI: ready, width={}, vt_input={vt_input}",
        terminal.size().map(|s| s.width).unwrap_or(0)
    );

    // 任务事件队列不阻塞 IO：工作线程只做入队，进度事件按 4 MiB 限流；UI 侧对任务列表设上限。
    // shortcut: unbounded queue + rate-limited progress; switch to a bounded lock-free queue if a job can outpace the UI by millions of events.
    let (tx, rx) = mpsc::channel();
    let job = match app.job_spec.take() {
        Some(spec) => match super::ops::spawn(spec, tx) {
            Ok(handle) => {
                info!("wbfp TUI: 后台任务已启动 / background job started");
                Some(handle)
            }
            Err(e) => {
                error!("wbfp TUI: 后台任务启动失败 / job spawn failed: {e}");
                drop(mouse);
                if let Err(restore) = ratatui::try_restore() {
                    eprintln!("恢复终端失败 / failed to restore terminal: {restore}");
                }
                return Err(e.into());
            }
        },
        None => None,
    };

    let mut result = event_loop(&mut terminal, &mut app, &logs, &rx, vt_input);
    if let Err(e) = &result {
        error!("wbfp TUI: 主循环退出 / main loop failed: {e}");
    }
    drop(mouse);
    let restore = ratatui::try_restore();

    if let Some(handle) = job
        && handle.join().is_err()
        && result.is_ok()
    {
        error!("wbfp TUI: 任务线程 panic / job thread panicked");
        result = Err(io::Error::other("wbfp 任务线程异常退出 / wbfp job thread panicked").into());
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
    vt_input: bool,
) -> Result<(), Box<dyn Error>> {
    let mut vt = VtInput::new();
    loop {
        while let Ok(line) = logs.try_recv() {
            app.push_log(line);
        }
        drain_events(app, events);

        terminal.draw(|frame| ui::render(app, frame))?;

        if event::poll(TICK)? {
            if vt_input {
                drain_vt_input(app, &mut vt)?;
            } else {
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
        }

        if app.should_quit && !app.job_running {
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
                if app.job_running {
                    error!("wbfp TUI: 任务事件通道断开 / job event channel disconnected");
                    app.job_running = false;
                    if let Some(job) = app.job.as_mut() {
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
        app.job_running,
        app.selected,
        app.entries.len()
    );
    app.should_quit = true;
    if app.job_running {
        app.quit_after_job = true;
    }
}

fn handle_key(code: KeyCode, modifiers: KeyModifiers, app: &mut App) {
    debug!(
        "key: {code:?} mods={modifiers:?}, popup(help={}, logs={}), selected={}/{}, cwd={}",
        app.show_help,
        app.show_logs,
        app.selected,
        app.entries.len(),
        app.display_cwd()
    );
    if app.show_help || app.show_logs {
        match code {
            KeyCode::Esc => {
                app.show_help = false;
                app.show_logs = false;
            }
            // 弹层打开时再次按 d / ? 也要能关闭 / d and ? must close their own popups too
            KeyCode::Char('d') | KeyCode::F(12) => app.show_logs = !app.show_logs,
            KeyCode::Char('?') => app.show_help = !app.show_help,
            KeyCode::Char('q') => request_quit(app),
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => request_quit(app),
            _ => {}
        }
        return;
    }

    match code {
        KeyCode::Char('q') => request_quit(app),
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => request_quit(app),
        KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => app.back(),
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => app.enter(),
        KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
        KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
        KeyCode::PageUp => app.page_selection(-1),
        KeyCode::PageDown => app.page_selection(1),
        KeyCode::Home => app.select_first(),
        KeyCode::End => app.select_last(),
        KeyCode::Char('p') => app.show_preview = !app.show_preview,
        KeyCode::Char('d') | KeyCode::F(12) => app.show_logs = !app.show_logs,
        KeyCode::Char('?') => app.show_help = !app.show_help,
        _ => {}
    }
}

fn handle_mouse(mouse: MouseEvent, app: &mut App) {
    handle_mouse_kind(mouse.kind, mouse.column, mouse.row, app);
}

fn handle_mouse_kind(kind: MouseEventKind, column: u16, row: u16, app: &mut App) {
    debug!(
        "mouse: {kind:?} at ({column},{row}), list_area={:?}",
        app.list_area
    );
    if !app
        .list_area
        .contains(ratatui::layout::Position::new(column, row))
    {
        debug!("mouse ignored: outside list");
        return;
    }
    match kind {
        MouseEventKind::ScrollUp => app.scroll(-3),
        MouseEventKind::ScrollDown => app.scroll(3),
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(index) = app.list_index_at(column, row) {
                let double_click = app.last_click.take().is_some_and(|(at, previous)| {
                    previous == index && at.elapsed() < Duration::from_millis(500)
                });
                app.select_index(index);
                if double_click {
                    // 双击进入目录 / double-click enters the directory
                    app.enter();
                } else {
                    app.last_click = Some((Instant::now(), index));
                }
            }
        }
        _ => {}
    }
}

// 复用 `mp` 的 VT 解析器与输入模式：ConPTY 把方向键/鼠标作为 ANSI 字节流交付。
// shortcut: reusing mp's VT input avoids a second copy; extract it to a shared TUI module if a third TUI appears.
fn key_bytes(code: KeyCode) -> Option<Vec<u8>> {
    match code {
        KeyCode::Char(c) => Some(c.to_string().into_bytes()),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Backspace => Some(vec![0x7f]),
        _ => None,
    }
}

fn drain_vt_input(app: &mut App, vt: &mut VtInput) -> io::Result<()> {
    let mut bytes = Vec::new();
    loop {
        match event::read()? {
            Event::Key(key) if key.code == KeyCode::Esc => {
                // Esc 的 press/release 都保留为 VT 起始字节，否则 SGR 鼠标序列会被拆成普通字符。
                // Keep Esc bytes for both press and release; otherwise SGR mouse reports become literal keys.
                debug!("vt: Esc kind={:?} modifiers={:?}", key.kind, key.modifiers);
                bytes.push(0x1b);
            }
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    flush_vt(vt, &mut bytes, app);
                    handle_key(key.code, key.modifiers, app);
                } else if let Some(encoded) = key_bytes(key.code) {
                    bytes.extend_from_slice(&encoded);
                } else {
                    flush_vt(vt, &mut bytes, app);
                    handle_key(key.code, key.modifiers, app);
                }
            }
            Event::Mouse(mouse) => {
                flush_vt(vt, &mut bytes, app);
                handle_mouse(mouse, app);
            }
            _ => {}
        }

        let more = if bytes.as_slice() == [0x1b] {
            event::poll(Duration::from_millis(2))?
        } else {
            event::poll(Duration::ZERO)?
        };
        if !more {
            break;
        }
    }

    if bytes.as_slice() == [0x1b] {
        handle_key(KeyCode::Esc, KeyModifiers::empty(), app);
    } else {
        vt.feed(&bytes);
        dispatch_vt(vt, app);
    }
    Ok(())
}

fn flush_vt(vt: &mut VtInput, bytes: &mut Vec<u8>, app: &mut App) {
    if bytes.is_empty() {
        return;
    }
    vt.feed(bytes);
    bytes.clear();
    dispatch_vt(vt, app);
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
mod test {
    use super::app::{App, TaskItem};
    use super::handle_key;
    use super::ui;
    use crate::command::wbfp::events::RowStatus;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
        let width = usize::from(buffer.area.width);
        buffer
            .content
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_yazi_layout() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = App::preview();
        let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
        terminal.draw(|frame| ui::render(&mut app, frame))?;
        let text = buffer_text(terminal.backend().buffer()).replace(' ', "");
        assert!(text.contains("预览"));
        assert!(text.contains("hero_idle.png"));
        assert!(text.contains("READONLY"));
        assert!(text.contains("q退出"));
        assert!(text.contains("✓"));
        Ok(())
    }

    #[test]
    fn task_list_renders_active_file_and_progress() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = App::preview_verify();
        app.active_tasks.push_back(TaskItem {
            path: std::path::PathBuf::from("assets/hero.png"),
            name: "hero.png".to_string(),
            done: 50,
            total: 100,
            status: RowStatus::Running,
        });
        let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
        terminal.draw(|frame| ui::render(&mut app, frame))?;
        let text = buffer_text(terminal.backend().buffer()).replace(' ', "");
        assert!(text.contains("任务/Tasks"));
        assert!(text.contains("hero.png"));
        assert!(text.contains("50%"));
        Ok(())
    }

    #[test]
    fn verify_job_card_has_no_output_field() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = App::preview_verify();
        let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
        terminal.draw(|frame| ui::render(&mut app, frame))?;
        let text = buffer_text(terminal.backend().buffer()).replace(' ', "");
        assert!(text.contains("校验/verify"));
        assert!(!text.contains("输出/Output"));
        Ok(())
    }

    #[test]
    fn log_and_help_popups_toggle_closed() {
        let mut app = App::preview();

        handle_key(KeyCode::Char('d'), KeyModifiers::NONE, &mut app);
        assert!(app.show_logs);
        handle_key(KeyCode::Char('d'), KeyModifiers::NONE, &mut app);
        assert!(!app.show_logs);

        handle_key(KeyCode::Char('?'), KeyModifiers::NONE, &mut app);
        assert!(app.show_help);
        handle_key(KeyCode::Char('?'), KeyModifiers::NONE, &mut app);
        assert!(!app.show_help);

        app.show_logs = true;
        app.show_help = true;
        handle_key(KeyCode::Esc, KeyModifiers::NONE, &mut app);
        assert!(!app.show_logs);
        assert!(!app.show_help);
    }
}
