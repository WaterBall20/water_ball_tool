#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};

//内部路径工具
pub struct PathTool;

impl PathTool {
/// 将文件路径分割为字符串数组，排除根目录分隔符。
///
/// Split a file path into a vector of string segments, excluding root separators.
///
/// # 算法 / Algorithm
///
/// 遍历 `Path` 的 `Components` 迭代器，将每个非根段转换为 `String` 并收集到 `Vec` 中。
/// 根目录 `/` 被自动排除。
///
/// Iterates over the `Path` `Components` iterator, converts each non-root segment to a `String`,
/// and collects them into a `Vec`. The root `/` is automatically excluded.
///
/// # 示例 / Example
///
/// ```rust
/// use water_ball_tool::tools::PathTool;
/// let segments = PathTool::path_to_string_vec("a/b/c");
/// assert_eq!(segments, vec!["a", "b", "c"]);
/// ```
pub fn path_to_string_vec<P: AsRef<Path>>(path: P) -> Vec<String> {
        let path = path.as_ref();
        /*TODO:路径前部处理
        let path = path
            .strip_prefix("./")
            .or_else(|_| path.strip_prefix("."))
            .or_else(|_| path.strip_prefix(".\\\\"))
            .map_or(path, |r| r);
        */
        let mut path_list: Vec<String> = Vec::new();
        for item in path {
            let item = item.to_string_lossy().to_string();
            //排除根
            if item != "/" {
                path_list.push(item);
            }
        }
        path_list
    }

/// 从完整路径中移除头部部分，得到相对路径。
///
/// Strip head prefix from a full path to obtain a relative path.
///
/// # 算法 / Algorithm
///
/// 先将 `path` 和 `head` 分别通过 `path_to_string_vec` 转为段数组。
/// 如果 `path` 的段数组长度大于 `head` 的段数组，则拼接 `path[head.len()..]` 段并返回。
/// 否则返回 `None`，表示 `path` 不以 `head` 开头。
///
/// Converts both `path` and `head` to segment arrays via `path_to_string_vec`.
/// If `path` has more segments than `head`, joins `path[head.len()..]` and returns it.
/// Otherwise returns `None`, meaning `path` does not start with `head`.
///
/// # 示例 / Example
///
/// ```rust
/// use water_ball_tool::tools::PathTool;
/// let relative = PathTool::path_remove_head("a/b/c/d", "a/b");
/// assert_eq!(relative, Some(std::path::PathBuf::from("c/d")));
/// assert!(PathTool::path_remove_head("x/y", "a/b").is_none());
/// ```
pub fn path_remove_head<P: AsRef<Path>>(path: P, head: P) -> Option<PathBuf> {
        let head_vec = PathTool::path_to_string_vec(head);
        let path_vec = PathTool::path_to_string_vec(path);
        let mut new_path = PathBuf::new();

        if path_vec.len() > head_vec.len() && path_vec[..head_vec.len()] == head_vec {
            for name in &path_vec[head_vec.len()..] {
                new_path = new_path.join(name);
            }
            Some(new_path)
        } else {
            None
        }
    }
}

/// 将字节数格式化为人类可读的字符串。
///
/// Format a byte count into a human-readable string.
///
/// # 算法 / Algorithm
///
/// 根据字节大小范围选择单位：
/// - 0..1024 → `B`（原样输出）
/// - 1024..1 MiB → `KiB`（保留两位小数）
/// - 1 MiB..1 GiB → `MiB`（保留两位小数）
/// - 1 GiB 及以上 → `GiB`（保留两位小数）
///
/// 小数计算采用整数乘法 `len * 100 / 单位` 避免浮点精度问题。
///
/// Selects unit based on byte range:
/// - 0..1024 → `B` (literal)
/// - 1024..1 MiB → `KiB` (2 decimal places)
/// - 1 MiB..1 GiB → `MiB` (2 decimal places)
/// - 1 GiB and above → `GiB` (2 decimal places)
///
/// Uses integer math `len * 100 / divisor` to avoid floating-point precision issues.
///
/// # 示例 / Example
///
/// ```rust
/// use water_ball_tool::tools::bytes_len_to_string;
/// assert_eq!(bytes_len_to_string(0), "0B");
/// assert_eq!(bytes_len_to_string(500), "500B");
/// assert_eq!(bytes_len_to_string(2048), "2.00KiB");
/// assert_eq!(bytes_len_to_string(3670016), "3.50MiB");   // 3584 KiB + 100 KiB
/// assert_eq!(bytes_len_to_string(1073741824), "1.00GiB");
/// ```
#[must_use]
pub fn bytes_len_to_string(len: u64) -> String {
    const B_LEN: u64 = 1024;
    const K_LEN: u64 = B_LEN * 1024;
    const M_LEN: u64 = K_LEN * 1024;

    match len {
        0..B_LEN => format!("{len}B"),
        B_LEN..K_LEN => format!("{:.2}KiB", (len * 100 / B_LEN) as f64 / 100.0),
        K_LEN..M_LEN => format!("{:.2}MiB", (len * 100 / K_LEN) as f64 / 100.0),
        M_LEN.. => format!("{:.2}GiB", (len * 100 / M_LEN) as f64 / 100.0),
    }
}

/// 预留的水球包文件工具结构体（未实现）/ Reserved WBFP tool struct (not yet implemented)
struct _WBFPTool {}

#[cfg(test)]
pub struct TestTool;
#[cfg(test)]
impl TestTool {
    /// 清理测试产生的包文件及其关联文件（`.wbm`、`.lock`）。
    ///
    /// Clean up test-generated pack files and their associated files (`.wbm`, `.lock`).
    pub fn remove_test_pack_files<P: AsRef<Path>>(path: &P) {
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
}
