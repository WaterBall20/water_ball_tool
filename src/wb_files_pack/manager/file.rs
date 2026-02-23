/*
开始时间：26/02/13 11：31
 */
use super::WBFPManager;
use crate::wb_files_pack::PackFileInfo;
use crate::wb_files_pack::PackFileKind;
use std::io;
use std::io::{Error, ErrorKind, Read, Seek, SeekFrom, Write};

pub struct PackFileWR {
    //相关文件信息
    pack_file_info: PackFileInfo,
    //管理器实例id, 随机数，确保不意外操作
    manager_id: u32,
    //文件位置
    file_pos: u64,
    //文件分配的位置
    file_pos_s: Vec<(u64, u64)>,
    //缓存_文件分配的位置当前索引
    temp_pos_index: usize,
    //缓存_文件分配的当前位置已占用大小
    temp_pos_this_len: u64,
}
impl PackFileWR {
    pub(in crate::wb_files_pack::manager) fn build(
        manager_id: u32,
        pack_file_info: &PackFileInfo,
    ) -> Result<PackFileWR, Error> {
        if let PackFileKind::File(file) = pack_file_info.file_kind() {
            Ok(PackFileWR {
                pack_file_info: pack_file_info.clone(),
                manager_id,
                file_pos: 0,
                file_pos_s: file.data_pos().clone(),
                temp_pos_index: 0,
                temp_pos_this_len: 0,
            })
        } else {
            Err(Error::new(ErrorKind::NotADirectory, "提供的参数不是文件"))
        }
    }
    //设置文件位置
    fn set_pos(&mut self, pos: u64) -> Result<(), Error> {
        //缓存处理===
        //获取需要添加的块列表
        let pos_s = self.get_pos_s(pos)?;
        //块索引
        let pos_index = pos_s.len() - 1;
        //块长度
        let (_, pos_len) = pos_s.get(pos_index).unwrap();
        self.temp_pos_index = pos_index;
        self.temp_pos_this_len = *pos_len;

        self.file_pos = pos;
        Ok(())
    }

    //获取位置列表
    fn get_pos_s(&self, pos: u64) -> Result<Vec<(u64, u64)>, Error> {
        self.get_add_pos_s2(0, 0, pos)
    }

    //获取追加位置列表
    fn get_add_pos_s(&self, add_pos: u64) -> Result<Vec<(u64, u64)>, Error> {
        self.get_add_pos_s2(self.temp_pos_index, self.temp_pos_this_len, add_pos)
    }

    //获取追加位置列表
    fn get_add_pos_s2(
        &self,
        start_pos_index: usize,
        start_pos_len: u64,
        add_pos: u64,
    ) -> Result<Vec<(u64, u64)>, Error> {
        let mut pos_index = start_pos_index;
        let mut r_pos: Vec<(u64, u64)> = Vec::new();
        let mut m_add_len: u64 = 0;
        while pos_index < self.file_pos_s.len() {
            let (mut pos, mut len) = *self.file_pos_s.get(start_pos_index).unwrap();
            //当前校准
            if pos_index == start_pos_index {
                //位置偏移
                pos += start_pos_len;
                //减去已偏移的长度
                len -= start_pos_len;
            }
            //计算
            if (add_pos - m_add_len) <= len {
                //小于等于直接添加并直接返回
                r_pos.push((pos, add_pos));
                return Ok(r_pos);
            } else {
                //大于就添加完所有空闲块
                r_pos.push((pos, len));
                //增值
                m_add_len += len;
                //附加索引
                pos_index += 1
            }
        }
        Err(Error::new(ErrorKind::Other, "空间越界"))
    }

    //追加文件位置
    fn add_pos(&mut self, length: u64) -> Result<(), Error> {
        //获取需要添加的块列表
        self.add_pos2(length, self.get_add_pos_s(length)?)
    }
    fn add_pos2(&mut self, length: u64, add_pos_s: Vec<(u64, u64)>) -> Result<(), Error> {
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
        self.file_pos += length;
        Ok(())
    }
    //减少文件位置
    fn sub_pos(&mut self, length: u64) -> io::Result<()> {
        self.sub_pos2(length, Vec::new())
    }
    fn sub_pos2(&mut self, length: u64, sub_pos_s: Vec<(u64, u64)>) -> Result<(), Error> {
        //TODO：暂时使用从头计算，可能存在性能损失，部分功能未实现
        let r_pos = self.file_pos as i64 - length as i64;
        if r_pos < 0 {
            self.set_pos(0)?;
            Ok(())
        } else {
            self.set_pos(r_pos as u64)?;
            Ok(())
        }
    }

    //写入方法
    pub fn write(&mut self, manager: &mut WBFPManager, data: &[u8]) -> io::Result<usize> {
        self.write2(manager, data, data.len())
    }

    pub fn write2(
        &mut self,
        manager: &mut WBFPManager,
        buf: &[u8],
        buf_len: usize,
    ) -> io::Result<usize> {
        manager.write_lock()?;
        //大小判断
        if buf.len() < buf_len {
            return Err(Error::new(
                ErrorKind::Other,
                "提供的缓冲区长度比提供的大小小",
            ));
        }
        //指定大小的切片
        let m_buf = &buf[..buf_len];
        //当前大小所需的位置列表
        let pos_s = self.get_add_pos_s(buf_len as u64)?;
        //当前已写入大小
        let mut write_len = 0;
        //写入
        for (pos, len) in pos_s {
            let len = len as usize;
            let this_data = &m_buf[write_len..(write_len - len)];
            //更改文件位置
            manager.set_pack_file_pos(pos)?;
            //写入数据
            //TODO:未来功能：写入优化、写时复制
            let mut pos_write_len = 0;
            while pos_write_len != len {
                if pos_write_len > len {
                    return Err(Error::new(ErrorKind::Other, "文件写出的大小大于预期"));
                } else {
                    let pos_pack_file_write_len =
                        manager.pack_file_write2_root(&this_data[pos_write_len..], true)?;
                    pos_write_len += pos_pack_file_write_len;
                    write_len += pos_pack_file_write_len;
                }
            }
        }

        Ok(write_len)
    }

    pub fn read(&mut self, manager: &mut WBFPManager, buf: &mut [u8]) -> io::Result<usize> {
        self.read2(manager, buf, buf.len())
    }

    pub fn read2(
        &mut self,
        manager: &mut WBFPManager,
        buf: &mut [u8],
        buf_len: usize,
    ) -> io::Result<usize> {
        //大小检查
        if buf.len() < buf_len {
            return Err(Error::new(
                ErrorKind::Other,
                "提供的缓冲区长度比提供的大小小",
            ));
        }
        //指定大小的切片
        let m_buf = &mut buf[..buf_len];
        //当前大小所需的位置列表
        let pos_s = self.get_add_pos_s(buf_len as u64)?;
        //当前已读取大小
        let mut read_len = 0;
        //读取
        for (pos, len) in pos_s {
            let len = len as usize;
            let this_buf = &mut m_buf[read_len..(read_len + len)];
            //更改文件位置
            manager.set_pack_file_pos(pos)?;
            //读取数据
            let mut pos_read_len = 0;
            while pos_read_len != len {
                if pos_read_len > len {
                    return Err(Error::new(ErrorKind::Other, "读取的大小大于预期"));
                } else {
                    let this_pack_file_read_len =
                        manager.pack_file_read_root(&mut this_buf[pos_read_len..]);
                    pos_read_len += this_pack_file_read_len;
                    read_len += this_pack_file_read_len;
                }
            }
        }
        Ok(read_len)
    }
}

impl Seek for PackFileWR {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(pos) => {
                self.set_pos(pos)?;
                Ok(pos)
            }
            SeekFrom::Current(pos) => {
                if pos == 0 {
                    Ok(self.file_pos)
                } else if pos > 0 {
                    self.add_pos(pos as u64)?;
                    Ok(self.file_pos)
                } else {
                    self.sub_pos(-pos as u64)?;
                    Ok(self.file_pos)
                }
            }
            SeekFrom::End(pos) => {
                if pos == 0 {
                    Ok(self.file_pos)
                } else if pos < 0 {
                    self.sub_pos(-pos as u64)?;
                    Ok(self.file_pos)
                } else {
                    Err(Error::new(ErrorKind::Other, "未实现动态扩容"))
                }
            }
        }
    }
}

impl Read for PackFileWR {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        todo!()
    }
}

impl Write for PackFileWR {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        todo!()
    }

    fn flush(&mut self) -> io::Result<()> {
        todo!()
    }
}
