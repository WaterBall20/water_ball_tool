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
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use std::{io, thread};

use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

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
/// 使用多线程工作队列并行扫描目录树。
/// Uses a multi-threaded work-queue to scan the directory tree in parallel.
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
        // 第一步 / Step 1: 按父目录分组 / Group by parent directory
        // 将 "路径 -> FileInfo" 转换为 "父路径 -> [子路径列表]"
        // Transform "path -> FileInfo" into "parent -> [child paths]"
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
            let mut total_len = 0u64; // 累计数据大小 / cumulative data size
            let mut file_count = 0u64; // 累计文件数 / cumulative file count
            let mut dir_count = 0u64; // 累计目录数 / cumulative dir count

            // 获取当前目录的所有直接子项 / Get all direct children of current directory
            if let Some(children) = by_parent.get(current) {
                for child_path in children {
                    if let Some(info) = flat.get(child_path) {
                        // 提取文件名（不含路径）/ Extract file name (without path)
                        let name = child_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string();

                        match info.file_kind() {
                            FileKind::File => {
                                // 普通文件：直接累加大小和计数
                                // Regular file: directly add size and count
                                total_len += info.length();
                                file_count += 1;
                                files.insert(name, info.clone());
                            }
                            FileKind::Dir(_) => {
                                // 目录：递归处理子目录
                                // Directory: recursively process subdirectory
                                let (sub_files, sub_data_len, sub_fc, sub_dc) =
                                    build_recursive(flat, by_parent, child_path);
                                // 目录计数 = 自身(1) + 子目录累计
                                // Dir count = self(1) + sub-directories cumulative
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

        // 第二步 / Step 2: 从根开始递归构建 / Recursively build from root
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
    /// 符号链接需要特殊处理：
    /// 1. 如果开启了跳过标志则跳过
    /// 2. 检测循环链接（如 a -> b -> a）
    /// 3. 根据链接目标类型（目录/文件）分别处理
    ///
    /// Symlinks require special handling:
    /// 1. Skip if skip flag is on
    /// 2. Detect circular links (e.g. a -> b -> a)
    /// 3. Handle differently based on target type (directory/file)
    fn process_symlink(
        path_buf: &Path,
        skip_symlink: bool,
        dir_queue: &Arc<Mutex<VecDeque<PathBuf>>>,
        results: &Arc<Mutex<HashMap<PathBuf, FileInfo>>>,
        pb: &Sender<(u64, u64)>,
        thread_file_count: &mut u64,
        thread_dir_count: &mut u64,
    ) {
        if skip_symlink {
            info!("已跳过符号链接:{path_buf:?}");
            return;
        }

        // 循环链接检测 / Circular link detection
        // 使用两种简单方法检测：
        //   1) 链接目标路径是当前路径的前缀 → 链接指向上级
        //   2) 链接目标以 '.' 开头和结尾 → 可能是循环的相对路径
        // Uses two simple detection methods:
        //   1) Link target is a prefix of current path → link points to ancestor
        //   2) Link target starts and ends with '.' → possibly circular relative path
        if let Ok(link_path) = path_buf.read_link()
            && let (Some(link_str), Some(path_str)) = (link_path.to_str(), path_buf.to_str())
            && (path_str.starts_with(link_str)
                || (link_str.starts_with('.') && link_str.ends_with('.')))
        {
            warn!(r#"检测到符号链接循环，已跳过:"{path_str}" 链接到 "{link_str}""#);
            return;
        }

        // 跟随符号链接判断实际类型 / Follow symlink to determine actual type
        // is_dir() 和 is_file() 会跟随符号链接（与 symlink_metadata() 不同）
        // is_dir() and is_file() follow symlinks (unlike symlink_metadata())
        if path_buf.is_dir() {
            *thread_dir_count += 1;
            if (*thread_dir_count).is_multiple_of(10) {
                pb.send((0, 10)).ok();
            }
            // 获取目标目录的元数据 / Get target directory metadata
            if let Ok(metadata) = path_buf.metadata() {
                let modified_time = Self::get_file_modified(&metadata);
                let name = Self::get_file_name(path_buf).unwrap_or("").to_string();
                // 插入占位目录信息，count/length 稍后在 build_tree 中填充
                // Insert placeholder directory info, count/length filled later in build_tree
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
            }
            // 将目录加入工作队列，让工作线程进一步扫描
            // Add directory to work queue for further scanning by workers
            dir_queue.lock().unwrap().push_back(path_buf.to_path_buf());
        } else if path_buf.is_file() {
            // 符号链接指向文件 / Symlink points to a file
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
    ///
    /// Gets file metadata (size, modified time), stores in shared results.
    /// Sends progress update every 50 files.
    fn process_file(
        path_buf: &Path,
        results: &Arc<Mutex<HashMap<PathBuf, FileInfo>>>,
        pb: &Sender<(u64, u64)>,
        thread_file_count: &mut u64,
    ) {
        // 获取完整元数据（跟随符号链接）/ Get full metadata (follows symlinks)
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

            // 将文件信息存入共享 HashMap
            // Store file info in the shared HashMap
            results.lock().unwrap().insert(
                path_buf.to_path_buf(),
                FileInfo {
                    name: name.to_string(),
                    length: len,
                    modified_time,
                    file_kind: FileKind::File,
                },
            );

            // 进度更新：每 50 个文件发送一次 (50, 0) 表示新增 50 个文件
            // Progress: send (50, 0) every 50 files, meaning 50 new files found
            *thread_file_count += 1;
            if (*thread_file_count).is_multiple_of(50) {
                pb.send((50, 0)).ok();
            }
        }
    }

    /// 工作线程的主循环 / Main loop of a worker thread
    ///
    /// 不断从共享队列取出目录，扫描其中的条目：
    /// - 普通文件 → 记录元数据
    /// - 目录 → 插入结果 + 推回队列
    /// - 符号链接 → 特殊处理
    ///
    /// 当队列为空时线程退出。多个线程同时从队列取任务实现并行扫描。
    ///
    /// Continuously pulls directories from the shared queue and scans entries:
    /// - Regular files → record metadata
    /// - Directories → insert into results + push back to queue
    /// - Symlinks → special handling
    ///
    /// Thread exits when queue is empty. Multiple threads pull from queue for parallel scanning.
    fn run_worker(
        dir_queue: Arc<Mutex<VecDeque<PathBuf>>>,
        results: Arc<Mutex<HashMap<PathBuf, FileInfo>>>,
        pb: Sender<(u64, u64)>,
        skip_symlink: bool,
    ) {
        // 每个线程统计自己的进度，用于限频发送
        // Each thread tracks its own progress for rate-limited sending
        let mut thread_file_count = 0u64;
        let mut thread_dir_count = 0u64;

        loop {
            // 从队列头部取出一个目录（加锁-取目录-解锁）
            // Pop a directory from the front of the queue (lock-pop-unlock)
            //
            // 用大括号限定 MutexGuard 的生命周期，确保在扫描目录前释放锁。
            // The braces limit the MutexGuard's lifetime to release the lock before scanning.
            let dir = {
                let mut queue = dir_queue.lock().unwrap();
                queue.pop_front()
            };

            // 队列为空 → 没有更多工作，线程退出
            // Queue is empty → no more work, thread exits
            let dir = match dir {
                Some(d) => d,
                None => break,
            };

            // 读取目录条目 / Read directory entries
            let rd = match dir.read_dir() {
                Ok(rd) => rd,
                Err(err) => match err.kind() {
                    // 权限不足 → 跳过这个目录 / Permission denied → skip this directory
                    ErrorKind::PermissionDenied => {
                        error!(r#"获取目录"{}"迭代器错误,err:{err:?}"#, dir.display());
                        continue;
                    }
                    // 其他错误也跳过 / Other errors also skipped
                    _ => {
                        error!(r#"获取目录"{}"迭代器错误,err:{err:?}"#, dir.display());
                        continue;
                    }
                },
            };

            // 遍历目录中的每个条目 / Iterate each entry in the directory
            for entry in rd.flatten() {
                let path_buf = entry.path();

                // 先使用 symlink_metadata() 获取文件类型
                // symlink_metadata() 不跟随符号链接，可以准确判断是否为符号链接
                // 相比之下 is_file()/is_dir() 会跟随符号链接，无法区分原始类型
                //
                // Use symlink_metadata() first to get file type
                // symlink_metadata() does NOT follow symlinks, so we can accurately
                // determine if it's a symlink. is_file()/is_dir() follow symlinks
                // and would misidentify the original type.
                let metadata = match path_buf.symlink_metadata() {
                    Ok(m) => m,
                    Err(_) => {
                        error!("无法获取文件:{path_buf:?}的元数据");
                        continue;
                    }
                };

                let ft = metadata.file_type();

                if ft.is_symlink() {
                    // 符号链接 → 特殊处理 / Symlink → special handling
                    Self::process_symlink(
                        &path_buf,
                        skip_symlink,
                        &dir_queue,
                        &results,
                        &pb,
                        &mut thread_file_count,
                        &mut thread_dir_count,
                    );
                } else if ft.is_dir() {
                    // 真实目录 / Real directory
                    thread_dir_count += 1;
                    // 每 10 个目录发送一次进度 / Send progress every 10 dirs
                    if thread_dir_count.is_multiple_of(10) {
                        pb.send((0, 10)).ok();
                    }

                    let modified_time = Self::get_file_modified(&metadata);
                    let name = Self::get_file_name(&path_buf).unwrap_or("").to_string();
                    // 插入占位目录信息（length/file_count/dir_count 稍后填充）
                    // Insert placeholder directory info (length/counts filled later)
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
                    // 将目录推回队列，让工作线程继续扫描
                    // Push directory back to queue for further scanning
                    dir_queue.lock().unwrap().push_back(path_buf);
                } else if ft.is_file() {
                    // 真实文件 / Real file
                    Self::process_file(&path_buf, &results, &pb, &mut thread_file_count);
                } else {
                    // 无法识别的类型 / Unknown type
                    error!("{path_buf:?} 无法访问");
                }
            }
        }

        // 线程退出前补发剩余进度 / Send remaining progress before thread exits
        // 例如处理了 137 个文件，会在 50,100 时各发一次，剩下 37 个在这里补发
        // e.g. processed 137 files: sent at 50 and 100, remaining 37 sent here
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
    /// Uses multi-threaded parallel scanning to return a `FilesList` with all file and subdirectory info.
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
    /// # 工作流程 / How it works
    ///
    /// ```text
    /// [主线程]                    [工作线程1]     [工作线程2]     ...
    ///    |                            |              |
    ///    |-- 初始化队列(放入根路径)     |              |
    ///    |-- 启动N个线程               |              |
    ///    |   |                        |              |
    ///    |   |   ┌────────────────────┘              |
    ///    |   |   |  从队列取目录 扫描条目              |
    ///    |   |   |  文件→保存结果  目录→推回队列       |
    ///    |   |   |────────────────────────────────────┘
    ///    |   |   |        (重复直到队列空)
    ///    |   |   |
    ///    |-- 等待所有线程完成
    ///    |-- 从扁平 HashMap 重建树结构
    ///    |-- 返回 FilesList
    /// ```
    pub fn search(
        &self,
        path: &Path,
        skip_symlink: bool,
        pb: Sender<(u64, u64)>,
        max_thread_count: usize,
    ) -> io::Result<FilesList> {
        // 验证输入路径 / Validate input path
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

        // 确保至少 1 个线程 / Ensure at least 1 thread
        let num_threads = max_thread_count.max(1);

        // === 并行扫描阶段 / Parallel scan phase ===

        // 工作队列：存放待扫描的目录路径
        // Work queue: holds directory paths to be scanned
        // Arc = 多线程共享所有权, Mutex = 互斥访问（同时只有一个线程能修改）
        // Arc = shared ownership across threads, Mutex = mutual exclusion (only one thread can modify at a time)
        let dir_queue = Arc::new(Mutex::new(VecDeque::<PathBuf>::new()));
        dir_queue.lock().unwrap().push_back(path.to_path_buf());

        // 结果集：扁平存储，key 为完整路径，value 为文件/目录信息
        // Results: flat storage, key = full path, value = file/dir info
        let results = Arc::new(Mutex::new(HashMap::<PathBuf, FileInfo>::new()));

        // 启动工作线程 / Launch worker threads
        let mut handles = Vec::with_capacity(num_threads);

        for _ in 0..num_threads {
            // 对每个线程克隆 Arc（引用计数+1）和 Sender
            // For each thread, clone Arc (increment ref count) and Sender
            let dir_queue = Arc::clone(&dir_queue);
            let results = Arc::clone(&results);
            let pb = pb.clone();

            // thread::spawn 启动一个新线程
            // move 关键字将克隆的变量所有权移入线程闭包
            // move keyword transfers ownership of cloned vars into the thread closure
            let handle = thread::spawn(move || {
                Self::run_worker(dir_queue, results, pb, skip_symlink);
            });

            handles.push(handle);
        }

        // 等待所有工作线程完成 / Wait for all worker threads to finish
        // join() 会阻塞当前线程直到对应线程结束
        // join() blocks the current thread until the corresponding thread finishes
        for handle in handles {
            handle.join().unwrap();
        }

        // 取回结果的所有权（此时所有 Arc 克隆已销毁，引用计数=1）
        // Reclaim ownership of results (all Arc clones are dropped, ref count = 1)
        let results = Arc::try_unwrap(results)
            .expect("results Arc still has references")
            .into_inner()
            .unwrap();

        // === 树构建阶段 / Tree building phase ===
        // 将扁平结构转换为嵌套树 / Convert flat structure to nested tree
        Ok(Self::build_tree(&results, path))
    }
}
