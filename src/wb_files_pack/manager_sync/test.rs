use crate::wb_files_pack::OverwriteStrategy;
use crate::wb_files_pack::pack_io::file::PackVirtualFile;
use crate::{tools::TestTool, wb_files_pack::manager_sync::ManagerSync};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

static WBFP_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/manager_sync/ok";
static _WBFP_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/manager_sync/err";

// === 辅助函数 / Helpers ===

/// 创建新包（读写模式）/ Create new pack in ReadWrite mode
fn create_new_rw(path: impl AsRef<std::path::Path>) -> ManagerSync {
    ManagerSync::options()
        .read(true)
        .write(true)
        .create_new(true)
        .cow(false)
        .separate_manifest(true)
        .open(&path)
        .expect("创建包文件失败")
}

/// 准备测试目录并清理旧文件
fn setup_ok_test(name: &str) -> (PathBuf, PathBuf) {
    let dir = PathBuf::from(WBFP_TEST_TEMP_OK_DIR_PATH).join(name);
    fs::create_dir_all(&dir).expect("创建测试目录失败");
    let pack = dir.join("pack");
    TestTool::remove_test_pack_files(&pack);
    (dir, pack)
}

/// 创建虚拟文件（读写模式）→ 写入数据 → 回读 → 断言一致（指定长度）
///
/// 使用 `virtual_file_options()` 而非 `create_virtual_file`，因为需要读写双重权限。
fn write_then_read_back(pack: &mut ManagerSync, path: &str, data: &[u8]) {
    let mut rw = pack.virtual_file_options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("创建虚拟文件失败");
    rw.set_len(data.len() as u64).expect("设置长度失败");
    rw.write_all(data).expect("写入数据失败");
    rw.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
    let mut buf = vec![0u8; data.len()];
    _ = rw.read(&mut buf).expect("读取数据失败");
    assert_eq!(data, buf.as_slice());
}

/// 创建虚拟文件（读写模式）→ 写入数据 → 回读 → 断言一致（不预指定长度）
fn write_then_read_back_no_len(pack: &mut ManagerSync, path: &str, data: &[u8]) {
    let mut rw = pack.virtual_file_options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("创建虚拟文件失败");
    rw.write_all(data).expect("写入数据失败");
    rw.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
    let mut buf = vec![0u8; data.len()];
    _ = rw.read(&mut buf).expect("读取数据失败");
    assert_eq!(data, buf.as_slice());
}

/// 打开已存在的虚拟文件（读写模式）/ Open an existing virtual file in read-write mode
fn open_rw(alloc: &mut ManagerSync, path: &str) -> PackVirtualFile {
    alloc.virtual_file_options()
        .read(true)
        .write(true)
        .open(&path)
        .expect("打开虚拟文件失败")
}

/// 打开已存在的虚拟文件 → 设置大小 → 覆写 → 回读 → 断言一致
///
/// 始终调用 `set_len` 设置精确大小（缩小和扩大都需要），不妥协。
/// Always call `set_len` to set the exact size (both shrink and extend).
fn reopen_write_then_read_back(pack: &mut ManagerSync, path: &str, data: &[u8]) {
    let mut rw = open_rw(pack, path);
    rw.set_len(data.len() as u64).expect("设置长度失败");
    rw.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
    rw.write_all(data).expect("写入数据失败");
    rw.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
    let mut buf = vec![0u8; data.len()];
    rw.read_exact(&mut buf).expect("读取数据失败");
    assert_eq!(data, buf.as_slice());
}

// === 基础功能 / Basics ===

#[test]
fn create_pack() {
    let (dir, pack) = setup_ok_test("create_pack");
    {
        ManagerSync::create_new(&pack).expect("创建包文件失败");
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn create_dir_all_and_get() {
    let (dir, pack) = setup_ok_test("create_dir_all_and_get");
    {
        let mut alloc = ManagerSync::create_new(&pack).expect("创建包文件失败");
        alloc
            .create_dir_all(&String::from("Test/Test2"))
            .expect("创建目录失败");
        alloc.get_dir(&"Test/Test2").expect("获取目录失败");
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

// === 文件创建+读写（分离清单） / File create + r/w (separate manifest) ===

#[test]
fn file_write_read_back() {
    let (dir, pack) = setup_ok_test("file_write_read_back");
    {
        let mut alloc = create_new_rw(&pack);
        write_then_read_back(&mut alloc, "Test/A", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        write_then_read_back(
            &mut alloc,
            "Test/B",
            &[10, 25, 33, 41, 53, 64, 57, 87, 89, 110],
        );
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn file_write_read_back_no_len() {
    let (dir, pack) = setup_ok_test("file_write_read_back_no_len");
    {
        let mut alloc = create_new_rw(&pack);
        write_then_read_back_no_len(&mut alloc, "Test/A", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        write_then_read_back_no_len(
            &mut alloc,
            "Test/B",
            &[10, 25, 33, 41, 53, 64, 57, 87, 89, 110],
        );
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

// === 文件创建+读写（不分离清单） / File create + r/w (no separate manifest) ===

#[test]
/// 回归：写入超过预分配大小（模拟源文件增长）必须完整落盘。块写满时
/// write 曾返回 Ok(0)（0 长度段跳过补分配），write_all 报 WriteZero 截断。
/// Regression: writes beyond the preallocated size (growing source file) must
/// fully land; a full block used to yield Ok(0) and truncate via WriteZero.
fn file_write_grow_beyond_preallocated() {
    let (dir, pack) = setup_ok_test("file_write_grow_beyond_preallocated");
    {
        let mut alloc = create_new_rw(&pack);
        let mut rw = alloc
            .virtual_file_options()
            .read(true)
            .write(true)
            .create_new(true)
            .open("Test/Grow")
            .expect("创建虚拟文件失败");
        //预分配小块，模拟搜索时记录的源文件大小
        rw.set_len(100).expect("set_len 失败");
        //写入远超预分配的数据（模拟活文件持续增长）
        let data: Vec<u8> = (0..(200 * 1024)).map(|i| (i % 251) as u8).collect();
        rw.write_all(&data).expect("写入超过预分配大小的数据失败");
        rw.seek(SeekFrom::Start(0)).expect("seek 失败");
        let mut buf = vec![0u8; data.len()];
        rw.read_exact(&mut buf).expect("读取失败");
        assert_eq!(data, buf.as_slice());
        drop(rw);
        //回归：增长文件重开后的哈希校验必须通过（曾因读取越界导致 read_hash_v 失败、
        //hash 未存储，校验报错；读取按 metadata.len 截断后修复）
        drop(alloc);
        let mut alloc2 = ManagerSync::options()
            .read(true)
            .write(true)
            .open(&pack)
            .expect("重新打开失败");
        let mut rw2 = alloc2
            .open_virtual_file(&std::path::PathBuf::from("Test/Grow"))
            .expect("打开虚拟文件失败");
        assert_eq!(rw2.get_len(), 200 * 1024);
        let ok = rw2.verify_hash(None).expect("verify_hash 出错");
        assert!(ok, "哈希校验未通过");
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
/// 已知大小通过 alloc_size 指定时按 128B 精确分配（而非 4MiB 整块），
/// 写入回读一致；分配大小由 alloc_size 决定，不随写入内容膨胀到 4MiB。
/// With a known size via alloc_size(Some(len)) the file is allocated at 128B
/// granularity instead of a whole 4MiB block; data round-trips correctly.
fn virtual_file_alloc_size_known() {
    let (dir, pack) = setup_ok_test("virtual_file_alloc_size_known");
    {
        let mut alloc = create_new_rw(&pack);
        let data: Vec<u8> = (0..(10 * 1024)).map(|i| (i % 251) as u8).collect();
        let mut rw = alloc
            .virtual_file_options()
            .read(true)
            .write(true)
            .create_new(true)
            .alloc_size(Some(data.len() as u64))
            .open("Test/AllocKnown")
            .expect("创建虚拟文件失败");
        rw.write_all(&data).expect("写入失败");
        rw.seek(SeekFrom::Start(0)).expect("seek 失败");
        let mut buf = vec![0u8; data.len()];
        rw.read_exact(&mut buf).expect("读取失败");
        assert_eq!(data, buf.as_slice());
        //分配大小由 alloc_size 决定：包文件不应膨胀到 4MiB 数据块
        drop(rw);
        drop(alloc);
        let pack_len = fs::metadata(&pack).expect("读取包文件大小失败").len();
        assert!(
            pack_len < 4 * 1024 * 1024,
            "已知大小分配不应使用 4MiB 整块，包文件大小 {pack_len}"
        );
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
/// 跨块拼接：小文件缩小释放的 <4MiB 碎片应被后续未知大小分配复用，
/// 包文件不随文件数线性膨胀，数据回读一致。
/// Stitching: fragments (<4MiB) from shrunk small files are reused by later
/// unknown-size allocations; the pack size must not grow linearly.
fn virtual_file_alloc_stitch_fragments() {
    let (dir, pack) = setup_ok_test("virtual_file_alloc_stitch_fragments");
    {
        let mut alloc = create_new_rw(&pack);
        //8 个小文件：未知大小分配（4MiB 整块），set_len 缩小到 13KB 释放碎片，再写入
        for i in 0..8 {
            let path = format!("Test/Small{i}");
            let data: Vec<u8> = (0..(13 * 1024)).map(|j| ((j + i) % 251) as u8).collect();
            let mut rw = alloc
                .virtual_file_options()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
                .expect("创建虚拟文件失败");
            rw.set_len(13 * 1024).expect("set_len 失败");
            rw.write_all(&data).expect("写入失败");
        }
        //Drop 触发 save_all → file_gc 合并碎片进 empty_data_list；重开后从 .wbm 加载
        drop(alloc);
        let mut alloc = ManagerSync::options()
            .read(true)
            .write(true)
            .open(&pack)
            .expect("重新打开包文件失败");
        let before = fs::metadata(&pack).expect("读取包文件大小失败").len();
        //再分配一个未知大小文件（4MiB 请求）：碎片总量远超 4MiB，应跨块拼接复用
        let data: Vec<u8> = (0..(2 * 1024 * 1024)).map(|i| (i % 251) as u8).collect();
        let mut rw = alloc
            .virtual_file_options()
            .read(true)
            .write(true)
            .create_new(true)
            .open("Test/Big")
            .expect("创建虚拟文件失败");
        rw.write_all(&data).expect("写入失败");
        rw.seek(SeekFrom::Start(0)).expect("seek 失败");
        let mut buf = vec![0u8; data.len()];
        rw.read_exact(&mut buf).expect("读取失败");
        assert_eq!(data, buf.as_slice());
        drop(rw);
        drop(alloc);
        let after = fs::metadata(&pack).expect("读取包文件大小失败").len();
        //未复用碎片会尾部扩容 4MiB；复用碎片则增长远小于 4MiB
        assert!(
            after - before < 4 * 1024 * 1024,
            "碎片未被跨块复用，包文件增长 {}({before}->{after})",
            after - before
        );
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
/// 段数上限：碎片拼接所需段数超过 MAX_DATA_SEGMENTS(16) 时拒绝分配。
/// Segment cap: allocation is rejected when stitching needs more than
/// MAX_DATA_SEGMENTS fragments.
fn virtual_file_alloc_segment_limit_rejected() {
    let (dir, pack) = setup_ok_test("virtual_file_alloc_segment_limit_rejected");
    {
        let mut alloc = create_new_rw(&pack);
        //每个文件 set_len 缩小到 3932416B，释放约 261888B 碎片（128B 对齐）。
        //17×261888B = 4452096B >= 4MiB，但需 17 段 > 16 → 应拒绝
        for i in 0..17 {
            let path = format!("Test/Frag{i}");
            let data: Vec<u8> = (0..3_932_416usize).map(|j| ((j + i) % 251) as u8).collect();
            let mut rw = alloc
                .virtual_file_options()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
                .expect("创建虚拟文件失败");
            rw.set_len(3_932_416).expect("set_len 失败");
            rw.write_all(&data).expect("写入失败");
        }
        //Drop 触发 save_all → file_gc 合并碎片进 empty_data_list；重开后从 .wbm 加载
        drop(alloc);
        let mut alloc = ManagerSync::options()
            .read(true)
            .write(true)
            .open(&pack)
            .expect("重新打开包文件失败");
        let result = alloc
            .virtual_file_options()
            .read(true)
            .write(true)
            .create_new(true)
            .open("Test/Rejected");
        assert!(result.is_err(), "碎片拼接超过段数上限应拒绝分配");
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn file_write_read_back_no_separate_manifest() {
    let (dir, pack) = setup_ok_test("file_write_read_back_no_separate_manifest");
    {
        let mut alloc = ManagerSync::options()
            .read(true)
            .write(true)
            .create_new(true)
            .cow(false)
            .separate_manifest(false)
            .open(&pack)
            .expect("创建包文件失败");
        write_then_read_back(&mut alloc, "Test/A", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        write_then_read_back(
            &mut alloc,
            "Test/B",
            &[10, 25, 33, 41, 53, 64, 57, 87, 89, 110],
        );
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn file_write_read_back_no_len_no_separate_manifest() {
    let (dir, pack) = setup_ok_test("file_write_read_back_no_len_no_separate_manifest");
    {
        let mut alloc = ManagerSync::options()
            .read(true)
            .write(true)
            .create_new(true)
            .cow(false)
            .separate_manifest(false)
            .open(&pack)
            .expect("创建包文件失败");
        write_then_read_back_no_len(&mut alloc, "Test/A", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        write_then_read_back_no_len(
            &mut alloc,
            "Test/B",
            &[10, 25, 33, 41, 53, 64, 57, 87, 89, 110],
        );
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
        let mut alloc = create_new_rw(&pack);
        write_then_read_back(&mut alloc, path_a, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        write_then_read_back(
            &mut alloc,
            path_b,
            &[10, 25, 33, 41, 53, 64, 57, 87, 89, 110],
        );
    }

    // 第二轮：重新打开，覆写（其中 A 从 10 字节扩到 18 字节，B 缩到不同内容）
    {
        let mut alloc = ManagerSync::options()
            .read(true)
            .write(true)
            .open(&pack)
            .expect("打开包文件失败");
        reopen_write_then_read_back(
            &mut alloc,
            path_a,
            &[
                101, 124, 35, 124, 73, 24, 62, 83, 61, 124, 124, 12, 55, 21, 21, 144, 56, 1,
            ],
        );
        reopen_write_then_read_back(&mut alloc, path_b, &[10, 25, 33, 41, 53, 62, 5, 12, 14, 1]);
    }

    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

// === 多次重写 + GC 触发 / Multiple rewrites + GC triggering ===

/// 多次随机重写同一虚拟文件，每次重写后读回验证。
/// 随机数据 + 随机大小 + 多次重写 + 每次写后读验证。
#[test]
fn virtual_file_multiple_rewrite_random() {
    let (dir, pack) = setup_ok_test("virtual_file_multiple_rewrite_random");
    {
        let mut alloc = create_new_rw(&pack);
        let path = "RewriteFile";

        let init_size = rand::random_range(50..=200);
        let init_data: Vec<u8> = (0..init_size).map(|_| rand::random()).collect();
        write_then_read_back(&mut alloc, path, &init_data);

        let rewrite_count = rand::random_range(8..=15);
        for _ in 0..rewrite_count {
            let new_size = rand::random_range(20..=500);
            let new_data: Vec<u8> = (0..new_size).map(|_| rand::random()).collect();
            reopen_write_then_read_back(&mut alloc, path, &new_data);
        }
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

/// 创建10文件 → 删除5个（暂存GC） → Drop触发GC → 重写剩余5个+新建5个 → 全部验证。
#[test]
fn virtual_file_rewrite_trigger_gc() {
    const FILE_COUNT: usize = 10;
    let (dir, pack) = setup_ok_test("virtual_file_rewrite_trigger_gc");

    // Phase 1: Create 10 files, delete last 5 (stage GC items)
    {
        let mut alloc = create_new_rw(&pack);
        for i in 0..FILE_COUNT {
            let size = rand::random_range(100..=1000);
            let data: Vec<u8> = (0..size).map(|_| rand::random()).collect();
            write_then_read_back(&mut alloc, &format!("File_{i}"), &data);
        }
        for i in 5..FILE_COUNT {
            alloc
                .delete_file(&format!("File_{i}"))
                .expect("delete failed");
            assert!(
                !alloc
                    .path_exists(&format!("File_{i}"))
                    .expect("无法判断路径是否存在")
            );
        }
        for i in 0..5 {
            assert!(
                alloc
                    .path_exists(&format!("File_{i}"))
                    .expect("无法判断路径是否存在")
            );
        }
    } // Drop → save_all() → file_gc()

    // Phase 2: Rewrite remaining 5 + create 5 new
    {
        let mut alloc = ManagerSync::options()
            .read(true)
            .write(true)
            .open(&pack)
            .expect("reopen pack failed");

        for i in 0..5 {
            let new_size = rand::random_range(50..=800);
            let new_data: Vec<u8> = (0..new_size).map(|_| rand::random()).collect();
            reopen_write_then_read_back(&mut alloc, &format!("File_{i}"), &new_data);
        }
        for i in 0..5 {
            let size = rand::random_range(100..=500);
            let data: Vec<u8> = (0..size).map(|_| rand::random()).collect();
            write_then_read_back(&mut alloc, &format!("NewFile_{i}"), &data);
        }

        assert!(alloc.path_exists(&"File_0").expect("无法判断路径是否存在"));
        assert!(alloc.path_exists(&"File_4").expect("无法判断路径是否存在"));
        assert!(
            alloc
                .path_exists(&"NewFile_0")
                .expect("无法判断路径是否存在")
        );
        assert!(
            alloc
                .path_exists(&"NewFile_4")
                .expect("无法判断路径是否存在")
        );
        assert!(!alloc.path_exists(&"File_5").expect("无法判断路径是否存在"));
        assert!(!alloc.path_exists(&"File_9").expect("无法判断路径是否存在"));
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

/// 创建3个大文件(5-10KB) → 全部删除 → Drop触发GC → 新建3个文件(GC后空间复用) → 哈希验证。
#[test]
fn virtual_file_gc_reuse_rewrite() {
    let (dir, pack) = setup_ok_test("virtual_file_gc_reuse_rewrite");

    // Phase 1: Create 3 large files (5-10KB each), delete all (stage GC items)
    {
        let mut alloc = create_new_rw(&pack);
        for i in 0..3 {
            let size = 5000 + rand::random_range(0..=5000);
            let data: Vec<u8> = (0..size).map(|_| rand::random()).collect();
            write_then_read_back(&mut alloc, &format!("LargeFile_{i}"), &data);
        }
        for i in 0..3 {
            alloc
                .delete_file(&format!("LargeFile_{i}"))
                .expect("delete failed");
        }
    } // Drop → save_all() → file_gc()

    // Phase 2: Create new files (should reuse GC'd space)
    {
        let mut alloc = ManagerSync::options()
            .read(true)
            .write(true)
            .open(&pack)
            .expect("reopen pack failed");

        for i in 0..3 {
            let size = 3000 + rand::random_range(0..=3000);
            let data: Vec<u8> = (0..size).map(|_| rand::random()).collect();
            write_then_read_back(&mut alloc, &format!("ReusedFile_{i}"), &data);
        }

        assert!(
            !alloc
                .path_exists(&"LargeFile_0")
                .expect("无法判断路径是否存在")
        );
        assert!(
            alloc
                .path_exists(&"ReusedFile_0")
                .expect("无法判断路径是否存在")
        );
        assert!(
            alloc
                .path_exists(&"ReusedFile_2")
                .expect("无法判断路径是否存在")
        );

        let vhr = alloc.verify_all_file_hash().expect("hash verify failed");
        assert!(
            vhr.err_path.is_empty(),
            "GC后哈希验证应全部通过: {:?}",
            vhr.err_path
        );
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

// === set_len 收缩 / set_len shrink ===

/// 验证 `set_len` 缩小文件大小时的行为。
///
/// 通过多次 `set_len` 扩展创建多个内部数据块，写入可识别数据，
/// 然后大幅收缩（释放超过一个数据块），验证缩小后长度和数据正确性。
///
/// Verify `set_len` shrink behavior: extend to create multiple internal data blocks,
/// write identifiable data, then shrink significantly (freeing more than one block),
/// and verify the truncated length and data are correct.
#[test]
fn virtual_file_set_len_shrink() {
    let (dir, pack) = setup_ok_test("virtual_file_set_len_shrink");
    {
        let mut alloc = create_new_rw(&pack);
        let mut vf = alloc
            .create_virtual_file(&"shrink_file")
            .expect("创建文件失败");

        // 通过多次 set_len 扩展，创建多个内部数据块
        // Create multiple internal data blocks by repeatedly extending via set_len
        vf.set_len(768).expect("set_len(768) 扩展失败");
        vf.set_len(1536).expect("set_len(1536) 扩展失败");
        vf.set_len(2304).expect("set_len(2304) 扩展失败");

        // 填充可识别数据
        // Fill with identifiable data
        let data: Vec<u8> = (0u8..192).cycle().take(2304).collect();
        vf.seek(SeekFrom::Start(0)).expect("seek 失败");
        vf.write_all(&data).expect("写入全量数据失败");

        // 关闭，持久化
    }
    // 重新打开，执行收缩
    {
        let mut alloc = ManagerSync::options()
            .read(true)
            .write(true)
            .open(&pack)
            .expect("打开包文件失败");
        let mut vf = alloc
            .open_virtual_file(&"shrink_file")
            .expect("打开文件失败");

        // 大幅缩小 — 必须释放多个数据块
        // Shrink significantly — must free more than one data block
        let shrunk_len: u64 = 50;
        vf.set_len(shrunk_len).expect("set_len 收缩失败");

        // 验证缩小后的逻辑长度
        // Verify the logical length after shrink
        assert_eq!(
            vf.get_len(),
            shrunk_len,
            "set_len 收缩后文件长度应等于 shrunk_len"
        );

        // 读取前 shrunk_len 字节，验证与原始数据一致
        // Read the first shrunk_len bytes and verify they match the original data
        let mut buf = vec![0u8; shrunk_len as usize];
        vf.seek(SeekFrom::Start(0)).expect("seek 失败");
        vf.read_exact(&mut buf).expect("读取数据失败");
        let expected: Vec<u8> = (0u8..192).cycle().take(shrunk_len as usize).collect();
        assert_eq!(
            buf, expected,
            "set_len 收缩后前 {} 字节应与原始数据一致",
            shrunk_len
        );
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

// === 错误场景 / Error cases ===

#[test]
#[should_panic(expected = "文件可能已存在")]
fn create_pack_twice_should_fail() {
    let (dir, pack) = setup_ok_test("create_pack_twice_should_fail");
    ManagerSync::create_new(&pack).expect("创建包文件失败");
    // 第二次创建应失败（文件已存在或已被锁定）
    let r = ManagerSync::create_new(&pack);
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
    if let Err(err) = r {
        panic!("{err}");
    }
}

// ============================================================
// 多实例并发读写 / Multi-instance concurrent read/write
// ============================================================

/// 三个实例同时对同一个虚拟文件写入非重叠区域，读回后验证数据完整性。
/// Three instances write to non-overlapping regions of the same virtual file,
/// then read back to verify data integrity.
#[test]
fn multi_instance_write_non_overlapping() {
    let (dir, pack) = setup_ok_test("multi_instance_write_non_overlapping");
    {
        let mut alloc = create_new_rw(&pack);

        // 准备初始文件区域（12 字节），预分配空间
        // Prepare initial file region (12 bytes), pre-allocate space
        let path = "shared_file";
        {
            let mut wr = alloc.create_virtual_file(&path).expect("创建虚拟文件失败");
            wr.set_len(12).expect("设置长度失败");
            // 写入初始占位数据 / Write initial placeholder data
            wr.write_all(&[0u8; 12]).expect("写入数据失败");
        }

// 克隆同步管理器，创建两个"独立"的实例入口
// Clone synchronized manager to create two "independent" instance entry points
        let mut alloc2 = alloc.clone();
        let mut alloc3 = alloc.clone();

        // 实例 1: 在偏移 0 处写入 4 字节 / Instance 1: write 4 bytes at offset 0
        let mut inst1 = open_rw(&mut alloc, path);
        inst1.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
        inst1.write_all(&[10, 20, 30, 40]).expect("写入数据失败");

        // 实例 2: 在偏移 4 处写入 4 字节 / Instance 2: write 4 bytes at offset 4
        let mut inst2 = open_rw(&mut alloc2, path);
        inst2.seek(SeekFrom::Start(4)).expect("设置文件位置失败");
        inst2.write_all(&[50, 60, 70, 80]).expect("写入数据失败");

        // 实例 3: 在偏移 8 处写入 4 字节 / Instance 3: write 8 bytes at offset 8
        let mut inst3 = open_rw(&mut alloc3, path);
        inst3.seek(SeekFrom::Start(8)).expect("设置文件位置失败");
        inst3.write_all(&[90, 100, 110, 120]).expect("写入数据失败");

        // 释放所有实例后读回验证 / Drop all instances, then read back to verify
        drop(inst1);
        drop(inst2);
        drop(inst3);

        let mut reader = alloc
            .open_virtual_file(&path)
            .expect("打开虚拟文件失败");
        reader.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
        let mut buf = vec![0u8; 12];
        let n = reader.read(&mut buf).expect("读取数据失败");
        assert_eq!(n, 12);
        assert_eq!(
            buf,
            vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120],
            "多实例写入非重叠区域后数据不一致"
        );
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

/// 三个实例在多个线程中并发写入同一文件的不同区域。
/// Three instances write to different regions of the same file concurrently in multiple threads.
#[test]
fn multi_instance_concurrent_write() {
    let (dir, pack) = setup_ok_test("multi_instance_concurrent_write");
    {
        let mut alloc = create_new_rw(&pack);
        let path = "concurrent_file";

        // 预分配 30 字节空间 / Pre-allocate 30 bytes
        {
            let mut wr = alloc.create_virtual_file(&path).expect("创建虚拟文件失败");
            wr.set_len(30).expect("设置长度失败");
            wr.write_all(&[0u8; 30]).expect("写入数据失败");
        }

        // 从主线程创建所有实例（open_file 需要 &mut alloc 且持有写锁）
        // Create all instances from main thread (open_file requires &mut alloc and holds write lock)
        let wr1 = open_rw(&mut alloc, path);
        let wr2 = open_rw(&mut alloc, path);
        let wr3 = open_rw(&mut alloc, path);

        let mut handles = Vec::new();

        // 线程 1: 写入偏移 0..10
        handles.push(thread::spawn(move || {
            let mut wr = wr1;
            wr.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
            wr.write_all(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10])
                .expect("写入数据失败");
        }));

        // 线程 2: 写入偏移 10..20
        handles.push(thread::spawn(move || {
            let mut wr = wr2;
            wr.seek(SeekFrom::Start(10)).expect("设置文件位置失败");
            wr.write_all(&[11, 12, 13, 14, 15, 16, 17, 18, 19, 20])
                .expect("写入数据失败");
        }));

        // 线程 3: 写入偏移 20..30
        handles.push(thread::spawn(move || {
            let mut wr = wr3;
            wr.seek(SeekFrom::Start(20)).expect("设置文件位置失败");
            wr.write_all(&[21, 22, 23, 24, 25, 26, 27, 28, 29, 30])
                .expect("写入数据失败");
        }));

        for h in handles {
            h.join().expect("等待线程失败");
        }

        // 主线程读回验证 / Main thread reads back and verifies
        let mut reader = alloc
            .open_virtual_file(&path)
            .expect("打开虚拟文件失败");
        reader.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
        let mut buf = vec![0u8; 30];
        let n = reader.read(&mut buf).expect("读取数据失败");
        assert_eq!(n, 30);

        let expected: Vec<u8> = (1..=30).collect();
        assert_eq!(buf, expected, "并发多实例写入后数据不一致");
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

/// 三个实例混合读写：实例1写 → 实例2读 → 实例3追加写 → 读回验证。
/// Three instances mixed read/write: instance1 writes → instance2 reads → instance3 appends → verify.
#[test]
fn multi_instance_mixed_read_write() {
    let (dir, pack) = setup_ok_test("multi_instance_mixed_read_write");
    {
        let mut alloc = create_new_rw(&pack);
        let path = "mixed_file";

        // 预分配空间并写入初始数据 / Pre-allocate space and write initial data
        {
            let mut wr = alloc.create_virtual_file(&path).expect("创建虚拟文件失败");
            wr.set_len(6).expect("设置长度失败");
            wr.write_all(b"HELLO ").expect("写入数据失败");
        }

        // 从主线程创建所有实例 / Create all instances from main thread
        let mut inst1 = open_rw(&mut alloc, path);
        let mut inst2 = alloc
            .open_virtual_file(&path)
            .expect("打开虚拟文件失败");
        let mut inst3 = alloc
            .open_virtual_file(&path)
            .expect("打开虚拟文件失败");

        // 实例 1: 追加写入 "WORLD"（seek 到末尾，扩展文件）
        // Instance 1: append "WORLD" (seek to end, extend file)
        inst1.seek(SeekFrom::End(0)).expect("设置文件位置失败");
        inst1.write_all(b"WORLD").expect("写入数据失败");
        drop(inst1);

        // 实例 2: 从偏移 0 读取前 6 字节
        // Instance 2: read first 6 bytes from offset 0
        inst2.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
        let mut buf = vec![0u8; 6];
        let n = inst2.read(&mut buf).expect("读取数据失败");
        assert_eq!(n, 6);
        assert_eq!(&buf, b"HELLO ", "实例2应读取到初始数据");
        drop(inst2);

        // 实例 3: 从偏移 6 开始读取后 5 字节（实例1 刚写入的）
        // Instance 3: read last 5 bytes from offset 6 (just written by instance 1)
        inst3.seek(SeekFrom::Start(6)).expect("设置文件位置失败");
        let mut buf = vec![0u8; 5];
        let n = inst3.read(&mut buf).expect("读取数据失败");
        assert_eq!(n, 5);
        assert_eq!(&buf, b"WORLD", "实例3应读取到实例1追加的数据");
        drop(inst3);

        // 最终读回全部 11 字节 / Final read-back of all 11 bytes
        let mut reader = alloc
            .open_virtual_file(&path)
            .expect("打开虚拟文件失败");
        reader.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
        let mut buf = vec![0u8; 11];
        let n = reader.read(&mut buf).expect("读取数据失败");
        assert_eq!(n, 11);
        assert_eq!(&buf, b"HELLO WORLD", "混合读写后完整数据不一致");
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

/// 三个实例同时对多个不同文件进行读写（跨文件多实例测试）。
/// Three instances read/write different files simultaneously (cross-file multi-instance test).
#[test]
fn multi_instance_cross_file_rw() {
    let (dir, pack) = setup_ok_test("multi_instance_cross_file_rw");
    {
        let mut alloc = create_new_rw(&pack);

        // 创建三个不同的文件 / Create three different files
        for (path, len) in [("file_a", 10u64), ("file_b", 10), ("file_c", 10)] {
            let mut wr = alloc.create_virtual_file(&path).expect("创建虚拟文件失败");
            wr.set_len(len).expect("设置长度失败");
            wr.write_all(&[0u8; 10]).expect("写入数据失败");
        }

        let mut alloc2 = alloc.clone();
        let mut alloc3 = alloc.clone();

        // 实例 1: 写 file_a / Instance 1: write file_a
        let mut inst1 = open_rw(&mut alloc, "file_a");
        inst1.write_all(b"AAAAAAAAAA").expect("写入数据失败");

        // 实例 2: 写 file_b / Instance 2: write file_b
        let mut inst2 = open_rw(&mut alloc2, "file_b");
        inst2.write_all(b"BBBBBBBBBB").expect("写入数据失败");

        // 实例 3: 写 file_c / Instance 3: write file_c
        let mut inst3 = open_rw(&mut alloc3, "file_c");
        inst3.write_all(b"CCCCCCCCCC").expect("写入数据失败");

        drop(inst1);
        drop(inst2);
        drop(inst3);

        // 分别读回验证 / Read back each file and verify
        for (name, expected) in [
            ("file_a", b"AAAAAAAAAA"),
            ("file_b", b"BBBBBBBBBB"),
            ("file_c", b"CCCCCCCCCC"),
        ] {
            let mut reader = alloc
                .open_virtual_file(&name)
                .expect("打开虚拟文件失败");
            let mut buf = vec![0u8; 10];
            reader.read_exact(&mut buf).expect("读取数据失败");
            assert_eq!(&buf[..], expected, "文件 {name} 数据不一致");
        }
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

/// 五个实例并发随机读写压力测试（使用 auto_sized 创建文件）。
/// Five-instance concurrent random read/write stress test (using auto_sized file creation).
/// 三个标准 stress 场景：每个线程先读→写→读验证。
/// Each thread: read initial data → write its own marker → read back and verify.
#[test]
fn multi_instance_stress_three_instances() {
    const N: usize = 3;
    const FS: usize = 60;
    let (dir, pack) = setup_ok_test("stress_3_instances");

    {
        let mut alloc = create_new_rw(&pack);
        let path = "s3";

        let initial: Vec<u8> = (0..FS).map(|_| rand::random::<u8>()).collect();
        {
            let mut wr = alloc.create_virtual_file(&path).expect("创建虚拟文件失败");
            wr.set_len(FS as u64).expect("设置长度失败");
            wr.write_all(&initial).expect("写入数据失败");
        }

        let instances: Vec<PackVirtualFile> = (0..N).map(|_| open_rw(&mut alloc, path)).collect();

        let initial = Arc::new(initial);
        let mut handles = Vec::new();

        for (id, mut wr) in instances.into_iter().enumerate() {
            let init_clone = Arc::clone(&initial);
            handles.push(thread::spawn(move || {
                let start = (id * (FS / N)) as u64;
                let len = (FS / N) as u64;

                // 读初始数据
                wr.seek(SeekFrom::Start(start)).expect("设置文件位置失败");
                let mut buf = vec![0u8; len as usize];
                wr.read_exact(&mut buf).expect("读取数据失败");
                assert_eq!(
                    buf,
                    init_clone[start as usize..][..len as usize],
                    "线程 {id}: 初始数据读不一致"
                );

                // 写标记
                let wdata: Vec<u8> = (0..len).map(|_| (id + 1) as u8).collect();
                wr.seek(SeekFrom::Start(start)).expect("设置文件位置失败");
                wr.write_all(&wdata).expect("写入数据失败");

                // 读回验证
                wr.seek(SeekFrom::Start(start)).expect("设置文件位置失败");
                let mut vbuf = vec![0u8; len as usize];
                wr.read_exact(&mut vbuf).expect("读取数据失败");
                assert_eq!(vbuf, wdata, "线程 {id}: 写后读回不一致");
            }));
        }

        for h in handles {
            h.join().expect("等待线程失败");
        }

        // 最终验证整个文件
        let mut reader = alloc
            .open_virtual_file(&path)
            .expect("打开虚拟文件失败");
        let mut fb = vec![0u8; FS];
        reader.read_exact(&mut fb).expect("读取数据失败");
        for id in 0..N {
            let start = id * (FS / N);
            let end = start + FS / N;
            let exp = (id + 1) as u8;
            assert!(
                fb[start..end].iter().all(|&b| b == exp),
                "区域 {id} 验证失败: 期望全 {exp}, 实际: {:?}",
                &fb[start..end]
            );
        }
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

/// 三个实例交替 seek + write，验证各实例位置独立性。
/// Three instances alternating seek + write, verifying each instance's position independence.
#[test]
fn multi_instance_independent_positions() {
    let (dir, pack) = setup_ok_test("multi_instance_independent_positions");
    {
        let mut alloc = create_new_rw(&pack);
        let path = "pos_file";

        // 预分配 20 字节 / Pre-allocate 20 bytes
        {
            let mut wr = alloc.create_virtual_file(&path).expect("创建虚拟文件失败");
            wr.set_len(20).expect("设置长度失败");
            wr.write_all(&[0u8; 20]).expect("写入数据失败");
        }

        let mut alloc2 = alloc.clone();
        let mut alloc3 = alloc.clone();

        let mut inst1 = open_rw(&mut alloc, path);
        let mut inst2 = open_rw(&mut alloc2, path);
        let mut inst3 = open_rw(&mut alloc3, path);

        // 实例 1: seek(2), write "AB"
        inst1.seek(SeekFrom::Start(2)).expect("设置文件位置失败");
        inst1.write_all(b"AB").expect("写入数据失败");

        // 实例 2: seek(10), write "CD"
        inst2.seek(SeekFrom::Start(10)).expect("设置文件位置失败");
        inst2.write_all(b"CD").expect("写入数据失败");

        // 实例 3: seek(15), write "EF"
        inst3.seek(SeekFrom::Start(15)).expect("设置文件位置失败");
        inst3.write_all(b"EF").expect("写入数据失败");

        // 实例 1 继续: seek(0), write "XY" — 验证位置不受实例 2/3 影响
        inst1.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
        inst1.write_all(b"XY").expect("写入数据失败");

        // 实例 2: seek(8), write "ZZ" — 覆盖已存在区域
        inst2.seek(SeekFrom::Start(8)).expect("设置文件位置失败");
        inst2.write_all(b"ZZ").expect("写入数据失败");

        // 实例 3: 从当前位置继续写 — 应该在偏移 17 处（之前写到 15+2=17）
        inst3.write_all(b"GH").expect("写入数据失败");

        drop(inst1);
        drop(inst2);
        drop(inst3);

        // 读回全量验证 / Read back all and verify
        let mut reader = alloc
            .open_virtual_file(&path)
            .expect("打开虚拟文件失败");
        let mut buf = vec![0u8; 20];
        reader.read_exact(&mut buf).expect("读取数据失败");

        // 预期布局:
        // Expected layout:
        // [0-1]: "XY" (inst1 最后写入) / "XY" (inst1 wrote last)
        // [2-3]: "AB" (inst1) / "AB" (inst1)
        // [4-7]: 0,0,0,0 (未修改) / 0,0,0,0 (untouched)
        // [8-9]: "ZZ" (inst2) / "ZZ" (inst2)
        // [10-11]: "CD" (inst2) / "CD" (inst2)
        // [12-14]: 0,0,0 (未修改)
        // [15-16]: "EF" (inst3, 第一次写入) / "EF" (inst3, first write)
        // [17-18]: "GH" (inst3, 第二次写入) / "GH" (inst3, second write)
        // [19]: 0 (未修改) / 0 (untouched)
        assert_eq!(&buf[0..2], b"XY");
        assert_eq!(&buf[2..4], b"AB");
        assert_eq!(&buf[4..8], &[0, 0, 0, 0]);
        assert_eq!(&buf[8..10], b"ZZ");
        assert_eq!(&buf[10..12], b"CD");
        assert_eq!(&buf[12..15], &[0, 0, 0]);
        assert_eq!(&buf[15..17], b"EF");
        assert_eq!(&buf[17..19], b"GH");
        assert_eq!(buf[19], 0);
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

// === 删除/擦除功能 / Delete/Erase ===

#[test]
fn delete_file_root() {
    let (dir, pack) = setup_ok_test("delete_file_root");
    {
        let mut alloc = create_new_rw(&pack);
        write_then_read_back(&mut alloc, "test.txt", &[1, 2, 3, 4, 5]);
        assert!(alloc.path_exists(&"test.txt").expect("检查路径存在失败"));
        alloc.delete_file(&"test.txt").expect("删除文件失败");
        assert!(!alloc.path_exists(&"test.txt").expect("检查路径存在失败"));
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn delete_file_subdir() {
    let (dir, pack) = setup_ok_test("delete_file_subdir");
    {
        let mut alloc = create_new_rw(&pack);
        alloc
            .create_dir_all(&String::from("A/B"))
            .expect("创建目录失败");
        write_then_read_back(&mut alloc, "A/B/file.txt", &[10, 20, 30]);
        assert!(alloc.path_exists(&"A/B/file.txt").expect("检查路径存在失败"));
        assert!(alloc.path_exists(&"A/B").expect("检查路径存在失败"));
        alloc.delete_file(&"A/B/file.txt").expect("删除文件失败");
        assert!(!alloc.path_exists(&"A/B/file.txt").expect("检查路径存在失败"));
        assert!(alloc.path_exists(&"A/B").expect("检查路径存在失败"));
        assert!(alloc.path_exists(&"A").expect("检查路径存在失败"));
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn delete_dir_all_empty() {
    let (dir, pack) = setup_ok_test("delete_dir_all_empty");
    {
        let mut alloc = ManagerSync::create_new(&pack).expect("创建包文件失败");
        alloc
            .create_dir_all(&String::from("empty_dir"))
            .expect("创建目录失败");
        assert!(alloc.path_exists(&"empty_dir").expect("检查路径存在失败"));
        alloc.delete_dir_all(&"empty_dir").expect("删除目录失败");
        assert!(!alloc.path_exists(&"empty_dir").expect("检查路径存在失败"));
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn delete_dir_all_with_files() {
    let (dir, pack) = setup_ok_test("delete_dir_all_with_files");
    {
        let mut alloc = create_new_rw(&pack);
        alloc
            .create_dir_all(&String::from("A/B"))
            .expect("创建目录失败");
        write_then_read_back(&mut alloc, "A/1", &[1]);
        write_then_read_back(&mut alloc, "A/2", &[2, 2]);
        write_then_read_back(&mut alloc, "A/B/3", &[3, 3, 3]);
        assert!(alloc.path_exists(&"A/1").expect("检查路径存在失败"));
        assert!(alloc.path_exists(&"A/2").expect("检查路径存在失败"));
        assert!(alloc.path_exists(&"A/B/3").expect("检查路径存在失败"));
        assert!(alloc.path_exists(&"A/B").expect("检查路径存在失败"));
        alloc.delete_dir_all(&"A").expect("删除目录失败");
        assert!(!alloc.path_exists(&"A").expect("检查路径存在失败"));
        assert!(!alloc.path_exists(&"A/1").expect("检查路径存在失败"));
        assert!(!alloc.path_exists(&"A/2").expect("检查路径存在失败"));
        assert!(!alloc.path_exists(&"A/B").expect("检查路径存在失败"));
        assert!(!alloc.path_exists(&"A/B/3").expect("检查路径存在失败"));
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn delete_dir_all_nonempty() {
    let (dir, pack) = setup_ok_test("delete_dir_all_nonempty");
    {
        let mut alloc = create_new_rw(&pack);
        alloc
            .create_dir_all(&String::from("data"))
            .expect("创建目录失败");
        write_then_read_back(&mut alloc, "data/f1", &[1, 2]);
        write_then_read_back(&mut alloc, "data/f2", &[3, 4]);
        assert!(alloc.path_exists(&"data").expect("检查路径存在失败"));
        alloc.delete_dir_all(&"data").expect("删除目录失败");
        assert!(!alloc.path_exists(&"data").expect("检查路径存在失败"));
        assert!(!alloc.path_exists(&"data/f1").expect("检查路径存在失败"));
        assert!(!alloc.path_exists(&"data/f2").expect("检查路径存在失败"));
        assert!(alloc.get_root_struct_item_name_list().is_ok());
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn erase_file() {
    let (dir, pack) = setup_ok_test("erase_file");
    // 写入数据后关闭包再重新打开，确保数据已持久化到磁盘再擦除
    // Write data, close then reopen to ensure data is on disk before erase
    {
        let mut alloc = create_new_rw(&pack);
        write_then_read_back(&mut alloc, "secret.txt", &[1u8; 256]);
        assert!(alloc.path_exists(&"secret.txt").expect("检查路径存在失败"));
    }
    {
        let mut alloc = ManagerSync::open(&pack).expect("打开包文件失败");
        assert!(alloc.path_exists(&"secret.txt").expect("检查路径存在失败"));
        alloc
            .erase_file("secret.txt", OverwriteStrategy::Zero)
            .expect("擦除文件失败");
        assert!(!alloc.path_exists(&"secret.txt").expect("检查路径存在失败"));
    }
    {
        let mut alloc = ManagerSync::open(&pack).expect("打开包文件失败");
        assert!(!alloc.path_exists(&"secret.txt").expect("检查路径存在失败"));
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn erase_dir_all() {
    let (dir, pack) = setup_ok_test("erase_dir_all");
    {
        let mut alloc = create_new_rw(&pack);
        alloc
            .create_dir_all(&String::from("top/mid"))
            .expect("创建目录失败");
        write_then_read_back(&mut alloc, "top/f1", &[1, 2, 3]);
        write_then_read_back(&mut alloc, "top/mid/f2", &[4, 5, 6]);
        assert!(alloc.path_exists(&"top/f1").expect("检查路径存在失败"));
        assert!(alloc.path_exists(&"top/mid/f2").expect("检查路径存在失败"));
    }
    {
        let mut alloc = ManagerSync::open(&pack).expect("打开包文件失败");
        assert!(alloc.path_exists(&"top/f1").expect("检查路径存在失败"));
        assert!(alloc.path_exists(&"top/mid/f2").expect("检查路径存在失败"));
        alloc
            .erase_dir_all("top", OverwriteStrategy::Zero)
            .expect("擦除目录失败");
        assert!(!alloc.path_exists(&"top").expect("检查路径存在失败"));
        assert!(!alloc.path_exists(&"top/f1").expect("检查路径存在失败"));
        assert!(!alloc.path_exists(&"top/mid").expect("检查路径存在失败"));
        assert!(!alloc.path_exists(&"top/mid/f2").expect("检查路径存在失败"));
    }
    {
        let mut alloc = ManagerSync::open(&pack).expect("打开包文件失败");
        assert!(!alloc.path_exists(&"top").expect("检查路径存在失败"));
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn erase_file_random() {
    let (dir, pack) = setup_ok_test("erase_file_random");
    {
        let mut alloc = create_new_rw(&pack);
        write_then_read_back(&mut alloc, "rnd.txt", &[10, 20, 30, 40, 50]);
        assert!(alloc.path_exists(&"rnd.txt").expect("检查路径存在失败"));
    }
    {
        let mut alloc = ManagerSync::open(&pack).expect("打开包文件失败");
        assert!(alloc.path_exists(&"rnd.txt").expect("检查路径存在失败"));
        alloc
            .erase_file("rnd.txt", OverwriteStrategy::Random)
            .expect("擦除文件失败");
        assert!(!alloc.path_exists(&"rnd.txt").expect("检查路径存在失败"));
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn erase_file_dod5220() {
    let (dir, pack) = setup_ok_test("erase_file_dod5220");
    {
        let mut alloc = create_new_rw(&pack);
        write_then_read_back(&mut alloc, "dod.txt", &[100, 200, 150]);
        assert!(alloc.path_exists(&"dod.txt").expect("检查路径存在失败"));
    }
    {
        let mut alloc = ManagerSync::open(&pack).expect("打开包文件失败");
        assert!(alloc.path_exists(&"dod.txt").expect("检查路径存在失败"));
        alloc
            .erase_file("dod.txt", OverwriteStrategy::Dod5220)
            .expect("擦除文件失败");
        assert!(!alloc.path_exists(&"dod.txt").expect("检查路径存在失败"));
    }
    {
        let mut alloc = ManagerSync::open(&pack).expect("打开包文件失败");
        assert!(!alloc.path_exists(&"dod.txt").expect("检查路径存在失败"));
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn delete_nonexistent() {
    let (dir, pack) = setup_ok_test("delete_nonexistent");
    {
        let mut alloc = ManagerSync::create_new(&pack).expect("创建包文件失败");
        let result = alloc.delete_file(&"nonexistent.txt");
        assert!(result.is_err());
        let result = alloc.delete_dir_all(&"nonexistent_dir");
        assert!(result.is_err());
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}

#[test]
fn reopen_after_delete() {
    let (dir, pack) = setup_ok_test("reopen_after_delete");
    let data_a = &[1, 2, 3, 4, 5];
    let data_b = &[10, 20, 30, 40, 50];
    let data_c = &[100, 200, 250];
    {
        let mut alloc = create_new_rw(&pack);
        write_then_read_back(&mut alloc, "keep_a", data_a);
        write_then_read_back(&mut alloc, "keep_b", data_b);
        write_then_read_back(&mut alloc, "delete_c", data_c);
        assert!(alloc.path_exists(&"keep_a").expect("检查路径存在失败"));
        assert!(alloc.path_exists(&"keep_b").expect("检查路径存在失败"));
        assert!(alloc.path_exists(&"delete_c").expect("检查路径存在失败"));
        alloc.delete_file(&"delete_c").expect("删除文件失败");
        assert!(!alloc.path_exists(&"delete_c").expect("检查路径存在失败"));
    }
    {
        let mut alloc = ManagerSync::open(&pack).expect("打开包文件失败");
        assert!(!alloc.path_exists(&"delete_c").expect("检查路径存在失败"));
        assert!(alloc.path_exists(&"keep_a").expect("检查路径存在失败"));
        assert!(alloc.path_exists(&"keep_b").expect("检查路径存在失败"));
        let mut buf = vec![0u8; data_a.len()];
        let mut r = alloc
            .open_virtual_file(&"keep_a")
            .expect("打开虚拟文件失败");
        r.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
        r.read_exact(&mut buf).expect("读取数据失败");
        assert_eq!(buf, data_a);
        drop(r);
        let mut buf = vec![0u8; data_b.len()];
        let mut r = alloc
            .open_virtual_file(&"keep_b")
            .expect("打开虚拟文件失败");
        r.seek(SeekFrom::Start(0)).expect("设置文件位置失败");
        r.read_exact(&mut buf).expect("读取数据失败");
        assert_eq!(buf, data_b);
    }
    TestTool::remove_test_pack_files(&pack);
    _ = fs::remove_dir_all(&dir);
}
