use crate::{tools::TestTool, wb_files_pack::allocator::Allocator};
use crate::wb_files_pack::pack_io::file::PackFileWR;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

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
    {
        Allocator::create_new_pack_file2(&pack).unwrap();
    }
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
        let mut alloc = Allocator::create_new_pack_file2(&pack).unwrap();
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
fn file_write_read_back_no_separate_manifest() {
    let (dir, pack) = setup_ok_test("file_write_read_back_no_separate_manifest");
    {
        let mut alloc = Allocator::create_pack_file(&pack, false, false, true).unwrap();
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
        let mut alloc = Allocator::create_pack_file(&pack, false, false, true).unwrap();
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
        let mut alloc = Allocator::create_new_pack_file2(&pack).unwrap();
        write_then_read_back(&mut alloc, path_a, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        write_then_read_back(
            &mut alloc,
            path_b,
            &[10, 25, 33, 41, 53, 64, 57, 87, 89, 110],
        );
    }

    // 第二轮：重新打开，覆写（其中 A 从 10 字节扩到 18 字节，B 缩到不同内容）
    {
        let mut alloc = Allocator::open_pack_file(&pack).unwrap();
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
        let mut alloc = Allocator::create_new_pack_file2(&pack).unwrap();

        // 准备初始文件区域（12 字节），预分配空间
        // Prepare initial file region (12 bytes), pre-allocate space
        let path = "shared_file";
        {
            let mut wr = alloc.create_file(path, 0, 12).unwrap();
            // 写入初始占位数据 / Write initial placeholder data
            wr.write_all(&[0u8; 12]).unwrap();
        }

        // 克隆分配器，创建两个"独立"的实例入口
        // Clone allocator to create two "independent" instance entry points
        let mut alloc2 = alloc.clone();
        let mut alloc3 = alloc.clone();

        // 实例 1: 在偏移 0 处写入 4 字节 / Instance 1: write 4 bytes at offset 0
        let mut inst1 = alloc.open_file(path, false).unwrap();
        inst1.seek(SeekFrom::Start(0)).unwrap();
        inst1.write_all(&[10, 20, 30, 40]).unwrap();

        // 实例 2: 在偏移 4 处写入 4 字节 / Instance 2: write 4 bytes at offset 4
        let mut inst2 = alloc2.open_file(path, false).unwrap();
        inst2.seek(SeekFrom::Start(4)).unwrap();
        inst2.write_all(&[50, 60, 70, 80]).unwrap();

        // 实例 3: 在偏移 8 处写入 4 字节 / Instance 3: write 4 bytes at offset 8
        let mut inst3 = alloc3.open_file(path, false).unwrap();
        inst3.seek(SeekFrom::Start(8)).unwrap();
        inst3.write_all(&[90, 100, 110, 120]).unwrap();

        // 释放所有实例后读回验证 / Drop all instances, then read back to verify
        drop(inst1);
        drop(inst2);
        drop(inst3);

        let mut reader = alloc.open_file(path, false).unwrap();
        reader.seek(SeekFrom::Start(0)).unwrap();
        let mut buf = vec![0u8; 12];
        let n = reader.read(&mut buf).unwrap();
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
        let mut alloc = Allocator::create_new_pack_file2(&pack).unwrap();
        let path = "concurrent_file";

        // 预分配 30 字节空间 / Pre-allocate 30 bytes
        {
            let mut wr = alloc.create_file(path, 0, 30).unwrap();
            wr.write_all(&[0u8; 30]).unwrap();
        }

        // 从主线程创建所有实例（open_file 需要 &mut alloc 且持有写锁）
        // Create all instances from main thread (open_file requires &mut alloc and holds write lock)
        let wr1 = alloc.open_file(path, false).unwrap();
        let wr2 = alloc.open_file(path, false).unwrap();
        let wr3 = alloc.open_file(path, false).unwrap();

        let mut handles = Vec::new();

        // 线程 1: 写入偏移 0..10
        handles.push(thread::spawn(move || {
            let mut wr = wr1;
            wr.seek(SeekFrom::Start(0)).unwrap();
            wr.write_all(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]).unwrap();
        }));

        // 线程 2: 写入偏移 10..20
        handles.push(thread::spawn(move || {
            let mut wr = wr2;
            wr.seek(SeekFrom::Start(10)).unwrap();
            wr.write_all(&[11, 12, 13, 14, 15, 16, 17, 18, 19, 20]).unwrap();
        }));

        // 线程 3: 写入偏移 20..30
        handles.push(thread::spawn(move || {
            let mut wr = wr3;
            wr.seek(SeekFrom::Start(20)).unwrap();
            wr.write_all(&[21, 22, 23, 24, 25, 26, 27, 28, 29, 30]).unwrap();
        }));

        for h in handles {
            h.join().unwrap();
        }

        // 主线程读回验证 / Main thread reads back and verifies
        let mut reader = alloc.open_file(path, false).unwrap();
        reader.seek(SeekFrom::Start(0)).unwrap();
        let mut buf = vec![0u8; 30];
        let n = reader.read(&mut buf).unwrap();
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
        let mut alloc = Allocator::create_new_pack_file2(&pack).unwrap();
        let path = "mixed_file";

        // 预分配空间并写入初始数据 / Pre-allocate space and write initial data
        {
            let mut wr = alloc.create_file(path, 0, 6).unwrap();
            wr.write_all(b"HELLO ").unwrap();
        }

        // 从主线程创建所有实例 / Create all instances from main thread
        let mut inst1 = alloc.open_file(path, false).unwrap();
        let mut inst2 = alloc.open_file(path, false).unwrap();
        let mut inst3 = alloc.open_file(path, false).unwrap();

        // 实例 1: 追加写入 "WORLD"（seek 到末尾，扩展文件）
        // Instance 1: append "WORLD" (seek to end, extend file)
        inst1.seek(SeekFrom::End(0)).unwrap();
        inst1.write_all(b"WORLD").unwrap();
        drop(inst1);

        // 实例 2: 从偏移 0 读取前 6 字节
        // Instance 2: read first 6 bytes from offset 0
        inst2.seek(SeekFrom::Start(0)).unwrap();
        let mut buf = vec![0u8; 6];
        let n = inst2.read(&mut buf).unwrap();
        assert_eq!(n, 6);
        assert_eq!(&buf, b"HELLO ", "实例2应读取到初始数据");
        drop(inst2);

        // 实例 3: 从偏移 6 开始读取后 5 字节（实例1 刚写入的）
        // Instance 3: read last 5 bytes from offset 6 (just written by instance 1)
        inst3.seek(SeekFrom::Start(6)).unwrap();
        let mut buf = vec![0u8; 5];
        let n = inst3.read(&mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, b"WORLD", "实例3应读取到实例1追加的数据");
        drop(inst3);

        // 最终读回全部 11 字节 / Final read-back of all 11 bytes
        let mut reader = alloc.open_file(path, false).unwrap();
        reader.seek(SeekFrom::Start(0)).unwrap();
        let mut buf = vec![0u8; 11];
        let n = reader.read(&mut buf).unwrap();
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
        let mut alloc = Allocator::create_new_pack_file2(&pack).unwrap();

        // 创建三个不同的文件 / Create three different files
        for (path, len) in [("file_a", 10u64), ("file_b", 10), ("file_c", 10)] {
            let mut wr = alloc.create_file(path, 0, len).unwrap();
            wr.write_all(&[0u8; 10]).unwrap();
        }

        let mut alloc2 = alloc.clone();
        let mut alloc3 = alloc.clone();

        // 实例 1: 写 file_a / Instance 1: write file_a
        let mut inst1 = alloc.open_file("file_a", false).unwrap();
        inst1.write_all(b"AAAAAAAAAA").unwrap();

        // 实例 2: 写 file_b / Instance 2: write file_b
        let mut inst2 = alloc2.open_file("file_b", false).unwrap();
        inst2.write_all(b"BBBBBBBBBB").unwrap();

        // 实例 3: 写 file_c / Instance 3: write file_c
        let mut inst3 = alloc3.open_file("file_c", false).unwrap();
        inst3.write_all(b"CCCCCCCCCC").unwrap();

        drop(inst1);
        drop(inst2);
        drop(inst3);

        // 分别读回验证 / Read back each file and verify
        for (name, expected) in [("file_a", b"AAAAAAAAAA"), ("file_b", b"BBBBBBBBBB"), ("file_c", b"CCCCCCCCCC")] {
            let mut reader = alloc.open_file(name, false).unwrap();
            let mut buf = vec![0u8; 10];
            reader.read_exact(&mut buf).unwrap();
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
    let (dir, pack) = setup_ok_test("stress_3_instances");
    const N: usize = 3;
    const FS: usize = 60;

    {
        let mut alloc = Allocator::create_new_pack_file2(&pack).unwrap();
        let path = "s3";

        let initial: Vec<u8> = (0..FS).map(|_| rand::random::<u8>()).collect();
        {
            let mut wr = alloc.create_file(path, 0, FS as u64).unwrap();
            wr.write_all(&initial).unwrap();
        }

        let instances: Vec<PackFileWR> = (0..N)
            .map(|_| alloc.open_file(path, false).unwrap())
            .collect();

        let initial = Arc::new(initial);
        let mut handles = Vec::new();

        for (id, mut wr) in instances.into_iter().enumerate() {
            let init_clone = Arc::clone(&initial);
            handles.push(thread::spawn(move || {
                let start = (id * (FS / N)) as u64;
                let len = (FS / N) as u64;

                // 读初始数据
                wr.seek(SeekFrom::Start(start)).unwrap();
                let mut buf = vec![0u8; len as usize];
                wr.read_exact(&mut buf).unwrap();
                assert_eq!(
                    buf, init_clone[start as usize..][..len as usize],
                    "线程 {id}: 初始数据读不一致"
                );

                // 写标记
                let wdata: Vec<u8> = (0..len).map(|_| (id + 1) as u8).collect();
                wr.seek(SeekFrom::Start(start)).unwrap();
                wr.write_all(&wdata).unwrap();

                // 读回验证
                wr.seek(SeekFrom::Start(start)).unwrap();
                let mut vbuf = vec![0u8; len as usize];
                wr.read_exact(&mut vbuf).unwrap();
                assert_eq!(vbuf, wdata, "线程 {id}: 写后读回不一致");
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // 最终验证整个文件
        let mut reader = alloc.open_file(path, false).unwrap();
        let mut fb = vec![0u8; FS];
        reader.read_exact(&mut fb).unwrap();
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
        let mut alloc = Allocator::create_new_pack_file2(&pack).unwrap();
        let path = "pos_file";

        // 预分配 20 字节 / Pre-allocate 20 bytes
        {
            let mut wr = alloc.create_file(path, 0, 20).unwrap();
            wr.write_all(&[0u8; 20]).unwrap();
        }

        let mut alloc2 = alloc.clone();
        let mut alloc3 = alloc.clone();

        let mut inst1 = alloc.open_file(path, false).unwrap();
        let mut inst2 = alloc2.open_file(path, false).unwrap();
        let mut inst3 = alloc3.open_file(path, false).unwrap();

        // 实例 1: seek(2), write "AB"
        inst1.seek(SeekFrom::Start(2)).unwrap();
        inst1.write_all(b"AB").unwrap();

        // 实例 2: seek(10), write "CD"
        inst2.seek(SeekFrom::Start(10)).unwrap();
        inst2.write_all(b"CD").unwrap();

        // 实例 3: seek(15), write "EF"
        inst3.seek(SeekFrom::Start(15)).unwrap();
        inst3.write_all(b"EF").unwrap();

        // 实例 1 继续: seek(0), write "XY" — 验证位置不受实例 2/3 影响
        inst1.seek(SeekFrom::Start(0)).unwrap();
        inst1.write_all(b"XY").unwrap();

        // 实例 2: seek(8), write "ZZ" — 覆盖已存在区域
        inst2.seek(SeekFrom::Start(8)).unwrap();
        inst2.write_all(b"ZZ").unwrap();

        // 实例 3: 从当前位置继续写 — 应该在偏移 17 处（之前写到 15+2=17）
        inst3.write_all(b"GH").unwrap();

        drop(inst1);
        drop(inst2);
        drop(inst3);

        // 读回全量验证 / Read back all and verify
        let mut reader = alloc.open_file(path, false).unwrap();
        let mut buf = vec![0u8; 20];
        reader.read_exact(&mut buf).unwrap();

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
