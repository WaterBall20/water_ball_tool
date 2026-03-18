use std::path::Path;

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