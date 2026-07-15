use crate::tools::PathTool;
use crate::wb_files_pack::manager::{WBFPManager, DEFAULT_COW, DEFAULT_SEPARATE_MANIFEST};
use crate::wb_files_pack::pack_io::file::PackFileWR;
use crate::wb_files_pack::pack_io::PackIO;
use crate::wb_files_pack::pack_io::file_handle::PackFileHandle;
use crate::wb_files_pack::{Attribute, PackStruct, PackStructItem, PackStructItemType};
use std::collections::HashMap;
use std::fs::File;
use crate::wb_files_pack::error::{Result, PackFileError};
use std::path::Path;
use std::sync::{Arc, Mutex};
#[cfg(test)]
mod test;

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
    manager: Arc<Mutex<WBFPManager>>,
    pack_io: Arc<Mutex<PackIO>>,
}
impl Allocator {
    /// 打开一个已存在的水球包文件。
    ///
    /// 读取文件头、属性、根结构，获取写入锁。
    ///
    /// Open an existing WaterBall pack file.
    ///
    /// Reads the file header, attributes, and root structure, and acquires a write lock.
    pub fn open_pack_file<P: AsRef<Path>>(path: &P) -> Result<Allocator> {
        let pack_file = File::options().read(true).write(true).open(path)?;
        let pack_io = PackIO::new(pack_file);
        let pack_io = Arc::new(Mutex::new(pack_io));
        let manager = WBFPManager::open_pack_file(path, pack_io.clone())?;
        let manager = Arc::new(Mutex::new(manager));
        Ok(Self { manager, pack_io })
    }

    /// 使用默认参数创建新包文件（默认 COW 和分离清单）。
    ///
    /// 如果文件已存在则返回错误。
    ///
    /// Create a new pack file with default settings (default COW and separate manifest).
    ///
    /// Returns an error if the file already exists.
    pub fn create_new_pack_file2<P: AsRef<Path>>(path: &P) -> Result<Allocator> {
        Self::create_new_pack_file(path, DEFAULT_COW, DEFAULT_SEPARATE_MANIFEST)
    }

    /// 创建新包文件，指定写时复制和清单分离策略。
    ///
    /// 如果文件已存在则返回错误。
    ///
    /// Create a new pack file with the specified COW and separate-manifest policy.
    ///
    /// Returns an error if the file already exists.
    pub fn create_new_pack_file<P: AsRef<Path>>(
        path: &P,
        cow: bool,
        separate_manifest: bool,
    ) -> Result<Allocator> {
        match path.as_ref().try_exists() {
            Ok(true) => Err(PackFileError::Other("文件可能已存在，无法创建！".into())),
            Ok(false) | Err(_) => Self::create_pack_file(path, cow, separate_manifest, true),
        }
    }

    /// 创建包文件（完整参数版本）。
    ///
    /// `create_new` 控制是否使用 `File::create_new`（失败时返回 `AlreadyExists` 错误）。
    ///
    /// Create a pack file (full-parameter version).
    ///
    /// `create_new` controls whether `File::create_new` is used (returns `AlreadyExists` on conflict).
    pub fn create_pack_file<P: AsRef<Path>>(
        path: &P,
        cow: bool,
        separate_manifest: bool,
        create_new: bool,
    ) -> Result<Allocator> {
        let pack_file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .create_new(create_new)
            .open(path)?;
        let pack_io = PackIO::new(pack_file);
        let pack_io = Arc::new(Mutex::new(pack_io));
        let mut manager = WBFPManager::create_pack_file(
            path,
            pack_io.clone(),
            cow,
            separate_manifest,
            create_new,
        )?;
        manager.init_new_pack()?;
        let manager = Arc::new(Mutex::new(manager));
        Ok(Self { manager, pack_io })
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

    /// 创建虚拟文件（使用默认写时复制和哈希设置）
    /// Create a virtual file (using default COW and hash settings)
    pub fn create_file<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
    ) -> Result<PackFileWR> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        manager.this_write_lock()?;
        let (path_list, metadata) = manager.create_file(path, modified, len)?;
        let handle = Arc::new(
            Mutex::new(
                PackFileHandle::create(
                    true,
                    self.manager.clone(),
                    &self.pack_io.clone(),
                    path_list,
                    metadata,
                    false,
                )?
            )
        );
        Ok(PackFileWR::create(0, handle))
    }

    /// 创建虚拟文件（自动分配初始大小，无需预先指定长度）
    /// Create a virtual file (auto-allocate initial size, no need to pre-specify length)
    pub fn create_file_auto_sized<P: AsRef<Path>>(&mut self, path: P) -> Result<PackFileWR> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        manager.this_write_lock()?;
        let (path_list, metadata) = manager.create_file_auto_sized(path)?;
        let handle = Arc::new(
            Mutex::new(
                PackFileHandle::create(
                    true,
                    self.manager.clone(),
                    &self.pack_io.clone(),
                    path_list,
                    metadata,
                    false,
                )?
            )
        );
        Ok(PackFileWR::create(0, handle))
    }
    /// 创建虚拟文件（完整参数版本，用于需要自定义写时复制和哈希算法的场景）
    /// Create a virtual file (full parameter version, for custom COW and hash algorithm scenarios)
    pub fn create_file_raw<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
        cow: bool,
        hash_type: u8,
    ) -> Result<PackFileWR> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        manager.this_write_lock()?;
        let (path_list, metadata) = manager.create_file_raw(path, modified, len, cow, hash_type)?;
        let handle = Arc::new(
            Mutex::new(
                PackFileHandle::create(
                    true,
                    self.manager.clone(),
                    &self.pack_io.clone(),
                    path_list,
                    metadata,
                    false,
                )?
            )
        );
        Ok(PackFileWR::create(0, handle))
    }

    /// 以读写模式打开已存在的虚拟文件。
    ///
    /// `end_pos` 为 `true` 时文件指针定位到末尾，为 `false` 时定位到开头。
    ///
    /// Open an existing virtual file for reading and writing.
    ///
    /// When `end_pos` is `true`, the file pointer is positioned at the end;
    /// when `false`, it is positioned at the beginning.
    pub fn open_file<P: AsRef<Path>>(&mut self, path: P, end_pos: bool) -> Result<PackFileWR> {
        self.get_file_wr(path, end_pos)
    }

    /// 获取虚拟文件的读写器（与 `open_file` 相同但语义更明确）。
    ///
    /// 锁定文件的元数据以确保独占写入访问。
    ///
    /// Get a read-writer for a virtual file (same as `open_file` with clearer semantics).
    ///
    /// Locks the file's metadata to ensure exclusive write access.
    pub fn get_file_wr<P: AsRef<Path>>(
        &mut self,
        path: P,
        end_pos: bool,
    ) -> Result<PackFileWR> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
        manager.this_write_lock()?;
        let path_list = PathTool::path_to_string_vec(path);
        let handle = manager.get_or_create_file_handle(
            &path_list,
            end_pos,
            &self.manager,
            &self.pack_io,
        )?;
        Ok(PackFileWR::create(0, handle))
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
                let mut file = self.get_file_wr(path, false)?;
                match file.verify_hash() {
                    Ok(true) => verify_hash_r.ok_path.push(path.to_str().unwrap().to_string()),
                    Ok(false) =>
                        verify_hash_r.err_path.push((path.to_str().unwrap().to_string(), None)),
                    Err(err) =>
                        verify_hash_r.err_path.push((
                            path.to_str().unwrap().to_string(),
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
