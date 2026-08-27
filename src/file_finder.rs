//! 文件搜索模块 / File search module
//!
//! 使用多线程工作队列并行扫描目录树，收集文件元数据并构建层级结构。
//! Uses a multi-threaded work-queue to scan the directory tree in parallel,
//! collecting file metadata and building a hierarchical structure.
//!
//! # 架构 / Architecture
//! 1. 工作线程从共享队列取目录、扫描条目、收集文件信息，
//!    同时通过 mpsc channel 发送进度更新（每 50 个文件/10 个目录限频）
//!    Workers pull directories from a shared queue, scan entries, collect file info,
//!    and send progress updates via mpsc channel (rate-limited every 50 files / 10 dirs)
//! 2. 所有工作线程退出后 drop pb 关闭进度通道，从扁平 HashMap 重建目录树
//!    After all workers exit, pb is dropped to close the progress channel,
//!    then rebuild the directory tree from the flat HashMap
//! 3. 目录树重建使用显式栈后序遍历（Pre/Post 状态机），避免递归导致的栈溢出风险
//!    Tree rebuilding uses explicit stack-based post-order traversal (Pre/Post state machine),
//!    avoiding stack overflow risk on deeply nested directory hierarchies

use std::collections::{HashMap, VecDeque};
use std::fs::Metadata;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::UNIX_EPOCH;
use std::{io, thread};

use serde::{Deserialize, Serialize};

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
    (u128::from(metadata.dev()) << 64) | u128::from(metadata.ino())
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
/// * `list` - 根目录下的直接子项 / Direct children of the root directory
#[derive(Debug, Serialize, Deserialize)]
pub struct FilesList {
    path: String,
    data_length: u64,
    file_count: u64,
    dir_count: u64,
    list: HashMap<String, FileInfo>,
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
        &self.list
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

    /// 创建一个新的 `FileInfo` 实例（搜索器内部使用）。
    /// Creates a new `FileInfo` instance (used internally by the search engine).
    pub fn new(name: String, length: u64, modified_time: u128, file_kind: FileKind) -> Self {
        Self {
            name,
            length,
            modified_time,
            file_kind,
        }
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

// === 搜索警告和事件类型 / Search warning and event types ===

/// 搜索警告类型 / Search warning type
#[derive(Debug, Clone)]
pub enum SearchWarningType {
    /// 权限拒绝（无权限读取目录）/ Permission denied
    PermissionDenied,
    /// 读取目录失败 / Directory read error
    ReadDirError(String),
    /// 获取文件元数据失败 / Failed to get file metadata
    MetadataError,
    /// 断开的符号链接 / Broken symlink
    BrokenSymlink,
    /// 无法访问的条目 / Inaccessible entry
    InaccessibleEntry,
    /// 线程错误 / Thread error
    ThreadError(String),
}

/// 搜索警告 / Search warning
#[derive(Debug, Clone)]
pub struct SearchWarning {
    /// 相关路径 / Related path
    pub path: PathBuf,
    /// 警告类型 / Warning type
    pub warning_type: SearchWarningType,
}

/// 搜索事件（用于流式搜索）/ Search event (for streaming search)
///
/// 流式搜索模式下，通过 Receiver 发送此枚举：
/// - `Entry`：发现的文件/目录
/// - `Warning`：非致命错误警告
///
/// In streaming mode, sent via Receiver:
/// - `Entry`: discovered file/directory
/// - `Warning`: non-fatal error warning
#[derive(Debug)]
pub enum SearchEvent {
    /// 发现的条目 / Discovered entry
    Entry(PathBuf, FileInfo),
    /// 搜索警告 / Search warning
    Warning(SearchWarning),
}

/// 搜索结果 / Search result
///
/// 包含搜索产生的 `FilesList` 树结构和搜索过程中产生的非致命警告列表。
/// Contains the `FilesList` tree and a list of non-fatal warnings from the search.
#[derive(Debug)]
pub struct SearchResult {
    files_list: FilesList,
    warnings: Vec<SearchWarning>,
}

impl SearchResult {
    /// 返回搜索结果的树结构 / Returns the search result tree
    #[must_use]
    pub fn files_list(&self) -> &FilesList {
        &self.files_list
    }

    /// 返回搜索过程中产生的非致命警告 / Returns non-fatal warnings from the search
    #[must_use]
    pub fn warnings(&self) -> &[SearchWarning] {
        &self.warnings
    }

    /// 消耗自身，返回内部的 `FilesList`（丢弃警告）/ Consumes self, returns `FilesList` (drops warnings)
    #[must_use]
    pub fn into_files_list(self) -> FilesList {
        self.files_list
    }

    /// 消耗自身，返回 `(FilesList, Vec<SearchWarning>)` / Consumes self, returns `(FilesList, Vec<SearchWarning>)`
    #[must_use]
    pub fn into_parts(self) -> (FilesList, Vec<SearchWarning>) {
        (self.files_list, self.warnings)
    }
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
    pub fn get_file_modified(metadata: &Metadata) -> u128 {
        metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis())
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
    /// 2. 用显式栈进行后序遍历（避免递归栈溢出）/ Post-order traversal with explicit stack (avoid recursion stack overflow)
    pub fn build_tree(flat: &HashMap<PathBuf, FileInfo>, root: &Path) -> FilesList {
        // 栈帧状态 / Stack frame state
        enum St {
            Pre,  // 首次访问，需先处理子目录 / First visit, needs to process subdirs first
            Post, // 子目录已处理，可以合并结果 / Subdirs done, can merge results
        }

        // 显式栈帧 / Explicit stack frame
        struct Fr {
            st: St,
            path: PathBuf,
            files: HashMap<String, FileInfo>,
            total_len: u64,
            file_count: u64,
            dir_count: u64,
        }
        
        let mut by_parent: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for path in flat.keys() {
            if let Some(parent) = path.parent() {
                by_parent
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(path.clone());
            }
        }

    
        // 初始栈：根目录 / Initial stack: root directory
        let mut stack = vec![Fr {
            st: St::Pre,
            path: root.to_path_buf(),
            files: HashMap::new(),
            total_len: 0,
            file_count: 0,
            dir_count: 0,
        }];

        while let Some(frame) = stack.last_mut() {
            match frame.st {
                St::Pre => {
                    // 标记为 Post，然后收集子目录路径（避免在持有栈引用时执行 push）
                    // Mark as Post, then collect child dir paths (avoid pushing while holding stack reference)
                    frame.st = St::Post;
                    let mut child_dirs: Vec<PathBuf> = Vec::new();
                    if let Some(children) = by_parent.get(&frame.path) {
                        for child in children {
                            if let Some(info) = flat.get(child) {
                                match info.file_kind() {
                                    FileKind::File => {
                                        let name = child
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("")
                                            .to_string();
                                        frame.total_len += info.length();
                                        frame.file_count += 1;
                                        frame.files.insert(name, info.clone());
                                    }
                                    FileKind::Dir(_) => {
                                        child_dirs.push(child.clone());
                                    }
                                }
                            }
                        }
                    }
                    // 批量推送子目录帧 / Batch-push child directory frames
                    for dir in child_dirs {
                        stack.push(Fr {
                            st: St::Pre,
                            path: dir,
                            files: HashMap::new(),
                            total_len: 0,
                            file_count: 0,
                            dir_count: 0,
                        });
                    }
                }
                St::Post => {
                    // 弹出已处理的帧（栈一定非空，因为 last_mut 刚返回了 Some）
                    // Pop the processed frame (stack is guaranteed non-empty)
                    let fr = stack.pop().unwrap_or_else(|| unreachable!());

                    // 如果是根帧，返回最终结果 / Root frame → return final result
                    if stack.is_empty() {
                        return FilesList {
                            path: root.to_str().unwrap_or("").to_string(),
                            data_length: fr.total_len,
                            file_count: fr.file_count,
                            dir_count: fr.dir_count,
                            list: fr.files,
                        };
                    }

                    // 非根帧：在栈中查找父帧（不能假设就是栈顶，可能存在兄弟帧）
                    // Non-root: find the parent frame in the stack (may not be top due to siblings)
                    let parent_path = match fr.path.parent() {
                        Some(p) => p.to_path_buf(),
                        None => unreachable!(),
                    };
                    let parent_idx = stack
                        .iter()
                        .rposition(|f| f.path == parent_path)
                        .unwrap_or_else(|| unreachable!());
                    let parent = &mut stack[parent_idx];

                    let dir_info = flat.get(&fr.path);
                    let dir_name = match dir_info {
                        Some(info) => info.name().to_string(),
                        None => String::new(),
                    };
                    let modified_time = match dir_info {
                        Some(info) => info.modified_time(),
                        None => 0,
                    };

                    parent.total_len += fr.total_len;
                    parent.file_count += fr.file_count;
                    parent.dir_count += 1 + fr.dir_count;
                    parent.files.insert(
                        dir_name.clone(),
                        FileInfo::new(
                            dir_name,
                            fr.total_len,
                            modified_time,
                            FileKind::Dir(Dir {
                                files_list: fr.files,
                                file_count: fr.file_count,
                                dir_count: fr.dir_count,
                            }),
                        ),
                    );
                }
            }
        }

        unreachable!()
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
        results: Option<&Arc<Mutex<HashMap<PathBuf, FileInfo>>>>,
        pb: &Sender<(u64, u64)>,
        stream_tx: Option<&Sender<SearchEvent>>,
        warnings: Option<&Arc<Mutex<Vec<SearchWarning>>>>,
        thread_file_count: &mut u64,
        thread_dir_count: &mut u64,
        condver: &Arc<Condvar>,
    ) {
        if skip_symlink {
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
                    Self::emit_warning(
                        SearchWarning {
                            path: path_buf.to_path_buf(),
                            warning_type: SearchWarningType::ThreadError(format!(
                                "检测到符号链接循环，已跳过: {}",
                                path_buf.display()
                            )),
                        },
                        stream_tx,
                        warnings,
                    );
                    return;
                }

                let modified_time = Self::get_file_modified(&metadata);
                let name = Self::get_file_name(path_buf).unwrap_or("").to_string();

                let info = FileInfo {
                    name,
                    length: 0,
                    modified_time,
                    file_kind: FileKind::Dir(Dir {
                        files_list: HashMap::new(),
                        file_count: 0,
                        dir_count: 0,
                    }),
                };

                if let Some(tx) = stream_tx {
                    tx.send(SearchEvent::Entry(path_buf.to_path_buf(), info.clone()))
                        .ok();
                }

                if let Some(results) = results {
                    results
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(path_buf.to_path_buf(), info);
                }

                // 将新的 inode 附加到链上，子目录携带扩展后的链
                // Append new inode to chain; subdirectories carry the extended chain
                let mut new_chain = chain.to_vec();
                new_chain.push(inode_key);
                dir_queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_back(DirEntry {
                        path: path_buf.to_path_buf(),
                        symlink_chain: new_chain,
                    });
                //唤起一个线程
                condver.notify_one();
            }
        } else if path_buf.is_file() {
            // 符号链接指向文件 / Symlink points to a file
            // 文件不会导致循环（文件不能包含目录），不需要 inode 检测
            Self::process_file(
                path_buf,
                results,
                pb,
                stream_tx,
                warnings,
                thread_file_count,
            );
        } else {
            // 断开的符号链接 / Broken symlink
            Self::emit_warning(
                SearchWarning {
                    path: path_buf.to_path_buf(),
                    warning_type: SearchWarningType::BrokenSymlink,
                },
                stream_tx,
                warnings,
            );
        }
    }

    /// 处理普通文件 / Process a regular file
    fn process_file(
        path_buf: &Path,
        results: Option<&Arc<Mutex<HashMap<PathBuf, FileInfo>>>>,
        pb: &Sender<(u64, u64)>,
        stream_tx: Option<&Sender<SearchEvent>>,
        warnings: Option<&Arc<Mutex<Vec<SearchWarning>>>>,
        thread_file_count: &mut u64,
    ) {
        let file_metadata = match path_buf.metadata() {
            Ok(m) => m,
            Err(_) => {
                Self::emit_warning(
                    SearchWarning {
                        path: path_buf.to_path_buf(),
                        warning_type: SearchWarningType::MetadataError,
                    },
                    stream_tx,
                    warnings,
                );
                return;
            }
        };

        if let Some(name) = Self::get_file_name(path_buf) {
            let len = file_metadata.len();
            let modified_time = Self::get_file_modified(&file_metadata);

            let info = FileInfo {
                name: name.to_string(),
                length: len,
                modified_time,
                file_kind: FileKind::File,
            };

            // 流式发送：先发送再插入结果集，避免在持锁期间发送
            // Stream first: send before inserting into results to avoid holding the lock during send
            if let Some(tx) = stream_tx {
                tx.send(SearchEvent::Entry(path_buf.to_path_buf(), info.clone()))
                    .ok();
            }

            if let Some(results) = results {
                results
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(path_buf.to_path_buf(), info);
            }

            *thread_file_count += 1;
            if (*thread_file_count).is_multiple_of(50) {
                pb.send((50, 0)).ok();
            }
        }
    }

    /// 发送警告到适当的收集器（同步模式用 warnings Vec，流式模式用 stream_tx）。
    ///
    /// Sends a warning to the appropriate collector (warnings Vec for sync mode, stream_tx for streaming mode).
    fn emit_warning(
        warning: SearchWarning,
        stream_tx: Option<&Sender<SearchEvent>>,
        warnings: Option<&Arc<Mutex<Vec<SearchWarning>>>>,
    ) {
        if let Some(tx) = stream_tx {
            tx.send(SearchEvent::Warning(warning.clone())).ok();
        }
        if let Some(w) = warnings {
            w.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(warning);
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
        results: Option<Arc<Mutex<HashMap<PathBuf, FileInfo>>>>,
        pb: Sender<(u64, u64)>,
        stream_tx: Option<Sender<SearchEvent>>,
        warnings: Option<Arc<Mutex<Vec<SearchWarning>>>>,
        skip_symlink: bool,
        running_count: Arc<Mutex<i32>>,
        condver: Arc<Condvar>,
    ) {
        let fn_running_count_add = || -> i32 {
            let mut lock = running_count.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            *lock += 1;
            *lock
        };

        let fn_running_count_sub = || -> i32 {
            let mut lock = running_count.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            *lock -= 1;
            *lock
        };

        let mut thread_file_count = 0;
        let mut thread_dir_count = 0;

        fn_running_count_add();
        'worker: loop {
            let entry = {
                let mut queue = dir_queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                queue.pop_front()
            };

            let entry = if let Some(d) = entry {
                d
            } else {
                //如果是空的
                let mut dir_queue = dir_queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                //当前线程计数
                let this_running_count = fn_running_count_sub();
                if this_running_count == 0 {
                    //如果没有其他在运行中的线程，则说明搜索结束，唤醒所有线程并退出
                    condver.notify_all();
                    break;
                }
                loop {
                    dir_queue = condver.wait(dir_queue).unwrap_or_else(std::sync::PoisonError::into_inner);
                    match dir_queue.pop_front() {
                        Some(d) => {
                            //添加线程计数
                            fn_running_count_add();
                            break d;
                        }
                        None => {
                            //如果队列是空的，且运行中的线程数为0
                            if *running_count.lock().unwrap_or_else(std::sync::PoisonError::into_inner) == 0 {
                                break 'worker;
                            }
                        }
                    }
                }
            };

            let rd = match entry.path.read_dir() {
                Ok(rd) => rd,
                Err(err) => if err.kind() == ErrorKind::PermissionDenied {
                    Self::emit_warning(
                        SearchWarning {
                            path: entry.path.clone(),
                            warning_type: SearchWarningType::PermissionDenied,
                        },
                        stream_tx.as_ref(),
                        warnings.as_ref(),
                    );
                    continue;
                } else {
                    Self::emit_warning(
                        SearchWarning {
                            path: entry.path.clone(),
                            warning_type: SearchWarningType::ReadDirError(err.to_string()),
                        },
                        stream_tx.as_ref(),
                        warnings.as_ref(),
                    );
                    continue;
                },
            };

            for dir_entry in rd.flatten() {
                let path_buf = dir_entry.path();

                let metadata = if let Ok(m) = path_buf.symlink_metadata() { m } else {
                    Self::emit_warning(
                        SearchWarning {
                            path: path_buf,
                            warning_type: SearchWarningType::MetadataError,
                        },
                        stream_tx.as_ref(),
                        warnings.as_ref(),
                    );
                    continue;
                };

                let ft = metadata.file_type();

                if ft.is_symlink() {
                    Self::process_symlink(
                        &path_buf,
                        skip_symlink,
                        &entry.symlink_chain,
                        &dir_queue,
                        results.as_ref(),
                        &pb,
                        stream_tx.as_ref(),
                        warnings.as_ref(),
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

                    let info = FileInfo {
                        name,
                        length: 0,
                        modified_time,
                        file_kind: FileKind::Dir(Dir {
                            files_list: HashMap::new(),
                            file_count: 0,
                            dir_count: 0,
                        }),
                    };

                    if let Some(tx) = stream_tx.as_ref() {
                        tx.send(SearchEvent::Entry(path_buf.clone(), info.clone()))
                            .ok();
                    }

                    if let Some(results) = results.as_ref() {
                        results
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(path_buf.clone(), info);
                    }
                    //添加队列
                    dir_queue
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push_back(DirEntry {
                            path: path_buf,
                            symlink_chain: entry.symlink_chain.clone(),
                        });
                    //唤起一个线程
                    condver.notify_one();
                } else if ft.is_file() {
                    // 普通文件：不检测循环
                    // Regular file: no cycle detection
                    Self::process_file(
                        &path_buf,
                        results.as_ref(),
                        &pb,
                        stream_tx.as_ref(),
                        warnings.as_ref(),
                        &mut thread_file_count,
                    );
                } else {
                    Self::emit_warning(
                        SearchWarning {
                            path: path_buf,
                            warning_type: SearchWarningType::InaccessibleEntry,
                        },
                        stream_tx.as_ref(),
                        warnings.as_ref(),
                    );
                }
            }
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
    /// * `pb` - 进度更新发送器，发送 (文件增量, 目录增量)。
    ///   所有工作线程退出后立即 drop，关闭通道以通知接收方。 /
    ///   Progress sender, sends (file_delta, dir_delta).
    ///   Dropped immediately after all workers exit, closing the channel to signal the receiver.
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
    pub fn search<P: AsRef<Path>>(
        &self,
        path: P,
        skip_symlink: bool,
        pb: Sender<(u64, u64)>,
        max_thread_count: usize,
    ) -> io::Result<SearchResult> {
        let path = path.as_ref();
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
        dir_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(DirEntry {
                path: path.to_path_buf(),
                symlink_chain: Vec::new(), // 根目录无符号链接链 / root has no symlink chain
            });

        //搜索结果
        let results = Arc::new(Mutex::new(HashMap::<PathBuf, FileInfo>::new()));
        //搜索警告
        let warnings: Arc<Mutex<Vec<SearchWarning>>> = Arc::new(Mutex::new(Vec::new()));
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
            let warnings = Arc::clone(&warnings);
            let condver = condver.clone();
            let running_count = running_count.clone();

            let pb = pb.clone();

            let handle = thread::spawn(move || {
                Self::run_worker(
                    dir_queue,
                    Some(results),
                    pb,
                    None,
                    Some(warnings),
                    skip_symlink,
                    running_count,
                    condver,
                );
            });

            handles.push(handle);
        }

        for handle in handles {
            match handle.join() {
                Ok(()) => {}
                Err(_) => {
                    return Err(Error::other("worker thread panicked"));
                }
            }
        }

        // 所有 worker 线程已退出，不再需要发送进度更新。
        // 尽早 drop pb 以关闭进度通道，避免 post-processing 阶段阻塞主线程的 rx。
        //
        // All worker threads have exited; no more progress updates needed.
        // Drop pb early to close the progress channel, preventing the main
        // thread's rx from blocking during post-processing.
        drop(pb);

        let results = Arc::try_unwrap(results)
            .map_err(|_| Error::other("results Arc still has references"))?
            .into_inner()
            .unwrap_or_else(|e| e.into_inner());

        let collected_warnings = Arc::try_unwrap(warnings)
            .map_err(|_| Error::other("warnings Arc still has references"))?
            .into_inner()
            .unwrap_or_else(|e| e.into_inner());

        Ok(SearchResult {
            files_list: Self::build_tree(&results, path),
            warnings: collected_warnings,
        })
    }

    /// 流式搜索目录 / Stream search a directory
    ///
    /// 通过后台线程异步执行搜索，立即返回接收器用于实时消费发现的文件/目录条目和警告。
    /// Worker 线程传 `None` 给 `results` 以跳过 HashMap 收集开销。
    /// 不构建树形结构，调用方通过 Receiver 按 `SearchEvent` 消费。
    ///
    /// Executes the search asynchronously in a background thread, returning a receiver
    /// immediately for real-time consumption of discovered file/directory entries and warnings.
    /// Workers pass `None` for `results` to skip HashMap collection overhead.
    /// No tree structure is built — the caller consumes `SearchEvent`s via the Receiver.
    ///
    /// # 参数 / Parameters (same as `search`)
    ///
    /// # 返回值 / Returns
    /// * `Ok((JoinHandle<io::Result<()>>, Receiver<SearchEvent>))`
    ///   - 线程句柄：用于等待搜索完成并获取结果 / Thread handle: await completion and check result
    ///   - 接收器：实时消费事件（条目 + 警告） / Receiver: consume events (entries + warnings)
    /// * `Err` - 路径无效 / Invalid path
    ///
    /// # 使用示例 / Usage
    /// ```ignore
    /// let (tx, rx) = mpsc::channel();
    /// let (handle, stream_rx) = finder.search_stream(path, false, tx, 8)?;
    /// for event in stream_rx {
    ///     match event {
    ///         SearchEvent::Entry(path, info) => { println!("发现: {}", path.display()); }
    ///         SearchEvent::Warning(w) => { eprintln!("警告: {:?}", w); }
    ///     }
    /// }
    /// handle.join().unwrap()?; // 等待搜索完成 / wait for completion
    /// ```
    pub fn search_stream<P: AsRef<Path>>(
        &self,
        path: P,
        skip_symlink: bool,
        pb_tx: Sender<(u64, u64)>,
        max_thread_count: usize,
    ) -> io::Result<(
        thread::JoinHandle<io::Result<()>>,
        std::sync::mpsc::Receiver<SearchEvent>,
    )> {
        use std::sync::mpsc;
        let path = path.as_ref();

        // 同步验证路径 / Synchronous path validation
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
        let path = path.to_path_buf();

        // 创建流式通道 / Create streaming channel
        let (stream_tx, stream_rx) = mpsc::channel();

        // 后台线程执行搜索 / Background thread executes the search
        let handle = thread::spawn(move || -> io::Result<()> {
            let dir_queue = Arc::new(Mutex::new(VecDeque::<DirEntry>::new()));
            dir_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_back(DirEntry {
                    path: path.clone(),
                    symlink_chain: Vec::new(),
                });

            let running_count = Arc::new(Mutex::new(0));
            let condver = Arc::new(Condvar::new());

            let mut handles = Vec::with_capacity(num_threads);
            for _ in 0..num_threads {
                let dir_queue = Arc::clone(&dir_queue);
                let condver = condver.clone();
                let running_count = running_count.clone();
                let pb = pb_tx.clone();
                let stream_tx = stream_tx.clone();

                let handle = thread::spawn(move || {
                    Self::run_worker(
                        dir_queue,
                        None, // 不收集 HashMap 结果 / Don't collect HashMap results
                        pb,
                        Some(stream_tx),
                        None, // 流式模式不用 Vec 收集警告
                        skip_symlink,
                        running_count,
                        condver,
                    );
                });

                handles.push(handle);
            }

            for handle in handles {
                match handle.join() {
                    Ok(()) => {}
                    Err(_) => {
                        return Err(Error::other("worker thread panicked"));
                    }
                }
            }

            drop(pb_tx);
            drop(stream_tx);

            Ok(())
        });

        Ok((handle, stream_rx))
    }
}

#[cfg(test)]
mod test;
