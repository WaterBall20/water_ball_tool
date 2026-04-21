/*
创建时间：2026/02/24 08:51
*/
use crate::tools::PathTool;
use crate::wb_files_pack::manager::{
    WBFPManager, DEFAULT_COW, DEFAULT_HASH_TYPE, DEFAULT_S_MANIFEST_FILE,
};
use crate::wb_files_pack::pack_io::file::PackFileWR;
use crate::wb_files_pack::pack_io::PackIO;
use pretty_assertions::assert_eq;
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::{fs, io};

static TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/manager/ok";
static TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/manager/err";

fn remove_test_pack_files<P: AsRef<Path>>(path: &P) {
    let pack_path = path
        .as_ref()
        .to_str()
        .expect("无法将路径转换成String")
        .to_string();
    _ = fs::remove_file(&pack_path);
    let mut pack_json_path = pack_path.clone();
    pack_json_path.push_str(".wbm");
    _ = fs::remove_file(pack_json_path);
    let mut pack_lock_path = pack_path.clone();
    pack_lock_path.push_str(".lock");
    _ = fs::remove_file(pack_lock_path);
}
fn create_new_pack_file2(pack_path: &Path) -> io::Result<(WBFPManager, Arc<Mutex<PackIO>>)> {
    create_pack_file(pack_path, DEFAULT_COW, DEFAULT_S_MANIFEST_FILE, true)
}
fn create_pack_file(
    pack_path: &Path,
    cow: bool,
    s_manifest_file: bool,
    create_new: bool,
) -> io::Result<(WBFPManager, Arc<Mutex<PackIO>>)> {
    let pack_file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .create_new(true)
        .open(pack_path)
        .map_err(|e| match e.kind() {
            ErrorKind::AlreadyExists => {
                Error::new(ErrorKind::AlreadyExists, format!("文件已存在, err: {e}"))
            }
            _ => panic!("创建文件错误. err:{e}"),
        })?;
    //创建包文件数据文件
    let pack_io = PackIO::new(pack_file);
    let pack_io = Arc::new(Mutex::new(pack_io));
    let mut manager = WBFPManager::create_pack_file(
        &pack_path,
        pack_io.clone(),
        cow,
        s_manifest_file,
        create_new,
    )
    .expect("无法创建包管理器");
    manager.init_new_pack().expect("初始化新包文件错误");
    Ok((manager, pack_io))
}
fn open_pack_file<P: AsRef<Path>>(pack_path: &P) -> (WBFPManager, Arc<Mutex<PackIO>>) {
    //打开水球包文件
    let pack_file = File::options()
        .read(true)
        .write(true)
        .open(pack_path)
        .expect("无法打开文件");
    let pack_io = PackIO::new(pack_file);
    let pack_io = Arc::new(Mutex::new(pack_io));
    (
        WBFPManager::open_pack_file(pack_path, pack_io.clone()).expect("无法创建包文件"),
        pack_io,
    )
}
fn get_file_rw<P: AsRef<Path>>(
    pack_path: &P,
    pack: &Arc<Mutex<WBFPManager>>,
    pack_io: &Arc<Mutex<PackIO>>,
) -> io::Result<PackFileWR> {
    let path_list = PathTool::path_to_string_vec(pack_path);
    let metadata = pack
        .clone()
        .lock()
        .expect("无法获得包文件锁")
        .file_metadata_lock(&path_list)
        .expect("无法获得元数据");
    PackFileWR::create(false, pack.clone(), &pack_io.clone(), path_list, metadata)
}
//OK===

//创建文件
#[test]
fn create_new_pack_file() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).expect("创建测试目录失败");
    let pack_path = pack_dir.join("pack");
    remove_test_pack_files(&pack_path);
    //创建文件
    {
        create_new_pack_file2(&pack_path).unwrap();
        println!("已创建文件");
    }
    remove_test_pack_files(&pack_path);
    _ = fs::remove_dir_all(pack_dir);
}

//创建文件并创建虚拟目录
#[test]
fn create_new_pack_file_and_create_dir() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file_and_create_dir");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).expect("创建测试目录失败");
    let pack_path = pack_dir.join("pack");
    remove_test_pack_files(&pack_path);
    //创建文件
    {
        let mut pack = create_new_pack_file2(&pack_path).unwrap().0;
        let test_pack_path = String::from("Test/Test2");
        pack.create_dir_all(&test_pack_path)
            .expect("创建虚假目录失败");
        pack.get_dir(test_pack_path).expect("获取虚拟目录失败");
        println!("已创建文件");
    }
    remove_test_pack_files(&pack_path);
    _ = fs::remove_dir_all(pack_dir);
}

//创建包文件同时创建虚拟文件并测试读写

#[test]
fn create_new_pack_file_and_create_file_wr() {
    const LENGTH: usize = 10;
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_pack_file_and_create_file_wr");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).expect("创建测试目录失败");
    //测试文件
    let pack_file = pack_dir.join("pack");
    remove_test_pack_files(&pack_file);
    //开始创建
    {
        let (pack, pack_io) = create_new_pack_file2(&pack_file).unwrap();
        let man = Arc::new(Mutex::new(pack));
        let modified_time = 0;
        //file1
        let write_data1: [u8; LENGTH] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let test_file_path = "Test/Test1";
        let rw1 = man
            .clone()
            .lock()
            .expect("获得包管理器失败")
            .create_file2(test_file_path, modified_time, LENGTH as u64)
            .unwrap_or_else(|e| panic!(r#"创建虚拟文件"{test_file_path}"失败, err:{e}"#));
        let mut rw1 = PackFileWR::create(true, man.clone(), &pack_io.clone(), rw1.0, rw1.1)
            .expect("无法获得虚拟文件实例");
        _ = rw1.write(&write_data1[..]).expect("写入虚拟文件失败");
        let mut read_data1: [u8; LENGTH] = [0; 10];
        rw1.seek(SeekFrom::Start(0)).expect("写入虚拟文件失败");
        _ = rw1.read(&mut read_data1[..]).expect("读取虚拟文件失败");
        //file2
        let write_data2: [u8; LENGTH] = [10, 25, 33, 41, 53, 64, 57, 87, 89, 110];
        let test_file_path = "Test/Test2";
        let rw2 = man
            .clone()
            .lock()
            .expect("无法获得包管理器")
            .create_file2(test_file_path, modified_time, LENGTH as u64)
            .unwrap_or_else(|e| panic!(r#"创建虚拟文件"{test_file_path}"失败, e" {e}"#));
        let mut rw2 = PackFileWR::create(true, man.clone(), &pack_io.clone(), rw2.0, rw2.1)
            .expect("写入虚拟文件失败");
        _ = rw2.write(&write_data2[..]).unwrap();
        let mut read_data2: [u8; LENGTH] = [0; 10];
        rw2.seek(SeekFrom::Start(0)).unwrap();
        _ = rw2.read(&mut read_data2[..]).unwrap();
        assert_eq!(write_data2, read_data2);
        assert_eq!(write_data1, read_data1);
    } //使用作用域实现自动释放
    remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

//创建包文化并写入虚拟文件，不分离数据文件
#[test]
fn create_new_pack_file_no_s_data_file_and_create_file_wr() {
    const LENGTH: usize = 10;
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_pack_file_no_s_data_file_and_create_file_wr");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    //测试文件
    let pack_file = pack_dir.join("pack");
    remove_test_pack_files(&pack_file);
    //开始创建
    {
        let (pack, pack_io) = create_pack_file(&pack_file, false, false, true).unwrap();
        let man = Arc::new(Mutex::new(pack));
        let modified_time = 0;
        //file1
        //w
        let write_data1: [u8; LENGTH] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let rw1 = man
            .clone()
            .lock()
            .unwrap()
            .create_file2("Test/Test1", modified_time, LENGTH as u64)
            .unwrap();
        let mut rw1 = PackFileWR::create(true, man.clone(), &pack_io.clone(), rw1.0, rw1.1)
            .unwrap();
        _ = rw1.write(&write_data1[..]).unwrap();
        //r
        let mut read_data1: [u8; LENGTH] = [0; 10];
        rw1.seek(SeekFrom::Start(0)).unwrap();
        _ = rw1.read(&mut read_data1[..]).unwrap();
        drop(rw1);
        //file2
        //w
        let write_data2: [u8; LENGTH] = [10, 25, 33, 41, 53, 64, 57, 87, 89, 110];
        let rw2 = man
            .clone()
            .lock()
            .unwrap()
            .create_file2("Test/Test2", modified_time, LENGTH as u64)
            .unwrap();
        let mut rw2 = PackFileWR::create(true, man.clone(), &pack_io.clone(), rw2.0, rw2.1)
            .unwrap();
        _ = rw2.write(&write_data2[..]).unwrap();
        //r
        let mut read_data2: [u8; LENGTH] = [0; 10];
        rw2.seek(SeekFrom::Start(0)).unwrap();
        _ = rw2.read(&mut read_data2[..]).unwrap();
        assert_eq!(write_data2, read_data2);
        assert_eq!(write_data1, read_data1);
    } //使用作用域实现自动释放
    remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

//创建包文件并打开刚创建的包文件
#[test]
fn create_new_file_and_open_pack() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file_and_open_pack");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    remove_test_pack_files(&pack_file);
    let test_file_path = String::from("/Test/Test2/Test3");
    let test_data = vec![51, 31, 55, 6, 7, 8, 3, 67, 93];
    //
    let (root_struct, other_name_list) = {
        //创建文件
        let (pack, pack_io) = create_new_pack_file2(&pack_file).unwrap();
        let man = Arc::new(Mutex::new(pack));
        //随机创建文件
        let mut other_name_list = Vec::new();
        for index in 0..1000 {
            let name = rand::random_range(0..100_000_000).to_string();
            let len = rand::random_range(0..1_000_100);
            let modified = rand::random_range(0..100_000_000_000);
            other_name_list.push(name.clone());
            let wr = man
                .clone()
                .lock()
                .unwrap()
                .create_file(&name, modified, len, false, DEFAULT_HASH_TYPE)
                .unwrap_or_else(|err| panic!("无法创建虚拟文件: {name}, err: {err}"));
            let mut wr = PackFileWR::create(true, man.clone(), &pack_io.clone(), wr.0, wr.1)
                .unwrap();
            wr.write_all(&test_data)
                .unwrap_or_else(|_| panic!("循环第{index}次，无法写入虚拟随机文件:{name}"));
        }
        let rw = man
            .clone()
            .lock()
            .unwrap()
            .create_file2(&test_file_path, 0, test_data.len() as u64)
            .expect("无法创建虚拟文件");
        let mut rw = PackFileWR::create(true, man.clone(), &pack_io.clone(), rw.0, rw.1).unwrap();
        _ = rw.write(&test_data).expect("无法写入虚拟文件");
        drop(rw);
        (
            man.clone().lock().unwrap().manifest.root_struct.clone(),
            other_name_list,
        )
    };
    //打开已创建并关闭的文件
    {
        let (pack, pack_io) = open_pack_file(&pack_file);
        let pack = Arc::new(Mutex::new(pack));
        let mut rw = get_file_rw(&test_file_path, &pack.clone(), &pack_io).unwrap();
        let mut test_data_read = vec![0; test_data.len()];
        let len = rw.read(&mut test_data_read).expect("无法读取虚拟文件");
        drop(rw);
        pack.clone()
            .lock()
            .unwrap()
            .load_all_data(false)
            .expect("无法加载所有元数据");
        assert_eq!(len, test_data.len());
        assert_eq!(test_data, test_data_read);
        //细分判断
        for name in &other_name_list {
            let pack = pack.clone();
            let pack = pack.lock().unwrap();
            let a_item = root_struct
                .items
                .get(name)
                .unwrap_or_else(|| panic!("获取列表项失败: name={name}"));
            let b_item = pack.manifest.root_struct.items.get(name).unwrap();
            assert_eq!(a_item, b_item);
        }
    }
    remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

#[test]
fn create_new_file_and_open_pack_manifest_ver() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file_and_open_pack_manifest_ver");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    remove_test_pack_files(&pack_file);
    //
    {
        //创建文件
        let mut pack = create_new_pack_file2(&pack_file).unwrap().0;
        //更改实例内部的数据版本
        pack.manifest.attribute.version = super::super::MANIFEST_VERSION + 1;
        pack.manifest.attribute.version_compatible = super::super::MANIFEST_VERSION_COMPATIBLE - 1;
    }
    //打开已创建并关闭的文件
    {
        open_pack_file(&pack_file);
    }
    remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

//ERR===
//创建文件_应失败
#[test]
#[should_panic(expected = "文件已存在")]
fn create_new_pack_file_err() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_ERR_DIR_PATH);
    pack_dir.push_str("/create_new_file");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    remove_test_pack_files(&pack_file);
    //
    create_new_pack_file2(&pack_file).unwrap();
    //当上锁时，无法创建是正确的。
    let r = create_new_pack_file2(&pack_file);
    if let Err(err) = r {
        remove_test_pack_files(&pack_file);
        _ = fs::remove_dir_all(pack_dir);
        panic!("{}", err)
    }
}
#[test]
#[should_panic(expected = "版本过高")]
fn create_new_file_and_open_pack_err_manifest_ver1() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_ERR_DIR_PATH);
    pack_dir.push_str("/create_new_file_and_open_pack_err_json_ver1");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    remove_test_pack_files(&pack_file);
    //
    {
        //创建文件
        let mut pack = create_new_pack_file2(&pack_file).unwrap().0;
        //更改实例内部的数据版本
        pack.manifest.attribute.version = super::super::MANIFEST_VERSION + 1;
        pack.manifest.attribute.version_compatible = super::super::MANIFEST_VERSION + 1;
    }
    //打开已创建并关闭的文件
    {
        open_pack_file(&pack_file);
    }
    remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

#[test]
#[should_panic(expected = "版本过低")]
fn create_new_file_and_open_pack_err_manifest_ver2() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_ERR_DIR_PATH);
    pack_dir.push_str("/create_new_file_and_open_pack_err_json_ver2");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    remove_test_pack_files(&pack_file);
    //
    {
        //创建文件
        let mut pack = create_new_pack_file2(&pack_file).unwrap().0;
        //更改实例内部的数据版本
        pack.manifest.attribute.version = super::super::MANIFEST_VERSION_COMPATIBLE - 1;
        pack.manifest.attribute.version_compatible = super::super::MANIFEST_VERSION_COMPATIBLE - 1;
    }
    //打开已创建并关闭的文件
    {
        open_pack_file(&pack_file);
    }
    remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}
