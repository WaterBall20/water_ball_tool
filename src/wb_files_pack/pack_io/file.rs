/*
开始时间：26/02/13 11：31
 */
use crate::wb_files_pack::manager::WBFPManager;
use crate::wb_files_pack::pack_io::PackIO;
use crate::wb_files_pack::{PackFileMetadata, PackFileMetadataType, DATA_DATA_BLOCK_LEN};
use blake3::{Hash, Hasher};
use std::fs::File;
use std::io;
use std::io::{Error, Read, Seek, SeekFrom, Write};
#[cfg(not(target_os = "windows"))]
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Mutex};

pub struct PackFileWR {
    //管理器实例
    manager: Arc<Mutex<WBFPManager>>,
    //包文件io
    pack_io: Arc<Mutex<PackIO>>,
    //包文件文件实例
    pack_file: File,
    //文件位置
    pos: u64,
    //缓存_文件分配的位置当前索引
    temp_pos_index: usize,
    //缓存_文件分配的当前位置已占用大小
    temp_pos_this_len: u64,
    //虚拟路径
    path_list: Option<Vec<String>>,
    //元数据
    metadata: Option<PackFileMetadata>,
}
#[derive(Debug, Clone)]
pub enum PackFileHash {
    None,
    Blake3 {
        hasher: Box<Hasher>,
        hash_value: Vec<u8>,
    },
}

impl PackFileHash {
    fn new(type_: u8) -> Self {
        match type_ {
            1 => Self::Blake3 {
                hasher: Box::new(Hasher::new()),
                hash_value: Vec::new(),
            },
            _ => Self::None,
        }
    }
    fn from_new(type_: u8, value: &[u8]) -> Self {
        match type_ {
            1 => Self::Blake3 {
                hasher: Box::new(Hasher::new()),
                hash_value: value.to_vec(),
            },
            _ => Self::None,
        }
    }
}

impl PackFileHash {
    fn _to_u8_type(&self) -> u8 {
        match self {
            Self::None => 0,
            Self::Blake3 { .. } => 1,
        }
    }

    fn update(&mut self, input: &[u8]) {
        match self {
            PackFileHash::Blake3 { hasher, .. } => {
                hasher.update(input);
            }
            PackFileHash::None => (),
        }
    }

    fn get_hash_value(&self) -> Vec<u8> {
        match self {
            Self::None => Vec::new(),
            Self::Blake3 { hasher, .. } => {
                let this_hash = hasher.finalize();
                this_hash.as_bytes().to_vec()
            }
        }
    }

    fn eq(&mut self, other: &PackFileHash) -> io::Result<bool> {
        match self {
            Self::None => Err(Error::other("无法对没有哈希计算的进行比较")),
            Self::Blake3 { hasher, .. } => {
                if let Self::Blake3 { hash_value, .. } = other {
                    let hash = hasher.finalize();
                    let other_hash = Hash::from_slice(hash_value)
                        .map_err(|e| Error::other(format!("比较发生错误，err: {e}")))?;
                    Ok(hash == other_hash)
                } else {
                    Err(Error::other("不能对不同类型进行比较"))
                }
            }
        }
    }
}

impl PackFileWR {
    pub(in crate::wb_files_pack) fn create(
        manager: Arc<Mutex<WBFPManager>>,
        pack_io: Arc<Mutex<PackIO>>,
        path_list: Vec<String>,
        metadata: PackFileMetadata,
    ) -> io::Result<PackFileWR> {
        let pack_file = pack_io.clone().lock().unwrap().try_clone_pack_file()?;
        Ok(PackFileWR {
            manager,
            pack_io,
            pack_file,
            pos: 0,
            temp_pos_index: 0,
            temp_pos_this_len: 0,
            path_list: Some(path_list),
            metadata: Some(metadata),
        })
    }

    //获取大小
    pub fn get_len(&self) -> u64 {
        if let Some(metadata) = &self.metadata {
            metadata.len
        } else {
            panic!("逻辑错误");
        }
    }

    //获取位置列表
    fn get_pos_list(&self, pos: u64, is_read: bool) -> io::Result<Vec<(u64, u64)>> {
        self.get_add_pos_list(0, 0, 0, pos, is_read)
    }

    //获取追加位置列表
    fn get_add_pos_list2(&self, add_pos: u64, is_read: bool) -> io::Result<Vec<(u64, u64)>> {
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
    ) -> io::Result<Vec<(u64, u64)>> {
        let mut pos_index = start_pos_list_item_index;
        let mut r_pos: Vec<(u64, u64)> = Vec::new();
        let mut m_add_len: u64 = 0;
        while let Some(metadata) = &self.metadata
            && let PackFileMetadataType::File { data_pos_list, .. } = &metadata.file_type
            && pos_index < data_pos_list.list.len()
        {
            let (mut pos, mut len) = *data_pos_list.list.get(start_pos_list_item_index).unwrap();
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
            if is_read && start_pos + len > metadata.len {
                len = metadata.len - start_pos;
            }
            r_pos.push((pos, len));
            //增值
            m_add_len += len;
            //附加索引
            pos_index += 1;
        }
        if is_read {
            Ok(r_pos)
        } else {
            Err(Error::other("空间越界"))
        }
    }

    //设置文件大小
    //TODO:动态扩容实现
    /*fn _set_len(&mut self, _manager: &mut WBFPManager) -> io::Result<()> {
        Err(Error::other("未实现动态扩容"))
    }*/

    //设置文件位置
    fn set_pos(&mut self, pos: u64) -> io::Result<()> {
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
    fn add_pos(&mut self, length: u64) -> io::Result<()> {
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

    //减少文件位置
    fn sub_pos(&mut self, length: u64) -> io::Result<()> {
        self.sub_pos2(length, Vec::new())
    }
    fn sub_pos2(&mut self, length: u64, _sub_pos_s: Vec<(u64, u64)>) -> io::Result<()> {
        //TODO：暂时使用从头计算，可能存在性能损失，部分功能未实现
        let r_pos = self.pos.cast_signed() - length.cast_signed();
        if r_pos < 0 {
            self.set_pos(0)?;
            Ok(())
        } else {
            self.set_pos(r_pos.cast_unsigned())?;
            Ok(())
        }
    }

    //
    pub fn verify_hash(&mut self) -> io::Result<bool> {
        //缓冲区
        let old_pos = self.pos;
        let Some(metadata) = &self.metadata else {
            panic!("逻辑错误，实例未释放时元数据实例不存在")
        };
        //
        if let PackFileMetadataType::File {
            hash_type,
            hash_value,
            ..
        } = &metadata.file_type
        {
            let in_hash = PackFileHash::from_new(*hash_type, hash_value);
            //循环读取
            let mut read_hash = self.read_hash_v()?;
            self.set_pos(old_pos)?;
            Ok(read_hash.eq(&in_hash)?)
        } else {
            panic!("逻辑错误，元数据不是文件类型")
        }
    }

    fn read_hash_v(&mut self) -> io::Result<PackFileHash> {
        //计算哈希
        if let Some(metadata) = &self.metadata
            && let PackFileMetadataType::File { hash_type, .. } = &metadata.file_type
        {
            let len = metadata.len;
            let hash_type = *hash_type;
            let mut buf = vec![0; usize::try_from(DATA_DATA_BLOCK_LEN).unwrap()];
            //设置位置
            self.set_pos(0)?;
            let mut read_hash = PackFileHash::new(hash_type);
            let mut hash_read_len = 0;
            while hash_read_len < len {
                let this_read_len = self.read(&mut buf)?;
                read_hash.update(&buf[..this_read_len]);
                hash_read_len += this_read_len as u64;
            }
            Ok(read_hash)
        } else {
            panic!("逻辑错误")
        }
    }

    pub fn submit(self) -> io::Result<()> {
        let mut m = self;
        m.commit_data()
    }

    fn commit_data(&mut self) -> io::Result<()> {
        //计算哈希
        let read_hash = self.read_hash_v();
        if let Ok(read_hash) = read_hash
            && let Some(metadata) = &mut self.metadata
            && let PackFileMetadataType::File { hash_value, .. } = &mut metadata.file_type
        {
            *hash_value = read_hash.get_hash_value();
        }
        //返还元数据
        if let Some(path_list) = self.path_list.take()
            && let Some(metadata) = self.metadata.take()
        {
            let manager = self.manager.clone();
            let mut manager = manager
                .lock()
                .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
            manager.file_metadata_update(path_list, metadata)?;
        }
        Ok(())
    }
}

impl Drop for PackFileWR {
    fn drop(&mut self) {
        _ = self.commit_data();
    }
}

impl Seek for PackFileWR {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(pos) => {
                self.set_pos(pos)?;
                Ok(pos)
            }
            SeekFrom::Current(pos) => match pos {
                0 => Ok(self.pos),
                //大于0
                1.. => {
                    self.add_pos(pos.cast_unsigned())?;
                    Ok(self.pos)
                }
                //小于0
                ..0 => {
                    self.sub_pos((-pos).cast_unsigned())?;
                    Ok(self.pos)
                }
            },
            SeekFrom::End(pos) => match pos {
                0 => Ok(self.pos),
                1.. => {
                    self.sub_pos((-pos).cast_unsigned())?;
                    Ok(self.pos)
                }
                ..0 => Err(Error::other("未实现动态扩容")),
            },
        }
    }
}

impl Read for PackFileWR {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(target_os = "windows")]
        let pack_file = self.pack_io.clone();
        #[cfg(target_os = "windows")]
        let mut pack_file = pack_file
            .lock()
            .map_err(|e| Error::other(format!("无法获得包文件锁, err:{e}")))?;
        //当前大小所需的位置列表
        let pos_s = self.get_add_pos_list2(buf.len() as u64, true)?;
        //当前已读取大小
        let mut read_len = 0;
        //读取
        for (pos, len) in pos_s {
            let this_buf = &mut buf[read_len..read_len + usize::try_from(len).unwrap()];
            #[cfg(target_os = "windows")]
            {
                //更改文件位置
                pack_file.set_pos_read(pos)?;
                //读取数据
                pack_file.read_exact(this_buf)?;
            }
            #[cfg(not(target_os = "windows"))]
            self.pack_file.read_exact_at(this_buf, pos)?;
            read_len += usize::try_from(len).unwrap();
        }
        self.add_pos(read_len as u64)?;
        Ok(read_len)
    }
}

impl Write for PackFileWR {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        #[cfg(target_os = "windows")]
        let manager = self.manager.clone();
        #[cfg(target_os = "windows")]
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        #[cfg(target_os = "windows")]
        manager.this_write_lock()?;
        #[cfg(target_os = "windows")]
        let pack_file = self.pack_io.clone();
        #[cfg(target_os = "windows")]
        let mut pack_file = pack_file
            .lock()
            .map_err(|e| Error::other(format!("无法获得包文件锁, err:{e}")))?;
        //当前大小所需的位置列表
        let pos_s = self.get_add_pos_list2(buf.len() as u64, false)?;
        //当前已写入大小
        let mut write_len = 0;
        //写入
        for (pos, len) in pos_s {
            let len = usize::try_from(len).unwrap();
            let this_data = &buf[write_len..write_len + len];
            #[cfg(target_os = "windows")]
            {
                //更改文件位置
                pack_file.set_pos_write(pos)?;
                //写入数据
                pack_file.write_all(this_data)?;
            }
            #[cfg(not(target_os = "windows"))]
            self.pack_file.write_all_at(this_data, pos)?;
            write_len += len;

            //TODO:未来功能：写入优化、写时复制
        }
        self.add_pos(write_len as u64)?;
        Ok(write_len)
    }

    fn flush(&mut self) -> io::Result<()> {
        todo!()
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write(buf)?;
        Ok(())
    }
}
