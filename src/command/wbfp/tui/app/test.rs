//AI=== wbfp TUI 状态与事件测试 / wbfp TUI state and event tests
use super::*;
use std::fs;
use water_ball_tool_lib::wb_files_pack::manager_sync::ManagerSync;

impl App {
    /// 测试用静态状态 / static state for tests.
    pub(crate) fn preview() -> Self {
        let mut app = Self::new(Mode::Browse);
        app.ui.title = "D:\\packs\\game.wbfp".to_string();
        app.ui.pack_info = Some(PackInfo {
            version: 1,
            file_count: 3,
            dir_count: 1,
            data_len: 13_312,
            cow: false,
        });
        app.nav.cwd = PathBuf::from("assets");
        app.nav.entries = vec![
            Entry {
                name: "..".to_string(),
                path: PathBuf::new(),
                is_dir: true,
                size: 0,
                modified: 0,
                hash: None,
                pack_version: None,
                synthetic: true,
            },
            Entry {
                name: "fonts".to_string(),
                path: PathBuf::from("assets/fonts"),
                is_dir: true,
                size: 0,
                modified: 0,
                hash: None,
                pack_version: None,
                synthetic: false,
            },
            Entry {
                name: "hero_idle.png".to_string(),
                path: PathBuf::from("assets/hero_idle.png"),
                is_dir: false,
                size: 12_684,
                modified: 1_761_000_000_000,
                hash: Some((1, vec![0x9f, 0x2c, 0xa7, 0x1b])),
                pack_version: None,
                synthetic: false,
            },
        ];
        app.jobs
            .statuses
            .insert(PathBuf::from("assets/hero_idle.png"), RowStatus::Done);
        app.nav.selected = 2;
        app
    }

    /// 测试用 Manager 状态（无包）/ static manager state without a pack.
    pub(crate) fn manager_no_pack() -> Self {
        let mut app = Self::preview();
        app.ui.mode = Mode::Manager;
        app.ui.pack_info = None;
        app.ui.title = "manager".to_string();
        app.local_nav = std::mem::take(&mut app.nav);
        app.local_nav.cwd = PathBuf::from("/home/user");
        app.focus = Focus::Local;
        app.pack = None;
        app
    }

    /// 校验任务卡片的静态状态（无输出字段）/ static verify job card (no output field).
    pub(crate) fn preview_verify() -> Self {
        let mut app = Self::preview();
        app.ui.mode = Mode::Verify;
        app.jobs.state = Some(JobState::new(Mode::Verify.label(), String::new()));
        app.jobs.running = false;
        app
    }
}

#[test]
fn selection_moves_and_clamps() {
    let mut app = App::preview();
    app.nav.height = 3;
    app.move_selection(1);
    assert_eq!(app.nav.selected, 2);
    app.move_selection(10);
    assert_eq!(app.nav.selected, 2);
    app.select_first();
    assert_eq!(app.nav.selected, 0);
    app.select_last();
    assert_eq!(app.nav.selected, 2);
    assert_eq!(app.nav.offset, 0);
}

#[test]
fn job_events_fill_row_status_and_counts() {
    let mut app = App::preview();
    app.jobs.state = Some(JobState::new("test", String::new()));
    let path = PathBuf::from("assets/fonts/new.ttf");
    app.apply_event(JobEvent::Start {
        path: path.clone(),
        total: 5,
    });
    assert_eq!(app.jobs.statuses.get(&path), Some(&RowStatus::Running));
    app.apply_event(JobEvent::Done {
        path: path.clone(),
        status: RowStatus::Failed,
    });
    assert_eq!(app.jobs.statuses.get(&path), Some(&RowStatus::Failed));
    assert_eq!(app.jobs.state.as_ref().map(|job| job.failed), Some(1));
}

#[test]
fn directory_status_aggregates_descendants() {
    let mut app = App::preview();
    let fonts = app
        .nav
        .entries
        .iter()
        .position(|entry| entry.name == "fonts");
    let Some(fonts) = fonts else {
        panic!("preview fixture is missing the fonts directory");
    };

    let first = PathBuf::from("assets/fonts/a.ttf");
    app.apply_event(JobEvent::File {
        path: first.clone(),
    });
    assert_eq!(
        app.status_of(&app.nav.entries[fonts]),
        Some(RowStatus::Running)
    );
    app.apply_event(JobEvent::Done {
        path: first,
        status: RowStatus::Done,
    });
    assert_eq!(
        app.status_of(&app.nav.entries[fonts]),
        Some(RowStatus::Done)
    );

    let second = PathBuf::from("assets/fonts/b.ttf");
    app.apply_event(JobEvent::File {
        path: second.clone(),
    });
    assert_eq!(
        app.status_of(&app.nav.entries[fonts]),
        Some(RowStatus::Running)
    );
    app.apply_event(JobEvent::Done {
        path: second,
        status: RowStatus::Failed,
    });
    assert_eq!(
        app.status_of(&app.nav.entries[fonts]),
        Some(RowStatus::Failed)
    );
}

#[test]
fn pending_child_directory_keeps_parent_running() {
    let mut app = App::preview();
    let fonts = app
        .nav
        .entries
        .iter()
        .position(|entry| entry.name == "fonts");
    let Some(fonts) = fonts else {
        panic!("preview fixture is missing the fonts directory");
    };
    let nested = PathBuf::from("assets/fonts/nested");
    app.apply_event(JobEvent::Dir {
        path: nested.clone(),
    });
    assert_eq!(
        app.status_of(&app.nav.entries[fonts]),
        Some(RowStatus::Running)
    );
    app.apply_event(JobEvent::Done {
        path: nested,
        status: RowStatus::Done,
    });
    assert_eq!(
        app.status_of(&app.nav.entries[fonts]),
        Some(RowStatus::Done)
    );
}

#[test]
fn task_list_keeps_active_files_first_and_updates_by_path() {
    let mut app = App::preview();
    let first = PathBuf::from("assets/fonts/a.ttf");
    let second = PathBuf::from("assets/fonts/b.ttf");

    app.apply_event(JobEvent::Start {
        path: first.clone(),
        total: 100,
    });
    app.apply_event(JobEvent::Start {
        path: second.clone(),
        total: 200,
    });
    assert_eq!(app.jobs.active.len(), 2);
    assert_eq!(app.jobs.active[0].path, second);
    assert_eq!(app.jobs.active[1].path, first);

    app.apply_event(JobEvent::Progress {
        path: second.clone(),
        done: 50,
        total: 200,
    });
    assert_eq!(app.jobs.active[0].done, 50);
    assert_eq!(app.jobs.done.len(), 0);

    app.apply_event(JobEvent::Done {
        path: first,
        status: RowStatus::Done,
    });
    assert_eq!(app.jobs.active.len(), 1);
    assert_eq!(app.jobs.done.len(), 1);
    assert_eq!(app.jobs.done[0].status, RowStatus::Done);
}

#[test]
fn task_list_skips_directories_and_finishes_active_on_job_end() {
    let mut app = App::preview();
    app.jobs.state = Some(JobState::new("test", String::new()));
    let dir = PathBuf::from("assets/fonts");
    app.apply_event(JobEvent::Dir { path: dir.clone() });
    app.apply_event(JobEvent::Start {
        path: dir.clone(),
        total: 0,
    });
    app.apply_event(JobEvent::Done {
        path: dir,
        status: RowStatus::Done,
    });
    assert!(app.jobs.active.is_empty());
    assert!(app.jobs.done.is_empty());

    let file = PathBuf::from("assets/fonts/c.ttf");
    app.apply_event(JobEvent::Start {
        path: file,
        total: 10,
    });
    app.apply_event(JobEvent::Finished {
        result: Ok(Default::default()),
    });
    assert!(app.jobs.active.is_empty());
    assert_eq!(app.jobs.done.len(), 1);
    assert_eq!(app.jobs.done[0].status, RowStatus::Done);
}

#[test]
fn display_cwd_is_relative_to_local_root() {
    let mut app = App::new(Mode::Pack);
    app.nav.local_root = Some(PathBuf::from("src"));
    app.nav.cwd = PathBuf::from("src/sub");
    assert_eq!(app.display_cwd(), "/sub");
    assert!(!app.is_at_root());
    app.nav.cwd = PathBuf::from("src");
    assert!(app.is_at_root());
}
//AI_END===

//AI=== 本地↔包复制辅助函数测试 / local↔pack copy helper tests
#[test]
fn manager_opens_probed_pack_and_closes_it() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from("./temp/test/command/wbfp/tui/app/ok/manager_opens_probed_pack");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    let pack_path = dir.join("game.pack");
    {
        ManagerSync::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&pack_path)?;
    }
    let mut app = App::manager_no_pack();
    app.local_nav.cwd = dir.clone();
    app.open_pack(&pack_path)?;
    assert_eq!(app.focus, Focus::Pack);
    assert!(app.pack.is_some());
    assert_eq!(app.pack_format_version, Some(2));
    assert!(app.pack_path.is_some());
    app.close_pack();
    assert_eq!(app.focus, Focus::Local);
    assert!(app.pack.is_none());
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn copy_local_file_to_pack_and_back() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from("./temp/test/command/wbfp/tui/app/ok/copy_local_file_to_pack_and_back");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    let pack_path = dir.join("pack");
    let mut pack = ManagerSync::options()
        .read(true)
        .write(true)
        .create_new(true)
        .cow(false)
        .separate_manifest(true)
        .open(&pack_path)?;

    let source = dir.join("source.txt");
    fs::write(&source, b"copy me")?;
    copy_local_to_pack(&mut pack, &source, Path::new("nested/copied.txt"))?;
    assert!(pack.path_exists(&"nested/copied.txt")?);

    let output = dir.join("output.txt");
    copy_pack_to_local(&mut pack, Path::new("nested/copied.txt"), &output)?;
    assert_eq!(fs::read(&output)?, b"copy me");
    drop(pack);
    fs::remove_dir_all(&dir)?;
    Ok(())
}
//AI_END===
