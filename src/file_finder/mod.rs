/*
开始时间:2026-02-08 22:37
 */

use std::collections::HashMap;
use std::fs::Metadata;
use std::io;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Result;
use tracing::{error, info, warn};

#[derive(Debug, Serialize, Deserialize)]
pub struct FilesList {
    //搜索路径
    path: String,
    //数据总大小
    data_length: u64,
    //文件数量，不包括目录
    file_count: u64,
    //目录数量
    dir_count: u64,
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

    pub fn file_count(&self) -> u64 {
        self.file_count
    }

    pub fn dir_count(&self) -> u64 {
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
    file_count: u64,
    //目录列表
    dir_count: u64,
} //文件夹独有
impl Dir {
    pub fn files_list(&self) -> &HashMap<String, FileInfo> {
        &self.files_list
    }

    pub fn file_count(&self) -> u64 {
        self.file_count
    }

    pub fn dir_count(&self) -> u64 {
        self.dir_count
    }
}

//文件类型
#[derive(Debug, Serialize, Deserialize)]
pub enum FileKind {
    File,
    Dir(Dir),
}

pub struct FileFinder;

impl FileFinder {
    fn get_file_name(path_buf: &PathBuf) -> Option<&str> {
        if let Some(name) = path_buf.file_name() { if let Some(name) = name.to_str() { Some(name) } else {
            warn!("无法将OsStr:{name:?} 转换成Str");
            None
        } } else {
            warn!("目录:（{path_buf:?}）无法获取文件名。");
            None
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

    fn m_search<'a>(
        &'a self,
        path: &Path,
        skip_symlink: bool,
        hash_map: &mut HashMap<String, FileInfo>,
        mut callback: Option<&'a mut (dyn FnMut(u64, u64) + 'a)>,
    ) -> MSearchReturn<'a> {
        let mut data_length = 0;
        let mut file_count = 0;
        let mut dir_count = 0;
        //获取文件列表
        for entry in match path.read_dir() {
            Ok(rd) => rd,
            Err(err) => {
                error!("获取目录迭代器错误:path:{path:?},err:{err}");
                return MSearchReturn {
                    callback,
                    add_length: 0,
                    add_file_count: 0,
                    add_dir_count: 0,
                };
            }
        }
            .flatten()
        {
            let path_buf = entry.path();
            //println!("[消息]找到: '{path_buf:?}' ");
            if path_buf.is_symlink() && skip_symlink {
                info!("已跳过符号链接:{path_buf:?}");
                continue;
            } else if path_buf.is_file() {
                file_count += 1;
                if let Some(ref mut cb) = callback {
                    cb(1, 0);
                }
                //文件
                let name = Self::get_file_name(&path_buf);
                if let Some(name) = name {
                    let name = String::from(name);
                    //获取文件元数据
                    match path_buf.metadata() {
                        Ok(metadata) => {
                            let len = metadata.len();
                            let modified_time = Self::get_file_modified(&metadata);
                            let file_info = FileInfo {
                                name: String::from(&name),
                                length: len,
                                modified_time,
                                file_kind: FileKind::File,
                            };
                            hash_map.insert(name, file_info);
                            data_length += len;
                        }
                        Err(_) => {
                            error!("无法获取文件:{path_buf:?}的元数据");
                        }
                    }
                }
            } else if path_buf.is_dir() {
                //目录
                //循环链接判断
                if path_buf.is_symlink() {
                    warn!("目录：{path_buf:?}'，是符号链接");
                    //链接循环检测
                    let link_path = path_buf.read_link();
                    if let Ok(link_path) = link_path
                        && let Some(link_path) = link_path.to_str()
                        && let Some(path) = path_buf.to_str()
                    {
                        //判断链接的目标路径是否为父路径
                        if path.starts_with(link_path)
                            || (link_path.starts_with('.') && link_path.ends_with('.'))
                        {
                            warn!(r#"检测到符号链接循环，已跳过:"{path}" 链接到 "{link_path}""#);
                            continue;
                        }
                    }
                }
                let name = Self::get_file_name(&path_buf);
                if let Some(name) = name {
                    let name = String::from(name);
                    //获取目录元数据
                    match path_buf.metadata() {
                        Ok(metadata) => {
                            dir_count += 1;
                            if let Some(ref mut cb) = callback {
                                cb(0, 1);
                            }
                            let mut files_list = HashMap::new();
                            let modified_time = Self::get_file_modified(&metadata);

                            let r = self.m_search(
                                path_buf.as_path(),
                                skip_symlink,
                                &mut files_list,
                                callback,
                            );
                            callback = r.callback;

                            let file_info = FileInfo {
                                name: String::from(&name),
                                length: r.add_length,
                                modified_time,
                                file_kind: FileKind::Dir(Dir {
                                    files_list,
                                    file_count: r.add_file_count,
                                    dir_count: r.add_dir_count,
                                }),
                            };
                            hash_map.insert(name, file_info);
                            data_length += r.add_length;
                            file_count += r.add_file_count;
                            dir_count += r.add_dir_count;
                        }
                        Err(_) => {
                            error!("无法获取目录: {path_buf:?}的元数据");
                        }
                    }
                }
            } else if path_buf.is_symlink() {
                warn!("符号链接 {path_buf:?} 已断。");
            } else {
                error!(" {path_buf:?} 无法访问");
            }
        }
        MSearchReturn {
            callback,
            add_length: data_length,
            add_file_count: file_count,
            add_dir_count: dir_count,
        }
    }

    pub fn search<'a>(
        &'a self,
        path: &Path,
        skip_symlink: bool,
        callback: Option<&'a mut dyn FnMut(u64, u64)>,
    ) -> io::Result<FilesList> {
        //判断是否为目录
        if path.is_dir() {
            let mut files_list = HashMap::new();
            let mut r = self.m_search(path, skip_symlink, &mut files_list, callback);
            if let Some(ref mut cb) = r.callback {
                cb(0, 0);
            }
            //返回值
            Ok(FilesList {
                path: path.to_str().unwrap().to_string(),
                data_length: r.add_length,
                file_count: r.add_file_count,
                dir_count: r.add_dir_count,
                files_list,
            })
        } else if path.is_file() {
            Err(Error::new(
                ErrorKind::NotADirectory,
                "提供的路径是文件不是目录",
            ))
        } else if path.is_symlink() {
            Err(Error::new(
                ErrorKind::NotADirectory,
                "提供的路径是符号链接，但链接已断",
            ))
        } else {
            Err(Error::new(
                ErrorKind::NotFound,
                "未找到目录，提供的路径不存在或拒绝访问",
            ))
        }
    }

    //将结果转换成json文本
    pub fn data_to_json_json(files_list: &FilesList) -> Result<String> {
        serde_json::to_string_pretty(files_list)
    }
}

struct MSearchReturn<'a> {
    callback: Option<&'a mut dyn FnMut(u64, u64)>,
    add_length: u64,
    add_file_count: u64,
    add_dir_count: u64,
}
