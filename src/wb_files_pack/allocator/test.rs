use crate::{tools::TestTool, wb_files_pack::allocator::Allocator};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

static WBFP_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/allocator/ok";
static _WBFP_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/allocator/err";

// === 辅助函数 / Helpers ===

/// 准备测试目录并清理旧文件
fn setup_ok_test(name: &str) -> (PathBuf, PathBuf) {
    let dir = PathBuf::from(WBFP_TEST_TEMP_OK_DIR_PATH).join(name);
    fs::create_dir_all(&dir).unwrap();
    let pack = dir.join("pack");
    TestTool::remove_test_pack_files(&pack);
    (dir, pack)
}

/// 创建虚拟文件 → 写入数据 → 回读 → 断言一致（指定长度）
fn write_then_read_back(pack: &mut Allocator, path: &str, data: &[u8]) {
    let mut rw = pack.create_file(path, 0, data.len() as u64).unwrap();
    rw.write_all(data).unwrap();
    rw.seek(SeekFrom::Start(0)).unwrap();
    let mut buf = vec![0u8; data.len()];
    _ = rw.read(&mut buf).unwrap();
    assert_eq!(data, buf.as_slice());
}

/// 创建虚拟文件 → 写入数据 → 回读 → 断言一致（不预指定长度）
fn write_then_read_back_no_len(pack: &mut Allocator, path: &str, data: &[u8]) {
    let mut rw = pack.create_file_auto_sized(path).unwrap();
    rw.write_all(data).unwrap();
    rw.seek(SeekFrom::Start(0)).unwrap();
    let mut buf = vec![0u8; data.len()];
    _ = rw.read(&mut buf).unwrap();
    assert_eq!(data, buf.as_slice());
}

/// 打开已存在的虚拟文件 → 覆写 → 回读 → 断言一致
fn reopen_write_then_read_back(pack: &mut Allocator, path: &str, data: &[u8]) {
    let mut rw = pack.open_file(path, false).unwrap();
    rw.write_all(data).unwrap();
    rw.seek(SeekFrom::Start(0)).unwrap();
    let mut buf = vec![0u8; data.len()];
    _ = rw.read(&mut buf).unwrap();
    assert_eq!(data, buf.as_slice());
}


// === 基础功能 / Basics ===

#[test]
fn create_pack() {
    let (dir, pack) = setup_ok_test("create_pack");
    { Allocator::create_new_pack_file2(&pack).unwrap(); }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn create_dir_all_and_get() {
    let (dir, pack) = setup_ok_test("create_dir_all_and_get");
    {
        let mut alloc = Allocator::create_new_pack_file2(&pack).unwrap();
        alloc.create_dir_all(&String::from("Test/Test2")).unwrap();
        alloc.get_dir("Test/Test2").unwrap();
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}


// === 文件创建+读写（分离清单） / File create + r/w (separate manifest) ===

#[test]
fn file_write_read_back() {
    let (dir, pack) = setup_ok_test("file_write_read_back");
    {
        let mut alloc = Allocator::create_new_pack_file2(&pack).unwrap();
        write_then_read_back(&mut alloc, "Test/A", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        write_then_read_back(&mut alloc, "Test/B", &[10, 25, 33, 41, 53, 64, 57, 87, 89, 110]);
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn file_write_read_back_no_len() {
    let (dir, pack) = setup_ok_test("file_write_read_back_no_len");
    {
        let mut alloc = Allocator::create_new_pack_file2(&pack).unwrap();
        write_then_read_back_no_len(&mut alloc, "Test/A", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        write_then_read_back_no_len(&mut alloc, "Test/B", &[10, 25, 33, 41, 53, 64, 57, 87, 89, 110]);
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}


// === 文件创建+读写（不分离清单） / File create + r/w (no separate manifest) ===

#[test]
fn file_write_read_back_no_separate_manifest() {
    let (dir, pack) = setup_ok_test("file_write_read_back_no_separate_manifest");
    {
        let mut alloc = Allocator::create_pack_file(&pack, false, false, true).unwrap();
        write_then_read_back(&mut alloc, "Test/A", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        write_then_read_back(&mut alloc, "Test/B", &[10, 25, 33, 41, 53, 64, 57, 87, 89, 110]);
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn file_write_read_back_no_len_no_separate_manifest() {
    let (dir, pack) = setup_ok_test("file_write_read_back_no_len_no_separate_manifest");
    {
        let mut alloc = Allocator::create_pack_file(&pack, false, false, true).unwrap();
        write_then_read_back_no_len(&mut alloc, "Test/A", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        write_then_read_back_no_len(&mut alloc, "Test/B", &[10, 25, 33, 41, 53, 64, 57, 87, 89, 110]);
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}


// === 重新打开并修改 / Reopen and modify ===

#[test]
fn reopen_and_modify_file() {
    let (dir, pack) = setup_ok_test("reopen_and_modify_file");
    let path_a = "Test/A";
    let path_b = "Test/B";

    // 第一轮：创建包并写入初始数据
    {
        let mut alloc = Allocator::create_new_pack_file2(&pack).unwrap();
        write_then_read_back(&mut alloc, path_a, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        write_then_read_back(&mut alloc, path_b, &[10, 25, 33, 41, 53, 64, 57, 87, 89, 110]);
    }

    // 第二轮：重新打开，覆写（其中 A 从 10 字节扩到 18 字节，B 缩到不同内容）
    {
        let mut alloc = Allocator::open_pack_file(&pack).unwrap();
        reopen_write_then_read_back(&mut alloc, path_a,
            &[101, 124, 35, 124, 73, 24, 62, 83, 61, 124, 124, 12, 55, 21, 21, 144, 56, 1]);
        reopen_write_then_read_back(&mut alloc, path_b,
            &[10, 25, 33, 41, 53, 62, 5, 12, 14, 1]);
    }

    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}


// === 错误场景 / Error cases ===

#[test]
#[should_panic(expected = "文件可能已存在")]
fn create_pack_twice_should_fail() {
    let (dir, pack) = setup_ok_test("create_pack_twice_should_fail");
    Allocator::create_new_pack_file2(&pack).unwrap();
    // 第二次创建应失败（文件已存在或已被锁定）
    let r = Allocator::create_new_pack_file2(&pack);
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
    if let Err(err) = r {
        panic!("{err}");
    }
}
