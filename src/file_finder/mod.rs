/*
开始时间:2026-02-08 22:37
 */

use std::collections::HashMap;
use std::fs::{File, Metadata};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use std::{env, fs};

use serde::{Deserialize, Serialize};
use serde_json::Result;

#[derive(Debug, Serialize, Deserialize)]
pub struct FilesList {
    //搜索路径
    path: String,
    //数据总大小
    data_length: u64,
    //文件数量，不包括目录
    file_count: u32,
    //目录数量
    dir_count: u32,
    //文件列表
    files_list: HashMap<String, FileInfo>,
} //搜索结果
impl FilesList {
    pub fn file_path(&self) -> &str {
        &self.path
    }

    pub fn data_length(&self) -> u64 {
        self.data_length
    }

    pub fn file_count(&self) -> u32 {
        self.file_count
    }

    pub fn dir_count(&self) -> u32 {
        self.dir_count
    }

    pub fn files_list(&self) -> &HashMap<String, FileInfo> {
        &self.files_list
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct FileInfo {
    //文件名
    name: String,
    //大小
    length: u64,
    //上次修改时间
    modified_time: u128,
    //文件类型
    file_kind: FileKind,
} //文件信息
impl FileInfo {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn length(&self) -> u64 {
        self.length
    }

    pub fn modified_time(&self) -> u128 {
        self.modified_time
    }

    pub fn file_kind(&self) -> &FileKind {
        &self.file_kind
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Dir {
    //子文件列表
    files_list: HashMap<String, FileInfo>,
    //文件列表
    file_count: u32,
    //目录列表
    dir_count: u32,
} //文件夹独有
impl Dir {
    pub fn files_list(&self) -> &HashMap<String, FileInfo> {
        &self.files_list
    }

    pub fn file_count(&self) -> u32 {
        self.file_count
    }

    pub fn dir_count(&self) -> u32 {
        self.dir_count
    }
}

//文件类型
#[derive(Debug, Serialize, Deserialize)]
pub enum FileKind {
    File,
    Dir(Dir),
}

pub fn command(args: &[String]) {
    //参数格式：[程序路径,模块类型,指定搜索路径,输出路径...]
    //获取参数中的指定的路径，若没有则使用程序路径
    let path = match args.get(2) {
        Some(value) => value,
        None => match args.first() {
            Some(vales) => vales,
            None => "",
        },
    };

    //跳过符号链接参数（暂用）
    let skip_symlink = match args.get(4) {
        Some(value) => value.contains("-s"),
        None => false,
    };

    //搜索
    let files_list = search(path, skip_symlink);

    //输出到输出文件(若存在参数)
    if let Some(out_path) = args.get(3) {
        //只写模式打开文件，
        match File::create(out_path) {
            Ok(f) => {
                println!("正在将搜索结果输出到文件");
                serde_json::to_writer_pretty(f, &files_list).expect("保存到文件错误");
                //注意：'f'的所有权已经移动，不可再用，不需要关心是否释放，
                //Rust在离开作用域时，自动释放所在作用域所有持有所有权的变量，文件也会自动关闭。
                println!(r#"搜索结果已输出到文件: "{out_path}""#)
            }
            Err(err) => {
                panic!(r#"[致命错误]无法打开输出文件: "{out_path}" , Error: '{err}'"#)
            }
        }
    } else {
        println!("搜索结果：{files_list:?}")
    }
}

fn get_file_name(path_buf: &PathBuf) -> Option<&str> {
    match path_buf.file_name() {
        Some(name) => match name.to_str() {
            Some(name) => Some(name),
            None => {
                println!("[警告]无法将OsStr:{name:?} 转换成Str");
                None
            }
        },
        None => {
            println!("[警告]目录:（{path_buf:?}）无法获取文件名。");
            None
        }
    }
}
fn get_file_modified(metadata: &Metadata) -> u128 {
    match metadata.modified() {
        Ok(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(time) => time.as_millis(),
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}

fn m_search_add_file(hash_map: &mut HashMap<String, FileInfo>, path_buf: PathBuf) -> Option<u64> {
    //文件
    let name = get_file_name(&path_buf);
    if let Some(name) = name {
        let name = String::from(name);
        //获取文件元数据
        match path_buf.metadata() {
            Ok(metadata) => {
                let len = metadata.len();
                let modified_time = get_file_modified(&metadata);
                let file_info = FileInfo {
                    name: String::from(&name),
                    length: len,
                    modified_time,
                    file_kind: FileKind::File,
                };
                hash_map.insert(name, file_info);
                Some(len)
            }
            Err(_) => {
                println!("[错误]无法获取文件:{path_buf:?}的元数据");
                None
            }
        }
    } else {
        None
    }
}

fn m_search_add_dir(
    skip_symlink: bool,
    hash_map: &mut HashMap<String, FileInfo>,
    path_buf: PathBuf,
) -> Option<(u64, u32, u32)> {
    //目录
    if path_buf.is_symlink() {
        println!("[警告]目录：{path_buf:?}'，是符号链接")
    }
    let name = get_file_name(&path_buf);
    if let Some(name) = name {
        let name = String::from(name);
        //获取目录元数据
        match path_buf.metadata() {
            Ok(metadata) => {
                let mut files_list = HashMap::new();
                let modified_time = get_file_modified(&metadata);

                let (len, file_count, dir_count) =
                    m_search(skip_symlink, path_buf, &mut files_list);

                let file_info = FileInfo {
                    name: String::from(&name),
                    length: len,
                    modified_time,
                    file_kind: FileKind::Dir(Dir {
                        files_list,
                        file_count,
                        dir_count,
                    }),
                };
                hash_map.insert(name, file_info);
                Some((len, file_count, dir_count))
            }
            Err(_) => {
                print!("[错误]无法获取目录: {path_buf:?}的元数据");
                None
            }
        }
    } else {
        None
    }
}

fn m_search(
    skip_symlink: bool,
    path_buf: PathBuf,
    hash_map: &mut HashMap<String, FileInfo>,
) -> (u64, u32, u32) {
    let mut data_length = 0;
    let mut file_count = 0;
    let mut dir_count = 0;
    //获取文件列表
    for entry in match path_buf.read_dir() {
        Ok(rd) => rd,
        Err(err) => {
            println!("[错误]获取目录迭代器错误:{err}");
            return (0, 0, 0);
        }
    } {
        match entry {
            Ok(entry) => {
                let path_buf = entry.path();
                //println!("[消息]找到: '{path_buf:?}' ");
                if path_buf.is_symlink() && skip_symlink {
                    println!("[消息]已跳过符号链接:{path_buf:?}");
                    continue;
                } else if path_buf.is_file() {
                    if let Some(len) = m_search_add_file(hash_map, path_buf) {
                        data_length += len;
                    }
                    file_count += 1;
                } else if path_buf.is_dir() {
                    if let Some(re) = m_search_add_dir(skip_symlink, hash_map, path_buf) {
                        let (len, m_file_count, m_dir_count) = re;
                        data_length += len;
                        file_count += m_file_count;
                        dir_count += m_dir_count;
                    }
                    dir_count += 1;
                } else if path_buf.is_symlink() {
                    println!("[警告]符号链接 {path_buf:?} 已断。")
                } else {
                    println!("[错误] {path_buf:?} 无法访问");
                }
            }
            Err(err) => {
                println!("[错误]目录: {path_buf:?} 迭代器发生错误:{err}")
            }
        }
    }
    (data_length, file_count, dir_count)
}

pub fn search(path: &str, skip_symlink: bool) -> FilesList {
    let mut files_list = HashMap::new();

    let path_path = Path::new(&path);

    match path_path.try_exists() {
        Ok(true) => {
            if path_path.is_dir() {
                if path_path.is_symlink() {
                    println!(r#"[警告]设定的目录: "{path}" 是符号链接。"#);
                }
                let (data_length, file_count, dir_count) =
                    m_search(skip_symlink, path_path.to_path_buf(), &mut files_list);
                //返回值
                FilesList {
                    path: String::from(path),
                    data_length,
                    file_count,
                    dir_count,
                    files_list,
                }
            } else {
                panic!(r#"[致命错误]指定的目录 "{path}" ， 已存在，但是是文件，不是目录。"#)
            }
        }
        Ok(false) => {
            if path_path.is_symlink() {
                panic!(r#"[致命错误]指定的目录 "{path}" 是符号链接(symlink)，但链接已断。"#)
            } else {
                panic!(r#"[致命错误]指定的目录 "{path}" 不存在"#);
            }
        }
        Err(e) => {
            panic!("[致命错误]指定的目录不存在，或没有权限访问。 Error:{e}")
        }
    }
}

//将结果转换成json文本
pub fn data_to_json_json(files_list: &FilesList) -> Result<String> {
    serde_json::to_string_pretty(files_list)
}

//TEST===
static TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/ff/ok";
static TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/ff/err";

//OK===
#[test]
fn test_command_out_file_skip_symlink() {
    let out_dir_path = TEST_TEMP_OK_DIR_PATH;
    _ = fs::create_dir_all(out_dir_path);
    let mut out_file_path = out_dir_path.to_string();
    out_file_path.push_str("/test_ff_skip_symlink.json");
    _ = fs::remove_file(&out_file_path);
    //命令行参数处理
    let args: Vec<String> = env::args().collect();
    let args: Vec<String> = vec![
        args[0].clone(),
        String::from("ff"),
        String::from("/home/waterball/Downloads"),
        out_file_path.clone(),
        String::from("-s"),
    ];
    command(args.as_slice());
    _ = fs::remove_file(&out_file_path);
}

#[test]
fn test_command_out_file() {
    let out_dir_path = TEST_TEMP_OK_DIR_PATH;
    _ = fs::create_dir_all(out_dir_path);
    let mut out_file_path = out_dir_path.to_string();
    out_file_path.push_str("/test_ff.json");
    _ = fs::remove_file(&out_file_path);
    //命令行参数处理
    let args: Vec<String> = env::args().collect();
    let args: Vec<String> = vec![
        args[0].clone(),
        String::from("ff"),
        String::from("/home/waterball/Downloads"),
        out_file_path.clone(),
    ];
    command(args.as_slice());
    _ = fs::remove_file(&out_file_path);
}
#[test]
fn test_command_no_out_file() {
    //命令行参数处理
    let args: Vec<String> = env::args().collect();
    let args: Vec<String> = vec![
        args[0].clone(),
        String::from("ff"),
        String::from("/home/waterball/Downloads")
    ];
    command(args.as_slice());
}
