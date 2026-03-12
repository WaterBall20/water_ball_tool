/*
创建时间：2026/02/24 08:45
*/
use crate::command::{ff, wbfp};
use std::fs;

//TEST===
static FF_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/ff/ok";
static FF_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/ff/err";
static WBFP_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/ok";
static WBFP_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/err";

//OK===
//文件查找器输出文件跳过符号链接
#[test]
fn ff_out_file_skip_symlink() {
    let out_dir_path = FF_TEST_TEMP_OK_DIR_PATH;
    _ = fs::create_dir_all(out_dir_path);
    let mut out_file_path = out_dir_path.to_string();
    out_file_path.push_str("/test_ff_skip_symlink.json");
    _ = fs::remove_file(&out_file_path);
    //命令行参数处理
    let args: Vec<String> = vec![String::from("."), out_file_path.clone(), String::from("-s")];
    ff(args.as_slice(), None);
    _ = fs::remove_file(&out_file_path);
}
//文件查找器输出文件
#[test]
fn ff_out_file() {
    let out_dir_path = FF_TEST_TEMP_OK_DIR_PATH;
    _ = fs::create_dir_all(out_dir_path);
    let mut out_file_path = out_dir_path.to_string();
    out_file_path.push_str("/test_ff.json");
    _ = fs::remove_file(&out_file_path);
    //命令行参数处理
    let args: Vec<String> = vec![String::from("."), out_file_path.clone()];
    ff(args.as_slice(), None);
    _ = fs::remove_file(&out_file_path);
}
//文件查找器输出文件,长时间
#[test]
#[ignore = "longtime"]
fn ff_out_file_longtime() {
    let out_dir_path = FF_TEST_TEMP_OK_DIR_PATH;
    _ = fs::create_dir_all(out_dir_path);
    let mut out_file_path = out_dir_path.to_string();
    out_file_path.push_str("/test_ff_long_time.json");
    _ = fs::remove_file(&out_file_path);
    //命令行参数处理
    #[cfg(not(target_os = "windows"))]
    let args: Vec<String> = vec![String::from("/home"), out_file_path.clone()];
    #[cfg(target_os = "windows")]
    let args: Vec<String> = vec![String::from("c:/users"), out_file_path.clone()];
    ff(args.as_slice(), None);
    _ = fs::remove_file(&out_file_path);
}
//文件查找器不输出文件
#[test]
fn ff_no_out_file() {
    //命令行参数处理
    let args: Vec<String> = vec![String::from(".")];
    ff(args.as_slice(), None);
}

//水球包文件打包===
#[test]
fn wbfp_create_new_pack_m() {
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/create_new_pack_m");
    _ = fs::remove_dir_all(&out_dir_path);
    _ = fs::create_dir_all(&out_dir_path);
    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    //命令行参数处理
    let args: Vec<String> = vec![
        String::from("-m"),
        String::from("./src"),
        out_file_path.clone(),
    ];
    wbfp(args.as_slice(), None);
    _ = fs::remove_dir_all(&out_file_path);
}
// 长时间
#[test]
#[ignore = "长时间"]
fn wbfp_create_new_pack_m_longtime() {
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/create_new_pack_m");
    _ = fs::remove_dir_all(&out_dir_path);
    _ = fs::create_dir_all(&out_dir_path);
    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    //命令行参数处理
    let args: Vec<String> = vec![
        String::from("-m"),
        // String::from("/home/waterball/Documents/Dev/JavaRust/MC-MMD-rust/rust_engine/src/animation/"),
        String::from("/usr/bin"),
        out_file_path.clone(),
    ];
    wbfp(args.as_slice(), None);
}

// 不分离数据
#[test]
fn wbfp_create_new_pack_m_no_s_data_file() {
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/create_new_pack_m_no_s_data_file");
    _ = fs::remove_dir_all(&out_dir_path);
    _ = fs::create_dir_all(&out_dir_path);
    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    //命令行参数处理
    let args: Vec<String> = vec![
        String::from("-m"),
        String::from("./src"),
        out_file_path.clone(),
        String::from("-f"),
    ];
    wbfp(args.as_slice(), None);
    _ = fs::remove_dir_all(&out_file_path);
}

// 水球包文件解包===
//分离
// 长时间
#[test]
#[ignore = "长时间"]
fn wbfp_create_new_pack_s_longtime() {
    let mut in_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    in_dir_path.push_str("/create_new_pack_m");
    _ = fs::create_dir_all(&in_dir_path);
    let mut in_file_path = in_dir_path.clone();
    in_file_path.push_str("/pack");
    //命令行参数处理
    let args: Vec<String> = vec![String::from("-s"), in_file_path.clone(), {
        let mut out_dir_path = in_dir_path.clone();
        out_dir_path.push_str("/s_pack");
        _ = fs::create_dir_all(&out_dir_path);
        out_dir_path
    }];
    wbfp(args.as_slice(), None);
    _ = fs::remove_dir_all(&in_file_path);
}
// 不分离数据打包和解包
#[test]
fn wbfp_create_new_pack_m_no_s_data_file_s() {
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/create_new_pack_m_no_s_data_file_s");
    _ = fs::remove_dir_all(&out_dir_path);
    _ = fs::create_dir_all(&out_dir_path);
    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    //前提：打包
    {
        //命令行参数处理
        let args: Vec<String> = vec![
            String::from("-m"),
            String::from("./src"),
            out_file_path.clone(),
            String::from("-f"),
        ];
        wbfp(args.as_slice(), None);
    }
    //解包
    {
        out_dir_path.push_str("/s");
        //命令行参数处理
        let args: Vec<String> = vec![
            String::from("-s"),
            out_file_path.clone(),
            out_dir_path.clone(),
        ];
        wbfp(args.as_slice(), None);
    }
    _ = fs::remove_dir_all(&out_file_path);
}

//ERR===
//文件查找器输出文件跳过符号链接，但文件不存在
#[test]
#[should_panic(expected = "NotFound")]
fn ff_out_file_skip_symlink_err_not_found_dir() {
    let out_dir_path = FF_TEST_TEMP_ERR_DIR_PATH;
    _ = fs::create_dir_all(out_dir_path);
    let mut out_file_path = out_dir_path.to_string();
    out_file_path.push_str("/out_file_skip_symlink_err_not_found_dir.json");
    _ = fs::remove_file(&out_file_path);
    //命令行参数处理
    let args: Vec<String> = vec![
        String::from("/～"),
        out_file_path.clone(),
        String::from("-s"),
    ];
    ff(args.as_slice(), None);
    _ = fs::remove_file(&out_file_path);
}
//水球包文件打包，但输入路径不存在
#[test]
#[should_panic(expected = "NotFound")]
fn wbfp_create_new_pack_m_err_not_found_in_dir() {
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_ERR_DIR_PATH);
    out_dir_path.push_str("/create_new_pack_m_err_not_found_in_dir");
    _ = fs::remove_dir_all(&out_dir_path);
    _ = fs::create_dir_all(&out_dir_path);
    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    //命令行参数处理
    let args: Vec<String> = vec![
        String::from("-m"),
        String::from("/~"),
        out_file_path.clone(),
    ];
    wbfp(args.as_slice(), None);
    _ = fs::remove_dir_all(&out_file_path);
}
