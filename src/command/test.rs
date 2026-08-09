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
static FF_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/command/ok";
static WBFP_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/command/ok";
// 错误测试 / Error tests
static FF_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/command/err";
static WBFP_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/command/err";

// =========================================================================
// ff 命令测试 / ff command tests
// =========================================================================
/// ff：输出文件 + 跳过符号链接 / ff: output file with skip symlinks
#[test]
fn ff_out_file_skip_symlink() -> Result<(), Box<dyn std::error::Error>> {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let out_dir_path = PathBuf::from(FF_TEST_TEMP_OK_DIR_PATH).join("ff_out_file_skip_symlink");
    prepare_test_dir(Path::new(&out_dir_path));

    // ===== TEST-ONLY RESOURCE GENERATION: START =====
    let fixture = get_small_fixture("ff_out_file_skip_symlink");
    // ===== TEST-ONLY RESOURCE GENERATION: END =====
    let fixture_str = fixture.to_str().expect("转换夹具路径失败").to_string();

    let out_file_path = out_dir_path.join("out.json");
    _ = fs::remove_file(&out_file_path);
    let r = ff(
        FileFinderArgs::new(
            fixture_str,
            Some(
                out_file_path
                    .to_str()
                    .expect("转换输出路径失败")
                    .to_string(),
            ),
            true,
            None,
        ),
        Some(&mp),
    );
    r?;
    fs::remove_dir_all(&out_dir_path)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}
/// ff：输出文件 / ff: output file
#[test]
fn ff_out_file() -> Result<(), Box<dyn std::error::Error>> {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let out_dir_path = PathBuf::from(FF_TEST_TEMP_OK_DIR_PATH).join("ff_out_file");
    prepare_test_dir(Path::new(&out_dir_path));

    // ===== TEST-ONLY RESOURCE GENERATION: START =====
    let fixture = get_small_fixture("ff_out_file");
    // ===== TEST-ONLY RESOURCE GENERATION: END =====
    let fixture_str = fixture.to_str().expect("转换夹具路径失败").to_string();

    let out_file_path = out_dir_path.join("out.json");
    _ = fs::remove_file(&out_file_path);
    let r = ff(
        FileFinderArgs::new(
            fixture_str,
            Some(
                out_file_path
                    .to_str()
                    .expect("转换输出路径失败")
                    .to_string(),
            ),
            false,
            None,
        ),
        Some(&mp),
    );
    r?;
    fs::remove_dir_all(&out_dir_path)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}
/// ff：大规模随机文件搜索测试 / ff: large-scale random file search test
#[test]
#[ignore = "longtime"]
fn ff_out_file_longtime() -> Result<(), Box<dyn std::error::Error>> {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);

    let out_dir_path = PathBuf::from(FF_TEST_TEMP_OK_DIR_PATH).join("ff_out_file_longtime");
    prepare_test_dir(Path::new(&out_dir_path));

    let fixture = create_large_fixture("ff_out_file_longtime");
    let fixture_str = fixture.to_str().expect("转换夹具路径失败").to_string();

    let out_file_path = out_dir_path.join("out.json");
    _ = fs::remove_file(&out_file_path);

    let r = ff(
        FileFinderArgs::new(
            fixture_str,
            Some(
                out_file_path
                    .to_str()
                    .expect("转换输出路径失败")
                    .to_string(),
            ),
            false,
            None,
        ),
        Some(&mp),
    );
    r?;
    fs::remove_dir_all(&out_dir_path)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}

/// ff：不输出文件（仅打印日志）/ ff: no output file (log only)
#[test]
fn ff_no_out_file() -> Result<(), Box<dyn std::error::Error>> {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let no_out_dir = PathBuf::from(FF_TEST_TEMP_OK_DIR_PATH).join("ff_no_out_file");
    prepare_test_dir(&no_out_dir);

    // ===== TEST-ONLY RESOURCE GENERATION: START =====
    let fixture = get_small_fixture("ff_no_out_file");
    // ===== TEST-ONLY RESOURCE GENERATION: END =====
    let fixture_str = fixture.to_str().expect("转换夹具路径失败").to_string();
    let r = ff(
        FileFinderArgs::new(fixture_str, None, false, None),
        Some(&mp),
    );
    r?;
    fs::remove_dir_all(&no_out_dir)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}

// =========================================================================
// wbfp 打包测试 / wbfp pack tests
// =========================================================================

/// 打包目录（分离清单）/ Pack directory (separate manifest)
#[test]
fn wbfp_create_new_pack_m() -> Result<(), Box<dyn std::error::Error>> {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/wbfp_create_new_pack_m");
    prepare_test_dir(Path::new(&out_dir_path));

    // ===== TEST-ONLY RESOURCE GENERATION: START =====
    let fixture = get_small_fixture("wbfp_create_new_pack_m");
    // ===== TEST-ONLY RESOURCE GENERATION: END =====
    let fixture_str = fixture.to_str().expect("转换夹具路径失败").to_string();

    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    let r = wbfp(
        &WaterBallFilePackCommand::new(WaterBallFilePackCommands::Pack(
            WaterBallFilePackCommandsPack::new(
                fixture_str,
                Some(out_file_path.clone()),
                false,
                None,
                true,
            ),
        )),
        Some(&mp),
    );
    r?;
    fs::remove_dir_all(&out_dir_path)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}
/// wbfp：大规模随机文件打包+校验+解包 / wbfp: large-scale random file pack+verify+unpack
#[test]
#[ignore = "longtime"]
fn wbfp_pack_verify_unpack_longtime() -> Result<(), Box<dyn std::error::Error>> {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH) + "/wbfp_pack_verify_unpack_longtime";
    prepare_test_dir(Path::new(&out_dir_path));

    let fixture = create_large_fixture("wbfp_pack_verify_unpack_longtime");
    let pack_path = out_dir_path.clone() + "/pack";

    // Step 1: 打包 / Pack
    let r = || -> Result<(), Box<dyn std::error::Error>> {
        wbfp(
            &WaterBallFilePackCommand::new(WaterBallFilePackCommands::Pack(
                WaterBallFilePackCommandsPack::new(
                    fixture.to_str().expect("转换夹具路径失败").to_string(),
                    Some(pack_path.clone()),
                    false,
                    None,
                    true,
                ),
            )),
            Some(&mp),
        )?;

        // Step 2: 哈希校验 / Hash verify
        let wbfp_path = pack_path.clone() + ".wbfp";
        wbfp(
            &WaterBallFilePackCommand::new(WaterBallFilePackCommands::HashVerify(
                WaterBallFilePackCommandsHashVerify::new(wbfp_path.clone(),false),
            )),
            Some(&mp),
        )?;

        // Step 3: 解包 / Unpack
        let unpack_dir = out_dir_path.clone() + "/unpacked";
        _ = fs::create_dir_all(&unpack_dir);
        wbfp(
            &WaterBallFilePackCommand::new(WaterBallFilePackCommands::Unpack(
                WaterBallFilePackCommandsUnpack::new(
                    wbfp_path.clone(),
                    Some(unpack_dir),
                    false,
                    None,
                ),
            )),
            Some(&mp),
        )?;
        Ok(())
    }();

    // 清理 / Cleanup
    r?;
    fs::remove_dir_all(&out_dir_path)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}

/// 打包目录 — 不分离数据文件 / Pack directory — no separate data file
#[test]
fn wbfp_create_new_pack_m_no_s_data_file() -> Result<(), Box<dyn std::error::Error>> {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/wbfp_create_new_pack_m_no_s_data_file");
    prepare_test_dir(Path::new(&out_dir_path));

    // ===== TEST-ONLY RESOURCE GENERATION: START =====
    let fixture = get_small_fixture("wbfp_create_new_pack_m_no_s_data_file");
    // ===== TEST-ONLY RESOURCE GENERATION: END =====
    let fixture_str = fixture.to_str().expect("转换夹具路径失败").to_string();

    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    let r = wbfp(
        &WaterBallFilePackCommand::new(WaterBallFilePackCommands::Pack(
            WaterBallFilePackCommandsPack::new(
                fixture_str,
                Some(out_file_path.clone()),
                true,
                None,
                true,
            ),
        )),
        Some(&mp),
    );
    r?;
    fs::remove_dir_all(&out_dir_path)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}

// =========================================================================
// wbfp 解包测试 / wbfp unpack tests
// =========================================================================

/// 打包 + 哈希校验 + 解包 完整流程 / Pack + hash verify + unpack full flow
#[test]
fn wbfp_create_new_pack_m_no_s_data_file_s() -> Result<(), Box<dyn std::error::Error>> {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/wbfp_create_new_pack_m_no_s_data_file_s");
    prepare_test_dir(Path::new(&out_dir_path));

    // ===== TEST-ONLY RESOURCE GENERATION: START =====
    let fixture = get_small_fixture("wbfp_create_new_pack_m_no_s_data_file_s");
    // ===== TEST-ONLY RESOURCE GENERATION: END =====
    let fixture_str = fixture.to_str().expect("转换夹具路径失败").to_string();

    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    let r = || -> Result<(), Box<dyn std::error::Error>> {
        //前提：打包
        {
            wbfp(
                &WaterBallFilePackCommand::new(WaterBallFilePackCommands::Pack(
                    WaterBallFilePackCommandsPack::new(
                        fixture_str,
                        Some(out_file_path.clone()),
                        true,
                        None,
                        true,
                    ),
                )),
                Some(&mp),
            )?;
        }
        out_file_path.push_str(".wbfp");
        //哈希校验（仅需包路径）
        {
            wbfp(
                &WaterBallFilePackCommand::new(WaterBallFilePackCommands::HashVerify(
                    WaterBallFilePackCommandsHashVerify::new(out_file_path.clone(), false),
                )),
                Some(&mp),
            )?;
        }
        //解包
        {
            let mut s_out_path = out_dir_path.clone();
            s_out_path.push_str("/s");
            wbfp(
                &WaterBallFilePackCommand::new(WaterBallFilePackCommands::Unpack(
                    WaterBallFilePackCommandsUnpack::new(
                        out_file_path.clone(),
                        Some(s_out_path),
                        false,
                        None,
                    ),
                )),
                Some(&mp),
            )?;
        }
        Ok(())
    }();
    r?;
    fs::remove_dir_all(&out_dir_path)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}

/// 递归收集目录下所有文件的相对路径（排序）/ Recursively collect relative file paths (sorted)
fn collect_relative_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    walk(&p, root, out);
                } else if let Ok(rel) = p.strip_prefix(root) {
                    out.push(rel.to_path_buf());
                }
            }
        }
    }
    let mut result = Vec::new();
    walk(root, root, &mut result);
    result.sort();
    result
}

/// 在包数据文件三个分散内容块翻转字节，返回损坏位置。
/// 128B 对齐块内大部分是填充区，固定偏移翻转会落在填充区而检测不到损坏，
/// 必须扫描定位"content_" 内容字节。
/// / Flip bytes in 3 content blocks of the pack data file, returning the
/// corrupt positions. 128B-aligned blocks are mostly padding, so fixed
/// offsets may hit padding and go undetected; must locate "content_" bytes.
fn corrupt_pack_content_bytes(pack_path: &Path) -> io::Result<Vec<u64>> {
    let mut f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pack_path)?;
    let flen = f.metadata()?.len();
    assert!(flen > 1024, "包文件过小，无法可靠构造损坏");
    let mut all = Vec::new();
    f.seek(io::SeekFrom::Start(0))?;
    f.read_to_end(&mut all)?;
    let corrupt_pos: Vec<u64> = (0..all.len() - 8)
        .filter(|&i| &all[i..i + 8] == b"content_")
        .take(3)
        .map(|i| (i + 2) as u64)
        .collect();
    assert_eq!(corrupt_pos.len(), 3, "未找到足够的文件内容块");
    for pos in &corrupt_pos {
        let mut byte = [0u8; 1];
        f.seek(io::SeekFrom::Start(*pos))?;
        f.read_exact(&mut byte)?;
        byte[0] ^= 0xFF;
        f.seek(io::SeekFrom::Start(*pos))?;
        f.write_all(&byte)?;
    }
    Ok(corrupt_pos)
}

/// 解包内嵌哈希校验：损坏数据字节的文件被跳过（磁盘无输出），其余正常解包
/// Unpack with built-in hash verify: corrupted files are skipped (no disk
/// output), the rest are unpacked normally.
#[test]
fn wbfp_unpack_skip_corrupted_file() -> Result<(), Box<dyn std::error::Error>> {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/wbfp_unpack_skip_corrupted_file");
    prepare_test_dir(Path::new(&out_dir_path));

    // ===== TEST-ONLY RESOURCE GENERATION: START =====
    let fixture = get_small_fixture("wbfp_unpack_skip_corrupted_file");
    // ===== TEST-ONLY RESOURCE GENERATION: END =====
    let fixture_str = fixture.to_str().expect("转换夹具路径失败").to_string();

    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    let r = || -> Result<(), Box<dyn std::error::Error>> {
        //打包
        wbfp(
            &WaterBallFilePackCommand::new(WaterBallFilePackCommands::Pack(
                WaterBallFilePackCommandsPack::new(
                    fixture_str,
                    Some(out_file_path.clone()),
                    true,
                    None,
                    true,
                ),
            )),
            Some(&mp),
        )?;
        out_file_path.push_str(".wbfp");
        //在数据文件（.wbfp）三个分散位置翻转字节，损坏至少一个文件的数据
        //（清单在 .wbfp.wbm，数据文件内任意数据字节损坏必导致所属文件哈希不匹配）
        corrupt_pack_content_bytes(Path::new(&out_file_path))?;
        //解包（hash_verify = true：校验失败的文件被跳过）
        let mut s_out_path = out_dir_path.clone();
        s_out_path.push_str("/s");
        wbfp(
            &WaterBallFilePackCommand::new(WaterBallFilePackCommands::Unpack(
                WaterBallFilePackCommandsUnpack::new(
                    out_file_path.clone(),
                    Some(s_out_path.clone()),
                    true,
                    None,
                ),
            )),
            Some(&mp),
        )?;
        //断言：损坏文件被跳过（缺失），其余文件正常解包
        let fixture_files = collect_relative_files(&fixture);
        let out_files = collect_relative_files(Path::new(&s_out_path));
        let missing = fixture_files
            .iter()
            .filter(|p| !out_files.contains(p))
            .count();
        assert!(missing >= 1, "损坏文件未被跳过，解包输出与源完全一致");
        assert!(
            out_files.len() as i64 >= fixture_files.len() as i64 - 3,
            "跳过文件过多: 输出 {} vs 源 {}",
            out_files.len(),
            fixture_files.len()
        );
        Ok(())
    }();
    r?;
    fs::remove_dir_all(&out_dir_path)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}

/// 解包哈希校验可选：未指定 -H/--hash-verify 时直接解包，损坏文件也照常输出
/// Optional hash verify: without -H/--hash-verify, unpack extracts directly
/// and corrupted files are still written to disk
#[test]
fn wbfp_unpack_no_verify_extracts_all() -> Result<(), Box<dyn std::error::Error>> {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    out_dir_path.push_str("/wbfp_unpack_no_verify_extracts_all");
    prepare_test_dir(Path::new(&out_dir_path));

    // ===== TEST-ONLY RESOURCE GENERATION: START =====
    let fixture = get_small_fixture("wbfp_unpack_no_verify_extracts_all");
    // ===== TEST-ONLY RESOURCE GENERATION: END =====
    let fixture_str = fixture.to_str().expect("转换夹具路径失败").to_string();

    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    let r = || -> Result<(), Box<dyn std::error::Error>> {
        //打包
        wbfp(
            &WaterBallFilePackCommand::new(WaterBallFilePackCommands::Pack(
                WaterBallFilePackCommandsPack::new(
                    fixture_str,
                    Some(out_file_path.clone()),
                    true,
                    None,
                    true,
                ),
            )),
            Some(&mp),
        )?;
        out_file_path.push_str(".wbfp");
        //与 skip 测试相同构造损坏，但解包不指定哈希校验
        corrupt_pack_content_bytes(Path::new(&out_file_path))?;
        //解包（hash_verify = false）
        let mut s_out_path = out_dir_path.clone();
        s_out_path.push_str("/s");
        wbfp(
            &WaterBallFilePackCommand::new(WaterBallFilePackCommands::Unpack(
                WaterBallFilePackCommandsUnpack::new(
                    out_file_path.clone(),
                    Some(s_out_path.clone()),
                    false,
                    None,
                ),
            )),
            Some(&mp),
        )?;
        //断言：不校验时损坏文件也全部解包（输出与源完全一致）
        let fixture_files = collect_relative_files(&fixture);
        let out_files = collect_relative_files(Path::new(&s_out_path));
        assert_eq!(
            out_files.len(),
            fixture_files.len(),
            "未指定哈希校验时应解包全部文件（含损坏文件）"
        );
        Ok(())
    }();
    r?;
    fs::remove_dir_all(&out_dir_path)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}

// =========================================================================
// 错误路径测试 / Error path tests
// =========================================================================

/// ff：搜索不存在的目录应 panic / ff: search nonexistent dir should panic
#[test]
fn ff_out_file_skip_symlink_err_not_found_dir() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let out_dir_path =
        PathBuf::from(FF_TEST_TEMP_ERR_DIR_PATH).join("ff_out_file_skip_symlink_err_not_found_dir");
    prepare_test_dir(Path::new(&out_dir_path));
    let out_file_path = out_dir_path.join("out.json");
    _ = fs::remove_file(&out_file_path);
    // 捕获 ff 对不存在目录的 panic：验证预期错误确实发生；
    // panic 消息含外部 io 错误文本，跨平台不可靠，仅断言错误发生；
    // 断言失败时测试 panic，跳过清理以保留证据
    let r = std::panic::catch_unwind(|| {
        //命令行参数处理
        ff(
            FileFinderArgs::new(
                "/~".to_string(),
                Some(
                    out_file_path
                        .to_str()
                        .expect("转换输出路径失败")
                        .to_string(),
                ),
                true,
                None,
            ),
            Some(&mp),
        )
        .expect("执行命令行操作时错误");
    });
    assert!(r.is_err(), "搜索不存在的目录应 panic");
    fs::remove_dir_all(&out_dir_path).expect("测试通过后清理临时目录失败");
}
/// wbfp：打包不存在的目录应记录错误而非 panic / wbfp: pack nonexistent dir should log error, not panic
#[test]
fn wbfp_create_new_pack_m_err_not_found_in_dir() {
    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let mut out_dir_path = String::from(WBFP_TEST_TEMP_ERR_DIR_PATH);
    out_dir_path.push_str("/wbfp_create_new_pack_m_err_not_found_in_dir");
    prepare_test_dir(Path::new(&out_dir_path));
    let mut out_file_path = out_dir_path.clone();
    out_file_path.push_str("/pack");
    //命令行参数处理
    let result = wbfp(
        &WaterBallFilePackCommand::new(WaterBallFilePackCommands::Pack(
            WaterBallFilePackCommandsPack::new(
                "/~".to_string(),
                Some(out_file_path.clone()),
                false,
                None,
                true,
            ),
        )),
        Some(&mp),
    );
    // 不 panic，而是返回 Ok（搜索错误已记录）
    assert!(result.is_ok());
    fs::remove_dir_all(&out_dir_path).expect("测试通过后清理临时目录失败");
}

// === 符号链接循环检测 / Symlink cycle detection ===

use crate::command::ff::{FileFinderArgs, ff};
use crate::command::wbfp::{
    WaterBallFilePackCommand, WaterBallFilePackCommands, WaterBallFilePackCommandsHashVerify,
    WaterBallFilePackCommandsPack, WaterBallFilePackCommandsUnpack, wbfp,
};
use std::io;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

const SYMLINK_TEST_DIR: &str = "./temp/test/command/ok";

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

/// 测试开始准备：删除旧的测试临时目录（不存在不算错误），
/// 其他非预期错误直接 panic 终止测试；然后创建测试目录。
/// bin target 测试无法访问 lib 中 #[cfg(test)] 的 TestTool，故在本地实现。
///
/// Prepare a test temp dir: remove any stale directory first (missing is fine),
/// any other unexpected error aborts the test; then create the directory.
fn prepare_test_dir(dir: &Path) {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => panic!("清理测试临时目录失败: {e}"),
        Ok(()) => {}
    }
    fs::create_dir_all(dir).expect("创建测试临时目录失败");
}

/// 创建清理后的测试子目录 / Create a clean test subdirectory
fn setup_symlink_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(SYMLINK_TEST_DIR).join(name);
    prepare_test_dir(&dir);
    dir
}

/// 在指定目录下创建一个内容为 "ok" 的测试文件 / Create a test file with "ok" content
fn create_file_cmd(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let mut f = fs::File::create(&path).expect("创建测试文件失败");
    f.write_all(b"ok").expect("写入测试文件内容失败");
    path
}

/// 小型稳定测试夹具资源目录 / Small stable test fixture resource directory
///
/// 固定内容夹具（确定性、跨运行复用）存放在 `resources/test/`，只读；
/// 测试输出（pack/out.json 等）仍存放在 `temp/test/`。
/// Fixed-content fixtures (deterministic, reused across runs) live under
/// `resources/test/`, read-only; test outputs still live under `temp/test/`.
const SMALL_FIXTURE_RESOURCE_DIR: &str = "./resources/test/command/ok";

/// 大型随机文件测试夹具目录 / Large random file test fixture directory
///
/// 随机内容夹具是测试产物（随机规范），每次运行在 `temp/test/` 内重新生成。
/// Randomized fixtures are test artifacts (random strategy), regenerated
/// each run under `temp/test/`.
const LARGE_FIXTURE_DIR: &str = "./temp/test/command/ok";

/// 创建 1000 个随机文件的测试夹具 / Create a test fixture with 1000 random files
///
/// `name` 用于拼接子目录，避免并行测试间冲突。
/// 生成 25 个子目录 × 40 个文件 = 1000 个文件
/// 每个文件内容和大小均随机（50~2048 字节）
/// 用于大规模文件搜索和打包的压力测试
fn create_large_fixture(name: &str) -> PathBuf {
    // 夹具位于 {模块}/{ok}/{测试函数名}/fixture 子目录，与测试输出同目录
    // Fixture lives under {module}/{ok}/{test_fn}/fixture, sharing the dir with outputs
    let dir = PathBuf::from(LARGE_FIXTURE_DIR).join(name).join("fixture");
    prepare_test_dir(&dir);

    let files_per_dir = 40;
    let dir_count = 25;

    for i in 0..dir_count {
        let sub = dir.join(format!("batch_{i:02}"));
        fs::create_dir_all(&sub).expect("创建夹具子目录失败");

        for j in 0..files_per_dir {
            let size = rand::random_range(50..=2048);
            let content: Vec<u8> = (0..size).map(|_| rand::random::<u8>()).collect();
            let fpath = sub.join(format!("data_{j:03}.bin"));
            fs::write(&fpath, &content).expect("写入夹具文件失败");
        }
    }
    dir
}

/// 获取小型稳定测试夹具资源目录（存在即用，缺失才生成）
///
/// Get the small stable test fixture resource dir (reused if present,
/// generated only when missing).
///
/// 返回 `resources/test/command/ok/{name}/fixture`；生成后即作为只读资源
/// 跨运行复用，不随每次测试重建。测试输出（pack/out.json）仍在 temp。
/// Returns `resources/test/command/ok/{name}/fixture`; after generation it is
/// reused as a read-only resource across runs, never rebuilt per test run.
fn get_small_fixture(name: &str) -> PathBuf {
    let dir = PathBuf::from(SMALL_FIXTURE_RESOURCE_DIR).join(name).join("fixture");
    // 固定内容夹具：3 子目录 × 5 文件 = 15 文件 "content_{i}_{j}\n"。
    // 仅资源缺失时生成（首次运行），生成后即作为只读资源复用；
    // 资源生成的作用域以调用方测试函数内的 TEST-ONLY 分界标注。
    if !dir.exists() {
        for i in 0..3 {
            let sub = dir.join(format!("sub_{i}"));
            fs::create_dir_all(&sub).expect("创建夹具资源子目录失败");
            for j in 0..5 {
                let path = sub.join(format!("file_{j}.txt"));
                fs::write(&path, format!("content_{i}_{j}\n")).expect("写入夹具资源文件失败");
            }
        }
    }
    dir
}

/// ff 命令：搜索含符号链接的目录，符号链接应出现在 JSON 输出中
#[test]
fn ff_with_symlinks_in_output() -> Result<(), Box<dyn std::error::Error>> {
    let root = setup_symlink_dir("ff_with_symlinks_in_output");
    create_file_cmd(&root, "real.txt");
    let link = root.join("link.txt");
    if try_create_symlink_cmd(&root.join("real.txt"), &link).is_none() {
        panic!("[测试]创建符号链接测试失败");
    }

    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let json_path = root.join("result.json");
    _ = fs::remove_file(&json_path);

    let r = ff(
        FileFinderArgs::new(
            root.to_str()
                .expect("转换根目录路径为字符串失败")
                .to_string(),
            Some(
                json_path
                    .to_str()
                    .expect("转换 JSON 文件路径为字符串失败")
                    .to_string(),
            ),
            false,
            None,
        ),
        Some(&mp),
    );

    let json_str = fs::read_to_string(&json_path).expect("读取 JSON 结果文件失败");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("解析 JSON 结果失败");
    let file_count = parsed["file_count"].as_u64().expect("获取文件计数字段失败");
    assert!(file_count >= 1, "JSON 输出应包含文件");

    r?;
    fs::remove_dir_all(&root)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}

/// ff -s：跳过符号链接，输出中不应包含符号链接
#[test]
fn ff_skip_symlinks_excludes_them() -> Result<(), Box<dyn std::error::Error>> {
    let root = setup_symlink_dir("ff_skip_symlinks_excludes_them");
    create_file_cmd(&root, "real.txt");
    let link = root.join("link.txt");
    if try_create_symlink_cmd(&root.join("real.txt"), &link).is_none() {
        panic!("[测试]创建符号链接测试失败");
    }

    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);
    let json_path = root.join("result.json");
    _ = fs::remove_file(&json_path);

    let r = ff(
        FileFinderArgs::new(
            root.to_str()
                .expect("转换根目录路径为字符串失败")
                .to_string(),
            Some(
                json_path
                    .to_str()
                    .expect("转换 JSON 文件路径为字符串失败")
                    .to_string(),
            ),
            true,
            None,
        ),
        Some(&mp),
    );

    let json_str = fs::read_to_string(&json_path).expect("读取 JSON 结果文件失败");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("解析 JSON 结果失败");
    let file_count = parsed["file_count"].as_u64().expect("获取文件计数字段失败");
    assert_eq!(file_count, 1, "跳过符号链接后应只有 1 个真实文件");

    r?;
    fs::remove_dir_all(&root)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}

/// ff：搜索含祖先符号链接循环的目录，不应崩溃
#[test]
fn ff_ancestor_symlink_does_not_crash() -> Result<(), Box<dyn std::error::Error>> {
    let root = setup_symlink_dir("ff_ancestor_symlink_does_not_crash");
    let sub = root.join("sub");
    fs::create_dir_all(&sub).expect("创建子目录失败");
    create_file_cmd(&sub, "child.txt");
    let link_back = sub.join("back_to_root");
    if try_create_symlink_cmd(&root, &link_back).is_none() {
        panic!("[测试]创建符号链接测试失败");
    }

    let mp = MultiProgress::new();
    crate::init_global_logging(&mp);

    let r = ff(
        FileFinderArgs::new(
            root.to_str()
                .expect("转换根目录路径为字符串失败")
                .to_string(),
            None,
            false,
            None,
        ),
        Some(&mp),
    );

    r?;
    fs::remove_dir_all(&root)
        .map_err(|e| format!("测试通过后清理临时目录失败: {e}"))?;
    Ok(())
}
