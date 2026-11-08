//AI=== wbfp TUI 渲染与按键测试 / wbfp TUI rendering and key tests
use super::app::{App, Focus, Nav, Picker, TaskItem, Window};
use super::browse::Entry;
use super::handle_key;
use super::ui;
use crate::command::wbfp::events::RowStatus;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use water_ball_tool_lib::wb_files_pack::manager_sync::ManagerSync;

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

#[test]
fn esc_cancels_picker_without_navigating_parent() {
    let mut app = App::manager_no_pack();
    let source = app
        .local_nav
        .entries
        .iter()
        .find(|entry| !entry.is_dir && !entry.synthetic)
        .cloned()
        .expect("manager fixture needs a file");
    let cwd = app.local_nav.cwd.clone();
    app.focus = Focus::Local;
    app.picker = Some(Picker {
        move_entry: false,
        source_local: true,
        source,
        nav: Nav::default(),
    });

    handle_key(KeyCode::Esc, KeyModifiers::NONE, &mut app);
    assert!(app.picker.is_none());
    assert_eq!(app.local_nav.cwd, cwd);
}

#[test]
fn rename_editor_consumes_typing_and_quit() {
    let mut app = App::manager_no_pack();
    let index = app
        .local_nav
        .entries
        .iter()
        .position(|entry| !entry.is_dir && !entry.synthetic)
        .expect("manager fixture needs a file");
    app.local_nav.selected = index;
    app.focus = Focus::Local;
    app.start_rename();
    assert!(app.rename.is_some());

    handle_key(KeyCode::Char('q'), KeyModifiers::NONE, &mut app);
    assert!(!app.ui.should_quit);
    assert_eq!(
        app.rename.as_ref().map(|rename| rename.name.as_str()),
        Some("q")
    );

    handle_key(KeyCode::Esc, KeyModifiers::NONE, &mut app);
    assert!(app.rename.is_none());
}

#[test]
fn delete_key_opens_confirmation_and_can_cancel() {
    let mut app = App::manager_no_pack();
    let index = app
        .local_nav
        .entries
        .iter()
        .position(|entry| !entry.is_dir && !entry.synthetic)
        .expect("manager fixture needs a file");
    app.local_nav.selected = index;
    app.focus = Focus::Local;

    handle_key(KeyCode::Delete, KeyModifiers::NONE, &mut app);
    assert_eq!(app.ui.window, Window::Confirm);
    handle_key(KeyCode::Char('n'), KeyModifiers::NONE, &mut app);
    assert_eq!(app.ui.window, Window::None);
    assert!(app.delete.is_none());
}
#[test]
fn narrow_cross_copy_switches_pane_and_esc_cancels() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::path::PathBuf::from(
        "./temp/test/command/wbfp/tui/ok/narrow_cross_copy_switches_pane_and_esc_cancels",
    );
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let pack_path = dir.join("pack");
    let pack = ManagerSync::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&pack_path)?;
    let file = dir.join("a.txt");
    std::fs::write(&file, b"a")?;

    let mut app = App::manager_no_pack();
    app.pack = Some(pack);
    app.focus = Focus::Local;
    app.frame_width = 70;
    app.local_nav.cwd = dir.clone();
    app.local_nav.entries = vec![Entry {
        name: "a.txt".to_string(),
        path: file,
        is_dir: false,
        size: 1,
        modified: 0,
        hash: None,
        pack_version: None,
        synthetic: false,
    }];
    app.local_nav.selected = 0;

    app.copy_cross_namespace();
    assert!(app.pending.is_some());
    assert_eq!(app.focus, Focus::Pack);

    handle_key(KeyCode::Esc, KeyModifiers::NONE, &mut app);
    assert!(app.pending.is_none());
    assert_eq!(app.focus, Focus::Local);

    drop(app);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
//AI_END===
