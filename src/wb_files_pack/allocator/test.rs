use crate::{tools::TestTool, wb_files_pack::allocator::Allocator};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

//TEST===
static WBFP_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/allocator/ok";
static _WBFP_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/allocator/err";

//OK===

//哈希校验===
// 长时间
#[test]
#[ignore = "longtime"]
fn wbfp_verify_all_file_hash_longtime() {
    let mut in_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    in_dir_path.push_str("/create_new_pack_m");
    let mut in_file_path = in_dir_path.clone();
    in_file_path.push_str("/pack");
    let mut allocator = Allocator::open_pack_file(&in_file_path).expect("测试打开包文件失败");
    let hash_e = allocator.verify_all_file_hash();
    println!("{hash_e:#?}");
}

//创建文件
#[test]
fn create_new_pack_file() {
    //测试目录
    let mut pack_dir = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).expect("创建测试目录失败");
    let pack_path = pack_dir.join("pack");
    TestTool::remove_test_pack_files(&pack_path);
    //创建文件
    {
        Allocator::create_new_pack_file2(&pack_path).unwrap();
        println!("已创建文件");
    }
    TestTool::remove_test_pack_files(&pack_path);
    _ = fs::remove_dir_all(pack_dir);
}

//创建文件并创建虚拟目录
#[test]
fn create_new_pack_file_and_create_dir() {
    //测试目录
    let mut pack_dir = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file_and_create_dir");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).expect("创建测试目录失败");
    let pack_path = pack_dir.join("pack");
    TestTool::remove_test_pack_files(&pack_path);
    //创建文件
    {
        let mut pack = Allocator::create_new_pack_file2(&pack_path).unwrap();
        let test_pack_path = String::from("Test/Test2");
        pack.create_dir_all(&test_pack_path)
            .expect("创建虚假目录失败");
        pack.get_dir(test_pack_path).expect("获取虚拟目录失败");
        println!("已创建文件");
    }
    TestTool::remove_test_pack_files(&pack_path);
    _ = fs::remove_dir_all(pack_dir);
}

//创建包文件同时创建虚拟文件并测试读写

#[test]
fn create_new_pack_file_and_create_file_wr() {
    const LENGTH: usize = 10;
    //测试目录
    let mut pack_dir = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_pack_file_and_create_file_wr");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).expect("创建测试目录失败");
    //测试文件
    let pack_file = pack_dir.join("pack");
    TestTool::remove_test_pack_files(&pack_file);
    //开始创建
    {
        let mut pack = Allocator::create_new_pack_file2(&pack_file).unwrap();
        let modified_time = 0;
        //file1
        let write_data1: [u8; LENGTH] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let test_file_path1 = "Test/Test1";
        let mut rw1 = pack
            .create_file2(test_file_path1, modified_time, LENGTH as u64)
            .unwrap_or_else(|e| panic!(r#"创建虚拟文件"{test_file_path1}"失败, err:{e}"#));
        _ = rw1.write(&write_data1[..]).expect("写入虚拟文件失败");
        let mut read_data1: [u8; LENGTH] = [0; LENGTH];
        rw1.seek(SeekFrom::Start(0)).expect("设置虚拟文件位置失败");
        _ = rw1.read(&mut read_data1[..]).expect("读取虚拟文件失败");
        //file2
        let write_data2: [u8; LENGTH] = [10, 25, 33, 41, 53, 64, 57, 87, 89, 110];
        let test_file_path2 = "Test/Test2";
        let mut rw2 = pack
            .create_file2(test_file_path2, modified_time, LENGTH as u64)
            .unwrap_or_else(|e| panic!(r#"创建虚拟文件"{test_file_path2}"失败, e" {e}"#));
        _ = rw2.write(&write_data2[..]).unwrap();
        let mut read_data2: [u8; LENGTH] = [0; LENGTH];
        rw2.seek(SeekFrom::Start(0)).unwrap();
        _ = rw2.read(&mut read_data2[..]).unwrap();
        pretty_assertions::assert_eq!(write_data2, read_data2);
        pretty_assertions::assert_eq!(write_data1, read_data1);
    } //使用作用域实现自动释放
    TestTool::remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

//创建包文件同时创建虚拟文件写，并重新打开包文件测试虚拟文件修改

#[test]
fn create_new_pack_file_and_create_file_wr_and_open_pack_file_wr() {
    const LENGTH: usize = 10;
    const LENGTH2: usize = 18;
    //测试目录
    let mut pack_dir = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_pack_file_and_create_file_wr_and_open_pack_file_wr");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).expect("创建测试目录失败");
    //测试文件
    let pack_file = pack_dir.join("pack");
    TestTool::remove_test_pack_files(&pack_file);
    //开始创建
    let test_file_path1 = "Test/Test1";
    let test_file_path2 = "Test/Test2";
    {
        let write_data1: [u8; LENGTH] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let write_data2: [u8; LENGTH2] = [
            10, 25, 33, 41, 53, 64, 57, 87, 89, 110, 12, 124, 51, 164, 156, 11, 24, 15,
        ];
        let mut pack = Allocator::create_new_pack_file2(&pack_file).unwrap();
        let modified_time = 0;
        //file1
        let mut rw1 = pack
            .create_file2(test_file_path1, modified_time, LENGTH as u64)
            .unwrap_or_else(|e| panic!(r#"创建虚拟文件"{test_file_path1}"失败, err:{e}"#));
        _ = rw1.write(&write_data1[..]).expect("写入虚拟文件失败");
        rw1.seek(SeekFrom::Start(0)).expect("写入虚拟文件失败");
        //file2
        let mut rw2 = pack
            .create_file2(test_file_path2, modified_time, LENGTH as u64)
            .unwrap_or_else(|e| panic!(r#"创建虚拟文件"{test_file_path2}"失败, e" {e}"#));
        _ = rw2.write(&write_data2[..]).unwrap();
        rw2.seek(SeekFrom::Start(0)).unwrap();
    } //使用作用域实现自动释放
    {
        let write_data1: [u8; LENGTH2] = [
            101, 124, 35, 124, 73, 24, 62, 83, 61, 124, 124, 12, 55, 21, 21, 144, 56, 1,
        ];
        let write_data2: [u8; LENGTH] = [10, 25, 33, 41, 53, 62, 5, 12, 14, 1];
        let mut pack = Allocator::open_pack_file(&pack_file).unwrap();
        let mut rw1 = pack
            .open_file(test_file_path1, false)
            .unwrap_or_else(|e| panic!(r#"打开虚拟文件"{test_file_path1}"失败, err:{e}"#));
        _ = rw1.write(&write_data1[..]).unwrap();
        let mut read_data1: [u8; LENGTH2] = [0; LENGTH2];
        rw1.seek(SeekFrom::Start(0)).unwrap();
        _ = rw1.read(&mut read_data1[..]).unwrap();

        let mut rw2 = pack
            .open_file(test_file_path2, false)
            .unwrap_or_else(|e| panic!(r#"打开虚拟文件"{test_file_path2}"失败, err:{e}"#));
        _ = rw2.write(&write_data2[..]).unwrap();
        let mut read_data2: [u8; LENGTH] = [0; LENGTH];
        rw2.seek(SeekFrom::Start(0)).unwrap();
        _ = rw2.read(&mut read_data2[..]).unwrap();
        pretty_assertions::assert_eq!(write_data1, read_data1);
        pretty_assertions::assert_eq!(write_data2, read_data2);
    }
    TestTool::remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}
//创建包文化并写入虚拟文件，不分离数据文件
#[test]
fn create_new_pack_file_no_s_data_file_and_create_file_wr() {
    const LENGTH: usize = 10;
    //测试目录
    let mut pack_dir = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_pack_file_no_s_data_file_and_create_file_wr");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    //测试文件
    let pack_file = pack_dir.join("pack");
    TestTool::remove_test_pack_files(&pack_file);
    //开始创建
    {
        let mut pack = Allocator::create_pack_file(&pack_file, false, false, true).unwrap();
        let modified_time = 0;
        //file1
        //w
        let write_data1: [u8; LENGTH] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut rw1 = pack
            .create_file2("Test/Test1", modified_time, LENGTH as u64)
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
        let mut rw2 = pack
            .create_file2("Test/Test2", modified_time, LENGTH as u64)
            .unwrap();
        _ = rw2.write(&write_data2[..]).unwrap();
        //r
        let mut read_data2: [u8; LENGTH] = [0; 10];
        rw2.seek(SeekFrom::Start(0)).unwrap();
        _ = rw2.read(&mut read_data2[..]).unwrap();
        pretty_assertions::assert_eq!(write_data2, read_data2);
        pretty_assertions::assert_eq!(write_data1, read_data1);
    } //使用作用域实现自动释放
    TestTool::remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

//创建包文件（不提供大小）同时创建虚拟文件并测试读写

#[test]
fn create_new_pack_file_no_len_and_create_file_wr() {
    const LENGTH: usize = 10;
    //测试目录
    let mut pack_dir = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_pack_file_no_len_and_create_file_wr");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).expect("创建测试目录失败");
    //测试文件
    let pack_file = pack_dir.join("pack");
    TestTool::remove_test_pack_files(&pack_file);
    //开始创建
    {
        let mut pack = Allocator::create_new_pack_file2(&pack_file).unwrap();
        //file1
        let write_data1: [u8; LENGTH] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let test_file_path = "Test/Test1";
        let mut rw2 = pack
            .create_file_no_len(test_file_path)
            .unwrap_or_else(|e| panic!(r#"创建虚拟文件"{test_file_path}"失败, err:{e}"#));
        _ = rw2.write(&write_data1[..]).expect("写入虚拟文件失败");
        let mut read_data1: [u8; LENGTH] = [0; 10];
        rw2.seek(SeekFrom::Start(0)).expect("写入虚拟文件失败");
        _ = rw2.read(&mut read_data1[..]).expect("读取虚拟文件失败");
        //file2
        let write_data2: [u8; LENGTH] = [10, 25, 33, 41, 53, 64, 57, 87, 89, 110];
        let test_file_path = "Test/Test2";
        let mut rw2 = pack
            .create_file_no_len(test_file_path)
            .unwrap_or_else(|e| panic!(r#"创建虚拟文件"{test_file_path}"失败, e" {e}"#));
        _ = rw2.write(&write_data2[..]).unwrap();
        let mut read_data2: [u8; LENGTH] = [0; 10];
        rw2.seek(SeekFrom::Start(0)).unwrap();
        _ = rw2.read(&mut read_data2[..]).unwrap();
        pretty_assertions::assert_eq!(write_data2, read_data2);
        pretty_assertions::assert_eq!(write_data1, read_data1);
    } //使用作用域实现自动释放
    TestTool::remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

//创建包文化（不提供大小）并写入虚拟文件，不分离数据文件
#[test]
fn create_new_pack_file_no_len_and_no_s_data_file_and_create_file_wr() {
    const LENGTH: usize = 10;
    //测试目录
    let mut pack_dir = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_pack_file_no_len_and_no_s_data_file_and_create_file_wr");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    //测试文件
    let pack_file = pack_dir.join("pack");
    TestTool::remove_test_pack_files(&pack_file);
    //开始创建
    {
        let mut pack = Allocator::create_pack_file(&pack_file, false, false, true).unwrap();
        //file1
        //w
        let write_data1: [u8; LENGTH] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut rw1 = pack.create_file_no_len("Test/Test1").unwrap();
        _ = rw1.write(&write_data1[..]).unwrap();
        //r
        let mut read_data1: [u8; LENGTH] = [0; 10];
        rw1.seek(SeekFrom::Start(0)).unwrap();
        _ = rw1.read(&mut read_data1[..]).unwrap();
        drop(rw1);
        //file2
        //w
        let write_data2: [u8; LENGTH] = [10, 25, 33, 41, 53, 64, 57, 87, 89, 110];
        let mut rw2 = pack.create_file_no_len("Test/Test2").unwrap();
        _ = rw2.write(&write_data2[..]).unwrap();
        //r
        let mut read_data2: [u8; LENGTH] = [0; 10];
        rw2.seek(SeekFrom::Start(0)).unwrap();
        _ = rw2.read(&mut read_data2[..]).unwrap();
        pretty_assertions::assert_eq!(write_data2, read_data2);
        pretty_assertions::assert_eq!(write_data1, read_data1);
    } //使用作用域实现自动释放
    TestTool::remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

//ERR===
//创建文件_应失败
#[test]
#[should_panic(expected = "文件可能已存在")]
fn create_new_pack_file_err() {
    //测试目录
    let mut pack_dir = String::from(_WBFP_TEST_TEMP_ERR_DIR_PATH);
    pack_dir.push_str("/create_new_file");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    TestTool::remove_test_pack_files(&pack_file);
    //
    Allocator::create_new_pack_file2(&pack_file).unwrap();
    //当上锁时，无法创建是正确的。
    let r = Allocator::create_new_pack_file2(&pack_file);
    if let Err(err) = r {
        TestTool::remove_test_pack_files(&pack_file);
        _ = fs::remove_dir_all(pack_dir);
        panic!("{}", err)
    }
}
