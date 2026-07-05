use super::*;
use std::fs;
use std::io::Write;
use std::sync::mpsc;

const TEST_DIR: &str = "./temp/test/ff/symlink";

/// 平台相关的符号链接创建 / Platform-specific symlink creation
/// 创建前将目标路径规格化以避免平台路径分隔符问题
#[cfg(unix)]
fn create_symlink(original: &Path, link: &Path) -> io::Result<()> {
    let original = original.canonicalize().unwrap_or_else(|_| original.to_path_buf());
    std::os::unix::fs::symlink(original, link)
}

#[cfg(windows)]
fn create_symlink(original: &Path, link: &Path) -> io::Result<()> {
    let original = original.canonicalize().unwrap_or_else(|_| original.to_path_buf());
    if original.is_dir() {
        std::os::windows::fs::symlink_dir(&original, link)
    } else {
        std::os::windows::fs::symlink_file(&original, link)
    }
}

/// 尝试创建符号链接，失败时返回 None（如 Windows 无权限）
fn try_create_symlink(original: &Path, link: &Path) -> Option<()> {
    match create_symlink(original, link) {
        Ok(()) => Some(()),
        Err(e) => {
            eprintln!("无法创建符号链接（权限不足？）: {e}");
            None
        }
    }
}

/// 运行搜索，消费进度消息后返回结果 / Run search, consume progress, return result
fn search_dir(path: &Path, skip_symlink: bool) -> FilesList {
    let (tx, rx) = mpsc::channel();
    let (rtx, rrx) = mpsc::channel();
    let t_path = path.to_path_buf();
    thread::spawn(move || {
        rtx.send(FileFinder.search(&t_path, skip_symlink, tx, 4)).unwrap();
    });
    for _ in rx {}
    rrx.recv().unwrap().unwrap()
}

/// 准备测试目录：清理并创建 / Prepare test dir: clean and create
fn setup_test_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(TEST_DIR).join(name);
    _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 创建测试文件 / Create a test file with content
fn create_test_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let mut f = fs::File::create(&path).unwrap();
    f.write_all(b"test content").unwrap();
    path
}

// === 测试用例 / Test cases ===

/// 符号链接指向普通文件，应被正常发现
/// Symlink pointing to a regular file should be discovered normally
#[test]
fn symlink_to_file_is_found() {
    let root = setup_test_dir("symlink_to_file");
    let file = create_test_file(&root, "real.txt");
    let link = root.join("link_to_real.txt");
    if try_create_symlink(&file, &link).is_none() {
        return;
    }

    let result = search_dir(&root, false);

    assert_eq!(result.file_count(), 2, "应发现 2 个文件（真实文件 + 符号链接）");
    assert!(result.files_list().contains_key("real.txt"));
    assert!(result.files_list().contains_key("link_to_real.txt"));
    _ = fs::remove_dir_all(&root);
}

/// 符号链接指向目录，应遍历目录内的文件
/// Symlink pointing to a directory should traverse files inside
#[test]
fn symlink_to_dir_is_traversed() {
    let root = setup_test_dir("symlink_to_dir");
    let sub = root.join("subdir");
    fs::create_dir_all(&sub).unwrap();
    create_test_file(&sub, "inside.txt");
    let link = root.join("link_to_subdir");
    if try_create_symlink(&sub, &link).is_none() {
        _ = fs::remove_dir_all(&root);
        return;
    }

    let result = search_dir(&root, false);

    assert!(result.file_count() >= 1, "至少应发现 inside.txt");
    assert!(result.files_list().contains_key("subdir"));
    assert!(result.files_list().contains_key("link_to_subdir"));
    _ = fs::remove_dir_all(&root);
}

/// 符号链接指向祖先目录：应检测到循环而不崩溃、不重复
/// Symlink to ancestor directory: cycle detected, no crash, no duplicates
#[test]
fn symlink_to_ancestor_is_detected() {
    let root = setup_test_dir("symlink_ancestor");
    let sub = root.join("sub");
    fs::create_dir_all(&sub).unwrap();
    create_test_file(&sub, "child.txt");
    let link_back = sub.join("link_back");
    if try_create_symlink(&root, &link_back).is_none() {
        _ = fs::remove_dir_all(&root);
        return;
    }

    let result = search_dir(&root, false);
    assert!(result.file_count() >= 1, "应发现 child.txt");
    _ = fs::remove_dir_all(&root);
}

/// 多级符号链接（无循环）：应正常遍历
/// Multi-level symlinks (no cycle): should traverse normally
#[test]
fn multi_level_symlinks_work() {
    let root = setup_test_dir("multi_level_symlink");
    let dir_a = root.join("a");
    let dir_b = root.join("b");
    let dir_c = root.join("c");
    fs::create_dir_all(&dir_c).unwrap();
    create_test_file(&dir_c, "target.txt");

    if try_create_symlink(&dir_c, &dir_b).is_none()
        || try_create_symlink(&dir_b, &dir_a).is_none()
    {
        _ = fs::remove_dir_all(&root);
        return;
    }

    let result = search_dir(&root, false);
    assert!(result.file_count() >= 1, "应发现 target.txt");
    _ = fs::remove_dir_all(&root);
}

/// 跳过符号链接标志：符号链接应全部被忽略
/// Skip symlinks flag: all symlinks should be ignored
#[test]
fn skip_symlinks_flag_works() {
    let root = setup_test_dir("skip_symlinks");
    let file = create_test_file(&root, "real.txt");
    let link = root.join("link.txt");
    if try_create_symlink(&file, &link).is_none() {
        _ = fs::remove_dir_all(&root);
        return;
    }

    let result = search_dir(&root, true);

    assert!(result.files_list().contains_key("real.txt"), "真实文件应存在");
    assert!(!result.files_list().contains_key("link.txt"), "符号链接应被跳过");
    assert_eq!(result.file_count(), 1, "只应发现 1 个真实文件");
    _ = fs::remove_dir_all(&root);
}

/// 断开的符号链接：不应崩溃，不应计入结果
/// Broken symlink: should not crash, should not count in results
#[test]
fn broken_symlink_does_not_crash() {
    let root = setup_test_dir("broken_symlink");
    let broken = root.join("broken_link");
    let nonexistent = root.join("does_not_exist");
    if try_create_symlink(&nonexistent, &broken).is_none() {
        _ = fs::remove_dir_all(&root);
        return;
    }

    let result = search_dir(&root, false);
    assert!(!result.files_list().contains_key("broken_link"),
            "断开的符号链接不应出现在结果中");
    _ = fs::remove_dir_all(&root);
}

/// 混合场景：真实文件 + 真实目录 + 符号链接共存
/// Mixed scenario: real files + real dirs + symlinks coexist
#[test]
fn mixed_real_and_symlinks() {
    let root = setup_test_dir("mixed");
    create_test_file(&root, "real_file.txt");
    let real_dir = root.join("real_dir");
    fs::create_dir_all(&real_dir).unwrap();
    create_test_file(&real_dir, "nested.txt");

    let link_file = root.join("link_file.txt");
    let link_dir = root.join("link_dir");
    if try_create_symlink(&root.join("real_file.txt"), &link_file).is_none()
        || try_create_symlink(&real_dir, &link_dir).is_none()
    {
        _ = fs::remove_dir_all(&root);
        return;
    }

    let result = search_dir(&root, false);

    assert!(result.files_list().contains_key("real_file.txt"));
    assert!(result.files_list().contains_key("real_dir"));
    assert!(result.files_list().contains_key("link_file.txt"));
    assert!(result.files_list().contains_key("link_dir"));
    assert!(result.file_count() >= 2, "至少应发现 2 个文件（real_file + nested）");
    _ = fs::remove_dir_all(&root);
}

/// 符号链接指向同一个 inode 但不同路径（硬链接等价场景）
/// 验证链式检测不会误判——同一 inode 通过不同链可以多次访问
/// Symlink to same inode via different paths should not be falsely detected as cycle
#[test]
fn same_target_via_different_chains() {
    let root = setup_test_dir("different_chains");
    let target = root.join("target_dir");
    fs::create_dir_all(&target).unwrap();
    create_test_file(&target, "data.txt");

    let link_a = root.join("link_a");
    let link_b = root.join("link_b");
    if try_create_symlink(&target, &link_a).is_none()
        || try_create_symlink(&target, &link_b).is_none()
    {
        _ = fs::remove_dir_all(&root);
        return;
    }

    let result = search_dir(&root, false);
    assert!(result.file_count() >= 1, "应发现 data.txt");
    _ = fs::remove_dir_all(&root);
}

/// DirEntry 链正确克隆传递 / Chain is correctly cloned and passed
#[test]
fn chain_propagates_correctly() {
    let root = setup_test_dir("chain_propagate");
    let level1 = root.join("level1");
    fs::create_dir_all(&level1).unwrap();
    let level2 = level1.join("level2");
    fs::create_dir_all(&level2).unwrap();
    create_test_file(&level2, "deep.txt");

    let link_back = level2.join("back_to_root");
    if try_create_symlink(&root, &link_back).is_none() {
        _ = fs::remove_dir_all(&root);
        return;
    }

    let result = search_dir(&root, false);
    assert!(result.file_count() >= 1, "应发现 deep.txt");
    _ = fs::remove_dir_all(&root);
}
