use crate::tools::PathTool;
use crate::tools::TestTool;
use crate::wb_files_pack::manager::{
    WBFPManager,
    DEFAULT_COW,
    DEFAULT_HASH_TYPE,
    DEFAULT_SEPARATE_MANIFEST,
};
use crate::wb_files_pack::pack_io::file::PackFileWR;
use crate::wb_files_pack::pack_io::PackIO;
use crate::wb_files_pack::pack_io::file_handle::PackFileHandle;
use pretty_assertions::assert_eq;
use std::fs::File;
use std::io::{ Error, ErrorKind, Read, Write };
use std::path::Path;
use std::sync::{ Arc, Mutex };
use std::{ fs, io };

static TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/manager/ok";
static TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/manager/err";

// === 辅助函数：直接操作内部 API / Helpers bypassing Allocator ===

fn create_manager(
    pack_path: &Path,
    cow: bool,
    separate_manifest: bool
) -> io::Result<(WBFPManager, Arc<Mutex<PackIO>>)> {
    let pack_file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .create_new(true)
        .open(pack_path)
        .map_err(|e| {
            match e.kind() {
                ErrorKind::AlreadyExists => {
                    Error::new(ErrorKind::AlreadyExists, format!("文件已存在, err: {e}"))
                }
                _ => panic!("创建文件错误. err:{e}"),
            }
        })?;
    let pack_io = PackIO::new(pack_file);
    let pack_io = Arc::new(Mutex::new(pack_io));
    let mut manager = WBFPManager::create_pack_file(
        &pack_path,
        pack_io.clone(),
        cow,
        separate_manifest,
        true
    ).expect("无法创建包管理器");
    manager.init_new_pack().expect("初始化新包文件错误");
    Ok((manager, pack_io))
}

fn create_manager_default(pack_path: &Path) -> io::Result<(WBFPManager, Arc<Mutex<PackIO>>)> {
    create_manager(pack_path, DEFAULT_COW, DEFAULT_SEPARATE_MANIFEST)
}

fn open_manager<P: AsRef<Path>>(pack_path: &P) -> (WBFPManager, Arc<Mutex<PackIO>>) {
    let pack_file = File::options().read(true).write(true).open(pack_path).expect("无法打开文件");
    let pack_io = PackIO::new(pack_file);
    let pack_io = Arc::new(Mutex::new(pack_io));
    (WBFPManager::open_pack_file(pack_path, pack_io.clone()).expect("无法打开包文件"), pack_io)
}

fn open_file_rw<P: AsRef<Path>>(
    pack_path: &P,
    manager: &Arc<Mutex<WBFPManager>>,
    pack_io: &Arc<Mutex<PackIO>>,
    end_pos: bool
) -> io::Result<PackFileWR> {
    let path_list = PathTool::path_to_string_vec(pack_path);
    let metadata = manager
        .lock()
        .expect("无法获得包文件锁")
        .file_metadata_lock(&path_list)
        .expect("无法获得元数据");
    //TODO:若句柄已打开则直接获取
    let handle = Arc::new(
        Mutex::new(
            PackFileHandle::create(false, manager.clone(), pack_io, path_list, metadata, end_pos)?
        )
    );
    Ok(PackFileWR::create(0, handle))
}

fn setup_ok_test(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::path::PathBuf::from(TEST_TEMP_OK_DIR_PATH).join(name);
    fs::create_dir_all(&dir).unwrap();
    let pack = dir.join("pack");
    TestTool::remove_test_pack_files(&pack);
    (dir, pack)
}

fn setup_err_test(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::path::PathBuf::from(TEST_TEMP_ERR_DIR_PATH).join(name);
    fs::create_dir_all(&dir).unwrap();
    let pack = dir.join("pack");
    TestTool::remove_test_pack_files(&pack);
    (dir, pack)
}

// === 创建并重新打开（内部压力测试）/ Create and reopen (internal stress test) ===

#[test]
fn create_stress_and_reopen() {
    let (dir, pack_file) = setup_ok_test("create_stress_and_reopen");
    let test_path = "/Test/Test2/Test3";
    let test_data = vec![51, 31, 55, 6, 7, 8, 3, 67, 93];

    let (initial_root, random_names) = {
        let (manager, pack_io) = create_manager_default(&pack_file).unwrap();
        let man = Arc::new(Mutex::new(manager));

        let mut random_names = Vec::new();
        for i in 0..1000 {
            let name = rand::random_range(0..100_000_000).to_string();
            let len = rand::random_range(0..1_000_100);
            let modified = rand::random_range(0..100_000_000_000);
            random_names.push(name.clone());

            let (path_list, metadata) = man
                .lock()
                .unwrap()
                .create_file_raw(&name, modified, len, false, DEFAULT_HASH_TYPE)
                .unwrap_or_else(|err| panic!("无法创建虚拟文件: {name}, err: {err}"));
            let handle = Arc::new(
                Mutex::new(
                    PackFileHandle::create(
                        true,
                        man.clone(),
                        &pack_io,
                        path_list,
                        metadata,
                        false
                    ).unwrap()
                )
            );
            let mut wr = PackFileWR::create(0, handle);
            wr.write_all(&test_data).unwrap_or_else(|_|
                panic!("循环第{i}次，无法写入虚拟随机文件:{name}")
            );
        }

        let (path_list, metadata) = man
            .lock()
            .unwrap()
            .create_file(test_path, 0, test_data.len() as u64)
            .expect("无法创建虚拟文件");

        let handle = Arc::new(
            Mutex::new(
                PackFileHandle::create(
                    true,
                    man.clone(),
                    &pack_io,
                    path_list,
                    metadata,
                    false
                ).unwrap()
            )
        );
        let mut wr = PackFileWR::create(0, handle);
        _ = wr.write(&test_data).expect("无法写入虚拟文件");
        drop(wr);

        let man_guard = man.lock().unwrap();
        let root = man_guard.manifest.root_struct().clone();
        (root, random_names)
    };

    // 重新打开，验证数据一致性
    {
        let (manager, pack_io) = open_manager(&pack_file);
        let man = Arc::new(Mutex::new(manager));

        let mut rw = open_file_rw(&test_path, &man, &pack_io, false).unwrap();
        let mut read_buf = vec![0; test_data.len()];
        let bytes_read = rw.read(&mut read_buf).expect("无法读取虚拟文件");
        assert_eq!(bytes_read, test_data.len());
        assert_eq!(test_data, read_buf);
        drop(rw);

        man.lock().unwrap().load_all_data(false).expect("无法加载所有元数据");

        // 逐个比对随机文件的结构项
        let man_guard = man.lock().unwrap();
        let reopened = man_guard.manifest.root_struct();
        for name in &random_names {
            let a = initial_root
                .items()
                .get(name)
                .unwrap_or_else(|| panic!("初始结构缺少项: name={name}"));
            let b = reopened
                .items()
                .get(name)
                .unwrap_or_else(|| panic!("重开后结构缺少项: name={name}"));
            assert_eq!(a, b);
        }
    }

    TestTool::remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(&dir);
}

// === 版本兼容性 / Version compatibility ===

#[test]
fn open_pack_compatible_version() {
    let (dir, pack_file) = setup_ok_test("open_pack_compatible_version");
    {
        let mut manager = create_manager_default(&pack_file).unwrap().0;
        manager.manifest.attribute_mut().set_version(super::super::MANIFEST_VERSION + 1);
        manager.manifest
            .attribute_mut()
            .set_version_compatible(super::super::MANIFEST_VERSION_COMPATIBLE - 1);
    }
    {
        open_manager(&pack_file);
    }
    TestTool::remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(&dir);
}

#[test]
#[should_panic(expected = "版本过高")]
fn open_pack_version_too_high_should_panic() {
    let (dir, pack_file) = setup_err_test("open_pack_version_too_high_should_panic");
    {
        let mut manager = create_manager_default(&pack_file).unwrap().0;
        manager.manifest.attribute_mut().set_version(super::super::MANIFEST_VERSION + 1);
        manager.manifest.attribute_mut().set_version_compatible(super::super::MANIFEST_VERSION + 1);
    }
    {
        open_manager(&pack_file);
    }
    TestTool::remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(&dir);
}

#[test]
#[should_panic(expected = "版本过低")]
fn open_pack_version_too_low_should_panic() {
    let (dir, pack_file) = setup_err_test("open_pack_version_too_low_should_panic");
    {
        let mut manager = create_manager_default(&pack_file).unwrap().0;
        manager.manifest.attribute_mut().set_version(super::super::MANIFEST_VERSION_COMPATIBLE - 1);
        manager.manifest
            .attribute_mut()
            .set_version_compatible(super::super::MANIFEST_VERSION_COMPATIBLE - 1);
    }
    {
        open_manager(&pack_file);
    }
    TestTool::remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(&dir);
}
