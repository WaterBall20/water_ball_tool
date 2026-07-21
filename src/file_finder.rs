//! 文件搜索模块 / File search module
//!
//! 使用多线程工作队列并行扫描目录树，收集文件元数据并构建层级结构。
//! Uses a multi-threaded work-queue to scan the directory tree in parallel,
//! collecting file metadata and building a hierarchical structure.
//!
//! # 架构 / Architecture
//! 分三个阶段 / Three phases:
//! 1. 工作线程从共享队列取目录、扫描条目、收集文件信息
//!    Workers pull directories from a shared queue, scan entries, collect file info
//! 2. 所有线程结束后，从扁平 HashMap 重建目录树
//!    After all threads finish, rebuild the directory tree from a flat HashMap
//! 3. 进度更新通过 mpsc channel 发送，每 50 个文件/10 个目录限频
//!    Progress updates sent via mpsc channel, rate-limited to every 50 files / 10 dirs

use std::collections::{HashMap, VecDeque};
use std::fs::Metadata;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::UNIX_EPOCH;
use std::{io, thread};

use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

// === 平台相关的 inode 标识 / Platform-specific inode key ===

/// 文件/目录的唯一标识，用于符号链接循环检测。
/// Unix: `(dev << 64) | ino`；Windows: `canonicalize()` 后的路径。
///
/// Unique file/directory identifier used for symlink cycle detection.
/// Unix: `(dev << 64) | ino`; Windows: canonicalized path via `canonicalize()`.
#[cfg(unix)]
type InodeKey = u128;

#[cfg(windows)]
type InodeKey = PathBuf;

/// 从 Metadata 提取 inode 标识（Unix）/ Extract inode key from Metadata (Unix)
#[cfg(unix)]
fn get_inode_key(_path: &Path, metadata: &Metadata) -> InodeKey {
    use std::os::unix::fs::MetadataExt;
    ((metadata.dev() as u128) << 64) | (metadata.ino() as u128)
}

/// 从规范路径获取标识（Windows）/ Get key from canonical path (Windows)
///
/// 使用 `canonicalize()` 解析符号链接的真实路径。
/// 如果解析失败则回退到原始路径。
///
/// 注意：Windows 上以 `PathBuf` 作为 `InodeKey`，不同路径名可能指向同一文件，
/// 因此可能无法检测所有符号链接循环。这是 Windows 平台固有的限制。
///
/// Uses `canonicalize()` to resolve symlink real path.
/// Falls back to the original path if resolution fails.
///
/// Note: On Windows, `PathBuf` is used as `InodeKey`; different path names may point
/// to the same file, so not all symlink cycles may be detected. This is an inherent
/// platform limitation.
#[cfg(windows)]
fn get_inode_key(path: &Path, _metadata: &Metadata) -> InodeKey {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

// === 工作队列项 / Work queue entry ===

/// 工作队列中的目录任务，携带从根到此目录经过的符号链接链。
///
/// 链记录了所有通过符号链接进入的目录的 `InodeKey`，
/// 用于在后续遇到符号链接时检测循环（同一线程内）。
///
/// A directory task in the work queue, carrying the symlink chain
/// from root to this directory. The chain records `InodeKey` of all
/// directories entered via symlinks, used to detect cycles when
/// encountering further symlinks (within the same thread).
#[derive(Clone)]
struct DirEntry {
    path: PathBuf,
    symlink_chain: Vec<InodeKey>,
}

/// 文件搜索的完整结果 / Complete file search result
///
/// 包含搜索路径下的所有文件和目录信息的树状结构。
/// Contains a tree structure of all file and directory information under the search path.
///
/// # 字段 / Fields
/// * `path` - 搜索的根路径 / Root path that was searched
/// * `data_length` - 所有文件的总大小（字节）/ Total size of all files (bytes)
/// * `file_count` - 文件总数（不含目录）/ Total number of files (excluding directories)
/// * `dir_count` - 目录总数 / Total number of directories
/// * `files_list` - 根目录下的直接子项 / Direct children of the root directory
#[derive(Debug, Serialize, Deserialize)]
pub struct FilesList {
    path: String,
    data_length: u64,
    file_count: u64,
    dir_count: u64,
    files_list: HashMap<String, FileInfo>,
}

impl FilesList {
    /// 返回搜索的根路径 / Returns the root search path
    #[must_use]
    pub fn file_path(&self) -> &str {
        &self.path
    }

    /// 返回所有文件的总字节数 / Returns total data size in bytes
    #[must_use]
    pub fn data_length(&self) -> u64 {
        self.data_length
    }

    /// 返回文件总数（不含目录）/ Returns total file count (excluding dirs)
    #[must_use]
    pub fn file_count(&self) -> u64 {
        self.file_count
    }

    /// 返回目录总数 / Returns total directory count
    #[must_use]
    pub fn dir_count(&self) -> u64 {
        self.dir_count
    }

    /// 返回根目录下的直接子项映射表 / Returns map of direct children under root
    ///
    /// key 是文件名, value 是文件/目录信息。
    /// Key is the file name, value is the file/directory info.
    #[must_use]
    pub fn files_list(&self) -> &HashMap<String, FileInfo> {
        &self.files_list
    }

    /// 将结果序列化为 JSON 字符串 / Serialize the result to a JSON string
    pub fn to_json_string(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}

/// 单个文件或目录的信息 / Information about a single file or directory
///
/// # 字段 / Fields
/// * `name` - 文件/目录名（不含路径）/ File/directory name (without path)
/// * `length` - 对于文件是实际大小，对于目录是所有子文件总大小
///   For files: actual size. For directories: total size of all descendant files
/// * `modified_time` - 最后修改时间（从 UNIX 纪元开始的毫秒数）/ Last modified time (ms since UNIX epoch)
/// * `file_kind` - 区分文件还是目录 / Distinguishes file from directory
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileInfo {
    name: String,
    length: u64,
    modified_time: u128,
    file_kind: FileKind,
}

impl FileInfo {
    /// 返回文件/目录名 / Returns the file/directory name
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 返回大小（文件=实际大小, 目录=子项总大小）/ Returns size (file=actual, dir=total of children)
    #[must_use]
    pub fn length(&self) -> u64 {
        self.length
    }

    /// 返回最后修改时间（毫秒）/ Returns last modified time in milliseconds
    #[must_use]
    pub fn modified_time(&self) -> u128 {
        self.modified_time
    }

    /// 返回文件类型（文件或目录）/ Returns the file kind (file or directory)
    #[must_use]
    pub fn file_kind(&self) -> &FileKind {
        &self.file_kind
    }
}

/// 目录专属信息 / Directory-specific information
///
/// # 字段 / Fields
/// * `files_list` - 该目录下的直接子项 / Direct children of this directory
/// * `file_count` - 该目录下所有后代文件总数（递归累计）/ Total file count of all descendants (cumulative)
/// * `dir_count` - 该目录下所有后代目录总数（递归累计）/ Total directory count of all descendants (cumulative)
///
/// 注意：`file_count` 和 `dir_count` 是包括所有子目录的累计值，不是仅直接子项。
/// Note: `file_count` and `dir_count` are cumulative across all subdirectories, not just direct children.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Dir {
    files_list: HashMap<String, FileInfo>,
    file_count: u64,
    dir_count: u64,
}

impl Dir {
    /// 返回该目录的直接子项 / Returns direct children of this directory
    #[must_use]
    pub fn files_list(&self) -> &HashMap<String, FileInfo> {
        &self.files_list
    }

    /// 返回所有后代文件累计数量 / Returns cumulative file count of all descendants
    #[must_use]
    pub fn file_count(&self) -> u64 {
        self.file_count
    }

    /// 返回所有后代目录累计数量 / Returns cumulative directory count of all descendants
    #[must_use]
    pub fn dir_count(&self) -> u64 {
        self.dir_count
    }
}

/// 条目类型 / Entry kind
///
/// `File` = 普通文件 / regular file
/// `Dir(Dir)` = 目录，包含子项信息 / directory, contains sub-item info
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum FileKind {
    /// 普通文件 / Regular file
    File,
    /// 目录 / Directory
    Dir(Dir),
}

/// 文件搜索器 / File search engine
///
/// 使用多线程工作队列并行扫描目录树，基于 inode 链检测符号链接循环。
/// Uses a multi-threaded work-queue to scan the directory tree in parallel,
/// with inode-chain-based symlink cycle detection.
///
/// # 使用示例 / Usage Example
/// ```ignore
/// use std::sync::mpsc;
/// let (tx, rx) = mpsc::channel();
/// let result = FileFinder.search(Path::new("./src"), true, tx, 8);
/// ```
pub struct FileFinder;

impl FileFinder {
    /// 从路径中提取文件名 / Extract file name from a path
    ///
    /// 将 OsStr 转换为 &str，如果文件名不存在或包含无效 Unicode 则返回 None。
    /// Converts OsStr to &str, returns None if no file name or invalid Unicode.
    fn get_file_name(path_buf: &Path) -> Option<&str> {
        path_buf.file_name().and_then(|n| n.to_str())
    }

    /// 获取文件的修改时间（毫秒）/ Get file modification time in milliseconds
    ///
    /// 返回自 UNIX 纪元以来的毫秒数。如果无法获取则返回 0。
    /// Returns milliseconds since UNIX epoch. Returns 0 if unavailable.
    fn get_file_modified(metadata: &Metadata) -> u128 {
        metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    /// 从扁平 Map 重建目录树 / Rebuild directory tree from flat HashMap
    ///
    /// 并行扫描阶段产生的是扁平结构（每个文件/目录以完整路径为 key）。
    /// 此函数将扁平结构还原为嵌套的 `FilesList` 树。
    ///
    /// The parallel scan phase produces a flat structure (each file/dir keyed by full path).
    /// This function converts the flat structure back into a nested `FilesList` tree.
    ///
    /// # 步骤 / Steps
    /// 1. 按父路径分组所有条目 / Group all entries by parent path
    /// 2. 从根开始递归构建 / Recursively build from root
    fn build_tree(flat: &HashMap<PathBuf, FileInfo>, root: &Path) -> FilesList {
        let mut by_parent: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for path in flat.keys() {
            if let Some(parent) = path.parent() {
                by_parent
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(path.clone());
            }
        }

        /// 递归构建函数 / Recursive build function
        ///
        /// 返回值为 (直接子项, 累计数据大小, 累计文件数, 累计目录数)
        /// Returns (direct children, cumulative data length, cumulative file count, cumulative dir count)
        fn build_recursive(
            flat: &HashMap<PathBuf, FileInfo>,
            by_parent: &HashMap<PathBuf, Vec<PathBuf>>,
            current: &Path,
        ) -> (HashMap<String, FileInfo>, u64, u64, u64) {
            let mut files = HashMap::new();
            let mut total_len = 0u64;
            let mut file_count = 0u64;
            let mut dir_count = 0u64;

            if let Some(children) = by_parent.get(current) {
                for child_path in children {
                    if let Some(info) = flat.get(child_path) {
                        let name = child_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string();

                        match info.file_kind() {
                            FileKind::File => {
                                total_len += info.length();
                                file_count += 1;
                                files.insert(name, info.clone());
                            }
                            FileKind::Dir(_) => {
                                let (sub_files, sub_data_len, sub_fc, sub_dc) =
                                    build_recursive(flat, by_parent, child_path);
                                dir_count += 1 + sub_dc;
                                file_count += sub_fc;
                                total_len += sub_data_len;
                                files.insert(
                                    name,
                                    FileInfo {
                                        name: info.name().to_string(),
                                        length: sub_data_len,
                                        modified_time: info.modified_time(),
                                        file_kind: FileKind::Dir(Dir {
                                            files_list: sub_files,
                                            file_count: sub_fc,
                                            dir_count: sub_dc,
                                        }),
                                    },
                                );
                            }
                        }
                    }
                }
            }

            (files, total_len, file_count, dir_count)
        }

        let (root_files, data_len, file_count, dir_count) = build_recursive(flat, &by_parent, root);

        FilesList {
            path: root.to_str().unwrap_or("").to_string(),
            data_length: data_len,
            file_count,
            dir_count,
            files_list: root_files,
        }
    }

    /// 处理符号链接条目 / Process a symlink entry
    ///
    /// 仅符号链接会触发循环检测。核心机制：
    /// 1. 读取链接目标，判断目标类型（目录/文件/断开）
    /// 2. 如果目标为目录：解析真实路径 → 提取 inode → 在链中查找
    ///    - 链中存在相同 inode → 循环！跳过
    ///    - 链中不存在 → 将 inode 附加到链上，推入工作队列
    /// 3. 如果目标为文件：直接处理（不检查循环——文件不能指向目录）
    ///
    /// Only symlinks trigger cycle detection. Core mechanism:
    /// 1. Read link target, determine target type (directory/file/broken)
    /// 2. If target is a directory: resolve real path → extract inode → look up in chain
    ///    - Same inode found in chain → cycle! skip
    ///    - Not in chain → append inode to chain, push to work queue
    /// 3. If target is a file: process directly (no cycle check — files cannot point to dirs)
    fn process_symlink(
        path_buf: &Path,
        skip_symlink: bool,
        chain: &[InodeKey],
        dir_queue: &Arc<Mutex<VecDeque<DirEntry>>>,
        results: &Arc<Mutex<HashMap<PathBuf, FileInfo>>>,
        pb: &Sender<(u64, u64)>,
        thread_file_count: &mut u64,
        thread_dir_count: &mut u64,
        condver: &Arc<Condvar>,
    ) {
        if skip_symlink {
            info!("已跳过符号链接:{path_buf:?}");
            return;
        }

        // 跟随符号链接判断实际类型 / Follow symlink to determine actual type
        if path_buf.is_dir() {
            *thread_dir_count += 1;
            if (*thread_dir_count).is_multiple_of(10) {
                pb.send((0, 10)).ok();
            }

            // 获取目标目录的元数据以提取 inode / Get target metadata to extract inode
            if let Ok(metadata) = path_buf.metadata() {
                let inode_key = get_inode_key(path_buf, &metadata);

                // inode 链查重：同一线程内，同一 inode 重复出现 = 循环
                // inode chain lookup: same inode appearing twice in one chain = cycle
                if chain.contains(&inode_key) {
                    warn!(r#"检测到符号链接循环，已跳过:"{}""#, path_buf.display());
                    return;
                }

                let modified_time = Self::get_file_modified(&metadata);
                let name = Self::get_file_name(path_buf).unwrap_or("").to_string();
                results.lock().unwrap().insert(
                    path_buf.to_path_buf(),
                    FileInfo {
                        name,
                        length: 0,
                        modified_time,
                        file_kind: FileKind::Dir(Dir {
                            files_list: HashMap::new(),
                            file_count: 0,
                            dir_count: 0,
                        }),
                    },
                );

                // 将新的 inode 附加到链上，子目录携带扩展后的链
                // Append new inode to chain; subdirectories carry the extended chain
                let mut new_chain = chain.to_vec();
                new_chain.push(inode_key);
                dir_queue.lock().unwrap().push_back(DirEntry {
                    path: path_buf.to_path_buf(),
                    symlink_chain: new_chain,
                });
                //唤起一个线程
                condver.notify_one();
            }
        } else if path_buf.is_file() {
            // 符号链接指向文件 / Symlink points to a file
            // 文件不会导致循环（文件不能包含目录），不需要 inode 检测
            Self::process_file(path_buf, results, pb, thread_file_count);
        } else {
            // 断开的符号链接 / Broken symlink
            warn!("符号链接 {path_buf:?} 已断。");
        }
    }

    /// 处理普通文件 / Process a regular file
    ///
    /// 获取文件的元数据（大小、修改时间），存入共享结果集。
    /// 每处理 50 个文件向进度通道发送一次更新。
    /// 注意：普通文件**不做**任何符号链接循环检测。
    ///
    /// Gets file metadata (size, modified time), stores in shared results.
    /// Sends progress update every 50 files.
    /// Note: regular files do NOT go through any symlink cycle detection.
    fn process_file(
        path_buf: &Path,
        results: &Arc<Mutex<HashMap<PathBuf, FileInfo>>>,
        pb: &Sender<(u64, u64)>,
        thread_file_count: &mut u64,
    ) {
        let file_metadata = match path_buf.metadata() {
            Ok(m) => m,
            Err(_) => {
                error!("无法获取文件:{path_buf:?}的元数据");
                return;
            }
        };

        if let Some(name) = Self::get_file_name(path_buf) {
            let len = file_metadata.len();
            let modified_time = Self::get_file_modified(&file_metadata);

            results.lock().unwrap().insert(
                path_buf.to_path_buf(),
                FileInfo {
                    name: name.to_string(),
                    length: len,
                    modified_time,
                    file_kind: FileKind::File,
                },
            );

            *thread_file_count += 1;
            if (*thread_file_count).is_multiple_of(50) {
                pb.send((50, 0)).ok();
            }
        }
    }

    /// 工作线程的主循环 / Main loop of a worker thread
    ///
    /// 不断从共享队列取出目录任务（包含路径和链），扫描其中的条目：
    /// - 普通文件 → 记录元数据（不检测循环）
    /// - 普通目录 → 插入结果 + 推回队列（不检测循环，链不变）
    /// - 符号链接 → 特殊处理（仅在符号链接→目录时检测 inode 循环）
    ///
    /// 当队列为空时线程退出。多个线程同时从队列取任务实现并行扫描。
    ///
    /// Continuously pulls directory tasks (path + chain) from the shared queue:
    /// - Regular files → record metadata (no cycle detection)
    /// - Regular directories → insert result + push back (no cycle detection, chain unchanged)
    /// - Symlinks → special handling (inode cycle detection only for symlink→dir)
    ///
    /// Thread exits when queue is empty. Multiple threads pull from queue for parallel scanning.
    fn run_worker(
        dir_queue: Arc<Mutex<VecDeque<DirEntry>>>,
        results: Arc<Mutex<HashMap<PathBuf, FileInfo>>>,
        pb: Sender<(u64, u64)>,
        skip_symlink: bool,
        running_count: Arc<Mutex<i32>>,
        condver: Arc<Condvar>,
    ) {
        fn running_count_add(running_count: Arc<Mutex<i32>>) -> i32 {
            let mut running_count = running_count.lock().unwrap();
            *running_count += 1;
            *running_count
        }
        fn running_count_sub(running_count: Arc<Mutex<i32>>) -> i32 {
            let mut running_count = running_count.lock().unwrap();
            *running_count -= 1;
            *running_count
        }
        let mut thread_file_count = 0u64;
        let mut thread_dir_count = 0u64;

        'worker: loop {
            //线程计数
            running_count_add(running_count.clone());
            let entry = {
                let mut queue = dir_queue.lock().unwrap();
                queue.pop_front()
            };

            let entry = match entry {
                Some(d) => d,
                None => {
                    //如果是空的
                    let mut dir_queue = dir_queue.lock().unwrap();
                    //当前线程计数
                    let this_running_count = running_count_sub(running_count.clone());
                    if this_running_count == 0 {
                        //如果没有其他在运行中的线程，则说明搜索结束，唤醒所有线程并退出
                        condver.notify_all();
                        break;
                    }
                    loop {
                        dir_queue = condver.wait(dir_queue).unwrap();
                        match dir_queue.pop_front() {
                            Some(d) => {
                                //添加线程计数
                                running_count_add(running_count.clone());
                                break d;
                            }
                            None => {
                                //如果队列是空的，且运行中的线程数为0
                                if *running_count.lock().unwrap() == 0 {
                                    break 'worker;
                                }
                            }
                        }
                    }
                }
            };

            let rd = match entry.path.read_dir() {
                Ok(rd) => rd,
                Err(err) => match err.kind() {
                    ErrorKind::PermissionDenied => {
                        error!(
                            r#"获取目录"{}"迭代器错误,err:{err:?}"#,
                            entry.path.display()
                        );
                        continue;
                    }
                    _ => {
                        error!(
                            r#"获取目录"{}"迭代器错误,err:{err:?}"#,
                            entry.path.display()
                        );
                        continue;
                    }
                },
            };

            for dir_entry in rd.flatten() {
                let path_buf = dir_entry.path();

                let metadata = match path_buf.symlink_metadata() {
                    Ok(m) => m,
                    Err(_) => {
                        error!("无法获取文件:{path_buf:?}的元数据");
                        continue;
                    }
                };

                let ft = metadata.file_type();

                if ft.is_symlink() {
                    Self::process_symlink(
                        &path_buf,
                        skip_symlink,
                        &entry.symlink_chain,
                        &dir_queue,
                        &results,
                        &pb,
                        &mut thread_file_count,
                        &mut thread_dir_count,
                        &condver,
                    );
                } else if ft.is_dir() {
                    // 普通目录：不检测循环，链不变
                    // Regular directory: no cycle detection, chain unchanged
                    thread_dir_count += 1;
                    if thread_dir_count.is_multiple_of(10) {
                        pb.send((0, 10)).ok();
                    }

                    let modified_time = Self::get_file_modified(&metadata);
                    let name = Self::get_file_name(&path_buf).unwrap_or("").to_string();
                    results.lock().unwrap().insert(
                        path_buf.clone(),
                        FileInfo {
                            name,
                            length: 0,
                            modified_time,
                            file_kind: FileKind::Dir(Dir {
                                files_list: HashMap::new(),
                                file_count: 0,
                                dir_count: 0,
                            }),
                        },
                    );
                    //添加队列
                    dir_queue.lock().unwrap().push_back(DirEntry {
                        path: path_buf,
                        symlink_chain: entry.symlink_chain.clone(),
                    });
                    //唤起一个线程
                    condver.notify_one();
                } else if ft.is_file() {
                    // 普通文件：不检测循环
                    // Regular file: no cycle detection
                    Self::process_file(&path_buf, &results, &pb, &mut thread_file_count);
                } else {
                    error!("{path_buf:?} 无法访问");
                }
            }
            running_count_sub(running_count.clone());
        }

        let remaining_files = thread_file_count % 50;
        if remaining_files > 0 {
            pb.send((remaining_files, 0)).ok();
        }
        let remaining_dirs = thread_dir_count % 10;
        if remaining_dirs > 0 {
            pb.send((0, remaining_dirs)).ok();
        }
    }

    /// 搜索目录 / Search a directory
    ///
    /// 使用多线程并行扫描目录树，返回包含所有文件和子目录信息的 `FilesList`。
    /// 符号链接循环通过 inode 链机制检测（每个线程独立维护）。
    ///
    /// Uses multi-threaded parallel scanning to return a `FilesList` with all file and subdirectory info.
    /// Symlink cycles are detected via per-thread inode chain mechanism.
    ///
    /// # 参数 / Parameters
    /// * `path` - 要搜索的目录路径 / Directory path to search
    /// * `skip_symlink` - 是否跳过符号链接 / Whether to skip symlinks
    /// * `pb` - 进度更新发送器，发送 (文件增量, 目录增量) / Progress sender, sends (file_delta, dir_delta)
    /// * `max_thread_count` - 最大工作线程数 / Maximum number of worker threads
    ///
    /// # 返回值 / Returns
    /// * `Ok(FilesList)` - 搜索成功 / Search succeeded
    /// * `Err` - 路径不存在、不是目录等情况 / Path not found, not a directory, etc.
    ///
    /// # 符号链接循环检测 / Symlink cycle detection
    ///
    /// 每个工作线程维护一个 inode 链。当遇到符号链接指向目录时：
    /// 1. 提取目标目录的 `(dev, ino)` 标识
    /// 2. 在当前线程的链中查找
    /// 3. 存在 = 循环 → 跳过；不存在 → 附加到链，继续扫描
    ///
    /// 普通文件和目录**不做**任何循环检测。
    ///
    /// Each worker maintains an inode chain. When encountering a symlink to a directory:
    /// 1. Extract target directory's `(dev, ino)` key
    /// 2. Look up in the thread's chain
    /// 3. Found = cycle → skip; not found → append to chain, continue scanning
    ///
    /// Regular files and directories do NOT go through any cycle detection.
    pub fn search(
        &self,
        path: &Path,
        skip_symlink: bool,
        pb: Sender<(u64, u64)>,
        max_thread_count: usize,
    ) -> io::Result<FilesList> {
        if !path.is_dir() {
            if path.is_file() {
                return Err(Error::new(
                    ErrorKind::NotADirectory,
                    "提供的路径是文件不是目录",
                ));
            } else if path.is_symlink() {
                return Err(Error::new(
                    ErrorKind::NotADirectory,
                    "提供的路径是符号链接，但链接已断",
                ));
            }
            return Err(Error::new(
                ErrorKind::NotFound,
                "未找到目录，提供的路径不存在或拒绝访问",
            ));
        }

        let num_threads = max_thread_count.max(1);

        // 工作队列：存放待扫描的目录任务（路径 + inode 链）
        // Work queue: holds directory tasks to scan (path + inode chain)
        let dir_queue = Arc::new(Mutex::new(VecDeque::<DirEntry>::new()));
        dir_queue.lock().unwrap().push_back(DirEntry {
            path: path.to_path_buf(),
            symlink_chain: Vec::new(), // 根目录无符号链接链 / root has no symlink chain
        });

        //搜索结果
        let results = Arc::new(Mutex::new(HashMap::<PathBuf, FileInfo>::new())); //线程句柄
        //线程工作计数
        let running_count = Arc::new(Mutex::new(0));
        //控制线程的条件变量
        let condver = Arc::new(Condvar::new());
        //线程池
        let mut handles = Vec::with_capacity(num_threads);
        //线程接收器
        for _ in 0..num_threads {
            let dir_queue = Arc::clone(&dir_queue);
            let results = Arc::clone(&results);
            let condver = condver.clone();
            let running_count = running_count.clone();

            let pb = pb.clone();

            let handle = thread::spawn(move || {
                Self::run_worker(dir_queue, results, pb, skip_symlink, running_count, condver);
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let results = Arc::try_unwrap(results)
            .expect("results Arc still has references")
            .into_inner()
            .unwrap();

        Ok(Self::build_tree(&results, path))
    }
}

#[cfg(test)]
mod test;
