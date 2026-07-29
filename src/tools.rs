#[cfg(test)]
use std::fs;
use std::{
    path::{Path, PathBuf},
};

//内部路径工具
pub struct PathTool;

impl PathTool {
    /// 将文件路径分割为字符串数组，排除根目录分隔符。
    ///
    /// 例如 `"a/b/c"` 返回 `["a", "b", "c"]`。
    ///
    /// Split a file path into a vector of string segments, excluding root separators.
    ///
    /// For example, `"a/b/c"` returns `["a", "b", "c"]`.
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
    /// 例如，`"a/b/c/d"` 去掉头部 `"a/b"` 得到 `"c/d"`。
    /// 如果路径不以头部开始则返回 `None`。
    ///
    /// Strip head prefix from a full path to obtain a relative path.
    ///
    /// For example, stripping `"a/b"` from `"a/b/c/d"` yields `"c/d"`.
    /// Returns `None` if the path does not start with the head prefix.
    pub fn path_remove_head<P: AsRef<Path>>(path: P, head: P) -> Option<PathBuf> {
        let head_vec = PathTool::path_to_string_vec(head);
        let path_vec = PathTool::path_to_string_vec(path);
        let mut new_path = PathBuf::new();

        if path_vec > head_vec {
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
/// 支持 B、KiB、MiB、GiB 单位，保留两位小数。
///
/// Format a byte count into a human-readable string.
///
/// Supports B, KiB, MiB, GiB units with two decimal places.
#[must_use]
pub fn bytes_len_to_string(len: u64) -> String {
    const B_LEN: u64 = 1024;
    const K_LEN: u64 = B_LEN * 1024;
    const M_LEN: u64 = K_LEN * 1024;

    match len {
        0..B_LEN => format!("{len}B"),
        B_LEN..K_LEN => format!("{}KiB", (len * 100 / B_LEN) as f64 / 100.0),
        K_LEN..M_LEN => format!("{}MiB", (len * 100 / K_LEN) as f64 / 100.0),
        M_LEN.. => format!("{}GiB", (len * 100 / M_LEN) as f64 / 100.0),
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
