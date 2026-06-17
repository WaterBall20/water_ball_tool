#[cfg(test)]
use std::fs;
use std::path::Path;

//内部路径工具
pub(crate) struct PathTool;

impl PathTool {
    //将路径转换为Vec
    pub(crate) fn path_to_string_vec<P: AsRef<Path>>(path: P) -> Vec<String> {
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
            let item = String::from(item.to_str().expect("转换文本错误"));
            //排除根
            if item != "/" {
                path_list.push(item);
            }
        }
        path_list
    }
}

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

struct _WBFPTool {}

#[cfg(test)]
pub struct TestTool;
#[cfg(test)]
impl TestTool {
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
