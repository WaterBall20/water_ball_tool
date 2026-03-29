use crate::tools;
use crate::wb_files_pack::{
    DataPosList, ManifestDataBlock, MANIFEST_ATTRIBUTE_BLOCK_LEN, MANIFEST_DATA_BLOCK_LEN,
};
use std::fs::File;
use std::io;
use std::io::{Error, Read, Seek, SeekFrom, Write};

pub mod file;

//文件头===
//文件头-文件名:WPFilesPack
pub(in crate::wb_files_pack) const FILE_HEADER_TYPE_NAME: [u8; 11] = [
    0x57u8, 0x42, 0x46, 0x69, 0x6c, 0x65, 0x73, 0x50, 0x61, 0x63, 0x6b,
];
//文件头版本
pub(in crate::wb_files_pack) const FILE_HEADER_VERSION_INDEX: usize = FILE_HEADER_TYPE_NAME.len();
pub(in crate::wb_files_pack) const FILE_HEADER_VERSION: [u8; 2] = [0, 2];
//文件头标签位长度
pub(in crate::wb_files_pack) const FILE_HEADER_BOOL_DATA_INDEX: usize =
    FILE_HEADER_VERSION_INDEX + FILE_HEADER_VERSION.len();
pub(in crate::wb_files_pack) const FILE_HEADER_BOOL_DATA_LENGTH: usize = 1;

//文件头数据长度位置
pub(in crate::wb_files_pack) const FILE_HEADER_DATA_LENGTH_INDEX: usize =
    FILE_HEADER_BOOL_DATA_INDEX + FILE_HEADER_BOOL_DATA_LENGTH;

//文件头数据长度长度
pub(in crate::wb_files_pack) const FILE_HEADER_DATA_LENGTH_LENGTH: usize = 8;

//文件头长度
pub(in crate::wb_files_pack) const FILE_HEADER_DATA_LENGTH: usize = FILE_HEADER_TYPE_NAME.len()
    + FILE_HEADER_VERSION.len()
    + FILE_HEADER_BOOL_DATA_LENGTH
    + FILE_HEADER_DATA_LENGTH_LENGTH;

//文件头清单属性
pub(in crate::wb_files_pack) const FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX: usize =
    MANIFEST_DATA_BLOCK_LEN;

//文件头块长度
pub(in crate::wb_files_pack) const FILE_HEADER_BLOCK_LEN: usize =
    MANIFEST_DATA_BLOCK_LEN + MANIFEST_ATTRIBUTE_BLOCK_LEN;

#[derive(Default, Debug)]
pub(crate) struct RunData {
    pos: u64,
    pub(crate) all_write_len: u64,
    //上次总写入的长度
    pub(crate) last_all_write_len: u64,
    //运行时总创建文件数量
    pub(crate) all_cr_file_count: u64,
    //上次创建总创建文件数量
    pub(crate) last_all_cr_file_count: u64,
    //GC数据列表
    gc_data_pos_list: DataPosList,
}

#[derive(Debug)]
pub(crate) struct PackIO {
    file: File,
    pub(crate) len: u64,
    //空数据列表
    pub(crate) empty_data_list: DataPosList,
    pub(crate) run_data: RunData,
}
impl PackIO {
    pub(crate) fn new(file: File) -> Self {
        Self {
            file,
            len: 0,
            empty_data_list: DataPosList {
                data_block: Some(ManifestDataBlock::default()),
                list: Vec::new(),
            },
            run_data: RunData::default(),
        }
    }

    pub(crate) fn new2(file: File, len: u64) -> Self {
        Self {
            file,
            len,
            empty_data_list: DataPosList {
                data_block: Some(ManifestDataBlock::default()),
                list: Vec::new(),
            },
            run_data: RunData::default(),
        }
    }
}

impl Write for PackIO {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let file = &mut self.file;
        let len = file.write(buf)?;
        {
            let len = len as u64;
            self.run_data.pos += len;
            self.run_data.all_write_len += len;
        }
        self.up_len();
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let file = &mut self.file;
        file.write_all(buf)?;
        let len = buf.len() as u64;
        self.run_data.pos += len;
        self.run_data.all_write_len += len;
        self.up_len();
        Ok(())
    }
}
impl Read for PackIO {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.file.read_exact(buf)
    }
}

impl PackIO /*核心*/ {
    //垃圾回收提交
    pub(crate) fn file_gc_add(&mut self, gc_pos_list: Vec<(u64, u64)>) {
        for pos in gc_pos_list {
            //直接添加
            if pos.0 != 0 && pos.1 != 0 {
                self.run_data.gc_data_pos_list.list.push(pos);
            }
        }
    }
    //垃圾回收
    pub(crate) fn file_gc(&mut self) {
        //准备：排序
        let pos_gc_list = &self.run_data.gc_data_pos_list.list;
        let pos_list = &mut self.empty_data_list.list;
        'gc_for: for (gc_pos, gc_len) in pos_gc_list {
            let gc_pos = *gc_pos;
            let gc_len = *gc_len;
            //排序插入
            let mut j = 0;
            while j < pos_list.len() {
                let (pos, _) = *pos_list.get(j).unwrap();
                //插入判断
                if gc_pos < pos {
                    //如果位置在前
                    pos_list.insert(j, (gc_pos, gc_len));
                    continue 'gc_for;
                }
                j += 1;
            }
            pos_list.push((gc_pos, gc_len));
        }
        //清空缓存
        self.run_data.gc_data_pos_list.list.clear();

        //合并功能
        //当前索引
        let mut index = 0;
        //如果有下一个则循环
        while let Some(v) = pos_list.get(index + 1) {
            let (next_pos, next_len) = *v;

            //当前索引内容
            if let Some((this_pos, this_len)) = pos_list.get_mut(index) {
                let this_end_pos = *this_pos + *this_len;
                //检查，判断当前位置加当前长度是否等于下一个位置
                if this_end_pos == next_pos {
                    //合并，将下一个占用的大小加到当前大小
                    *this_len += next_len;
                    assert!(
                        this_pos.is_multiple_of(MANIFEST_DATA_BLOCK_LEN as u64)
                            && this_len.is_multiple_of(MANIFEST_DATA_BLOCK_LEN as u64)
                    );
                    let r = pos_list.remove(index + 1);
                    assert!(
                        r.0.is_multiple_of(MANIFEST_DATA_BLOCK_LEN as u64)
                            && r.1.is_multiple_of(MANIFEST_DATA_BLOCK_LEN as u64)
                    );
                } else {
                    //否则什么都不做，并附加索引
                    index += 1;
                }
            }
        }
    }

    //获取可用的文件位置
    pub(crate) fn get_file_pos(&mut self, length: u64) -> (u64, u64) {
        //块对齐
        const DATA_BLOCK_LEN_U64: u64 = MANIFEST_DATA_BLOCK_LEN as u64;
        let length = if length.is_multiple_of(DATA_BLOCK_LEN_U64) {
            length
        } else {
            let length = length / DATA_BLOCK_LEN_U64 + 1;
            length * DATA_BLOCK_LEN_U64
        };
        let empty_data_pos = &mut self.empty_data_list.list;

        let value = if let Some(value) = Self::get_pos_gc(length, empty_data_pos) {
            value
        } else {
            //扩容处理
            (self.len, length)
        };
        //分配空间
        let this_end_pos = value.0 + value.1;
        if this_end_pos > self.len {
            self.len = this_end_pos;
        }
        value
    }

    fn get_pos_gc(length: u64, empty_data_pos: &mut Vec<(u64, u64)>) -> Option<(u64, u64)> {
        //优先使用空数据，但必须完整一块
        let mut index = 0;
        while !empty_data_pos.is_empty() {
            //从第一个开始
            let Some((pos, len)) = empty_data_pos.get_mut(index) else {
                break;
            };
            //判断是否能占用完
            //剩余大小
            return Some(match (*len).cast_signed() - length.cast_signed() {
                //能占用完，等于
                0 => empty_data_pos.remove(index),
                //可用，比较大
                1.. => {
                    //不能则切出
                    let r = (*pos, length);
                    //修改，位置加大小使其向后移动，长度减大小使其边界不变
                    *pos += length;
                    *len -= length;
                    r
                }
                //不够
                ..0 => {
                    index += 1;
                    continue;
                }
            });
        }
        None
    }

    //更新文件大小
    pub(crate) fn up_len(&mut self) {
        //判断是否需要设置
        if self.run_data.pos > self.len {
            self.len = self.run_data.pos;
        }
    }

    //设置包文件大小
    pub(crate) fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len)?;
        self.len = len;
        self.up_len();
        Ok(())
    }

    pub(crate) fn _sync_data(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }

    pub(crate) fn try_clone_pack_file(&self) -> io::Result<File> {
        self.file
            .try_clone()
            .map_err(|e| Error::other(format!("尝试复制包文件实例失败，err: {e}")))
    }
}

impl PackIO /*读*/ {
    pub(crate) fn manifest_data_block_read(&self, file_pos: u64) -> io::Result<ManifestDataBlock> {
        let mut file = &self.file;
        let block_data_buf = vec![0; MANIFEST_DATA_BLOCK_LEN];
        let mut block_data_buf = block_data_buf;
        //设置文件指针
        file.seek(SeekFrom::Start(file_pos))?;
        //读取
        file.read_exact(&mut block_data_buf)?;
        //分析是否需要再加载
        let block_len = ManifestDataBlock::get_block_len(&block_data_buf)
            .map_err(|err| Error::other(format!("包文件IO属性数据块读取错误，err:{err:?}")))?;
        if block_len > u64::from(u32::MAX) {
            Err(Error::other(format!(
                "解析的数据大小过大，可能是错误的:{}[{}]",
                tools::bytes_len_to_string(block_len),
                block_len
            )))?;
        }
        let l_len = usize::try_from(block_len).unwrap() - MANIFEST_DATA_BLOCK_LEN;
        let block_data = if l_len > 0 {
            let mut l_block_buf = vec![0; l_len];
            file.read_exact(&mut l_block_buf)?;
            //合并
            let mut block_data = Vec::with_capacity(usize::try_from(block_len).unwrap());
            for byte in block_data_buf {
                block_data.push(byte);
            }
            for byte in l_block_buf {
                block_data.push(byte);
            }
            block_data
        } else {
            block_data_buf
        };
        ManifestDataBlock::from_block_data_new(block_data, file_pos)
    }

    //设置文件地址
    pub(crate) fn set_pos_read(&self, pos: u64) -> io::Result<()> {
        if self.run_data.pos != pos {
            let mut file = &self.file;
            file.seek(SeekFrom::Start(pos))?;
        }
        Ok(())
    }
}

//写
impl PackIO /*写*/ {
    pub(crate) fn unlock(&mut self) -> io::Result<()> {
        self.file.unlock()
    }

    pub(crate) fn lock(&mut self) -> io::Result<()> {
        self.file.lock()
    }

    //设置文件地址
    pub(crate) fn set_pos_write(&mut self, pos: u64) -> io::Result<()> {
        self.set_pos_read(pos)?;
        self.run_data.pos = pos;
        Ok(())
    }

    pub(crate) fn manifest_data_block_write(
        &mut self,
        block_data: &[u8],
        new_block: bool,
        old_pos: u64,
        old_block_len: u64,
    ) -> io::Result<u64> {
        Ok(if new_block {
            let (new_pos, _) = self.get_file_pos(block_data.len() as u64);
            self.set_pos_write(new_pos)?;
            self.write_all(block_data)?;
            self.file_gc_add(vec![(old_pos, old_block_len)]);
            new_pos
        } else {
            self.set_pos_write(old_pos)?;
            self.write_all(block_data)?;
            old_pos
        })
    }
}

/*//算法测试
#[test]
fn from_gc() {
    //排序===
    let mut empty_data_list = DataPosList {
        data_block: None,
        list: vec![(500, 100)],
    };
    let mut gc_data_list = DataPosList {
        data_block: None,
        list: vec![(1000, 100), (0, 100)],
    };
    PackIO::from_gc(&mut gc_data_list, &mut empty_data_list);
    assert_eq!(
        empty_data_list.list,
        vec![(0, 100), (500, 100), (1000, 100)]
    );
    //合并===
    let mut empty_data_list = DataPosList {
        data_block: None,
        list: vec![(0, 100)],
    };
    let mut gc_data_list = DataPosList {
        data_block: None,
        list: vec![(100, 100), (200, 100)],
    };
    WBFPManager::from_gc(&mut gc_data_list, &mut empty_data_list);
    assert_eq!(empty_data_list.list, vec![(0, 300)]);
    //排序与合并===
    let mut empty_data_list = DataPosList {
        data_block: None,
        list: vec![(100, 100)],
    };
    let mut gc_data_list = DataPosList {
        data_block: None,
        list: vec![(200, 100), (0, 100)],
    };
    WBFPManager::from_gc(&mut gc_data_list, &mut empty_data_list);
    assert_eq!(empty_data_list.list, vec![(0, 300)]);
}*/
