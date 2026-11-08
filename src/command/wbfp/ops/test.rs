//AI=== wbfp 后台任务测试 / wbfp background-job tests
use super::*;
use std::fs;

fn prepare(dir: &Path) {
    if dir.exists() {
        let _ = fs::remove_dir_all(dir);
    }
    fs::create_dir_all(dir).expect("创建测试目录失败 / failed to create test dir");
}

#[test]
fn tui_ops_pack_verify_unpack_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let base = Path::new("temp/test/command/wbfp/ops/test/ok/tui_ops_pack_verify_unpack_roundtrip");
    prepare(base);
    let src = base.join("src");
    fs::create_dir_all(src.join("sub"))?;
    fs::write(src.join("a.txt"), b"hello")?;
    fs::write(src.join("sub/b.bin"), [0u8, 1, 2, 3])?;

    let pack = base.join("pack.wbfp");
    let out = base.join("out");
    let (tx, _rx) = mpsc::channel();

    let pack_summary = run_pack(&src, &pack, true, true, 2, &tx)?;
    assert_eq!(pack_summary.files_done, 2);
    assert_eq!(pack_summary.failed, 0);

    let verify_pack = ManagerSync::open(&pack)?;
    let verify_summary = run_verify(verify_pack, 2, &tx)?;
    assert_eq!(verify_summary.failed, 0);

    let unpack_pack = ManagerSync::open(&pack)?;
    let unpack_summary = run_unpack(unpack_pack, &out, false, 2, &tx)?;
    assert_eq!(unpack_summary.files_done, 2);
    assert_eq!(unpack_summary.failed, 0);

    assert_eq!(fs::read(out.join("a.txt"))?, b"hello");
    assert_eq!(fs::read(out.join("sub/b.bin"))?, vec![0u8, 1, 2, 3]);

    fs::remove_dir_all(base)?;
    Ok(())
}

#[test]
fn tui_pack_unchanged_file_is_done_not_warning() -> Result<(), Box<dyn std::error::Error>> {
    let base =
        Path::new("temp/test/command/wbfp/ops/test/ok/tui_pack_unchanged_file_is_done_not_warning");
    prepare(base);
    let src = base.join("src");
    fs::create_dir_all(&src)?;
    fs::write(src.join("a.txt"), b"hello")?;
    let pack = base.join("pack.wbfp");

    let (tx, _rx) = mpsc::channel();
    run_pack(&src, &pack, true, true, 2, &tx)?;

    let (tx, rx) = mpsc::channel();
    let summary = run_pack(&src, &pack, true, true, 2, &tx)?;
    drop(tx);
    assert_eq!(summary.files_done, 1);
    assert_eq!(summary.warnings, 0);

    let statuses: Vec<RowStatus> = rx
        .iter()
        .filter_map(|event| match event {
            JobEvent::Done { status, .. } => Some(status),
            _ => None,
        })
        .collect();
    assert_eq!(statuses, vec![RowStatus::Done]);

    fs::remove_dir_all(base)?;
    Ok(())
}
//AI_END===
