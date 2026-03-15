/*
创建时间：2026/02/24 08:51
*/
use crate::wb_files_pack::manager::{create_new_file, create_new_file2, open_file};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

static TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/ok";
static TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/err";

fn remove_test_pack_files<P: AsRef<Path>>(path: &P) {
    let pack_path = path.as_ref().to_str().unwrap().to_string();
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
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    remove_test_pack_files(&pack_file);
    //创建文件
    {
        create_new_file(&pack_file).expect("无法创建文件");
        println!("已创建文件");
    }
    remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

//创建文件并创建虚拟目录
#[test]
fn create_new_pack_file_and_create_dir() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file_and_create_dir");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    remove_test_pack_files(&pack_file);
    //创建文件
    {
        let mut pack = create_new_file(&pack_file).expect("无法创建文件");
        let test_pack_path = "Test/Test2";
        pack.create_dir_all(test_pack_path)
            .expect("创建虚假目录失败");
        pack.get_dir(test_pack_path).expect("获取虚拟目录失败");
        println!("已创建文件");
    }
    remove_test_pack_files(&pack_file);
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
    fs::create_dir_all(pack_dir).unwrap();
    //测试文件
    let pack_file = pack_dir.join("pack");
    remove_test_pack_files(&pack_file);
    //开始创建
    {
        let mut pack = create_new_file(&pack_file).expect("无法创建文件");
        let modified_time = 0;
        //file1
        let write_data1: [u8; LENGTH] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut rw2 = pack
            .create_file_new_wr("Test/Test1", modified_time, LENGTH as u64)
            .unwrap();
        _ = rw2.write(&write_data1[..]).unwrap();
        let mut read_data1: [u8; LENGTH] = [0; 10];
        rw2.seek(SeekFrom::Start(0)).unwrap();
        _ = rw2.read(&mut read_data1[..]).unwrap();
        drop(rw2);
        //file2
        let write_data2: [u8; LENGTH] = [10, 25, 33, 41, 53, 64, 57, 87, 89, 110];
        let mut rw2 = pack
            .create_file_new_wr("Test/Test2", modified_time, LENGTH as u64)
            .unwrap();
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
        let mut pack = create_new_file2(&pack_file, false, false).expect("无法创建文件");
        let modified_time = 0;
        //file1
        //w
        let write_data1: [u8; LENGTH] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut rw1 = pack
            .create_file_new_wr("Test/Test1", modified_time, LENGTH as u64)
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
            .create_file_new_wr("Test/Test2", modified_time, LENGTH as u64)
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
    let test_file_path = "/Test/Test2/Test3";
    let test_data = vec![51, 31, 55, 6, 7, 8, 3, 67, 93];
    //
    let (attribute, root_struct, other_name_list) = {
        //创建文件
        let mut pack = create_new_file(&pack_file).expect("无法创建文件");
        //随机创建文件
        let mut other_name_list = Vec::new();
        for _ in 0..10 {
            let name = rand::random_range(0..100_000_000).to_string();
            let len = rand::random_range(0..1_000_100);
            let modified = rand::random_range(0..100_000_000_000);
            other_name_list.push(name.clone());
            pack.create_file_new2(&name, modified, len, false, false)
                .unwrap_or_else(|err| panic!("无法创建虚拟文件: {name}, err: {err}"));
        }
        let mut rw = pack
            .create_file_new_wr(test_file_path, 0, test_data.len() as u64)
            .expect("无法创建虚拟文件");
        _ = rw.write(&test_data).expect("无法写入虚拟文件");
        drop(rw);
        (
            pack.manifest.attribute.clone(),
            pack.manifest.root_struct.clone(),
            other_name_list,
        )
    };
    //打开已创建并关闭的文件
    {
        let mut pack = open_file(&pack_file).expect("无法打开包文件");
        let mut rw = pack
            .get_file_rw(test_file_path)
            .expect("无法打开虚拟文件读写器");
        let mut test_data_read = vec![0; test_data.len()];
        let len = rw.read(&mut test_data_read).expect("无法读取虚拟文件");
        drop(rw);
        pack.load_all_data(false).expect("无法加载所有元数据");
        assert_eq!(len, test_data.len());
        assert_eq!(test_data, test_data_read);
        //assert_eq!(&attribute, pack.manifest.attribute());
        //细分判断
        for name in other_name_list {
            let a_item = root_struct.items.get(&name).unwrap();
            let b_item = pack.manifest.root_struct.items.get(&name).unwrap();
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
        let mut pack = create_new_file(&pack_file).expect("无法创建文件");
        //更改实例内部的数据版本
        pack.manifest.attribute.version = super::super::MANIFEST_VERSION + 1;
        pack.manifest.attribute.version_compatible = super::super::MANIFEST_VERSION_COMPATIBLE - 1;
    }
    //打开已创建并关闭的文件
    {
        open_file(&pack_file).expect("无法打开包文件");
    }
    remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

//ERR===
//创建文件_应失败
#[test]
#[should_panic(expected = "文件可能已存在，无法创建！")]
fn create_new_pack_file_err() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_ERR_DIR_PATH);
    pack_dir.push_str("/create_new_file");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    remove_test_pack_files(&pack_file);
    //
    let r = {
        create_new_file(&pack_file).expect("无法创建文件");
        //当上锁时，无法创建是正确的。
        create_new_file(&pack_file)
    };
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
        let mut pack = create_new_file(&pack_file).expect("无法创建文件");
        //更改实例内部的数据版本
        pack.manifest.attribute.version = super::super::MANIFEST_VERSION + 1;
        pack.manifest.attribute.version_compatible = super::super::MANIFEST_VERSION + 1;
    }
    //打开已创建并关闭的文件
    {
        open_file(&pack_file).expect("无法打开包文件");
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
        let mut pack = create_new_file(&pack_file).expect("无法创建文件");
        //更改实例内部的数据版本
        pack.manifest.attribute.version = super::super::MANIFEST_VERSION_COMPATIBLE - 1;
        pack.manifest.attribute.version_compatible = super::super::MANIFEST_VERSION_COMPATIBLE - 1;
    }
    //打开已创建并关闭的文件
    {
        open_file(&pack_file).expect("无法打开包文件");
    }
    remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}
