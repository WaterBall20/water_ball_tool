//AI=== 解包任务共享包句柄测试 / unpack-job shared pack handle test
use super::*;
use crate::command::wbfp::WaterBallFilePackCommandsUnpack;
use std::fs;

fn prepare(dir: &Path) {
    if dir.exists() {
        let _ = fs::remove_dir_all(dir);
    }
    fs::create_dir_all(dir).expect("创建测试目录失败 / failed to create test dir");
}

#[test]
fn unpack_job_shares_pack_handle() -> Result<(), Box<dyn std::error::Error>> {
    let base = Path::new(
        "temp/test/command/wbfp/tui/app/unpack_job_test/ok/unpack_job_shares_pack_handle",
    );
    prepare(base);
    let src = base.join("src");
    let out = base.join("out");
    let pack = base.join("pack.wbfp");
    fs::create_dir_all(src.join("sub"))?;
    fs::write(src.join("a.txt"), b"hello")?;
    fs::write(src.join("sub/b.txt"), b"world")?;

    let (pack_tx, _pack_rx) = std::sync::mpsc::channel();
    crate::command::wbfp::ops::run_pack(&src, &pack, true, true, 2, &pack_tx)?;

    let args = WaterBallFilePackCommand::new(WaterBallFilePackCommands::Unpack(
        WaterBallFilePackCommandsUnpack::new(
            pack.to_string_lossy().into_owned(),
            Some(out.to_string_lossy().into_owned()),
            false,
            Some(2),
        ),
    ));
    let mut app = App::from_args(args)?;
    let spec = app
        .jobs
        .spec
        .take()
        .ok_or_else(|| std::io::Error::other("missing unpack job"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = crate::command::wbfp::ops::spawn(spec, tx)?;
    for event in rx {
        app.apply_event(event);
    }
    handle
        .join()
        .map_err(|_| std::io::Error::other("unpack job panicked"))?;

    assert_eq!(fs::read(out.join("a.txt"))?, b"hello");
    assert_eq!(fs::read(out.join("sub/b.txt"))?, b"world");
    assert_eq!(app.jobs.state.as_ref().map(|job| job.finished), Some(true));
    assert_eq!(
        app.jobs.statuses.get(&PathBuf::from("a.txt")),
        Some(&RowStatus::Done)
    );
    let sub = app.nav.entries.iter().position(|entry| entry.name == "sub");
    let Some(sub) = sub else {
        panic!("unpacked root is missing the sub directory");
    };
    assert_eq!(app.status_of(&app.nav.entries[sub]), Some(RowStatus::Done));

    drop(app);
    fs::remove_dir_all(base)?;
    Ok(())
}
//AI_END===
