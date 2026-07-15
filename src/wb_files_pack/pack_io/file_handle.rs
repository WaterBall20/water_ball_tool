/*
开始时间：2026/06/26 08:42
*/
use crate::wb_files_pack::manager::WBFPManager;
use crate::wb_files_pack::pack_io::file_hash::PackFileHash;
use crate::wb_files_pack::pack_io::PackIO;
use crate::wb_files_pack::{
    PackFileMetadata,
    PackFileMetadataType,
    DATA_BLOCK_LEN,
    DATA_DATA_BLOCK_LEN,
};
use std::io::{self, Read, Seek, SeekFrom, Write};
use crate::wb_files_pack::error::{Result, PackFileError};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub(crate) struct PackFileHandle {
    /// 管理器实例 / Manager instance
    manager: Arc<Mutex<WBFPManager>>,
    /// 包文件 IO 实例 / Pack file IO instance
    pack_io: Arc<Mutex<PackIO>>,
    /// 当前文件位置 / Current file position
    pos: u64,
    /// 缓存：文件分配位置的当前索引 / Cache: current index in allocated positions
    temp_pos_index: usize,
    /// 缓存：当前分配位置已占用大小 / Cache: used size at current allocated position
    temp_pos_this_len: u64,
    /// 虚拟路径 / Virtual path
    path_list: Option<Vec<String>>,
    /// 文件元数据 / File metadata
    metadata: Option<PackFileMetadata>,
    /// 是否为写入模式 / Whether in write mode
    is_write: bool,
}

impl PackFileHandle {
    /// 创建虚拟文件读写器。
    ///
    /// `new` = `true` 表示新创建的文件（写入模式），`false` 表示打开已存在的文件。
    ///
    /// Create a virtual file read-writer.
    ///
    /// `new` = `true` means newly created file (write mode), `false` means opening an existing file.
    pub(in crate::wb_files_pack) fn create(
        new: bool,
        manager: Arc<Mutex<WBFPManager>>,
        pack_io: &Arc<Mutex<PackIO>>,
        path_list: Vec<String>,
        metadata: PackFileMetadata,
        end_pos: bool,
    ) -> Result<Self> {
        Ok(Self {
            manager,
            pack_io: pack_io.clone(),
            pos: if end_pos {
                metadata.len()
            } else {
                0
            },
            temp_pos_index: 0,
            temp_pos_this_len: 0,
            path_list: Some(path_list),
            metadata: Some(metadata),
            is_write: new,
        })
    }

    /// 返回虚拟文件的总长度（字节）。
    /// Returns the total length of this virtual file in bytes.
    //获取大小
    pub fn get_len(&self) -> u64 {
        if let Some(metadata) = &self.metadata {
            metadata.len()
        } else {
            panic!("逻辑错误");
        }
    }

    //获取位置列表
    fn get_pos_list(&self, pos: u64, is_read: bool) -> Result<Vec<(u64, u64)>> {
        self.get_add_pos_list(0, 0, 0, pos, is_read)
    }

    //获取追加位置列表
    fn get_add_pos_list2(&self, add_pos: u64, is_read: bool) -> Result<Vec<(u64, u64)>> {
        self.get_add_pos_list(
            self.temp_pos_index,
            self.temp_pos_this_len,
            self.pos,
            add_pos,
            is_read,
        )
    }
    fn get_add_pos_list(
        &self,
        start_pos_list_item_index: usize,
        start_pos_list_item_len: u64,
        start_pos: u64,
        add_pos: u64,
        is_read: bool,
    ) -> Result<Vec<(u64, u64)>> {
        let mut pos_index = start_pos_list_item_index;
        let mut r_pos: Vec<(u64, u64)> = Vec::new();
        let mut m_add_len: u64 = 0;
        while
        let Some(metadata) = &self.metadata &&
            let PackFileMetadataType::File { data_pos_list, .. } = metadata.file_type() &&
            pos_index < data_pos_list.list().len()
        {
            let (mut pos, mut len) = *data_pos_list.list().get(pos_index).unwrap();
            //当前校准
            if pos_index == start_pos_list_item_index {
                //位置偏移
                pos += start_pos_list_item_len;
                //减去已偏移的长度
                len -= start_pos_list_item_len;
            }
            //计算
            if add_pos - m_add_len <= len {
                //小于等于直接添加并直接返回
                r_pos.push((pos, add_pos));
                return Ok(r_pos);
            }
            //大于就添加完所有空闲块
            //读取额外判断
            if is_read && start_pos + len > metadata.len() {
                len = metadata.len() - start_pos;
            }
            r_pos.push((pos, len));
            //增值
            m_add_len += len;
            //附加索引
            pos_index += 1;
        }
        Ok(r_pos)
    }

    //增加分配大小
    fn add_running_len(&mut self, add_len: u64) -> Result<()> {
        if
        let Some(metadata) = &mut self.metadata &&
            let PackFileMetadataType::File { data_pos_list, .. } = metadata.file_type_mut()
        {
            let pack_file = self.pack_io.clone();
            let mut pack_file = pack_file
                .lock()
                .map_err(|e| PackFileError::Lock(format!("无法获得包文件锁, err:{e}")))?;
            data_pos_list.list_mut().push(pack_file.get_file_pos(add_len));
        }
        Ok(())
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
        const DATA_BLOCK_LEN_U64: u64 = DATA_BLOCK_LEN as u64;
        //提前读取len避免借用冲突
        let metadata_len = self.metadata.as_ref().map(|m| m.len());
        //大小判断
        if
        let Some(metadata) = &mut self.metadata &&
            let PackFileMetadataType::File { data_pos_list, .. } = metadata.file_type_mut()
        {
            let pack_file = self.pack_io.clone();
            let mut pack_file = pack_file
                .lock()
                .map_err(|e| PackFileError::Lock(format!("无法获得包文件锁, err:{e}")))?;
            let old_metadata_len = metadata_len.unwrap_or(0);
            if len > old_metadata_len {
                //增加大小
                let add_len = len - old_metadata_len;
                //获取分配
                let add_pos = pack_file.get_file_pos(add_len);
                data_pos_list.list_mut().push(add_pos);
                metadata.add_len(add_len);
            } else {
                //减少大小
                //更新数据块列表和垃圾回收提交
                let mut back_len_cnt = 0u64;

                let mut new_pos_list = Vec::new();
                let mut gc_list = Vec::new();
                for value in data_pos_list.list() {
                    let (pos, item_len) = *value;
                    back_len_cnt += item_len;
                    let back_len_c = back_len_cnt / DATA_BLOCK_LEN_U64;
                    let this_back_len_c = len.div_ceil(DATA_BLOCK_LEN_U64);
                    //大于实际大小
                    if back_len_c > this_back_len_c {
                        let s_len = (back_len_c - this_back_len_c) * DATA_BLOCK_LEN_U64;
                        if item_len > s_len {
                            //删除的大小小于快大小
                            new_pos_list.push((pos, item_len - s_len));
                            gc_list.push((pos + s_len, s_len));
                        } else {
                            //删除的大小等于快大小
                            assert_eq!(s_len, item_len); //逻辑判断
                            //不执行任何操作
                        }
                    } else {
                        new_pos_list.push(*value);
                    }
                }
                //更新元数据
                *data_pos_list.list_mut() = new_pos_list;
                //垃圾提交
                pack_file.file_gc_add(gc_list);
            }
        }
        // Now update metadata length outside the file_type_mut borrow
        if let Some(metadata) = &mut self.metadata {
            metadata.set_len(len);
        }
        Ok(())
    }

    //设置文件位置
    fn set_pos(&mut self, pos: u64) -> Result<()> {
        //缓存处理===
        if self.pos != pos {
            //获取需要添加的块列表
            let pos_s = self.get_pos_list(pos, false)?;
            //块索引
            let pos_index = pos_s.len() - 1;
            //块长度
            let (_, pos_len) = pos_s.get(pos_index).unwrap();
            self.temp_pos_index = pos_index;
            self.temp_pos_this_len = *pos_len;
            self.pos = pos;
        }
        Ok(())
    }

    //追加文件位置
    fn add_pos(&mut self, length: u64) -> Result<()> {
        //获取需要添加的块列表
        self.add_pos2(length, &self.get_add_pos_list2(length, false)?);
        Ok(())
    }
    fn add_pos2(&mut self, length: u64, add_pos_s: &[(u64, u64)]) {
        //需要添加的索引数
        let add_pos_index = add_pos_s.len() - 1;
        //缓存_当前块添加的大小
        let (_, add_pos_len) = add_pos_s.get(add_pos_index).unwrap();
        //更新位置缓存
        self.temp_pos_index += add_pos_index;
        if add_pos_index == 0 {
            //如果不改变索引则直接追加
            self.temp_pos_this_len += *add_pos_len;
        } else {
            //更改索引则替换
            self.temp_pos_this_len = *add_pos_len;
        }
        //更新位置
        self.pos += length;
    }

    //减少文件位置 / Move file position backward
    fn sub_pos(&mut self, mut length: u64) -> Result<()> {
        if length == 0 {
            return Ok(());
        }
        if length > self.pos {
            length = self.pos;
        }
        let new_pos = self.pos - length;
        // 如果仍在当前数据块内，直接递减偏移量（快速路径）
        // If still within the current data block, just decrement the offset (fast path)
        if length <= self.temp_pos_this_len {
            self.temp_pos_this_len -= length;
            self.pos = new_pos;
        } else {
            // 跨越块边界，从头计算位置 / Crossed block boundary, recalculate from new position
            self.set_pos(new_pos)?;
        }
        Ok(())
    }

    /// 验证文件数据的完整性哈希。
    ///
    /// 从文件开头重新读取全部数据计算哈希，与元数据中存储的哈希值比较。
    ///
    /// Verify the integrity hash of the file data.
    ///
    /// Reads all data from the beginning, computes the hash, and compares with the stored hash value.
    pub fn verify_hash(&mut self) -> Result<bool> {
        //缓冲区
        let old_pos = self.pos;
        let Some(metadata) = &self.metadata else {
            panic!("逻辑错误，实例未释放时元数据实例不存在")
        };
        //
        if let PackFileMetadataType::File { hash_type, hash_value, .. } = metadata.file_type() {
            let in_hash = PackFileHash::from_new(*hash_type, hash_value.as_slice());
            //循环读取
            let mut read_hash = self.read_hash_v()?;
            self.set_pos(old_pos)?;
            Ok(read_hash.eq(&in_hash)?)
        } else {
            panic!("逻辑错误，元数据不是文件类型")
        }
    }

    fn read_hash_v(&mut self) -> Result<PackFileHash> {
        //计算哈希
        if
        let Some(metadata) = &self.metadata &&
            let PackFileMetadataType::File { hash_type, .. } = metadata.file_type()
        {
            let len = metadata.len();
            let hash_type = *hash_type;
            let mut buf = vec![0; usize::try_from(DATA_DATA_BLOCK_LEN).unwrap()];
            //设置位置
            self.set_pos(0)?;
            let mut read_hash = PackFileHash::new(hash_type);
            let mut hash_read_len = 0;
            while hash_read_len < len {
                let this_read_len = self.read(self.pos, &mut buf)?;
                read_hash.update(&buf[..this_read_len]);
                hash_read_len += this_read_len as u64;
            }
            Ok(read_hash)
        } else {
            panic!("逻辑错误")
        }
    }

    fn commit_data(&mut self) -> Result<()> {
        //计算哈希
        if self.is_write {
            let read_hash = self.read_hash_v();
            if
            let Ok(read_hash) = read_hash &&
                let Some(metadata) = &mut self.metadata &&
                let PackFileMetadataType::File { hash_value, .. } = metadata.file_type_mut()
            {
                *hash_value = read_hash.get_hash_value();
            }
        }
        //预分配空间释放
        if let Some(metadata) = &self.metadata {
            let metadata_len = metadata.len();
            self.set_len(metadata_len)?; //通过设置大小触发释放
        }
        //返还元数据
        if let Some(path_list) = self.path_list.take() && let Some(metadata) = self.metadata.take() {
            let manager = self.manager.clone();
            let mut manager = manager
                .lock()
                .map_err(|e| PackFileError::Lock(format!("无法获得管理器锁, err:{e}")))?;
            manager.file_metadata_update(path_list, metadata)?;
        }
        Ok(())
    }
}

impl Drop for PackFileHandle {
    fn drop(&mut self) {
        _ = self.commit_data();
    }
}

impl Seek for PackFileHandle {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(pos) => {
                self.set_pos(pos).map_err(io::Error::other)?;
                Ok(pos)
            }
            SeekFrom::Current(pos) =>
                match pos {
                    0 => Ok(self.pos),
                    //大于0
                    1.. => {
                        self.add_pos(pos.cast_unsigned()).map_err(io::Error::other)?;
                        Ok(self.pos)
                    }
                    //小于0
                    ..0 => {
                        self.sub_pos((-pos).cast_unsigned()).map_err(io::Error::other)?;
                        Ok(self.pos)
                    }
                }
            SeekFrom::End(pos) => {
                let end = self.get_len();
                match pos {
                    0 => {
                        self.set_pos(end).map_err(io::Error::other)?;
                        Ok(end)
                    }
                    1.. => {
                        let add = pos.cast_unsigned();
                        let new_end = end + add;
                        self.set_len(new_end).map_err(io::Error::other)?;
                        self.set_pos(new_end).map_err(io::Error::other)?;
                        Ok(new_end)
                    }
                    ..0 => {
                        let sub = (-pos).cast_unsigned();
                        let new_pos = end.saturating_sub(sub);
                        self.set_pos(new_pos).map_err(io::Error::other)?;
                        Ok(new_pos)
                    }
                }
            }
        }
    }
}

impl PackFileHandle {
    pub fn read(&mut self, pos: u64, buf: &mut [u8]) -> Result<usize> {
        self.set_pos(pos)?;
        let pack_file = self.pack_io.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得包文件锁, err:{e}")))?;
        //当前大小所需的位置列表
        let pos_s = self.get_add_pos_list2(buf.len() as u64, true)?;
        //当前已读取大小
        let mut read_len = 0;
        //读取
        for (pos, len) in pos_s {
            let this_buf = &mut buf[read_len..read_len + usize::try_from(len).unwrap()];

            //更改文件位置
            pack_file.set_pos_read(pos)?;
            //读取数据
            pack_file.read_exact(this_buf)?;
            read_len += usize::try_from(len).unwrap();
        }
        self.add_pos(read_len as u64)?;
        Ok(read_len)
    }
}

impl PackFileHandle {
    pub fn write(&mut self, pos: u64, buf: &[u8]) -> Result<usize> {
        self.is_write = true;
        self.set_pos(pos)?;
        let pack_file = self.pack_io.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得包文件锁, err:{e}")))?;
        //当前大小所需的位置列表
        let mut pos_s = self.get_add_pos_list2(buf.len() as u64, false)?;
        //如果没有空间就尝试分配
        if pos_s.is_empty() {
            let data_len = buf.len() as u64;
            let add_running_len = (data_len / DATA_DATA_BLOCK_LEN + 1) * DATA_DATA_BLOCK_LEN;
            self.add_running_len(add_running_len)?;
            pos_s = self.get_add_pos_list2(buf.len() as u64, false)?;
            assert!(!pos_s.is_empty());
        }
        //当前已写入大小
        let mut write_len = 0;
        //写入
        for (pos, len) in pos_s {
            let len = usize::try_from(len).unwrap();
            let this_data = &buf[write_len..write_len + len];

            //更改文件位置
            pack_file.set_pos_write(pos)?;
            //写入数据
            pack_file.write_all(this_data)?;

            write_len += len;

            //TODO:未来功能：写入优化、写时复制
        }
        self.add_pos(write_len as u64)?;
        //大小判断，更新大小
        if let Some(metadata) = &mut self.metadata && self.pos > metadata.len() {
            metadata.set_len(self.pos);
        }
        Ok(write_len)
    }

    pub fn flush(&mut self) -> Result<()> {
        self.pack_io
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得包文件锁, err:{e}")))?
            .flush()?;
        Ok(())
    }
}
