//AI=== wbfp TUI 状态与事件测试 / wbfp TUI state and event tests
use super::*;
use crate::command::wbfp::{
    WaterBallFilePackCommand, WaterBallFilePackCommands, WaterBallFilePackCommandsPack,
    WaterBallFilePackCommandsUnpack,
};
use pretty_assertions::assert_eq;
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
        result: Ok(JobSummary::default()),
    });
    assert!(app.jobs.active.is_empty());
    assert_eq!(app.jobs.done.len(), 1);
    assert_eq!(app.jobs.done[0].status, RowStatus::Done);
}

#[test]
fn display_cwd_follows_the_active_pane() {
    let mut app = App::new(Mode::Pack);
    app.local_nav.local_root = Some(PathBuf::from("src"));
    app.local_nav.cwd = PathBuf::from("src/sub");
    app.focus = Focus::Local;
    assert_eq!(app.display_cwd(), "src/sub");
    assert!(!app.is_at_root());
    app.local_nav.cwd = PathBuf::from("src");
    assert!(app.is_at_root());

    app.focus = Focus::Pack;
    app.nav.cwd = PathBuf::from("assets/sub");
    assert_eq!(app.display_cwd(), "/assets/sub");
    assert!(!app.is_at_root());
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
fn background_copy_local_file_into_pack() -> Result<(), Box<dyn std::error::Error>> {
    let dir =
        PathBuf::from("./temp/test/command/wbfp/tui/app/ok/background_copy_local_file_into_pack");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    let pack_path = dir.join("pack");
    let pack = ManagerSync::options()
        .read(true)
        .write(true)
        .create_new(true)
        .cow(false)
        .separate_manifest(true)
        .open(&pack_path)?;

    let source = dir.join("source.txt");
    fs::write(&source, b"copy me")?;
    let mut app = App::manager_no_pack();
    app.pack = Some(pack);
    app.ui.title = pack_path.display().to_string();
    app.focus = Focus::Local;
    app.frame_width = 120;
    app.local_nav.cwd = dir.clone();
    app.local_nav.entries = vec![Entry {
        name: "source.txt".to_string(),
        path: source.clone(),
        is_dir: false,
        size: 7,
        modified: 0,
        hash: None,
        pack_version: None,
        synthetic: false,
    }];
    app.local_nav.selected = 0;

    let (tx, rx) = std::sync::mpsc::channel();
    app.set_job_sender(tx);
    app.copy_cross_namespace();
    assert!(app.jobs.running);
    drain_job(&mut app, &rx)?;

    let pack = app.pack.as_mut().ok_or("pack should stay open")?;
    assert!(pack.path_exists(&"source.txt")?);
    let mut file = pack.open_virtual_file(&"source.txt")?;
    let mut buf = Vec::new();
    io::Read::read_to_end(&mut file, &mut buf)?;
    assert_eq!(buf, b"copy me");

    drop(app);
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn background_copy_pack_file_to_local() -> Result<(), Box<dyn std::error::Error>> {
    let dir =
        PathBuf::from("./temp/test/command/wbfp/tui/app/ok/background_copy_pack_file_to_local");
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
    {
        use std::io::Write as _;
        let mut file = pack.create_virtual_file(&"asset.bin")?;
        file.set_len(7)?;
        file.write_all(b"payload")?;
    }

    let mut app = App::manager_no_pack();
    app.pack = Some(pack);
    app.ui.title = pack_path.display().to_string();
    app.focus = Focus::Pack;
    app.frame_width = 120;
    app.local_nav.cwd = dir.clone();
    app.nav.cwd = PathBuf::new();
    app.nav.entries = vec![Entry {
        name: "asset.bin".to_string(),
        path: PathBuf::from("asset.bin"),
        is_dir: false,
        size: 7,
        modified: 0,
        hash: None,
        pack_version: None,
        synthetic: false,
    }];
    app.nav.selected = 0;

    let (tx, rx) = std::sync::mpsc::channel();
    app.set_job_sender(tx);
    app.copy_cross_namespace();
    assert!(app.jobs.running);
    drain_job(&mut app, &rx)?;

    assert_eq!(fs::read(dir.join("asset.bin"))?, b"payload");
    drop(app);
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn picker_copies_local_file_to_another_local_dir() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        "./temp/test/command/wbfp/tui/app/ok/picker_copies_local_file_to_another_local_dir",
    );
    let _ = fs::remove_dir_all(&dir);
    let src_dir = dir.join("src");
    let dst_dir = dir.join("dst");
    fs::create_dir_all(&src_dir)?;
    fs::create_dir_all(&dst_dir)?;
    let source = src_dir.join("source.txt");
    fs::write(&source, b"local copy")?;

    let mut app = App::manager_no_pack();
    app.local_nav.cwd = src_dir.clone();
    app.local_nav.entries = vec![Entry {
        name: "source.txt".to_string(),
        path: source.clone(),
        is_dir: false,
        size: 10,
        modified: 0,
        hash: None,
        pack_version: None,
        synthetic: false,
    }];
    app.local_nav.selected = 0;
    app.focus = Focus::Local;
    app.start_picker(false);
    assert!(app.picker.is_some());
    if let Some(picker) = app.picker.as_mut() {
        picker.nav.cwd = dst_dir.clone();
    }
    app.load_picker_dir().map_err(io::Error::other)?;

    let (tx, rx) = std::sync::mpsc::channel();
    app.set_job_sender(tx);
    app.confirm_picker();
    assert!(app.picker.is_none());
    assert!(app.jobs.running);
    drain_job(&mut app, &rx)?;

    assert_eq!(fs::read(dst_dir.join("source.txt"))?, b"local copy");
    assert!(source.exists());
    fs::remove_dir_all(&dir)?;
    Ok(())
}

//AI=== 目标面板沿用源列表视口 / target pane keeps the source viewport
#[test]
fn picker_keeps_source_list_viewport() -> Result<(), Box<dyn std::error::Error>> {
    let dir =
        PathBuf::from("./temp/test/command/wbfp/tui/app/ok/picker_keeps_source_list_viewport");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    for index in 0..20 {
        fs::write(dir.join(format!("f{index:02}.txt")), b"x")?;
    }

    let mut app = App::manager_no_pack();
    app.local_nav.cwd = dir.clone();
    app.load_local_dir().map_err(io::Error::other)?;
    app.local_nav.height = 8;
    app.local_nav.selected = 15;
    app.local_nav.offset = 10;
    app.focus = Focus::Local;
    app.start_picker(false);

    let picker = app.picker.as_ref().ok_or("picker should open")?;
    assert_eq!(picker.nav.selected, 15);
    assert_eq!(picker.nav.offset, 10);
    assert_eq!(picker.nav.height, 8);

    fs::remove_dir_all(&dir)?;
    Ok(())
}
//AI_END===

#[test]
fn picker_blocks_directory_into_own_subtree() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        "./temp/test/command/wbfp/tui/app/err/picker_blocks_directory_into_own_subtree",
    );
    let _ = fs::remove_dir_all(&dir);
    let src_dir = dir.join("src");
    let sub_dir = src_dir.join("sub");
    fs::create_dir_all(&sub_dir)?;

    let mut app = App::manager_no_pack();
    app.local_nav.cwd = dir.clone();
    app.local_nav.entries = vec![Entry {
        name: "src".to_string(),
        path: src_dir.clone(),
        is_dir: true,
        size: 0,
        modified: 0,
        hash: None,
        pack_version: None,
        synthetic: false,
    }];
    app.local_nav.selected = 0;
    app.focus = Focus::Local;
    app.start_picker(true);
    if let Some(picker) = app.picker.as_mut() {
        picker.nav.cwd = sub_dir.clone();
    }
    app.load_picker_dir().map_err(io::Error::other)?;
    app.confirm_picker();

    assert!(
        app.picker.is_some(),
        "picker must stay open after a forbidden target"
    );
    assert!(app.ui.error.is_some());
    assert!(src_dir.exists());
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn background_copy_pack_directory_to_pack() -> Result<(), Box<dyn std::error::Error>> {
    let dir =
        PathBuf::from("./temp/test/command/wbfp/tui/app/ok/background_copy_pack_directory_to_pack");
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
    {
        use std::io::Write as _;
        pack.create_dir_all(&"dst")?;
        pack.create_dir_all(&"src/empty")?;
        pack.create_dir_all(&"src/sub")?;
        let mut file = pack
            .virtual_file_options()
            .write(true)
            .create_new(true)
            .alloc_size(Some(1))
            .open(&"src/a.txt")?;
        file.set_len(1)?;
        file.write_all(b"a")?;
        let mut file = pack
            .virtual_file_options()
            .write(true)
            .create_new(true)
            .alloc_size(Some(1))
            .open(&"src/sub/b.txt")?;
        file.set_len(1)?;
        file.write_all(b"b")?;
    }

    let mut app = App::manager_no_pack();
    app.pack = Some(pack);
    app.ui.title = pack_path.display().to_string();
    app.focus = Focus::Pack;
    app.nav.cwd = PathBuf::new();
    app.nav.entries = vec![Entry {
        name: "src".to_string(),
        path: PathBuf::from("src"),
        is_dir: true,
        size: 0,
        modified: 0,
        hash: None,
        pack_version: None,
        synthetic: false,
    }];
    app.nav.selected = 0;
    app.start_picker(false);
    if let Some(picker) = app.picker.as_mut() {
        picker.nav.cwd = PathBuf::from("dst");
    }
    app.load_picker_dir().map_err(io::Error::other)?;

    let (tx, rx) = std::sync::mpsc::channel();
    app.set_job_sender(tx);
    app.confirm_picker();
    drain_job(&mut app, &rx)?;

    let pack = app.pack.as_mut().ok_or("pack should stay open")?;
    assert!(pack.path_exists(&"dst/src/a.txt")?);
    assert!(pack.path_exists(&"dst/src/sub/b.txt")?);
    assert!(pack.path_exists(&"dst/src/empty")?);
    {
        let mut file = pack.open_virtual_file(&"dst/src/sub/b.txt")?;
        let mut buf = Vec::new();
        io::Read::read_to_end(&mut file, &mut buf)?;
        assert_eq!(buf, b"b");
    }

    drop(app);
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn background_move_local_file_to_local_dir() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        "./temp/test/command/wbfp/tui/app/ok/background_move_local_file_to_local_dir",
    );
    let _ = fs::remove_dir_all(&dir);
    let src_dir = dir.join("src");
    let dst_dir = dir.join("dst");
    fs::create_dir_all(&src_dir)?;
    fs::create_dir_all(&dst_dir)?;
    let source = src_dir.join("move.txt");
    fs::write(&source, b"move me")?;

    let mut app = App::manager_no_pack();
    app.local_nav.cwd = src_dir.clone();
    app.local_nav.entries = vec![Entry {
        name: "move.txt".to_string(),
        path: source.clone(),
        is_dir: false,
        size: 7,
        modified: 0,
        hash: None,
        pack_version: None,
        synthetic: false,
    }];
    app.local_nav.selected = 0;
    app.focus = Focus::Local;
    app.start_picker(true);
    if let Some(picker) = app.picker.as_mut() {
        picker.nav.cwd = dst_dir.clone();
    }
    app.load_picker_dir().map_err(io::Error::other)?;

    let (tx, rx) = std::sync::mpsc::channel();
    app.set_job_sender(tx);
    app.confirm_picker();
    drain_job(&mut app, &rx)?;

    assert!(!source.exists());
    assert_eq!(fs::read(dst_dir.join("move.txt"))?, b"move me");
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn background_move_pack_entry_into_directory() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        "./temp/test/command/wbfp/tui/app/ok/background_move_pack_entry_into_directory",
    );
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
    {
        use std::io::Write as _;
        pack.create_dir_all(&"dst")?;
        let mut file = pack
            .virtual_file_options()
            .write(true)
            .create_new(true)
            .alloc_size(Some(1))
            .open(&"move.bin")?;
        file.set_len(1)?;
        file.write_all(b"m")?;
    }

    let mut app = App::manager_no_pack();
    app.pack = Some(pack);
    app.ui.title = pack_path.display().to_string();
    app.focus = Focus::Pack;
    app.nav.cwd = PathBuf::new();
    app.nav.entries = vec![Entry {
        name: "move.bin".to_string(),
        path: PathBuf::from("move.bin"),
        is_dir: false,
        size: 1,
        modified: 0,
        hash: None,
        pack_version: None,
        synthetic: false,
    }];
    app.nav.selected = 0;
    app.start_picker(true);
    if let Some(picker) = app.picker.as_mut() {
        picker.nav.cwd = PathBuf::from("dst");
    }
    app.load_picker_dir().map_err(io::Error::other)?;

    let (tx, rx) = std::sync::mpsc::channel();
    app.set_job_sender(tx);
    app.confirm_picker();
    drain_job(&mut app, &rx)?;

    let pack = app.pack.as_mut().ok_or("pack should stay open")?;
    assert!(!pack.path_exists(&"move.bin")?);
    assert!(pack.path_exists(&"dst/move.bin")?);

    drop(app);
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn delete_blocks_the_open_pack_file() -> Result<(), Box<dyn std::error::Error>> {
    let dir =
        PathBuf::from("./temp/test/command/wbfp/tui/app/err/delete_blocks_the_open_pack_file");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    let pack_path = dir.join("game.wbfp");
    let pack = ManagerSync::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&pack_path)?;
    let canonical = fs::canonicalize(&pack_path)?;

    let mut app = App::manager_no_pack();
    app.pack = Some(pack);
    app.pack_path = Some(canonical.clone());
    app.focus = Focus::Local;
    app.local_nav.cwd = dir.clone();
    app.local_nav.entries = vec![Entry {
        name: "game.wbfp".to_string(),
        path: canonical.clone(),
        is_dir: false,
        size: 0,
        modified: 0,
        hash: None,
        pack_version: Some(2),
        synthetic: false,
    }];
    app.local_nav.selected = 0;

    app.request_delete();
    assert_eq!(app.ui.window, Window::None);
    assert!(app.delete.is_none());
    assert!(app.ui.error.is_some());
    assert!(canonical.exists());

    drop(app);
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn open_pack_path_blocks_sidecars_and_ancestor_dirs() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        "./temp/test/command/wbfp/tui/app/ok/open_pack_path_blocks_sidecars_and_ancestor_dirs",
    );
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    let pack_path = dir.join("game.wbfp");
    let pack = ManagerSync::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&pack_path)?;
    let canonical = fs::canonicalize(&pack_path)?;
    let canonical_dir = fs::canonicalize(&dir)?;

    let mut app = App::manager_no_pack();
    app.pack = Some(pack);
    app.pack_path = Some(canonical.clone());
    assert!(app.is_open_pack_path(&canonical));
    assert!(app.is_open_pack_path(&canonical_dir));

    let mut lock_name = canonical.as_os_str().to_os_string();
    lock_name.push(".lock");
    assert!(app.is_open_pack_path(&PathBuf::from(lock_name)));

    let other = dir.join("other.txt");
    fs::write(&other, b"x")?;
    assert!(!app.is_open_pack_path(&fs::canonicalize(&other)?));

    drop(app);
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn cross_transfer_blocks_open_pack_into_itself() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        "./temp/test/command/wbfp/tui/app/err/cross_transfer_blocks_open_pack_into_itself",
    );
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    let pack_path = dir.join("game.wbfp");
    let pack = ManagerSync::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&pack_path)?;
    let canonical = fs::canonicalize(&pack_path)?;

    let mut app = App::manager_no_pack();
    app.pack = Some(pack);
    app.pack_path = Some(canonical.clone());
    app.ui.title = pack_path.display().to_string();
    app.focus = Focus::Local;
    app.frame_width = 120;
    app.local_nav.cwd = dir.clone();
    app.local_nav.entries = vec![Entry {
        name: "game.wbfp".to_string(),
        path: canonical.clone(),
        is_dir: false,
        size: 0,
        modified: 0,
        hash: None,
        pack_version: Some(2),
        synthetic: false,
    }];
    app.local_nav.selected = 0;

    app.move_cross_namespace();
    assert!(!app.jobs.running);
    assert!(app.pending.is_none());
    assert!(app.ui.error.is_some());

    let pack = app.pack.as_mut().ok_or("pack should stay open")?;
    assert!(!pack.path_exists(&"game.wbfp")?);

    drop(app);
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn pack_mode_opens_result_and_marks_changed_files() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        "./temp/test/command/wbfp/tui/app/ok/pack_mode_opens_result_and_marks_changed_files",
    );
    let _ = fs::remove_dir_all(&dir);
    let src = dir.join("src");
    fs::create_dir_all(src.join("sub"))?;
    fs::write(src.join("a.txt"), b"a")?;
    fs::write(src.join("sub/b.txt"), b"b")?;
    let pack_path = dir.join("out.wbfp");

    let args = WaterBallFilePackCommand {
        commands: Some(WaterBallFilePackCommands::Pack(
            WaterBallFilePackCommandsPack::new(
                src.display().to_string(),
                Some(pack_path.display().to_string()),
                false,
                Some(1),
                false,
            ),
        )),
        pack_path: None,
        no_tui: false,
    };
    let mut app = App::from_args(args)?;
    let (tx, rx) = std::sync::mpsc::channel();
    app.set_job_sender(tx);
    app.start_pending_job()?;
    drain_job(&mut app, &rx)?;

    let pack = app.pack.as_mut().ok_or("packed result should be open")?;
    assert!(pack.path_exists(&"a.txt")?);
    assert!(pack.path_exists(&"sub/b.txt")?);
    assert!(app.jobs.changed.contains(std::path::Path::new("a.txt")));
    assert!(app.jobs.changed.contains(std::path::Path::new("sub/b.txt")));

    let local = app
        .local_nav
        .entries
        .iter()
        .find(|entry| entry.name == "a.txt")
        .cloned()
        .ok_or("local pane should list a.txt")?;
    assert!(app.is_changed(true, &local));
    let pack_entry = app
        .nav
        .entries
        .iter()
        .find(|entry| entry.name == "a.txt")
        .cloned()
        .ok_or("pack pane should list a.txt")?;
    assert!(app.is_changed(false, &pack_entry));

    drop(app);
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn unpack_mode_shows_output_and_marks_changed_files() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        "./temp/test/command/wbfp/tui/app/ok/unpack_mode_shows_output_and_marks_changed_files",
    );
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    let pack_path = dir.join("src.wbfp");
    {
        use std::io::Write as _;
        let mut pack = ManagerSync::options()
            .read(true)
            .write(true)
            .create_new(true)
            .cow(false)
            .separate_manifest(false)
            .open(&pack_path)?;
        pack.create_dir_all(&"sub")?;
        let mut file = pack
            .virtual_file_options()
            .write(true)
            .create_new(true)
            .alloc_size(Some(1))
            .open(&"sub/a.txt")?;
        file.set_len(1)?;
        file.write_all(b"a")?;
    }
    let out_dir = dir.join("out");

    let args = WaterBallFilePackCommand {
        commands: Some(WaterBallFilePackCommands::Unpack(
            WaterBallFilePackCommandsUnpack::new(
                pack_path.display().to_string(),
                Some(out_dir.display().to_string()),
                false,
                Some(1),
            ),
        )),
        pack_path: None,
        no_tui: false,
    };
    let mut app = App::from_args(args)?;
    let (tx, rx) = std::sync::mpsc::channel();
    app.set_job_sender(tx);
    app.start_pending_job()?;
    drain_job(&mut app, &rx)?;

    assert_eq!(fs::read(out_dir.join("sub").join("a.txt"))?, b"a");
    assert!(app.jobs.changed.contains(std::path::Path::new("sub/a.txt")));

    app.local_nav.cwd = out_dir.join("sub");
    app.load_local_dir().map_err(std::io::Error::other)?;
    let local = app
        .local_nav
        .entries
        .iter()
        .find(|entry| entry.name == "a.txt")
        .cloned()
        .ok_or("unpacked pane should list a.txt")?;
    assert!(app.is_changed(true, &local));

    drop(app);
    fs::remove_dir_all(&dir)?;
    Ok(())
}

fn drain_job(
    app: &mut App,
    rx: &std::sync::mpsc::Receiver<JobEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err("background job timed out".into());
        }
        match rx.recv_timeout(remaining) {
            Ok(event) => app.apply_event(event),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                return Err("background job timed out".into());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if !app.jobs.running {
            break;
        }
    }
    for handle in app.take_job_handles() {
        handle
            .join()
            .map_err(|_| "background job thread panicked")?;
    }
    Ok(())
}
//AI_END===
