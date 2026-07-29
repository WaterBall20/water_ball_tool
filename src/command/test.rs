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
/// ff：大规模随机文件搜索测试 / ff: large-scale random file search test
#[test]
#[ignore = "longtime"]
fn ff_out_file_longtime() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);

    let fixture = create_large_fixture();
    let fixture_str = fixture.to_str().expect("转换夹具路径失败").to_string();

    let out_dir_path = FF_TEST_TEMP_OK_DIR_PATH;
    _ = fs::create_dir_all(out_dir_path);
    let mut out_file_path = out_dir_path.to_string();
    out_file_path.push_str("/test_ff_long_time.json");
    _ = fs::remove_file(&out_file_path);

    ff(
        FileFinderArgs::new(fixture_str, Some(out_file_path.clone()), false),
        Some(&mp),
    );
    _ = fs::remove_file(&out_file_path);
    _ = fs::remove_dir_all(&fixture);
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
/// wbfp：大规模随机文件打包+校验+解包 / wbfp: large-scale random file pack+verify+unpack
#[test]
#[ignore = "longtime"]
fn wbfp_pack_verify_unpack_longtime() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let fixture = create_large_fixture();

    let out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH) + "/pack_verify_unpack_longtime";
    _ = fs::remove_dir_all(&out_dir_path);
    _ = fs::create_dir_all(&out_dir_path);
    let pack_path = out_dir_path.clone() + "/pack";

    // Step 1: 打包 / Pack
    wbfp(
        WaterBallFilePackArgs::new(WaterBallFilePackCommands::Pack(
            WaterBallFilePackCommandsPack::new(
                fixture.to_str().expect("转换夹具路径失败").to_string(),
                Some(pack_path.clone()),
                false,
                None,
                true,
            ),
        )),
        Some(&mp),
    );

    // Step 2: 哈希校验 / Hash verify
    let wbfp_path = pack_path.clone() + ".wbfp";
    wbfp(
        WaterBallFilePackArgs::new(WaterBallFilePackCommands::HashVerify(
            WaterBallFilePackCommandsHashVerify::new(wbfp_path.clone()),
        )),
        Some(&mp),
    );

    // Step 3: 解包 / Unpack
    let unpack_dir = out_dir_path.clone() + "/unpacked";
    _ = fs::create_dir_all(&unpack_dir);
    wbfp(
        WaterBallFilePackArgs::new(WaterBallFilePackCommands::Unpack(
            WaterBallFilePackCommandsUnpack::new(wbfp_path.clone(), Some(unpack_dir), false, None),
        )),
        Some(&mp),
    );

    // 清理 / Cleanup
    _ = fs::remove_dir_all(&out_dir_path);
    _ = fs::remove_dir_all(&fixture);
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
/// wbfp：打包不存在的目录应记录错误而非 panic / wbfp: pack nonexistent dir should log error, not panic
#[test]
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
    let result = wbfp(
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
    // 不 panic，而是返回 Ok（搜索错误已记录）
    assert!(result.is_ok());
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
    fs::create_dir_all(&dir).expect("创建符号链接测试目录失败");
    dir
}

/// 在指定目录下创建一个内容为 "ok" 的测试文件 / Create a test file with "ok" content
fn create_file_cmd(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let mut f = fs::File::create(&path).expect("创建测试文件失败");
    f.write_all(b"ok").expect("写入测试文件内容失败");
    path
}

/// 大型随机文件测试夹具目录 / Large random file test fixture directory
const LARGE_FIXTURE_DIR: &str = "./temp/test/large_fixture";

/// 创建 1000 个随机文件的测试夹具 / Create a test fixture with 1000 random files
///
/// 生成 25 个子目录 × 40 个文件 = 1000 个文件
/// 每个文件内容和大小均随机（50~2048 字节）
/// 用于大规模文件搜索和打包的压力测试
fn create_large_fixture() -> PathBuf {
    let dir = PathBuf::from(LARGE_FIXTURE_DIR);
    _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建大型测试夹具目录失败");

    let files_per_dir = 40;
    let dir_count = 25;

    for i in 0..dir_count {
        let sub = dir.join(format!("batch_{:02}", i));
        fs::create_dir_all(&sub).expect("创建夹具子目录失败");

        for j in 0..files_per_dir {
            let size = rand::random_range(50..=2048);
            let content: Vec<u8> = (0..size).map(|_| rand::random()).collect();
            let fpath = sub.join(format!("data_{:03}.bin", j));
            fs::write(&fpath, &content).expect("写入夹具文件失败");
        }
    }
    dir
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
            root.to_str().expect("转换根目录路径为字符串失败").to_string(),
            Some(json_path.to_str().expect("转换 JSON 文件路径为字符串失败").to_string()),
            false,
        ),
        Some(&mp),
    );

    let json_str = fs::read_to_string(&json_path).expect("读取 JSON 结果文件失败");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("解析 JSON 结果失败");
    let file_count = parsed["file_count"].as_u64().expect("获取文件计数字段失败");
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
            root.to_str().expect("转换根目录路径为字符串失败").to_string(),
            Some(json_path.to_str().expect("转换 JSON 文件路径为字符串失败").to_string()),
            true,
        ),
        Some(&mp),
    );

    let json_str = fs::read_to_string(&json_path).expect("读取 JSON 结果文件失败");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("解析 JSON 结果失败");
    let file_count = parsed["file_count"].as_u64().expect("获取文件计数字段失败");
    assert_eq!(file_count, 1, "跳过符号链接后应只有 1 个真实文件");

    _ = fs::remove_dir_all(&root);
}

/// ff：搜索含祖先符号链接循环的目录，不应崩溃
#[test]
fn ff_ancestor_symlink_does_not_crash() {
    let root = setup_symlink_dir("ancestor_crash_test");
    let sub = root.join("sub");
    fs::create_dir_all(&sub).expect("创建子目录失败");
    create_file_cmd(&sub, "child.txt");
    let link_back = sub.join("back_to_root");
    if try_create_symlink_cmd(&root, &link_back).is_none() {
        _ = fs::remove_dir_all(&root);
        return;
    }

    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);

    ff(
        FileFinderArgs::new(root.to_str().expect("转换根目录路径为字符串失败").to_string(), None, false),
        Some(&mp),
    );

    _ = fs::remove_dir_all(&root);
}
