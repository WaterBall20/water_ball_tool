//AI=== 包内浏览测试 / pack browsing tests
use super::*;
use std::fs;

fn prepare(dir: &Path) {
    if dir.exists() {
        let _ = fs::remove_dir_all(dir);
    }
    fs::create_dir_all(dir).expect("创建测试目录失败 / failed to create test dir");
}

#[test]
fn browse_loads_pack_and_navigates() -> Result<(), Box<dyn std::error::Error>> {
    let base =
        Path::new("temp/test/command/wbfp/tui/app/browse_test/ok/browse_loads_pack_and_navigates");
    prepare(base);
    let pack_path = base.join("browse.wbfp");

    let src = base.join("src");
    fs::create_dir_all(src.join("sub"))?;
    fs::write(src.join("sub/a.txt"), b"hello")?;
    let (tx, _rx) = std::sync::mpsc::channel();
    crate::command::wbfp::ops::run_pack(&src, &pack_path, true, true, 2, &tx)?;

    let args = WaterBallFilePackCommand::new(WaterBallFilePackCommands::Browse(
        WaterBallFilePackCommandsBrowse {
            pack_path: pack_path.to_string_lossy().into_owned(),
        },
    ));
    let mut app = App::from_args(args)?;
    assert_eq!(app.nav.entries.len(), 1);
    assert_eq!(app.nav.entries[0].name, "sub");
    assert!(app.nav.entries[0].is_dir);

    // 列表操作：Enter 进入子目录，Back 返回 / list operation: Enter enters, Back returns
    app.enter();
    assert_eq!(app.nav.cwd, PathBuf::from("sub"));
    assert_eq!(app.nav.entries.len(), 2);
    assert_eq!(app.nav.entries[0].name, "..");
    assert_eq!(app.nav.entries[1].name, "a.txt");
    assert_eq!(app.nav.entries[1].size, 5);

    app.back();
    assert_eq!(app.nav.cwd, PathBuf::new());
    assert_eq!(app.nav.entries.len(), 1);
    assert_eq!(app.nav.entries[0].name, "sub");

    drop(app);
    fs::remove_dir_all(base)?;
    Ok(())
}
//AI_END===
