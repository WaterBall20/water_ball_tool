/*
开始时间：26/02/13 11：31
 */
use super::WBFPManager;
use crate::wb_files_pack::DataPosList;
use std::io;
use std::io::{ Error, Read, Seek, SeekFrom, Write };

pub struct PackFileWR<'a> {
    //管理器实例
    manager: &'a mut WBFPManager,
    //文件位置
    file_pos: u64,
    //文件分配的位置
    file_pos_list: DataPosList,
    //缓存_文件分配的位置当前索引
    temp_pos_index: usize,
    //缓存_文件分配的当前位置已占用大小
    temp_pos_this_len: u64,
}
impl PackFileWR<'_> {
    pub(in crate::wb_files_pack) fn new<'a>(
        manager: &'a mut WBFPManager,
        file_pos_list: DataPosList
    ) -> PackFileWR<'a> {
        PackFileWR {
            manager,
            file_pos: 0,
            file_pos_list,
            temp_pos_index: 0,
            temp_pos_this_len: 0,
        }
    }

    //获取位置列表
    fn get_pos_s(&self, pos: u64, is_read: bool) -> io::Result<Vec<(u64, u64)>> {
        self.get_add_pos_s2(0, 0, pos, is_read)
    }

    //获取追加位置列表
    fn get_add_pos_s(&self, add_pos: u64, is_read: bool) -> io::Result<Vec<(u64, u64)>> {
        self.get_add_pos_s2(self.temp_pos_index, self.temp_pos_this_len, add_pos, is_read)
    }
    fn get_add_pos_s2(
        &self,
        start_pos_index: usize,
        start_pos_len: u64,
        add_pos: u64,
        is_read: bool
    ) -> io::Result<Vec<(u64, u64)>> {
        let mut pos_index = start_pos_index;
        let mut r_pos: Vec<(u64, u64)> = Vec::new();
        let mut m_add_len: u64 = 0;
        while pos_index < self.file_pos_list.list.len() {
            let (mut pos, mut len) = *self.file_pos_list.list.get(start_pos_index).unwrap();
            //当前校准
            if pos_index == start_pos_index {
                //位置偏移
                pos += start_pos_len;
                //减去已偏移的长度
                len -= start_pos_len;
            }
            //计算
            if add_pos - m_add_len <= len {
                //小于等于直接添加并直接返回
                r_pos.push((pos, add_pos));
                return Ok(r_pos);
            }
            //大于就添加完所有空闲块
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
    fn _set_len(&mut self, _manager: &mut WBFPManager) -> io::Result<()> {
        Err(Error::other("未实现动态扩容"))
    }

    //设置文件位置
    fn set_pos(&mut self, pos: u64) -> io::Result<()> {
        //缓存处理===
        //获取需要添加的块列表
        let pos_s = self.get_pos_s(pos, false)?;
        //块索引
        let pos_index = pos_s.len() - 1;
        //块长度
        let (_, pos_len) = pos_s.get(pos_index).unwrap();
        self.temp_pos_index = pos_index;
        self.temp_pos_this_len = *pos_len;

        self.file_pos = pos;
        Ok(())
    }

    //追加文件位置
    fn add_pos(&mut self, length: u64) -> io::Result<()> {
        //获取需要添加的块列表
        self.add_pos2(length, self.get_add_pos_s(length, false)?);
        Ok(())
    }
    fn add_pos2(&mut self, length: u64, add_pos_s: Vec<(u64, u64)>) {
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
    }

    //减少文件位置
    fn sub_pos(&mut self, length: u64) -> io::Result<()> {
        self.sub_pos2(length, Vec::new())
    }
    fn sub_pos2(&mut self, length: u64, _sub_pos_s: Vec<(u64, u64)>) -> io::Result<()> {
        //TODO：暂时使用从头计算，可能存在性能损失，部分功能未实现
        let r_pos = self.file_pos.cast_signed() - length.cast_signed();
        if r_pos < 0 {
            self.set_pos(0)?;
            Ok(())
        } else {
            self.set_pos(r_pos.cast_unsigned())?;
            Ok(())
        }
    }
}

impl Seek for PackFileWR<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(pos) => {
                self.set_pos(pos)?;
                Ok(pos)
            }
            SeekFrom::Current(pos) =>
                match pos {
                    0 => { Ok(self.file_pos) }
                    //大于0
                    1.. => {
                        self.add_pos(pos.cast_unsigned())?;
                        Ok(self.file_pos)
                    }
                    //小于0
                    ..0 => {
                        self.sub_pos((-pos).cast_unsigned())?;
                        Ok(self.file_pos)
                    }
                }
            SeekFrom::End(pos) =>
                match pos {
                    0 => { Ok(self.file_pos) }
                    1.. => {
                        self.sub_pos((-pos).cast_unsigned())?;
                        Ok(self.file_pos)
                    }
                    ..0 => { Err(Error::other("未实现动态扩容")) }
                }
        }
    }
}

impl Read for PackFileWR<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        //当前大小所需的位置列表
        let pos_s = self.get_add_pos_s(buf.len() as u64, true)?;
        //当前已读取大小
        let mut read_len = 0;
        //读取
        for (pos, len) in pos_s {
            let len = usize::try_from(len).unwrap();
            let this_buf = &mut buf[read_len..read_len + len];
            //更改文件位置
            self.manager.set_pack_file_pos_read(pos)?;
            //读取数据
            self.manager.pack_file_read_root(this_buf)?;
            read_len += len;
        }
        self.add_pos(read_len as u64)?;
        Ok(read_len)
    }
}

impl Write for PackFileWR<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.manager.write_lock()?;
        //当前大小所需的位置列表
        let pos_s = self.get_add_pos_s(buf.len() as u64, false)?;
        //当前已写入大小
        let mut write_len = 0;
        //写入
        for (pos, len) in pos_s {
            let len = usize::try_from(len).unwrap();
            let this_data = &buf[write_len..write_len + len];
            //更改文件位置
            self.manager.set_pack_file_pos_write(pos)?;
            //写入数据
            self.manager.pack_file_write_root(this_data)?;
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
