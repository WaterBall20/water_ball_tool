use crate::tools::PathTool;
use crate::wb_files_pack::manager::{DEFAULT_COW, DEFAULT_SEPARATE_MANIFEST, WBFPManager};
use crate::wb_files_pack::pack_io::file::{PackVirtualFile, VirtualFileOpenOptions};
use crate::wb_files_pack::pack_io::file_handle::PackFileHandle;
use crate::wb_files_pack::pack_io::PackIO;
use crate::wb_files_pack::{Attribute, OverwriteStrategy, PackStruct, PackStructItem, PackStructItemType};
use std::collections::HashMap;
use std::fs::File;
use crate::wb_files_pack::error::{Result, PackFileError};
use std::path::Path;
use std::sync::{Arc, Mutex};
#[cfg(test)]
mod test;

/// 包文件访问模式 / Pack file access mode
///
/// 控制分配器级别的读写许可，用于验证虚拟文件工厂方法的权限。
/// Controls allocator-level read/write permissions, used to validate virtual file factory methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PackAccessMode {
    /// 只读模式 / Read-only mode
    Read,
    /// 只写模式 / Write-only mode
    Write,
    /// 读写模式 / Read-write mode
    ReadWrite,
}

/// 水球包文件分配器——公共 API 入口。
///
/// 封装 `WBFPManager` 和 `PackIO`，通过 `Arc<Mutex<>>` 提供线程安全访问。
/// 所有对外暴露的读取/写入操作均由此结构体代理。
///
/// WaterBall pack file allocator — public API entry point.
///
/// Wraps `WBFPManager` and `PackIO` behind `Arc<Mutex<>>` for thread-safe access.
/// All externally-facing read/write operations are proxied through this struct.
#[derive(Clone, Debug)]
pub struct Allocator {
    /// 分配器访问模式（读写许可）/ Allocator access mode (read/write permissions)
    pub(crate) access_mode: PackAccessMode,
    manager: Arc<Mutex<WBFPManager>>,
    pack_io: Arc<Mutex<PackIO>>,
}
impl Allocator {
    /// 打开已存在的水球包文件（只读模式）。
    ///
    /// Open an existing WaterBall pack file in Read mode.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::options().read(true).open(path)
    }

    /// 使用默认参数创建新水球包文件（写入模式）。
    ///
    /// 如果文件已存在则返回错误。
    ///
    /// Create a new pack file with default settings in Write mode.
    ///
    /// Returns an error if the file already exists.
    pub fn create_new<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::options()
            .write(true)
            .create_new(true)
            .open(path)
    }

    /// 获取 `PackOpenOptions` 构建器，用于自定义打开/创建参数。
    ///
    /// Get a `PackOpenOptions` builder for custom open/create parameters.
    pub fn options() -> PackOpenOptions {
        PackOpenOptions::new()
    }

    /// 以只读模式打开已存在的虚拟文件。
    ///
    /// Open an existing virtual file in read-only mode.
    pub fn open_virtual_file<P: AsRef<Path>>(&mut self, path: P, end_pos: bool) -> Result<PackVirtualFile> {
        Self::virtual_file_options()
            .read(true)
            .end_pos(end_pos)
            .open(self, path)
    }

    /// 创建新的虚拟文件（写入模式），行为类似于 `File::create`。
    ///
    /// Create a new virtual file in write-only mode, similar to `File::create`.
    pub fn create_virtual_file<P: AsRef<Path>>(&mut self, path: P) -> Result<PackVirtualFile> {
        Self::virtual_file_options()
            .write(true)
            .create_new(true)
            .open(self, path)
    }

    /// 获取 `VirtualFileOpenOptions` 构建器。
    ///
    /// Get a `VirtualFileOpenOptions` builder.
    pub fn virtual_file_options() -> VirtualFileOpenOptions {
        VirtualFileOpenOptions::new()
    }
}

/// 包文件打开选项构建器 / Pack file open options builder
pub struct PackOpenOptions {
    read: bool,
    write: bool,
    create: bool,
    create_new: bool,
    cow: bool,
    separate_manifest: bool,
}

impl PackOpenOptions {
    fn new() -> Self {
        Self {
            read: false,
            write: false,
            create: false,
            create_new: false,
            cow: DEFAULT_COW,
            separate_manifest: DEFAULT_SEPARATE_MANIFEST,
        }
    }

    /// 以读取模式打开 / Open in read mode
    pub fn read(&mut self, v: bool) -> &mut Self {
        self.read = v;
        self
    }

    /// 以写入模式打开 / Open in write mode
    pub fn write(&mut self, v: bool) -> &mut Self {
        self.write = v;
        self
    }

    /// 如果文件不存在则创建新包文件 / Create a new pack file if it does not exist
    pub fn create(&mut self, v: bool) -> &mut Self {
        self.create = v;
        self
    }

    /// 创建新包文件，如果已存在则报错 / Create a new pack file, fail if it exists
    pub fn create_new(&mut self, v: bool) -> &mut Self {
        self.create_new = v;
        self
    }

    /// 设置写时复制策略 / Set the copy-on-write policy
    pub fn cow(&mut self, v: bool) -> &mut Self {
        self.cow = v;
        self
    }

    /// 设置是否分离清单文件 / Set whether to use a separate manifest file
    pub fn separate_manifest(&mut self, v: bool) -> &mut Self {
        self.separate_manifest = v;
        self
    }

    /// 根据配置的选项打开或创建包文件。
    ///
    /// Open or create a pack file according to the configured options.
    pub fn open<P: AsRef<Path>>(&self, path: P) -> Result<Allocator> {
        let path = path.as_ref();
        let access_mode = match (self.read, self.write) {
            (true, false) => PackAccessMode::Read,
            (false, true) => PackAccessMode::Write,
            (true, true) => PackAccessMode::ReadWrite,
            (false, false) => {
                return Err(PackFileError::Other(
                    "必须指定至少 read 或 write 访问模式".into(),
                ));
            }
        };

        if self.create && self.create_new {
            return Err(PackFileError::Other(
                "create 和 create_new 不能同时为 true".into(),
            ));
        }

        let path_buf = path.to_path_buf();

        if self.create_new {
            // 创建新文件，如果已存在则报错 / Create new, fail if exists
            match path.try_exists() {
                Ok(true) => {
                    return Err(PackFileError::Other(
                        "文件可能已存在，无法创建！".into(),
                    ));
                }
                Ok(false) | Err(_) => {}
            }
            let pack_file = File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .create_new(true)
                .open(path)?;
            let pack_io = PackIO::new(pack_file);
            let pack_io = Arc::new(Mutex::new(pack_io));
            let mut manager = WBFPManager::create_pack_file(
                &path_buf,
                pack_io.clone(),
                self.cow,
                self.separate_manifest,
                true,
            )?;
            manager.init_new_pack()?;
            let manager = Arc::new(Mutex::new(manager));
            Ok(Allocator {
                manager,
                pack_io,
                access_mode,
            })
        } else if self.create {
            // 打开已存在的文件，不存在则创建新文件 / Open existing or create new
            let pack_file = File::options()
                .read(true)
                .write(true)
                .create(true)
                .open(path)?;
            let metadata = pack_file.metadata()?;
            let is_new = metadata.len() == 0;
            let pack_io = PackIO::new(pack_file);
            let pack_io = Arc::new(Mutex::new(pack_io));

            if is_new {
                let mut manager = WBFPManager::create_pack_file(
                    &path_buf,
                    pack_io.clone(),
                    self.cow,
                    self.separate_manifest,
                    false,
                )?;
                manager.init_new_pack()?;
                let manager = Arc::new(Mutex::new(manager));
                Ok(Allocator {
                    manager,
                    pack_io,
                    access_mode,
                })
            } else {
                let manager = WBFPManager::open_pack_file(&path_buf, pack_io.clone())?;
                let manager = Arc::new(Mutex::new(manager));
                Ok(Allocator {
                    manager,
                    pack_io,
                    access_mode,
                })
            }
        } else {
            // 打开已存在的文件 / Open existing file
            let pack_file = File::options().read(true).write(true).open(path)?;
            let pack_io = PackIO::new(pack_file);
            let pack_io = Arc::new(Mutex::new(pack_io));
            let manager = WBFPManager::open_pack_file(&path_buf, pack_io.clone())?;
            let manager = Arc::new(Mutex::new(manager));
            Ok(Allocator {
                manager,
                pack_io,
                access_mode,
            })
        }
    }
}

impl Allocator /*读*/ {
    /// 检查指定虚拟路径是否存在。
    /// Check whether the given virtual path exists.
    pub fn path_exists<P: AsRef<Path>>(&mut self, path: P) -> Result<bool> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.path_exists(path))
    }

    /// 已存在的虚拟文件——返回 PackVirtualFile（内部使用）。
    ///
    /// 通过管理器查找指定路径的虚拟文件句柄，包装为 PackVirtualFile 返回。
    ///
    /// Open an existing virtual file — returns a PackVirtualFile (internal use).
    ///
    /// Looks up the virtual file handle via the manager and wraps it in a PackVirtualFile.
    pub(crate) fn open_virtual_file_impl<P: AsRef<Path>>(&mut self, path: P, end_pos: bool) -> Result<PackVirtualFile> {
        let mgr = self.manager.clone();
        let mut mgr = mgr
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        mgr.this_write_lock()?;
        let path_list = PathTool::path_to_string_vec(path);
        let handle = mgr.get_or_create_file_handle(
            &path_list,
            end_pos,
            &self.manager,
            &self.pack_io,
        )?;
        Ok(PackVirtualFile::new(0, handle))
    }

    /// 创建新虚拟文件——返回 PackVirtualFile（内部使用）。
    ///
    /// 通过管理器创建虚拟文件并分配初始大小，包装为 PackVirtualFile 返回。
    ///
    /// Create a new virtual file — returns a PackVirtualFile (internal use).
    ///
    /// Creates the virtual file via the manager with auto-sized allocation.
    pub(crate) fn create_virtual_file_impl<P: AsRef<Path>>(&mut self, path: P) -> Result<PackVirtualFile> {
        let mgr = self.manager.clone();
        let mut mgr = mgr
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        mgr.this_write_lock()?;
        let (path_list, metadata) = mgr.create_file_auto_sized(path)?;
        let handle = Arc::new(Mutex::new(PackFileHandle::create(
            true,
            self.manager.clone(),
            &self.pack_io,
            path_list,
            metadata,
            false,
        )?));
        Ok(PackVirtualFile::new(0, handle))
    }

    /// 获取包文件的全局属性。
    /// Get the pack file's global attributes.
    pub fn get_manifest_attribute(&self) -> Result<Attribute> {
        let manager = self.manager.clone();
        let manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_manifest_attribute().clone())
    }

    /// 获取根目录的所有结构项。
    /// Get all struct items in the root directory.
    pub fn get_root_struct_items(&self) -> Result<HashMap<String, PackStructItem>> {
        let manager = self.manager.clone();
        let manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_root_struct_items().clone())
    }

    /// 获取根目录的子项名称列表。
    /// Get the list of child item names in the root directory.
    pub fn get_root_struct_item_name_list(&mut self) -> Result<Vec<String>> {
        let manager = self.manager.clone();
        let manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_root_struct_item_name_list())
    }

    /// 获取指定目录的子项名称列表。
    /// Get the list of child item names for the given directory.
    pub fn get_struct_item_name_list<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> Result<Vec<String>> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        manager.get_struct_item_name_list(path)
    }

    /// 获取指定目录的所有结构项。
    /// Get all struct items for the given directory.
    pub fn get_dir_pack_struct_items<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> Result<HashMap<String, PackStructItem>> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_dir_pack_struct_items(path)?.clone())
    }

    /// 获取指定路径的目录结构项（限定为目录类型）。
    /// Get the struct item for the given path, asserting it is a directory.
    pub fn get_pack_struct_item_dir<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> Result<PackStructItem> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_pack_struct_item_dir(path)?.clone())
    }

    /// 获取指定路径的结构项（可接受文件或目录）。
    /// Get the struct item for the given path (accepts file or directory).
    pub fn get_pack_struct_item<P: AsRef<Path>>(&mut self, path: P) -> Result<PackStructItem> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_pack_struct_item(path)?.clone())
    }

    /// 递归加载整个包中所有未加载的元数据和子目录结构。
    ///
    /// `no_err` 为 `true` 时遇到错误继续加载其余数据，为 `false` 时在首个错误处终止。
    ///
    /// Recursively load all unloaded metadata and subdirectory structures in the entire pack.
    ///
    /// When `no_err` is `true`, continues loading remaining data on error;
    /// when `false`, stops at the first error.
    pub fn load_all_data(&mut self, no_err: bool) -> Result<()> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        manager.load_all_data(no_err)
    }

    /// 按需加载指定路径的结构和元数据。
    /// Lazily load the structure and metadata for the given path.
    pub fn load_pack_struct_metadata_path<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        manager.load_pack_struct_metadata_path(path)
    }

    /// 获取指定路径的目录结构。
    /// Get the pack structure for the given directory path.
    pub fn get_dir<P: AsRef<Path>>(&mut self, path: P) -> Result<PackStruct> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_dir(path)?.clone())
    }
}

impl Allocator /*写*/ {
    /// 在包内创建目录（包括所有不存在的父目录）。
    /// Create a directory in the pack, including any missing parent directories.
    pub fn create_dir_all<P: AsRef<Path>>(&mut self, path: &P) -> Result<()> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        manager.create_dir_all(path)
    }
}

impl Allocator /*删除*/ {
    /// 删除虚拟文件（仅移除元数据和结构，数据块提交GC不覆写）。
    /// Delete a virtual file (remove metadata and structure only, data blocks submitted to GC without overwriting).
    pub fn delete_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        let path_list = PathTool::path_to_string_vec(path);
        manager.delete_file(path_list)
    }

    /// 删除虚拟目录及其所有子项（仅移除元数据和结构，数据块提交GC不覆写）。
    /// Delete a virtual directory and all its descendants (remove metadata and structure only, data blocks submitted to GC without overwriting).
    pub fn delete_dir_all<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        let path_list = PathTool::path_to_string_vec(path);
        manager.delete_dir_all(path_list)?;
        Ok(())
    }

    /// 擦除虚拟文件（覆写所有数据块和元数据后提交GC并删除结构）。
    /// Erase a virtual file (overwrite all data blocks and metadata, then submit to GC and remove structure).
    pub fn erase_file<P: AsRef<Path>>(&mut self, path: P, strategy: OverwriteStrategy) -> Result<()> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        let path_list = PathTool::path_to_string_vec(path);
        manager.erase_file(path_list, strategy)
    }

    /// 擦除虚拟目录及其所有子项（覆写所有数据块和元数据后提交GC并删除结构）。
    /// Erase a virtual directory and all its descendants (overwrite all data and metadata, then submit to GC and remove structure).
    pub fn erase_dir_all<P: AsRef<Path>>(&mut self, path: P, strategy: OverwriteStrategy) -> Result<()> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        let path_list = PathTool::path_to_string_vec(path);
        manager.erase_dir_all(path_list, strategy)?;
        Ok(())
    }
}

impl Allocator /*工具方法*/ {
    /// 遍历包内所有文件并验证其哈希值。
    ///
    /// 返回 `VerifyHashR` 包含验证通过和失败的路径列表。
    ///
    /// Recursively verify the hash of every file in the pack.
    ///
    /// Returns `VerifyHashR` containing lists of paths that passed and failed verification.
    pub fn verify_all_file_hash(&mut self) -> Result<VerifyHashR> {
        let mut vhr = VerifyHashR::new();
        let root_struct = self.get_root_struct_item_name_list()?;
        for name in root_struct {
            self.verify_all_file_hash_inner(name.as_ref(), &mut vhr)?;
        }
        Ok(vhr)
    }
    fn verify_all_file_hash_inner(
        &mut self,
        path: &Path,
        verify_hash_r: &mut VerifyHashR,
    ) -> Result<()> {
        let item = self.get_pack_struct_item(path)?;
        match item.item_type() {
            PackStructItemType::Dir { .. } => {
                let s_file_name = self.get_struct_item_name_list(path)?;
                for name in s_file_name {
                    self.verify_all_file_hash_inner(&path.join(name), verify_hash_r)?;
                }
                Ok(())
            }
            PackStructItemType::File { .. } => {
                let mut file = {
                    let mgr = self.manager.clone();
                    let mut mgr = mgr
                        .lock()
                        .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
                    mgr.this_write_lock()?;
                    let path_list = PathTool::path_to_string_vec(path);
                    let handle = mgr.get_or_create_file_handle(
                        &path_list,
                        false,
                        &self.manager,
                        &self.pack_io,
                    )?;
                    PackVirtualFile::new(0, handle)
                };
                let path_str = path
                    .to_str()
                    .ok_or(PackFileError::Format("路径包含无效 UTF-8".into()))?
                    .to_string();
                match file.verify_hash(None) {
                    Ok(true) => verify_hash_r.ok_path.push(path_str),
                    Ok(false) =>
                        verify_hash_r.err_path.push((path_str, None)),
                    Err(err) =>
                        verify_hash_r.err_path.push((
                            path_str,
                            Some(err),
                        )),
                }
                Ok(())
            }
        }
    }
}

/// 哈希验证结果。
///
/// Hash verification result.
#[derive(Debug)]
pub struct VerifyHashR {
    /// 哈希验证通过的路径列表。
    /// Paths that passed hash verification.
    ok_path: Vec<String>,
    /// 哈希验证失败的路径列表，附带可选的错误信息。
    /// Paths that failed hash verification, with optional error details.
    err_path: Vec<(String, Option<PackFileError>)>,
}
impl VerifyHashR {
    fn new() -> Self {
        Self {
            ok_path: Vec::new(),
            err_path: Vec::new(),
        }
    }
}
