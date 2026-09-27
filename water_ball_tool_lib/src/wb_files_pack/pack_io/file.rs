/*
开始时间：26/02/13 11：31
 */
use crate::wb_files_pack::manager_sync::{PackAccessMode, ManagerSync};
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

    /// 检查同步管理器访问模式与虚拟文件请求的访问模式是否兼容。
    ///
    /// 严格模式：只读同步管理器不能写，只写同步管理器不能读。
    /// Check synchronized manager access mode compatibility with the requested virtual file access mode.
    ///
    /// Strict mode: read-only synchronized managers cannot write, write-only synchronized managers cannot read.
    pub(crate) fn check_sync_compat(sync: &ManagerSync, desired: AccessMode) -> Result<()> {
        let allowed = sync.access_mode;
        match (allowed, desired) {
            (PackAccessMode::ReadWrite, _)
            | (PackAccessMode::Write, AccessMode::Write)
            | (PackAccessMode::Read, AccessMode::Read) => Ok(()),
            (PackAccessMode::Write, _) => Err(
                crate::wb_files_pack::error::PackFileError::PermissionDenied(
                    "ManagerSync 为只写模式，无法以读取方式打开虚拟文件".into(),
                ),
            ),
            (PackAccessMode::Read, _) => Err(
                crate::wb_files_pack::error::PackFileError::PermissionDenied(
                    "ManagerSync 为只读模式，无法以写入方式打开虚拟文件".into(),
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
        let mut handle = handle.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        handle.set_len(len)?;
        Ok(())
    }

    /// 设置虚拟文件的最后修改时间（毫秒时间戳）。
    /// Set the last modified time of this virtual file (millisecond timestamp).
    pub fn set_modified(&mut self, modified: u128) -> Result<()> {
        let mut handle = self.handle.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let mut handle = handle.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
pub struct VirtualFileOpenOptions<'a> {
    sync: &'a mut ManagerSync, 
    read: bool,
    write: bool,
    create_new: bool,
    end_pos: bool,
    /// 新建文件时的初始分配大小。
    /// 已知大小传 `Some(len)`（按 128B 精确对齐分配）；`None` 表示未知大小，按 4MiB 分配。
    /// 打开已存在的文件时该值不生效（不能缩小已有分配）。
    /// Initial allocation size when creating a new file: `Some(len)` for known size
    /// (128B-aligned exact allocation); `None` for unknown size (4MiB allocation).
    /// Ignored when opening an existing file — an existing allocation never shrinks.
    alloc_size: Option<u64>,
}

impl <'a>VirtualFileOpenOptions<'a> {
    /// 创建默认选项（所有标志为 false）。
    /// Create default options (all flags false).
    pub fn new(sync: &'a mut ManagerSync) -> Self {
        Self {
            sync,
            read: false,
            write: false,
            create_new: false,
            end_pos: true,
            alloc_size: None,
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

    /// 设置新建文件时的初始分配大小。
    ///
    /// 已知大小传 `Some(len)`：按 128B 精确对齐分配，不浪费空间；
    /// `None` 表示未知大小：按 4MiB 整块分配，多余空间由 GC 回收。
    /// 仅对 `create_new(true)` 生效；打开已存在的文件时忽略（不能缩小已有分配）。
    ///
    /// Set the initial allocation size when creating a new file.
    /// `Some(len)` (known size): exact 128B-aligned allocation, no waste;
    /// `None` (unknown size): whole 4MiB blocks, excess reclaimed by GC.
    /// Only applies with `create_new(true)`; ignored when opening an existing file.
    pub fn alloc_size(mut self, alloc_size: Option<u64>) -> Self {
        self.alloc_size = alloc_size;
        self
    }

    /// 根据配置打开虚拟文件。
    ///
    /// 根据 `read`/`write` 标志确定访问模式，并检查同步管理器的访问模式兼容性。
    /// 如果 `create_new` 为 `true` 则创建新文件，否则打开已存在的文件。
    ///
    /// Open a virtual file according to the configured options.
    ///
    /// Determines the access mode from `read`/`write` flags and checks synchronized manager
    /// access mode compatibility. If `create_new` is `true`, creates a new file;
    /// otherwise opens an existing one.
    pub fn open<P: AsRef<Path>>(
        &mut self,
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

        PackVirtualFile::check_sync_compat(self.sync, access_mode)?;

        if self.create_new {
            let mut vf = self.sync.create_virtual_file_impl(path, self.alloc_size)?;
            vf.access_mode = access_mode;
            Ok(vf)
        } else {
            let mut vf = self.sync.open_virtual_file_impl(path, self.end_pos)?;
            vf.access_mode = access_mode;
            Ok(vf)
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::PathBuf;
    use std::thread;

    /// 其他线程持虚拟文件句柄锁时 panic 使锁 poison；PackVirtualFile 通过
    /// `unwrap_or_else(PoisonError::into_inner)` 恢复后仍可正常读写。
    ///
    /// A thread panics while holding the virtual-file handle lock, poisoning it;
    /// PackVirtualFile recovers via into_inner and stays fully usable.
    #[test]
    fn handle_lock_poison_recovered() {
        let dir = PathBuf::from("./temp/test/wb_files_pack/pack_io/ok/handle_lock_poison_recovered");
        crate::tools::TestTool::prepare_test_dir(&dir);
        let pack = dir.join("pack");
        {
            let mut alloc = crate::tools::TestTool::expect_ok(
                ManagerSync::options()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .cow(false)
                    .separate_manifest(true)
                    .open(&pack),
                "打开测试包失败",
            );
            let mut vf = crate::tools::TestTool::expect_ok(
                alloc
                    .virtual_file_options()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open("poison_file"),
                "打开虚拟文件失败",
            );
            crate::tools::TestTool::expect_ok(vf.set_len(4), "设置长度失败");
            crate::tools::TestTool::expect_ok(vf.write_all(&[1, 2, 3, 4]), "写入初始数据失败");
            crate::tools::TestTool::expect_ok(vf.seek(SeekFrom::Start(0)), "定位到开头失败");

            // 子线程持句柄锁并故意 panic → 锁 poison
            let h = vf.handle.clone();
            let h_thread = h.clone();
            let handle = thread::spawn(move || {
                let _guard = h_thread.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                panic!("intentional panic while holding virtual file handle lock");
            });
            // 预期错误 1：线程必须 panic，且 panic 消息必须与预期完全一致；
            // 若未 panic 或消息不同（非预期错误）则测试失败
            let payload = handle.join().expect_err("子线程应 panic 崩溃");
            assert_eq!(
                payload.downcast_ref::<&str>(),
                Some(&"intentional panic while holding virtual file handle lock"),
                "panic 消息应为预期的 intentional panic"
            );
            assert!(h.is_poisoned(), "虚拟文件句柄锁应处于 poison 状态");

            // 预期错误 2（恢复前）：lock() 必须返回预期的 PoisonError；
            // 若返回 Ok 说明锁未被 poison（panic 未生效）→ 测试立即失败；
            // 确认为预期错误后再用 into_inner 恢复锁
            let guard = match h.lock() {
                Ok(_) => panic!("非预期：句柄锁未被 poison，lock() 返回 Ok"),
                Err(poison) => poison.into_inner(), // 预期错误：PoisonError → 恢复锁
            };
            drop(guard);

            // 恢复验证：句柄锁相关的长度查询/读/写均可用
            assert_eq!(vf.get_len(), 4);
            let mut buf = vec![0u8; 4];
            crate::tools::TestTool::expect_ok(vf.read_exact(&mut buf), "读取初始数据失败");
            assert_eq!(buf, vec![1, 2, 3, 4]);
            crate::tools::TestTool::expect_ok(vf.seek(SeekFrom::Start(0)), "重新定位到开头失败");
            crate::tools::TestTool::expect_ok(vf.write_all(&[9, 9, 9, 9]), "覆写数据失败");
            crate::tools::TestTool::expect_ok(vf.seek(SeekFrom::Start(0)), "再次定位到开头失败");
            let mut buf2 = vec![0u8; 4];
            crate::tools::TestTool::expect_ok(vf.read_exact(&mut buf2), "读取覆写后数据失败");
            assert_eq!(buf2, vec![9, 9, 9, 9]);
        }
        crate::tools::TestTool::cleanup_test_dir(&dir);
    }
}
