/*
开始时间：26/02/13 11：31
 */
use crate::wb_files_pack::allocator::{Allocator, PackAccessMode};
use crate::wb_files_pack::error::Result;
use crate::wb_files_pack::pack_io::file_handle::PackFileHandle;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// 虚拟文件访问模式 / Virtual file access mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessMode {
    /// 只读 / Read-only
    Read,
    /// 只写 / Write-only
    Write,
    /// 读写 / Read-write
    ReadWrite,
}

/// 虚拟文件读写器 / Virtual file reader-writer
///
/// 提供包内文件的读写操作，管理文件位置、元数据和哈希计算。
/// Provides read/write operations for files within a pack, managing file position, metadata, and hash computation.
pub struct PackVirtualFile {
    /// 当前文件位置 / Current file position
    pos: u64,
    /// 虚拟文件句柄 / Virtual file handle
    handle: Arc<Mutex<PackFileHandle>>,
    /// 访问模式 / Access mode
    access_mode: AccessMode,
}

impl PackVirtualFile {
    /// 从句柄创建虚拟文件（内部使用，默认读写模式）。
    ///
    /// Create a virtual file from a handle (internal use, defaults to read-write mode).
    pub(in crate::wb_files_pack) fn new(pos: u64, handle: Arc<Mutex<PackFileHandle>>) -> Self {
        Self {
            pos,
            handle,
            access_mode: AccessMode::ReadWrite,
        }
    }

    /// 检查分配器访问模式与虚拟文件请求的访问模式是否兼容。
    ///
    /// 严格模式：只读分配器不能写，只写分配器不能读。
    /// Check allocator access mode compatibility with the requested virtual file access mode.
    ///
    /// Strict mode: read-only allocators cannot write, write-only allocators cannot read.
    pub(crate) fn check_allocator_compat(allocator: &Allocator, desired: AccessMode) -> Result<()> {
        let allowed = allocator.access_mode;
        match (allowed, desired) {
            (PackAccessMode::ReadWrite, _)
            | (PackAccessMode::Write, AccessMode::Write)
            | (PackAccessMode::Read, AccessMode::Read) => Ok(()),
            (PackAccessMode::Write, _) => Err(
                crate::wb_files_pack::error::PackFileError::PermissionDenied(
                    "Allocator 为只写模式，无法以读取方式打开虚拟文件".into(),
                ),
            ),
            (PackAccessMode::Read, _) => Err(
                crate::wb_files_pack::error::PackFileError::PermissionDenied(
                    "Allocator 为只读模式，无法以写入方式打开虚拟文件".into(),
                ),
            ),
        }
    }

    /// 返回虚拟文件的总长度（字节）。
    /// Returns the total length of this virtual file in bytes.
    //获取大小
    pub fn get_len(&self) -> u64 {
        self.handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_len()
    }

    /// 返回虚拟文件的最后修改时间（毫秒时间戳）。
    /// Returns the last modified time of this virtual file (millisecond timestamp).
    pub fn get_modified(&self) -> u128 {
        self.handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_modified()
    }

    /// 设置虚拟文件的大小。
    ///
    /// 增大时自动分配新的数据块，减小时释放多余空间并提交垃圾回收。
    ///
    /// Set the size of this virtual file.
    ///
    /// When increasing, new data blocks are automatically allocated.
    /// When decreasing, excess space is released and submitted for garbage collection.
    //设置文件大小
    pub fn set_len(&mut self, len: u64) -> Result<()> {
        let handle = self.handle.clone();
        let mut handle = handle.lock().unwrap_or_else(|e| e.into_inner());
        handle.set_len(len)?;
        Ok(())
    }

    /// 设置虚拟文件的最后修改时间（毫秒时间戳）。
    /// Set the last modified time of this virtual file (millisecond timestamp).
    pub fn set_modified(&mut self, modified: u128) -> Result<()> {
        let mut handle = self.handle.lock().unwrap_or_else(|e| e.into_inner());
        handle.set_modified(modified);
        Ok(())
    }

    //设置文件位置
    fn set_pos(&mut self, pos: u64) {
        self.pos = pos;
    }

    //追加文件位置
    fn add_pos(&mut self, length: u64) {
        self.pos += length;
    }

    //减少文件位置 / Move file position backward
    fn sub_pos(&mut self, length: u64) {
        self.pos -= length;
    }

    /// 验证文件数据的完整性哈希。
    ///
    /// 从文件开头重新读取全部数据计算哈希，与元数据中存储的哈希值比较。
    ///
    /// Verify the integrity hash of the file data.
    ///
    /// Reads all data from the beginning, computes the hash, and compares with the stored hash value.
    pub fn verify_hash(&mut self, progress: Option<&dyn Fn(u64, u64)>) -> Result<bool> {
        let handle = self.handle.clone();
        let mut handle = handle.lock().unwrap_or_else(|e| e.into_inner());
        handle.verify_hash(progress)
    }
    fn add_pos_i64(&mut self, pos: i64) {
        match pos {
            0 => (),
            //大于0
            1.. => {
                self.add_pos(pos.cast_unsigned());
            }
            //小于0
            ..0 => {
                self.sub_pos((-pos).cast_unsigned());
            }
        }
    }
}
impl Seek for PackVirtualFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(pos) => {
                self.pos = pos;
            }
            SeekFrom::Current(pos) => {
                self.add_pos_i64(pos);
            }
            SeekFrom::End(pos) => {
                let end = self.get_len();
                self.set_pos(end);
                self.add_pos_i64(pos);
            }
        }
        Ok(self.pos)
    }
}

impl Read for PackVirtualFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !matches!(self.access_mode, AccessMode::Read | AccessMode::ReadWrite) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "虚拟文件未以读取方式打开",
            ));
        }
        let handle = self.handle.clone();
        let mut handle = handle.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let len = handle.read(self.pos, buf).map_err(io::Error::other)?;
        self.add_pos(len as u64);
        Ok(len)
    }
}

impl Write for PackVirtualFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !matches!(self.access_mode, AccessMode::Write | AccessMode::ReadWrite) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "虚拟文件未以写入方式打开",
            ));
        }
        let handle = self.handle.clone();
        let mut handle = handle.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let len = handle.write(self.pos, buf).map_err(io::Error::other)?;
        self.add_pos(len as u64);
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        if !matches!(self.access_mode, AccessMode::Write | AccessMode::ReadWrite) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "虚拟文件未以写入方式打开",
            ));
        }
        let handle = self.handle.clone();
        let mut handle = handle.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        handle.flush().map_err(io::Error::other)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write(buf)?;
        Ok(())
    }
}

/// 虚拟文件打开选项构建器 / Virtual file open options builder
///
/// 提供链式调用方式配置虚拟文件的打开参数，支持只读/只写/读写、新建等模式。
/// Provides a builder pattern for configuring virtual file open options,
/// supporting read-only / write-only / read-write and create-new modes.
pub struct VirtualFileOpenOptions {
    read: bool,
    write: bool,
    create_new: bool,
    end_pos: bool,
}

impl VirtualFileOpenOptions {
    /// 创建默认选项（所有标志为 false）。
    /// Create default options (all flags false).
    pub fn new() -> Self {
        Self {
            read: false,
            write: false,
            create_new: false,
            end_pos: false,
        }
    }

    /// 设置读取标志。
    /// Set the read flag.
    pub fn read(mut self, read: bool) -> Self {
        self.read = read;
        self
    }

    /// 设置写入标志。
    /// Set the write flag.
    pub fn write(mut self, write: bool) -> Self {
        self.write = write;
        self
    }

    /// 设置新建标志。
    /// Set the create-new flag.
    pub fn create_new(mut self, create_new: bool) -> Self {
        self.create_new = create_new;
        self
    }

    /// 设置打开位置是否位于文件末尾。
    /// Set whether the open position is at the end of the file.
    pub fn end_pos(mut self, end_pos: bool) -> Self {
        self.end_pos = end_pos;
        self
    }

    /// 根据配置打开虚拟文件。
    ///
    /// 根据 `read`/`write` 标志确定访问模式，并检查分配器的访问模式兼容性。
    /// 如果 `create_new` 为 `true` 则创建新文件，否则打开已存在的文件。
    ///
    /// Open a virtual file according to the configured options.
    ///
    /// Determines the access mode from `read`/`write` flags and checks allocator
    /// access mode compatibility. If `create_new` is `true`, creates a new file;
    /// otherwise opens an existing one.
    pub fn open<P: AsRef<Path>>(
        &self,
        allocator: &mut Allocator,
        path: P,
    ) -> Result<PackVirtualFile> {
        let access_mode = match (self.read, self.write) {
            (true, false) => AccessMode::Read,
            (false, true) => AccessMode::Write,
            (true, true) => AccessMode::ReadWrite,
            (false, false) => {
                return Err(
                    crate::wb_files_pack::error::PackFileError::PermissionDenied(
                        "必须启用读取或写入标志".into(),
                    ),
                );
            }
        };

        PackVirtualFile::check_allocator_compat(allocator, access_mode)?;

        if self.create_new {
            let mut vf = allocator.create_virtual_file_impl(path)?;
            vf.access_mode = access_mode;
            Ok(vf)
        } else {
            let mut vf = allocator.open_virtual_file_impl(path, self.end_pos)?;
            vf.access_mode = access_mode;
            Ok(vf)
        }
    }
}

impl Default for VirtualFileOpenOptions {
    fn default() -> Self {
        Self::new()
    }
}
