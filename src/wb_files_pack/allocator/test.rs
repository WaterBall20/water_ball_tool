use crate::wb_files_pack::allocator::Allocator;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::fs;

//TEST===
static WBFP_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/allocator/ok";
static _WBFP_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/allocator/err";
//哈希校验===
// 长时间
#[test]
#[ignore = "长时间"]
fn wbfp_verify_all_file_hash_longtime() {
    let mut in_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    in_dir_path.push_str("/create_new_pack_m");
    let mut in_file_path = in_dir_path.clone();
    in_file_path.push_str("/pack");
    let mut allocator =
        Allocator::open_pack_file(&in_file_path).expect("测试打开包文件失败");
    let hash_e = allocator.verify_all_file_hash();
    println!("{hash_e:#?}");
}

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
//OK===

//创建文件
#[test]
fn create_new_pack_file() {
    //测试目录
    let mut pack_dir = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).expect("创建测试目录失败");
    let pack_path = pack_dir.join("pack");
    remove_test_pack_files(&pack_path);
    //创建文件
    {
        Allocator::create_new_pack_file2(&pack_path).unwrap();
        println!("已创建文件");
    }
    remove_test_pack_files(&pack_path);
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
    remove_test_pack_files(&pack_path);
    //创建文件
    {
        let mut pack = Allocator::create_new_pack_file2(&pack_path).unwrap();
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
    let mut pack_dir = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_pack_file_and_create_file_wr");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).expect("创建测试目录失败");
    //测试文件
    let pack_file = pack_dir.join("pack");
    remove_test_pack_files(&pack_file);
    //开始创建
    {
        let mut pack = Allocator::create_new_pack_file2(&pack_file).unwrap();
        let modified_time = 0;
        //file1
        let write_data1: [u8; LENGTH] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let test_file_path = "Test/Test1";
        let mut rw2 = pack
            .create_file2(test_file_path, modified_time, LENGTH as u64)
            .unwrap_or_else(|e| panic!(r#"创建虚拟文件"{test_file_path}"失败, err:{e}"#));
        _ = rw2.write(&write_data1[..]).expect("写入虚拟文件失败");
        let mut read_data1: [u8; LENGTH] = [0; 10];
        rw2.seek(SeekFrom::Start(0)).expect("写入虚拟文件失败");
        _ = rw2.read(&mut read_data1[..]).expect("读取虚拟文件失败");
        //file2
        let write_data2: [u8; LENGTH] = [10, 25, 33, 41, 53, 64, 57, 87, 89, 110];
        let test_file_path = "Test/Test2";
        let mut rw2 = pack
            .create_file2(test_file_path, modified_time, LENGTH as u64)
            .unwrap_or_else(|e| panic!(r#"创建虚拟文件"{test_file_path}"失败, e" {e}"#));
        _ = rw2.write(&write_data2[..]).unwrap();
        let mut read_data2: [u8; LENGTH] = [0; 10];
        rw2.seek(SeekFrom::Start(0)).unwrap();
        _ = rw2.read(&mut read_data2[..]).unwrap();
        pretty_assertions::assert_eq!(write_data2, read_data2);
        pretty_assertions::assert_eq!(write_data1, read_data1);
    } //使用作用域实现自动释放
    remove_test_pack_files(&pack_file);
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
    remove_test_pack_files(&pack_file);
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
        let mut  rw2 = pack
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
    remove_test_pack_files(&pack_file);
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
    remove_test_pack_files(&pack_file);
    //
    Allocator::create_new_pack_file2(&pack_file).unwrap();
    //当上锁时，无法创建是正确的。
    let r = Allocator::create_new_pack_file2(&pack_file);
    if let Err(err) = r {
        remove_test_pack_files(&pack_file);
        _ = fs::remove_dir_all(pack_dir);
        panic!("{}", err)
    }
}