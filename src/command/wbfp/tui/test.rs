//AI=== wbfp TUI 渲染与按键测试 / wbfp TUI rendering and key tests
use super::app::{App, TaskItem, Window};
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
    app.jobs.active.push_back(TaskItem {
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
fn main_view_keys_work_without_a_window() {
    let mut app = App::preview();
    handle_key(KeyCode::Home, KeyModifiers::NONE, &mut app);
    assert_eq!(app.nav.selected, 0);
    handle_key(KeyCode::End, KeyModifiers::NONE, &mut app);
    assert_eq!(app.nav.selected, 2);
    assert_eq!(app.ui.window, Window::None);
}

#[test]
fn window_keys_are_isolated_and_esc_closes() {
    let mut app = App::preview();

    handle_key(KeyCode::F(12), KeyModifiers::NONE, &mut app);
    assert_eq!(app.ui.window, Window::Debug);
    // d 不再打开调试窗口，也不会把帮助叠加上来 / d no longer opens debug, and ? does not stack help
    handle_key(KeyCode::Char('d'), KeyModifiers::NONE, &mut app);
    assert_eq!(app.ui.window, Window::Debug);
    handle_key(KeyCode::Char('?'), KeyModifiers::NONE, &mut app);
    assert_eq!(app.ui.window, Window::Debug);
    handle_key(KeyCode::F(12), KeyModifiers::NONE, &mut app);
    assert_eq!(app.ui.window, Window::None);

    handle_key(KeyCode::Char('?'), KeyModifiers::NONE, &mut app);
    assert_eq!(app.ui.window, Window::Help);
    handle_key(KeyCode::F(12), KeyModifiers::NONE, &mut app);
    assert_eq!(app.ui.window, Window::Help);
    handle_key(KeyCode::Char('?'), KeyModifiers::NONE, &mut app);
    assert_eq!(app.ui.window, Window::None);

    handle_key(KeyCode::F(12), KeyModifiers::NONE, &mut app);
    handle_key(KeyCode::Esc, KeyModifiers::NONE, &mut app);
    assert_eq!(app.ui.window, Window::None);
}
//AI_END===
