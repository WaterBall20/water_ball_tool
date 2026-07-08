/*
创建时间：2026/02/24 08:45
*/
use indicatif::MultiProgress;
use std::fs;

//TEST===
static FF_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/ff/ok";
static FF_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/ff/err";
static WBFP_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/command/ok";
static WBFP_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/command/err";

//OK===
//文件查找器输出文件跳过符号链接
#[test]
fn ff_out_file_skip_symlink() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let out_dir_path = FF_TEST_TEMP_OK_DIR_PATH;
    _ = fs::create_dir_all(out_dir_path);
    let mut out_file_path = out_dir_path.to_string();
    out_file_path.push_str("/test_ff_skip_symlink.json");
    _ = fs::remove_file(&out_file_path);
    //命令行参数处理
    ff(
        FileFinderArgs::new(String::from("."), Some(out_file_path.clone()), true),
        Some(&mp),
    );
    _ = fs::remove_file(&out_file_path);
}
//文件查找器输出文件
#[test]
fn ff_out_file() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let out_dir_path = FF_TEST_TEMP_OK_DIR_PATH;
    _ = fs::create_dir_all(out_dir_path);
    let mut out_file_path = out_dir_path.to_string();
    out_file_path.push_str("/test_ff.json");
    _ = fs::remove_file(&out_file_path);
    //命令行参数处理
    ff(
        FileFinderArgs::new(String::from("."), Some(out_file_path.clone()), false),
        Some(&mp),
    );
    _ = fs::remove_file(&out_file_path);
}
//文件查找器输出文件,长时间
#[test]
#[ignore = "longtime"]
fn ff_out_file_longtime() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let out_dir_path = FF_TEST_TEMP_OK_DIR_PATH;
    _ = fs::create_dir_all(out_dir_path);
    let mut out_file_path = out_dir_path.to_string();
    out_file_path.push_str("/test_ff_long_time.json");
    _ = fs::remove_file(&out_file_path);
    //命令行参数处理
    #[cfg(not(target_os = "windows"))]
    let path = String::from("/home");
    #[cfg(target_os = "windows")]
    let pString::from("C:/");
    ff(
        FileFinderArgs::new(path, Some(out_file_path.clone()), false),
        Some(&mp),
    );
}
//文件查找器不输出文件
#[test]
fn ff_no_out_file() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    //命令行参数处理
    ff(FileFinderArgs::new(".".to_string(), None, false), Some(&mp));
}

//水球包文件打包===
#[test]
fn wbfp_create_new_pack_m() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/create_new_pack_m");
    _ = fs::remove_dir_all(&out_dir_path);
    _ = fs::create_dir_all(&out_dir_path);
    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    //命令行参数处理
    wbfp(
        WaterBallFilePackArgs::new(WaterBallFilePackCommands::Pack(
            WaterBallFilePackCommandsPack::new(
                "./src".to_string(),
                Some(out_file_path.clone()),
                false,
            ),
        )),
        Some(&mp),
    );
    _ = fs::remove_dir_all(&out_file_path);
}
// 长时间
#[test]
#[ignore = "longtime"]
fn wbfp_create_new_pack_m_longtime() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/create_new_pack_m_longtime");
    _ = fs::remove_dir_all(&out_dir_path);
    _ = fs::create_dir_all(&out_dir_path);
    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    //命令行参数处理
    #[cfg(target_os = "windows")]
    let path = String::from("C:\\Program Files");
    #[cfg(not(target_os = "windows"))]
    let path = String::from("/usr/lib/");
    wbfp(
        WaterBallFilePackArgs::new(WaterBallFilePackCommands::Pack(
            WaterBallFilePackCommandsPack::new(path, Some(out_file_path.clone()), false),
        )),
        Some(&mp),
    );
}

// 不分离数据
#[test]
fn wbfp_create_new_pack_m_no_s_data_file() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/create_new_pack_m_no_s_data_file");
    _ = fs::remove_dir_all(&out_dir_path);
    _ = fs::create_dir_all(&out_dir_path);
    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    //命令行参数处理
    wbfp(
        WaterBallFilePackArgs::new(WaterBallFilePackCommands::Pack(
            WaterBallFilePackCommandsPack::new(
                "./src".to_string(),
                Some(out_file_path.clone()),
                true,
            ),
        )),
        Some(&mp),
    );
    _ = fs::remove_dir_all(&out_file_path);
}

// 水球包文件解包===
//分离
// 长时间
#[test]
#[ignore = "longtime"]
fn wbfp_pack_s_longtime() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut in_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    in_dir_path.push_str("/create_new_pack_m_longtime");
    _ = fs::create_dir_all(&in_dir_path);
    let mut in_file_path = in_dir_path.clone();
    in_file_path.push_str("/pack");
    wbfp(
        WaterBallFilePackArgs::new(WaterBallFilePackCommands::Unpack(
            WaterBallFilePackCommandsUnpack::new(
                in_file_path.clone(),
                {
                    let mut out_dir_path = in_dir_path.clone();
                    out_dir_path.push_str("/s_pack");
                    _ = fs::create_dir_all(&out_dir_path);
                    out_dir_path
                },
                false,
            ),
        )),
        Some(&mp),
    );
    _ = fs::remove_dir_all(&in_file_path);
}
// 不分离数据打包和解包和哈希校验
#[test]
fn wbfp_create_new_pack_m_no_s_data_file_s() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/create_new_pack_m_no_s_data_file_s");
    _ = fs::remove_dir_all(&out_dir_path);
    _ = fs::create_dir_all(&out_dir_path);
    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    //前提：打包
    {
        //命令行参数处理
        wbfp(
            WaterBallFilePackArgs::new(WaterBallFilePackCommands::Pack(
                WaterBallFilePackCommandsPack::new(
                    "./src".to_string(),
                    Some(out_file_path.clone()),
                    true,
                ),
            )),
            Some(&mp),
        );
    }
    //哈希校验（仅需包路径）
    {
        wbfp(
            WaterBallFilePackArgs::new(WaterBallFilePackCommands::HashVerify(
                WaterBallFilePackCommandsHashVerify::new(out_file_path.clone()),
            )),
            Some(&mp),
        );
    }
    //解包
    {
        let mut s_out_path = out_dir_path.clone();
        s_out_path.push_str("/s");
        wbfp(
            WaterBallFilePackArgs::new(WaterBallFilePackCommands::Unpack(
                WaterBallFilePackCommandsUnpack::new(out_file_path.clone(), s_out_path, false),
            )),
            Some(&mp),
        );
    }
    _ = fs::remove_dir_all(&out_file_path);
}

//哈希校验===
//分离
// 长时间
#[test]
#[ignore = "longtime"]
fn wbfp_verify_all_file_hash_longtime() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut in_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    in_dir_path.push_str("/create_new_pack_m_longtime");
    _ = fs::create_dir_all(&in_dir_path);
    let mut in_file_path = in_dir_path.clone();
    in_file_path.push_str("/pack");
    //命令行参数处理
    wbfp(
        WaterBallFilePackArgs::new(WaterBallFilePackCommands::HashVerify(
            WaterBallFilePackCommandsHashVerify::new(in_file_path.clone()),
        )),
        Some(&mp),
    );
    _ = fs::remove_dir_all(&in_file_path);
}

//ERR===
//文件查找器输出文件跳过符号链接，但文件不存在
#[test]
#[should_panic(expected = "NotFound")]
fn ff_out_file_skip_symlink_err_not_found_dir() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let out_dir_path = FF_TEST_TEMP_ERR_DIR_PATH;
    _ = fs::create_dir_all(out_dir_path);
    let mut out_file_path = out_dir_path.to_string();
    out_file_path.push_str("/out_file_skip_symlink_err_not_found_dir.json");
    _ = fs::remove_file(&out_file_path);
    //命令行参数处理
    ff(
        FileFinderArgs::new("/~".to_string(), Some(out_file_path.clone()), true),
        Some(&mp),
    );
    _ = fs::remove_file(&out_file_path);
}
//水球包文件打包，但输入路径不存在
#[test]
#[should_panic(expected = "NotFound")]
fn wbfp_create_new_pack_m_err_not_found_in_dir() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_ERR_DIR_PATH);
    out_dir_path.push_str("/create_new_pack_m_err_not_found_in_dir");
    _ = fs::remove_dir_all(&out_dir_path);
    _ = fs::create_dir_all(&out_dir_path);
    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    //命令行参数处理
    wbfp(
        WaterBallFilePackArgs::new(WaterBallFilePackCommands::Pack(
            WaterBallFilePackCommandsPack::new(
                "/~".to_string(),
                Some(out_file_path.clone()),
                false,
            ),
        )),
        Some(&mp),
    );
    _ = fs::remove_dir_all(&out_file_path);
}

// === 符号链接循环检测 / Symlink cycle detection ===

use crate::command::ff::{ff, FileFinderArgs};
use crate::command::wbfp::{
    wbfp, WaterBallFilePackArgs, WaterBallFilePackCommands,
    WaterBallFilePackCommandsHashVerify, WaterBallFilePackCommandsPack, WaterBallFilePackCommandsUnpack,
};
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

const SYMLINK_TEST_DIR: &str = "./temp/test/ff/ok/symlink_cmd";

#[cfg(unix)]
fn create_symlink_cmd(original: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

#[cfg(windows)]
fn create_symlink_cmd(original: &Path, link: &Path) -> io::Result<()> {
    let original = original
        .canonicalize()
        .unwrap_or_else(|_| original.to_path_buf());
    if original.is_dir() {
        std::os::windows::fs::symlink_dir(&original, link)
    } else {
        std::os::windows::fs::symlink_file(&original, link)
    }
}

fn try_create_symlink_cmd(original: &Path, link: &Path) -> Option<()> {
    match create_symlink_cmd(original, link) {
        Ok(()) => Some(()),
        Err(_) => None,
    }
}

fn setup_symlink_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(SYMLINK_TEST_DIR).join(name);
    _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn create_file_cmd(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let mut f = fs::File::create(&path).unwrap();
    f.write_all(b"ok").unwrap();
    path
}

/// ff 命令：搜索含符号链接的目录，符号链接应出现在 JSON 输出中
#[test]
fn ff_with_symlinks_in_output() {
    let root = setup_symlink_dir("output_json");
    create_file_cmd(&root, "real.txt");
    let link = root.join("link.txt");
    if try_create_symlink_cmd(&root.join("real.txt"), &link).is_none() {
        _ = fs::remove_dir_all(&root);
        return;
    }

    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let json_path = root.join("result.json");
    _ = fs::remove_file(&json_path);

    ff(
        FileFinderArgs::new(
            root.to_str().unwrap().to_string(),
            Some(json_path.to_str().unwrap().to_string()),
            false,
        ),
        Some(&mp),
    );

    let json_str = fs::read_to_string(&json_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let file_count = parsed["file_count"].as_u64().unwrap();
    assert!(file_count >= 1, "JSON 输出应包含文件");

    _ = fs::remove_dir_all(&root);
}

/// ff -s：跳过符号链接，输出中不应包含符号链接
#[test]
fn ff_skip_symlinks_excludes_them() {
    let root = setup_symlink_dir("skip_in_output");
    create_file_cmd(&root, "real.txt");
    let link = root.join("link.txt");
    if try_create_symlink_cmd(&root.join("real.txt"), &link).is_none() {
        _ = fs::remove_dir_all(&root);
        return;
    }

    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let json_path = root.join("result.json");
    _ = fs::remove_file(&json_path);

    ff(
        FileFinderArgs::new(
            root.to_str().unwrap().to_string(),
            Some(json_path.to_str().unwrap().to_string()),
            true,
        ),
        Some(&mp),
    );

    let json_str = fs::read_to_string(&json_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let file_count = parsed["file_count"].as_u64().unwrap();
    assert_eq!(file_count, 1, "跳过符号链接后应只有 1 个真实文件");

    _ = fs::remove_dir_all(&root);
}

/// ff：搜索含祖先符号链接循环的目录，不应崩溃
#[test]
fn ff_ancestor_symlink_does_not_crash() {
    let root = setup_symlink_dir("ancestor_crash_test");
    let sub = root.join("sub");
    fs::create_dir_all(&sub).unwrap();
    create_file_cmd(&sub, "child.txt");
    let link_back = sub.join("back_to_root");
    if try_create_symlink_cmd(&root, &link_back).is_none() {
        _ = fs::remove_dir_all(&root);
        return;
    }

    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);

    ff(
        FileFinderArgs::new(root.to_str().unwrap().to_string(), None, false),
        Some(&mp),
    );

    _ = fs::remove_dir_all(&root);
}
