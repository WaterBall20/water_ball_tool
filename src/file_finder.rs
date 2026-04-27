/*
开始时间:2026-02-08 22:37
 */

use std::collections::HashMap;
use std::fs::Metadata;
use std::{io, thread};
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
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
    #[must_use]
    pub fn file_path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn data_length(&self) -> u64 {
        self.data_length
    }

    #[must_use]
    pub fn file_count(&self) -> u64 {
        self.file_count
    }

    #[must_use]
    pub fn dir_count(&self) -> u64 {
        self.dir_count
    }

    #[must_use]
    pub fn files_list(&self) -> &HashMap<String, FileInfo> {
        &self.files_list
    }

    pub fn to_json_string(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
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
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn length(&self) -> u64 {
        self.length
    }

    #[must_use]
    pub fn modified_time(&self) -> u128 {
        self.modified_time
    }

    #[must_use]
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
    #[must_use]
    pub fn files_list(&self) -> &HashMap<String, FileInfo> {
        &self.files_list
    }

    #[must_use]
    pub fn file_count(&self) -> u64 {
        self.file_count
    }

    #[must_use]
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
        if let Some(name) = path_buf.file_name() {
            Option::from(name.to_str().expect("无法将OsStr转换成Str"))
        } else {
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

    fn m_search(
        path: &Path,
        skip_symlink: bool,
        //父目录发送对象
        s_tx: Sender<(FileInfo, MSearchReturn)>,
        //进度发送对象
        pb: Sender<(u64, u64)>,
    ) {
        let mut data_length = 0;
        let mut file_count = 0;
        let mut dir_count = 0;

        //获取文件列表
        for entry in match path.read_dir() {
            Ok(rd) => rd,
            Err(err) => match err.kind() {
                ErrorKind::PermissionDenied => {
                    error!(r#"获取目录"{}"迭代器错误,err:{err:?}"#, path.display());
                    return;
                }
                _ => {
                    panic!(r#"获取目录"{}"迭代器错误,err:{err:?}"#, path.display());
                }
            },
        }
            .flatten()
        {
            let path_buf = entry.path();
            //println!("[消息]找到: '{path_buf:?}' ");
            if path_buf.is_symlink() && skip_symlink {
                info!("已跳过符号链接:{path_buf:?}");
            } else if path_buf.is_file() {
                file_count += 1;
                //更新进度条d
                pb.send((1, 0)).expect("发送进度更新失败");
                //文件
                let name = Self::get_file_name(&path_buf);
                if let Some(name) = name {
                    let name = String::from(name);
                    //获取文件元数据
                    if let Ok(metadata) = path_buf.metadata() {
                        let len = metadata.len();
                        let modified_time = Self::get_file_modified(&metadata);
                        let file_info = FileInfo {
                            name: String::from(&name),
                            length: len,
                            modified_time,
                            file_kind: FileKind::File,
                        };
                        s_tx.send((
                            file_info,
                            MSearchReturn {
                                add_length: len,
                                add_file_count: 1,
                                add_dir_count: 0,
                            },
                        ))
                            .unwrap_or_else(|e| panic!("多线程发送错误，文件:{path_buf:?}, err:{e}"));
                        data_length += len;
                    } else {
                        error!("无法获取文件:{path_buf:?}的元数据");
                    }
                }
            } else if path_buf.is_dir() {
                //目录
                let pb = pb.clone();
                let s_tx = s_tx.clone();
                let t_path_buf = path_buf.clone();
                thread::spawn(move || {
                    let path_buf = t_path_buf;
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
                                warn!(
                                    r#"检测到符号链接循环，已跳过:"{path}" 链接到 "{link_path}""#
                                );
                            }
                        }
                    }
                    let name = Self::get_file_name(&path_buf);
                    if let Some(name) = name {
                        let name = String::from(name);
                        //获取目录元数据
                        if let Ok(metadata) = path_buf.metadata() {
                            dir_count += 1;
                            pb.send((0, 1)).expect("发送进度失败");
                            let (tx, rx) = mpsc::channel();
                            let mut files_list = HashMap::new();
                            let modified_time = Self::get_file_modified(&metadata);
                            let mut r = MSearchReturn {
                                add_length: 0,
                                add_file_count: 0,
                                add_dir_count: 0,
                            };
                            Self::m_search(
                                path_buf.as_path(),
                                skip_symlink,
                                tx.clone(),
                                pb.clone(),
                            );
                            drop(tx);
                            for (info, sr) in rx {
                                files_list.insert(info.name.clone(), info);
                                r.add_length += sr.add_length;
                                r.add_file_count += sr.add_file_count;
                                r.add_dir_count += sr.add_dir_count;
                            }
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
                            data_length += r.add_length;
                            file_count += r.add_file_count;
                            dir_count += r.add_dir_count;
                            s_tx.send((
                                file_info,
                                MSearchReturn {
                                    add_length: data_length,
                                    add_file_count: file_count,
                                    add_dir_count: dir_count,
                                },
                            ))
                                .expect(&format!("多线程发送失败，目录: {path_buf:?}"));
                        } else {
                            error!("无法获取目录: {path_buf:?}的元数据");
                        }
                    }
                });
            } else if path_buf.is_symlink() {
                warn!("符号链接 {path_buf:?} 已断。");
            } else {
                error!("{path_buf:?} 无法访问");
            }
        }
    }

    pub fn search(
        &self,
        path: &Path,
        skip_symlink: bool,
        pb: Sender<(u64, u64)>,
    ) -> io::Result<FilesList> {
        //判断是否为目录
        if path.is_dir() {
            let mut files_list = HashMap::new();
            //多线程通道
            let (tx, rx) = mpsc::channel();
            let mut r = MSearchReturn {
                add_length: 0,
                add_dir_count: 0,
                add_file_count: 0,
            };
            Self::m_search(path, skip_symlink, tx.clone(), pb);
            drop(tx);
            for (file, sr) in rx {
                files_list.insert(file.name.clone(), file);
                r.add_length += sr.add_length;
                r.add_file_count += sr.add_file_count;
                r.add_dir_count += sr.add_dir_count;
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
}

struct MSearchReturn {
    add_length: u64,
    add_file_count: u64,
    add_dir_count: u64,
}
