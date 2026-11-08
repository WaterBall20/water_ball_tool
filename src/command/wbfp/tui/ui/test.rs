//AI=== wbfp TUI 渲染测试 / wbfp TUI rendering tests
use super::*;

#[test]
fn size_column_keeps_number_and_unit_aligned() {
    for bytes in [0, 180, 12_684, 1_234_567_890, 18_386_554_201] {
        let column = format_size_column(bytes);
        assert_eq!(display_width(&column), SIZE_WIDTH);
    }
    assert!(format_size_column(180).ends_with("B  "));
    assert!(format_size_column(12_684).ends_with("KiB"));
}

#[test]
fn job_card_bar_is_not_truncated_by_label() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::preview_verify();
    let Some(job) = app.jobs.state.as_mut() else {
        return Err("preview_verify is missing a job".into());
    };
    job.done = 50;
    job.total = 100;
    let Some(job) = app.jobs.state.as_ref() else {
        return Err("preview_verify is missing a job".into());
    };

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 10))?;
    terminal.draw(|frame| render_job_card(&app, job, frame, Rect::new(0, 0, 40, 10)))?;

    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    let rows: Vec<String> = buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect();
    assert!(
        rows[0]
            .chars()
            .all(|c| matches!(c, '█' | '▉' | '▊' | '▋' | '▌' | '▍' | '▎' | '▏' | ' '))
    );
    assert!(rows[1].contains("50%"));
    Ok(())
}
#[test]
fn manager_layout_folds_preview_before_local() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::manager_no_pack();

    let mut wide = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 20))?;
    wide.draw(|frame| render(&mut app, frame))?;
    let text = buffer_text(&wide)?;
    assert!(text.contains("Filesystem"), "buffer:\n{text}");
    assert!(text.contains("Preview"));
    assert!(!text.contains("Pack"));

    let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(70, 20))?;
    narrow.draw(|frame| render(&mut app, frame))?;
    let text = buffer_text(&narrow)?;
    assert!(text.contains("Filesystem"));
    assert!(!text.contains("Preview"));
    Ok(())
}

#[test]
fn probed_pack_file_uses_pack_marker() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::manager_no_pack();
    let entry = app
        .local_nav
        .entries
        .iter_mut()
        .find(|entry| !entry.is_dir && !entry.synthetic)
        .ok_or("manager fixture needs a file")?;
    entry.pack_version = Some(2);
    entry.name = "game.pack".to_string();

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20))?;
    terminal.draw(|frame| render(&mut app, frame))?;
    let text = buffer_text(&terminal)?;
    assert!(text.contains("◆"));
    Ok(())
}

#[test]
fn manager_job_card_is_pinned_to_the_right() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::manager_no_pack();
    app.jobs.state = Some(JobState {
        label: "复制 / Copy".to_string(),
        output: "D:\\dst".to_string(),
        ..JobState::default()
    });
    app.jobs.running = true;

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 25))?;
    terminal.draw(|frame| render(&mut app, frame))?;
    let text = buffer_text(&terminal)?;
    assert!(text.contains("Filesystem"), "buffer:\n{text}");
    assert!(text.contains("Copy"), "buffer:\n{text}");
    assert!(text.contains("D:\\dst"), "buffer:\n{text}");
    Ok(())
}

#[test]
fn picker_shows_target_pane_and_source_banner() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::path::PathBuf::from(
        "./temp/test/command/wbfp/tui/ui/ok/picker_shows_target_pane_and_source_banner",
    );
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("a.txt");
    std::fs::write(&file, b"x")?;

    let mut app = App::manager_no_pack();
    app.local_nav.cwd = dir.clone();
    app.local_nav.entries = vec![Entry {
        name: "a.txt".to_string(),
        path: file.clone(),
        is_dir: false,
        size: 1,
        modified: 0,
        hash: None,
        pack_version: None,
        synthetic: false,
    }];
    app.local_nav.selected = 0;
    app.focus = Focus::Local;
    app.copy_same_namespace();
    assert!(app.picker.is_some(), "picker should open");

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 25))?;
    terminal.draw(|frame| render(&mut app, frame))?;
    let text = buffer_text(&terminal)?;
    assert!(text.contains("Target"), "buffer:\n{text}");
    assert!(text.contains("Copy"), "buffer:\n{text}");
    assert!(text.contains("src:"), "buffer:\n{text}");

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

fn buffer_text(
    terminal: &ratatui::Terminal<ratatui::backend::TestBackend>,
) -> Result<String, Box<dyn std::error::Error>> {
    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    Ok(buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n"))
}
//AI_END===
