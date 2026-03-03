/*
创建时间：2026/02/24 08:51
*/
use crate::wb_files_pack::manager::{ create_new_file, open_file };
use std::fs;
use std::path::Path;

static TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/ok";
static TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/err";

fn _remove_test_pack_files<P: AsRef<Path>>(path: &P) {
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
    _remove_test_pack_files(&pack_file);
    //创建文件
    {
        create_new_file(&pack_file).expect("无法创建文件");
        println!("已创建文件");
    }
    _remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

//创建包文件同时创建虚拟文件并测试读写
/*
#[test]
fn create_new_pack_file_and_create_file_wr() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_pack_file_and_create_file_wr");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    //测试文件
    let pack_file = pack_dir.join("pack");
    _remove_test_pack_files(&pack_file);
    //开始创建
    {
        let mut pack = create_new_file(&pack_file).expect("无法创建文件");
        let modified_time = 0;
        const LENGTH: usize = 10;
        //file1
        let write_data1: [u8; LENGTH] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut rw2 = pack
            .create_file_new(&"Test/Test1", modified_time, LENGTH as u64)
            .unwrap();
        rw2.write(&mut pack, &write_data1[..]).unwrap();
        let mut read_data1: [u8; LENGTH] = [0; 10];
        rw2.seek(SeekFrom::Start(0)).unwrap();
        rw2.read(&mut pack, &mut read_data1[..]).unwrap();
        //file2
        let write_data2: [u8; LENGTH] = [10, 25, 33, 41, 53, 64, 57, 87, 89, 110];
        let mut rw2 = pack
            .create_file_new(&"Test/Test2", modified_time, LENGTH as u64)
            .unwrap();
        rw2.write(&mut pack, &write_data2[..]).unwrap();
        let mut read_data2: [u8; LENGTH] = [0; 10];
        rw2.seek(SeekFrom::Start(0)).unwrap();
        rw2.read(&mut pack, &mut read_data2[..]).unwrap();
        assert_eq!(write_data2, read_data2);
        assert_eq!(write_data1, read_data1);
    } //使用作用域实现自动释放
    _remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

//创建包文化并写入虚拟文件，不分离数据文件
#[test]
fn create_new_pack_file_no_s_data_file_and_create_file_wr() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_pack_file_no_s_data_file_and_create_file_wr");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    //测试文件
    let pack_file = pack_dir.join("pack");
    _remove_test_pack_files(&pack_file);
    //开始创建
    {
        let mut pack = create_new_file2(&pack_file, false, false).expect("无法创建文件");
        let modified_time = 0;
        const LENGTH: usize = 10;
        //w
        //file1
        let write_data1: [u8; LENGTH] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut rw1 = pack
            .create_file_new(&"Test/Test1", modified_time, LENGTH as u64)
            .unwrap();
        rw1.write(&mut pack, &write_data1[..]).unwrap();
        //file2
        let write_data2: [u8; LENGTH] = [10, 25, 33, 41, 53, 64, 57, 87, 89, 110];
        let mut rw2 = pack
            .create_file_new(&"Test/Test2", modified_time, LENGTH as u64)
            .unwrap();
        rw2.write(&mut pack, &write_data2[..]).unwrap();
        //r
        //file1
        let mut read_data1: [u8; LENGTH] = [0; 10];
        rw1.seek(SeekFrom::Start(0)).unwrap();
        rw1.read(&pack, &mut read_data1[..]).unwrap();
        //file2
        let mut read_data2: [u8; LENGTH] = [0; 10];
        rw2.seek(SeekFrom::Start(0)).unwrap();
        rw2.read(&pack, &mut read_data2[..]).unwrap();
        assert_eq!(write_data2, read_data2);
        assert_eq!(write_data1, read_data1);
    } //使用作用域实现自动释放
    _remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}*/

//创建包文件并打开刚创建的包文件
#[test]
fn create_new_file_and_open_pack() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file_and_open_pack");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    _remove_test_pack_files(&pack_file);
    //
    {
        //创建文件
        create_new_file(&pack_file).expect("无法创建文件");
    }
    //打开已创建并关闭的文件
    {
        open_file(&pack_file).expect("无法打开包文件");
    }
    _remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir);
}

/*
#[test]
fn create_new_file_and_open_pack_json_ver() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file_and_open_pack_json_ver");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    _remove_test_pack_files(&pack_file);
    //
    {
        //创建文件
        let mut pack = create_new_file(&pack_file).expect("无法创建文件");
        //更改实例内部的数据版本
        pack.pack_data.attribute.data_version.value = super::DATA_VERSION + 1;
        pack.pack_data.attribute.data_version.compatible = super::DATA_VERSION_COMPATIBLE - 1;
    }
    //打开已创建并关闭的文件
    {
        open_file(&pack_file).expect("无法打开包文件");
    }
    _remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir)
}*/

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
    _remove_test_pack_files(&pack_file);
    //
    let r = {
        create_new_file(&pack_file).expect("无法创建文件");
        //当上锁时，无法创建是正确的。
        create_new_file(&pack_file)
    };
    if let Err(err) = r {
        _remove_test_pack_files(&pack_file);
        _ = fs::remove_dir_all(pack_dir);
        panic!("{}", err)
    }
}
/*#[test]
#[should_panic(expected = "Json版本过高")]
fn create_new_file_and_open_pack_err_json_ver1() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_ERR_DIR_PATH);
    pack_dir.push_str("/create_new_file_and_open_pack_err_json_ver1");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    _remove_test_pack_files(&pack_file);
    //
    {
        //创建文件
        let mut pack = create_new_file(&pack_file).expect("无法创建文件");
        //更改实例内部的数据版本
        pack.manifest.attribute.version = super::MANIFEST_VERSION + 1;
        pack.manifest.attribute.version_compatible = super::MANIFEST_VERSION + 1;
    }
    //打开已创建并关闭的文件
    {
        open_file(&pack_file).expect("无法打开包文件");
    }
    _remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir)
}

#[test]
#[should_panic(expected = "Json版本过低")]
fn create_new_file_and_open_pack_err_json_ver2() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_ERR_DIR_PATH);
    pack_dir.push_str("/create_new_file_and_open_pack_err_json_ver2");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    _remove_test_pack_files(&pack_file);
    //
    {
        //创建文件
        let mut pack = create_new_file(&pack_file).expect("无法创建文件");
        //更改实例内部的数据版本
        pack.manifest.attribute.version = super::MANIFEST_VERSION_COMPATIBLE - 1;
        pack.manifest.attribute.version_compatible = super::MANIFEST_VERSION_COMPATIBLE - 1;
    }
    //打开已创建并关闭的文件
    {
        open_file(&pack_file).expect("无法打开包文件");
    }
    _remove_test_pack_files(&pack_file);
    _ = fs::remove_dir_all(pack_dir)
}
*/
