/*
开始时间：26/02/13 11：31
 */
use crate::wb_files_pack::error::Result;
use crate::wb_files_pack::pack_io::file_handle::PackFileHandle;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

/// 虚拟文件读写器 / Virtual file reader-writer
///
/// 提供包内文件的读写操作，管理文件位置、元数据和哈希计算。
/// Provides read/write operations for files within a pack, managing file position, metadata, and hash computation.
pub struct PackFileWR {
    /// 当前文件位置 / Current file position
    pos: u64,
    ///虚拟文件句柄
    handle: Arc<Mutex<PackFileHandle>>,
}

impl PackFileWR {
    /// 创建虚拟文件读写器。
    ///
    /// `new` = `true` 表示新创建的文件（写入模式），`false` 表示打开已存在的文件。
    ///
    /// Create a virtual file read-writer.
    ///
    /// `new` = `true` means newly created file (write mode), `false` means opening an existing file.
    pub(in crate::wb_files_pack) fn create(pos: u64, handle: Arc<Mutex<PackFileHandle>>) -> Self {
        Self { pos, handle }
    }

    /// 返回虚拟文件的总长度（字节）。
    /// Returns the total length of this virtual file in bytes.
    //获取大小
    pub fn get_len(&self) -> Result<u64> {
        Ok(self.handle.lock().expect("无法获得文件句柄锁").get_len())
    }

    pub fn get_modified(&self) -> Result<u128> {
        Ok(self.handle.lock().expect("无法获得文件句柄锁").get_modified())
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
        let mut handle = handle.lock().expect("无法获得文件句柄锁");
        handle.set_len(len)?;
        Ok(())
    }

    pub fn set_modified(&mut self, modified: u128) -> Result<()> {
        let mut handle = self.handle.lock().expect("无法获得文件句柄锁");
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
        let mut handle = handle.lock().expect("无法获得文件句柄锁");
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
impl Seek for PackFileWR {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(pos) => {
                self.pos = pos;
            }
            SeekFrom::Current(pos) => {
                self.add_pos_i64(pos);
            }
            SeekFrom::End(pos) => {
                let end = self.get_len().map_err(io::Error::other)?;
                self.set_pos(end);
                self.add_pos_i64(pos);
            }
        }
        Ok(self.pos)
    }
}

impl Read for PackFileWR {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let handle = self.handle.clone();
        let mut handle = handle.lock().expect("无法获得文件句柄锁");
        let len = handle.read(self.pos, buf).map_err(io::Error::other)?;
        self.add_pos(len as u64);
        Ok(len)
    }
}

impl Write for PackFileWR {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let handle = self.handle.clone();
        let mut handle = handle.lock().expect("无法获得文件句柄锁");
        let len = handle.write(self.pos, buf).map_err(io::Error::other)?;
        self.add_pos(len as u64);
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        let handle = self.handle.clone();
        let mut handle = handle.lock().expect("无法获得文件句柄锁");
        handle.flush().map_err(io::Error::other)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write(buf)?;
        Ok(())
    }
}
