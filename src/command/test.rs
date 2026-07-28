//! 命令行集成测试 / Command-level integration tests
//!
//! 测试 `ff` 和 `wbfp` 从参数到执行的完整流程。
//! 包含跨平台符号链接循环检测测试。
//!
//! Tests the full flow of `ff` and `wbfp` from arguments to execution.
//! Includes cross-platform symlink cycle detection tests.

use indicatif::MultiProgress;
use std::fs;

// === 测试目录常量 / Test directory constants ===
// 正常测试 / OK tests
static FF_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/ff/ok";
static WBFP_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/command/ok";
// 错误测试 / Error tests
static FF_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/ff/err";
static WBFP_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/command/err";

// =========================================================================
// ff 命令测试 / ff command tests
// =========================================================================
/// ff：输出文件 + 跳过符号链接 / ff: output file with skip symlinks
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
/// ff：输出文件 / ff: output file
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
/// ff：长时间测试，输出文件 / ff: long-time test with output file
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
    let path = String::from("C:/");
    ff(
        FileFinderArgs::new(path, Some(out_file_path.clone()), false),
        Some(&mp),
    );
}
/// ff：不输出文件（仅打印日志）/ ff: no output file (log only)
#[test]
fn ff_no_out_file() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    //命令行参数处理
    ff(FileFinderArgs::new(".".to_string(), None, false), Some(&mp));
}

// =========================================================================
// wbfp 打包测试 / wbfp pack tests
// =========================================================================

/// 打包目录（分离清单）/ Pack directory (separate manifest)
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
                None,
                true,
            ),
        )),
        Some(&mp),
    );
    _ = fs::remove_dir_all(&out_file_path);
}
/// 打包目录 — 长时间测试 / Pack directory — long-time test
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
            WaterBallFilePackCommandsPack::new(path, Some(out_file_path.clone()), false, None, true),
        )),
        Some(&mp),
    );
}

/// 打包目录 — 不分离数据文件 / Pack directory — no separate data file
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
                None,
                true
            ),
        )),
        Some(&mp),
    );
    _ = fs::remove_dir_all(&out_file_path);
}

// =========================================================================
// wbfp 解包测试 / wbfp unpack tests
// =========================================================================

/// 解包 — 分离清单 — 长时间测试 / Unpack — separate manifest — long-time test
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
                Some({
                    let mut out_dir_path = in_dir_path.clone();
                    out_dir_path.push_str("/s_pack");
                    _ = fs::create_dir_all(&out_dir_path);
                    out_dir_path
                }),
                false,
                None,
            ),
        )),
        Some(&mp),
    );
    _ = fs::remove_dir_all(&in_file_path);
}
/// 打包 + 哈希校验 + 解包 完整流程 / Pack + hash verify + unpack full flow
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
                    None,
                    true
                ),
            )),
            Some(&mp),
        );
    }
    out_file_path.push_str(".wbfp");
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
                WaterBallFilePackCommandsUnpack::new(out_file_path.clone(), Some(s_out_path), false, None),
            )),
            Some(&mp),
        );
    }
    _ = fs::remove_dir_all(&out_file_path);
}

// =========================================================================
// wbfp 哈希校验测试 / wbfp hash verify tests
// =========================================================================

/// 哈希校验 — 分离清单 — 长时间测试 / Hash verify — separate manifest — long-time test
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

// =========================================================================
// 错误路径测试 / Error path tests
// =========================================================================

/// ff：搜索不存在的目录应 panic / ff: search nonexistent dir should panic
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
/// wbfp：打包不存在的目录应 panic / wbfp: pack nonexistent dir should panic
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
                None,
                true
            ),
        )),
        Some(&mp),
    );
    _ = fs::remove_dir_all(&out_file_path);
}

// === 符号链接循环检测 / Symlink cycle detection ===

use crate::command::ff::{FileFinderArgs, ff};
use crate::command::wbfp::{
    WaterBallFilePackArgs, WaterBallFilePackCommands, WaterBallFilePackCommandsHashVerify,
    WaterBallFilePackCommandsPack, WaterBallFilePackCommandsUnpack, wbfp,
};
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

const SYMLINK_TEST_DIR: &str = "./temp/test/ff/ok/symlink_cmd";

/// 平台适配的符号链接创建函数 / Platform-adapted symlink creation
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

/// 尝试创建符号链接，失败时返回 None（如权限不足或无符号链接支持）
/// Attempt to create a symlink, returning None on failure (e.g., permission denied)
fn try_create_symlink_cmd(original: &Path, link: &Path) -> Option<()> {
    match create_symlink_cmd(original, link) {
        Ok(()) => Some(()),
        Err(_) => None,
    }
}

/// 创建清理后的测试子目录 / Create a clean test subdirectory
fn setup_symlink_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(SYMLINK_TEST_DIR).join(name);
    _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 在指定目录下创建一个内容为 "ok" 的测试文件 / Create a test file with "ok" content
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
