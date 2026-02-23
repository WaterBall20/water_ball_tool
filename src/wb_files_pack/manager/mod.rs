/*
开始时间：26/2/11 15：51
 */
use crate::wb_files_pack;
use crate::wb_files_pack::{
    Attribute, DataVersion, PackDir, PackFileInfo, PackFileKind, PackFilesList, WBFilesPackData,
};
use rand::RngExt;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::{fs, io};

pub mod file;

//当前解析器版本
pub static DATA_VERSION: u32 = 10;
//当前解析器兼任版本
pub static DATA_VERSION_COMPATIBLE: u32 = 10;

//默认写实复制
pub static DEF_COW: bool = false;

//默认分离数据为单独文件
pub static DEF_S_DATA_FILE: bool = true;

//文件头-文件名:WPFilesPack
pub static FILE_HEADER_TYPE_NAME: [u8; 11] = [
    0x57u8, 0x42, 0x46, 0x69, 0x6c, 0x65, 0x73, 0x50, 0x61, 0x63, 0x6b,
];
//文件头版本
static FILE_HEADER_VERSION: [u8; 2] = [0, 1];

static FILE_HEADER_TAG_LENGTH: usize = 1;

//文件头长度
static FILE_HEADER_JSON_DATA_START_POS_POS: u64 =
    (FILE_HEADER_TYPE_NAME.len() + FILE_HEADER_VERSION.len() + FILE_HEADER_TAG_LENGTH) as u64;

static FILE_HEADER_JSON_DATA_START_AND_END_LENGTH: u64 = 8 + 8;

static FILE_HEADER_DATA_LENGTH_LENGTH: u64 = 8;
static FILE_HEADER_LENGTH: u64 = FILE_HEADER_JSON_DATA_START_POS_POS
    + FILE_HEADER_JSON_DATA_START_AND_END_LENGTH
    + FILE_HEADER_DATA_LENGTH_LENGTH;

//缓冲区大小
static TEMP_DATA_LENGTH: u64 = 1024 * 1024;

pub struct WBFPManager {
    wb_files_pack_data: WBFilesPackData,
    //包文件路径
    pack_path: String,
    //包文件
    pack_file: File,
    //包数据文件
    json_data_file: Option<File>,
    //包数据文件B
    json_data_file_b: Option<File>,
    //启用写时复制
    cow: bool,
    //数据分离
    s_data_file: bool,
    //JSON数据起始位置
    json_data_start_pos: u64,
    //JSON数据结束位置
    json_data_end_pos: u64,
    //JSON数据长度
    json_data_length: u64,
    //当前写入B Json文件
    is_write_json_data_file_b: bool,
    //写入锁
    write_lock: bool,
    //写入锁路径
    write_lock_path: PathBuf,
    //锁文件对象
    write_lock_file: Option<File>,
    //当前包文件位置
    pack_file_pos: u64,
    //当前包文件长度
    pack_file_length: u64,
    //当前包文件的结束位置
    pack_file_end_pos: u64,
    //运行时id
    run_id: u32,
}
impl WBFPManager {
    fn new<P: AsRef<Path>>(
        pack_path: P,
        wb_files_pack_data: WBFilesPackData,
        pack_file: File,
        json_data_file: Option<File>,
        json_data_file_b: Option<File>,
        write_lock_file: Option<File>,
    ) -> WBFPManager {
        let cow = wb_files_pack_data.attribute().cow();
        let s_data_file = wb_files_pack_data.attribute().s_data_file();
        let mut write_lock_path =
            String::from(pack_path.as_ref().to_str().expect("无法将转换路径成文本"));
        write_lock_path.push_str(".lock");
        let write_lock_path = Path::new(&write_lock_path).to_path_buf();
        WBFPManager {
            wb_files_pack_data,
            pack_path: String::from(pack_path.as_ref().to_str().expect("无法将路径转换成文本")),
            pack_file,
            json_data_file,
            json_data_file_b,
            cow,
            s_data_file,
            json_data_start_pos: 0,
            json_data_end_pos: 0,
            json_data_length: 0,
            is_write_json_data_file_b: false,
            write_lock: false,
            write_lock_path,
            write_lock_file,
            pack_file_pos: 0,
            pack_file_length: 0,
            pack_file_end_pos: 0,
            run_id: rand::rng().random_range(0..100_000_000),
        }
    }

    pub fn init_pack(&mut self) {
        //写出文件头
        //类型名称
        self.pack_file_write_root(FILE_HEADER_TYPE_NAME.as_slice(), false);
        //写入文件版本
        self.pack_file_write_root(FILE_HEADER_VERSION.as_slice(), false);

        //写入标签===
        //文件头标签，二进制位:
        //|   0   |     1    |
        //|写实复制|数据文件分离|
        let mut header_tag: u8 = 0;
        if self.wb_files_pack_data.attribute().cow() {
            header_tag |= 0b10000000
        }
        if self.wb_files_pack_data.attribute().s_data_file() {
            header_tag |= 0b01000000
        }
        self.pack_file_write_root([header_tag].as_slice(), false);
        //===

        self.save_json_data().expect("无法保存索引数据");
        //设置文件大小
        self.set_pack_file_len(FILE_HEADER_LENGTH);

        self.write_unlock().expect("解除文件锁失败");
    }

    //读取===

    //验证是否为文件
    pub fn file_is_file<P: AsRef<Path>>(&self, path: &P) -> Option<bool> {
        self.file_exists(path).map(|v| !v)
    }

    //验证是否为目录,此方法和file_exists一致
    pub fn file_is_dir<P: AsRef<Path>>(&self, path: &P) -> Option<bool> {
        self.file_exists(path)
    }

    //验证文件是否存在, true表示目录，false表示文件,None表示不存在
    pub fn file_exists<P: AsRef<Path>>(&self, path: &P) -> Option<bool> {
        self.get_file_info(path).map(|info| info.is_dir)
    }

    //获取目录信息，带类型检查
    pub fn get_dir_file_info<P: AsRef<Path>>(&self, path: &P) -> Option<&PackFileInfo> {
        self.get_dir_file_info2(&Self::create_path_vec(path))
    }

    //获取目录信息，带类型检查
    fn get_dir_file_info2(&self, path_list: &[String]) -> Option<&PackFileInfo> {
        match self.get_file_info2(path_list) {
            Some(info) => {
                if info.is_dir {
                    Some(info)
                } else {
                    None
                }
            }
            None => None,
        }
    }

    //获取文件信息，仅文件，带类型检查
    pub fn get_file_file_info<P: AsRef<Path>>(&self, path: &P) -> Option<&PackFileInfo> {
        self.get_file_file_info2(&Self::create_path_vec(path))
    }

    //获取文件信息，仅文件，带类型检查
    fn get_file_file_info2(&self, path_list: &[String]) -> Option<&PackFileInfo> {
        match self.get_file_info2(path_list) {
            Some(info) => {
                if !info.is_dir {
                    Some(info)
                } else {
                    None
                }
            }
            None => None,
        }
    }

    //获取文件信息，不检查类型
    pub fn get_file_info<P: AsRef<Path>>(&self, path: &P) -> Option<&PackFileInfo> {
        let path_list = Self::create_path_vec(path);
        self.get_file_info2(&path_list)
    }

    fn get_file_info2(&self, path_list: &[String]) -> Option<&PackFileInfo> {
        let mut info_list = &self.wb_files_pack_data.pack_files_list.files_list;
        let mut info = None;
        for item in path_list {
            match info_list.get(item) {
                Some(value) => {
                    info = Some(value);
                    if let PackFileKind::None = value.file_kind {
                        panic!("存储非法数据：pack:{path_list:?}info:{value:?}")
                    }
                    if let PackFileKind::Dir(dir) = &value.file_kind {
                        info_list = dir.files_list()
                    }
                }
                None => return None,
            }
        }
        info
    }
    //获取文件信息的可变借用
    /*fn get_file_info_mut2(&mut self, path_list: &[String]) -> Option<&mut PackFileInfo> {
        let mut info_list = &self.wb_files_pack_data.pack_files_list.files_list;
        let mut info = None;
        for item in path_list {
            match info_list.get(item) {
                Some(value) => {
                    info = Some(value);
                    if let PackFileKind::Dir(dir) = &value.file_kind {
                        info_list = dir.files_list()
                    }
                }
                None => return None,
            }
        }
        info
    }*/

    //包文件读取，发生错误将恐慌(panic)
    fn pack_file_read_root(&mut self, data: &mut [u8]) -> usize {
        self.pack_file_read_root2(data).expect("包文件无法写入")
    }
    //包文件写入
    fn pack_file_read_root2(&mut self, data: &mut [u8]) -> io::Result<usize> {
        //自我注意：这里必须是引用，除非想一次性写入。
        let file = &mut self.pack_file;
        let len = file.read(data)?;
        self.pack_file_pos += len as u64;
        Ok(len)
    }

    //写入===

    //创建文件
    /*pub fn create_file<P: AsRef<Path>>(
        &mut self,
        path: &P,
        modified_time: u128,
        length: u64,
    ) -> io::Result<()> {
        let path_list = Self::create_path_vec(path);
        let dir_path_list = &path_list[..path_list.len() - 1];
        let dir_file_info = match self.get_dir_file_info2(dir_path_list) {
            Some(info) => {
                info
            }
            None => {}
        };
        //尝试创建文件夹，这是最快的方法
    }*/

    //创建目录
    pub fn create_dir<P: AsRef<Path>>(&mut self, path: &P, modified_time: u128) -> io::Result<()> {
        let path_list = Self::create_path_vec(path);
        let pack_files_list = &mut self.wb_files_pack_data.pack_files_list;
        let r = Self::create_dir_s(
            modified_time,
            &path_list,
            0,
            &mut pack_files_list.files_list,
        )?;
        pack_files_list.file_count += r.file_count_add;
        pack_files_list.dir_count += r.dir_count_add;
        Ok(())
    }

    fn create_dir_s(
        modified_time: u128,
        path_list: &Vec<String>,
        path_list_index: usize,
        s_info_list: &mut HashMap<String, PackFileInfo>,
    ) -> io::Result<CreateDirReturn> {
        //获取文件名
        let name = path_list.get(path_list_index).unwrap();
        //判断目录是否存在
        match s_info_list.get_mut(name) {
            Some(info) => {
                //存在则获取
                match &mut info.file_kind {
                    PackFileKind::Dir(dir) => {
                        let info_list = dir.files_list_mut();
                        //递归，带边界检查
                        let r = if path_list_index + 1 < path_list.len() {
                            Self::create_dir_s(
                                modified_time,
                                path_list,
                                path_list_index + 1,
                                info_list,
                            )?
                        } else {
                            CreateDirReturn {
                                dir_count_add: 0,
                                file_count_add: 0,
                            }
                        };
                        //附加
                        dir.file_count += r.file_count_add;
                        dir.dir_count += r.dir_count_add;
                        Ok(CreateDirReturn {
                            file_count_add: dir.file_count,
                            dir_count_add: dir.dir_count,
                        })
                    }
                    PackFileKind::File(_) => Err(Error::new(
                        ErrorKind::NotADirectory,
                        "虚拟文件存在但不是目录",
                    )),
                    PackFileKind::None => Err(Error::new(
                        ErrorKind::NotADirectory,
                        "虚拟文件存在但不是目录也不是普通文件",
                    )),
                }
            }
            None => {
                //不存在则创建
                let mut files_list = HashMap::new();
                //递归，带边界检查
                let r = if path_list_index + 1 < path_list.len() {
                    Self::create_dir_s(
                        modified_time,
                        path_list,
                        path_list_index + 1,
                        &mut files_list,
                    )?
                } else {
                    CreateDirReturn {
                        file_count_add: 0,
                        dir_count_add: 0,
                    }
                };
                let new_dir_file_info = PackFileInfo {
                    name: name.clone(),
                    length: 0,
                    modified_time,
                    is_dir: true,
                    file_kind: PackFileKind::Dir(PackDir {
                        file_count: r.file_count_add,
                        dir_count: r.dir_count_add,
                        files_list,
                    }),
                };
                s_info_list.insert(name.clone(), new_dir_file_info);
                Ok(CreateDirReturn {
                    dir_count_add: r.dir_count_add + 1,
                    file_count_add: r.file_count_add,
                })
            }
        }
    }

    fn create_new_file_to(
        &mut self,
        name: &str,
        length: u64,
        modified_time: u128,
        to_info_list: &mut HashMap<String, PackFileInfo>,
    ) -> Result<CreateDirReturn, Error> {
        //创建文件信息
        let file_info = PackFileInfo {
            name: name.to_string(),
            length,
            is_dir: false,
            modified_time,
            file_kind: PackFileKind::File(wb_files_pack::PackFile {
                data_pos: self.get_file_pos(length),
                hash: String::new(),
            }),
        };
        //添加
        to_info_list.insert(name.to_string(), file_info);

        Ok(CreateDirReturn {
            file_count_add: 1,
            dir_count_add: 0,
        })
    }

    /*fn create_dirs<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        let path = Self::create_path(path);
        let path_list: Vec<String> = Vec::new();

        //处理
        let mut info_list = &mut self.wb_files_pack_data.pack_files_list.files_list;
    }*/

    fn create_path_vec<P: AsRef<Path>>(path: P) -> Vec<String> {
        let binding = Self::_create_path(path);
        let path = binding.as_path();
        let mut path_list: Vec<String> = Vec::new();
        for item in path {
            path_list.push(String::from(item.to_str().expect("转换文本错误")));
        }
        path_list
    }

    fn _create_path<P: AsRef<Path>>(path: P) -> PathBuf {
        let path = path.as_ref();
        //去除不必要的前缀
        if let Ok(r) = path.strip_prefix("./").or_else(|_| path.strip_prefix(".")) {
            r
        } else {
            path
        }
            .to_path_buf()
    }

    //核心代码===

    //垃圾回收
    fn file_gc(&mut self, gc_pos_s: Vec<(u64, u64)>) -> io::Result<()> {
        let pos_s = &mut self.wb_files_pack_data.attribute.empty_data_pos;
        'gc_for: for (gc_pos, gc_len) in &gc_pos_s {
            //排序插入
            let mut j = 0;
            while j < pos_s.len() {
                let (pos, _) = pos_s.get(j).unwrap();
                //插入判断
                if gc_pos < pos {
                    //如果位置在前
                    pos_s.insert(j, (*gc_pos, *gc_len));
                    continue 'gc_for;
                }
                j += 1;
            }
            pos_s.push((*gc_pos, *gc_len));
        }
        //TODO:需要实现合并功能

        Ok(())
    }

    //获取可用的文件位置
    fn get_file_pos(&mut self, length: u64) -> Vec<(u64, u64)> {
        let mut add_pos: Vec<(u64, u64)> = Vec::new();
        let mut l_add_len = 0;
        //优先使用空隙
        let empty_data_pos = &mut self.wb_files_pack_data.attribute.empty_data_pos;
        //当大小没有写完，且空隙存在则优先使用空隙
        while l_add_len != length && !empty_data_pos.is_empty() {
            //从第一个开始
            let get_0 = empty_data_pos.get_mut(0).unwrap();
            let (pos, len) = get_0;
            //判断是否能占用完
            //剩余大小
            let m_len = length - l_add_len;
            if m_len >= *len {
                //能占用完，则附加并删除空隙
                add_pos.push((*pos, *len));
                l_add_len += *len;
                empty_data_pos.remove(0);
            } else {
                //不能则切出
                add_pos.push((*pos, *len - m_len));
                //修改，位置加大小使其向后移动，长度减大小使其边界不变
                *get_0 = (*pos + m_len, *len - m_len);
                return add_pos;
            }
        }
        //扩容处理
        if l_add_len != length {
            let this_add_len = length - l_add_len;
            add_pos.push((self.pack_file_end_pos, this_add_len));
            //附加包文件结束位置
            self.pack_file_end_pos += this_add_len;
        }
        add_pos
    }

    //写入索引数据
    fn save_json_data(&mut self) -> io::Result<()> {
        //Json数据转换
        let json_data = serde_json::to_vec_pretty(&self.wb_files_pack_data)?;
        self.write_json_data_root(json_data)
    }

    //写入数据文件
    fn write_json_data_root(&mut self, data: Vec<u8>) -> io::Result<()> {
        //设置写入锁
        self.write_lock()?;
        //判断是否分离
        if self.s_data_file {
            //写入判断
            if self.is_write_json_data_file_b {
                self.write_json_data2_root_b(data.as_slice())?;
                self.is_write_json_data_file_b = false;
            } else {
                self.write_json_data2_root_a(data.as_slice())?;
                self.is_write_json_data_file_b = true;
            }
            Ok(())
        } else {
            //获取包文件长度，并设为数据起始位置
            self.json_data_start_pos = self.pack_file_length;
            let json_data = serde_json::to_vec(&self.wb_files_pack_data)?;
            let json_len = json_data.len();
            self.set_pack_file_pos(FILE_HEADER_LENGTH + self.json_data_start_pos)?;
            let write_len = self.pack_file_write_root(json_data.as_slice(), true);
            if json_len != write_len {
                panic!("写出的大小不完整")
            }
            //设置为结束位置
            self.json_data_end_pos = self.json_data_start_pos + json_len as u64;
            //设置大小
            self.json_data_length = json_len as u64;
            //更新JSON数据的起始和结束位置
            self.up_json_data_pos()?;
            Ok(())
        }
    }

    fn write_json_data2_root_a(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(write_file) = &mut self.json_data_file {
            //清空内容
            write_file.set_len(0)?;
            //重设位置
            write_file.seek(SeekFrom::Start(0))?;
            //写入
            write_file.write(buf)
        } else {
            Err(Error::new(ErrorKind::Other, "数据文件b实例不存在"))
        }
    }
    fn write_json_data2_root_b(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(write_file) = &mut self.json_data_file_b {
            //清空内容
            write_file.set_len(0)?;
            //重设位置
            write_file.seek(SeekFrom::Start(0))?;
            //写入
            write_file.write(buf)
        } else {
            Err(Error::new(ErrorKind::Other, "数据文件b实例不存在"))
        }
    }

    //写入锁信息
    fn write_lock_info(&self) -> PackLockInfo {
        let run_lock = self.write_lock;
        let path = &self.write_lock_path;
        write_lock_info(run_lock, path)
    }

    //设置写入锁
    fn write_lock(&mut self) -> io::Result<()> {
        let lock_file = write_lock(true, &self.write_lock_path)?;
        if let Some(lock_file) = lock_file {
            self.write_lock_file = Some(lock_file);
        }
        self.write_lock = true;
        self.pack_file.lock()?;
        Ok(())
    }

    //解除写入锁
    fn write_unlock(&mut self) -> io::Result<()> {
        //锁文件路径
        let path = &self.write_lock_path;
        //获取锁信息
        let lock_info = self.write_lock_info();
        if lock_info.file_lock {
            if lock_info.file_lock_is_symlink {
                return Err(Error::new(
                    ErrorKind::Other,
                    "无法解锁，锁文件类型是符号链接",
                ));
            }
            if lock_info.file_lock_is_dir {
                Err(Error::new(
                    ErrorKind::IsADirectory,
                    "无法解锁，锁文件类型是目录",
                ))
            } else {
                //释放文件句柄
                if let Some(lock_file) = self.write_lock_file.take() {
                    lock_file.unlock()?;
                    println!("释放写入锁");
                    drop(lock_file);
                    fs::remove_file(path)?;
                }
                self.write_lock = false;
                self.pack_file.unlock()?;
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    //更新包文件的JSON数据起始和结束位置
    fn up_json_data_pos(&mut self) -> io::Result<()> {
        self.write_lock()?;
        //记录当前文件位置
        let pack_file_pos = self.pack_file_pos;
        //更改文件偏移量
        self.set_pack_file_pos2(FILE_HEADER_JSON_DATA_START_POS_POS, true)?;
        //写入起始位置
        self.pack_file_write2_root(self.json_data_start_pos.to_le_bytes().as_slice(), false)?;
        //写入结束位置
        self.pack_file_write2_root(self.json_data_end_pos.to_le_bytes().as_slice(), false)?;
        //还原位置
        self.set_pack_file_pos2(pack_file_pos, true)?;
        Ok(())
    }

    fn set_pack_file_pos(&mut self, pos: u64) -> io::Result<()> {
        self.set_pack_file_pos2(pos, false)
    }
    //设置文件大小，是否从头文件算，true ,表示从文件头开始算
    fn set_pack_file_pos2(&mut self, pos: u64, header_len: bool) -> io::Result<()> {
        let file = &mut self.pack_file;
        file.seek(SeekFrom::Start(if header_len {
            pos
        } else {
            FILE_HEADER_LENGTH + pos
        }))?;
        self.pack_file_pos = pos;
        Ok(())
    }

    //包文件写入，发生错误将恐慌(panic)
    fn pack_file_write_root(&mut self, data: &[u8], add_length: bool) -> usize {
        self.pack_file_write2_root(data, add_length)
            .expect("包文件无法写入")
    }
    //包文件写入
    fn pack_file_write2_root(&mut self, data: &[u8], add_length: bool) -> io::Result<usize> {
        self.write_lock()?;
        //自我注意：这里必须是引用，除非想一次性写入。
        let file = &mut self.pack_file;
        let len = file.write(data)?;
        self.pack_file_pos += len as u64;
        if add_length {
            self.pack_file_length += len as u64;
        }
        Ok(len)
    }

    //设置包文件大小,发生错误将恐慌(panic)
    fn set_pack_file_len(&mut self, len: u64) {
        self.set_pack_file_len2(len).expect("无法设置包文件大小")
    }

    //设置包文件大小
    fn set_pack_file_len2(&mut self, len: u64) -> io::Result<()> {
        self.write_lock()?;
        let file = &mut self.pack_file;
        file.set_len(len)
    }
}
impl Drop for WBFPManager {
    fn drop(&mut self) {
        //确保文件完全写入
        _ = self.pack_file.sync_all();
        //释放缓存文件===
        //更改数据文件写入标签，使其如果分离就指定下次写入是最终文件
        self.is_write_json_data_file_b = false; //false表示下次写入不是B文件，就是最终文件。
        //强制写入索引数据
        _ = self.save_json_data();
        //如果索引文件分离
        if self.s_data_file {
            //将B文件释放并删除
            let mut json_b_path = self.pack_path.clone();
            json_b_path.push_str(".json.b");
            if let Some(file) = self.json_data_file_b.take() {
                //显式释放
                drop(file);
                //删除
                _ = fs::remove_file(&json_b_path)
            }
        }
        //释放写入锁
        _ = self.write_unlock();
    }
}

struct PackFileWriteReturn {
    pack_file_write: file::PackFileWR,
    length_add: u64,
    file_count_add: u32,
    dir_count_add: u32,
}

struct CreateDirReturn {
    file_count_add: u32,
    dir_count_add: u32,
}

struct PackLockInfo {
    //运行时_锁状态
    run_lock: bool,
    //文件_锁状态
    file_lock: bool,
    //文件_锁_是目录
    file_lock_is_dir: bool,
    //文件_锁_是符号链接
    file_lock_is_symlink: bool,
    //文件锁存储的pid
    file_lock_pid: Option<u32>,
    //文件锁存储的pid进程在运行
    file_lock_pid_run: Option<bool>,
}

//获取锁信息
fn write_lock_info(run_lock: bool, path: &PathBuf) -> PackLockInfo {
    let is_symlink = path.is_symlink();
    let is_dir;
    let is_file;
    let mut file_lock_pid = None;
    let mut file_lock_pid_run = None;
    if path.try_exists().is_ok() {
        is_dir = path.is_dir();
        is_file = if path.is_file() {
            //获取文件存储的pid
            //通过运行时锁，排除自身
            if !run_lock {
                //读取文件内容
                let mut file = File::open(path).expect("无法打开锁文件");
                let mut buf = [0u8; 4];
                file.read_exact(&mut buf).expect("无法读取锁文件");
                let pid = u32::from_le_bytes(buf);
                file_lock_pid = Some(pid);
                file_lock_pid_run = Some(is_process_running(pid))
            }
            true
        } else {
            false
        };
    } else {
        is_dir = false;
        is_file = false;
    }
    PackLockInfo {
        run_lock,
        file_lock: !(!is_file && !is_dir),
        file_lock_is_dir: is_dir,
        file_lock_is_symlink: is_symlink,
        file_lock_pid,
        file_lock_pid_run,
    }
}

fn is_process_running(pid: u32) -> bool {
    // 1. 初始化系统句柄
    // 建议：如果需要频繁检查，请复用这个 System 对象以提高性能
    let mut sys = sysinfo::System::new_all();

    // 2. 刷新进程列表（sysinfo 采用快照机制，必须刷新才能获取最新状态）
    sys.refresh_all();

    // 3. 检查特定 PID 是否存在
    // sysinfo 使用自己的 Pid 类型，需要从 u32 转换
    sys.process(sysinfo::Pid::from(pid as usize)).is_some()
}

//设置写入锁
fn write_lock(run_lock: bool, write_lock_path: &PathBuf) -> Result<Option<File>, Error> {
    let lock_info = write_lock_info(run_lock, write_lock_path);
    if lock_info.run_lock {
        //若锁文件不存在就写入
        if !lock_info.file_lock {
            Ok(Some(write_lock_file(write_lock_path)?))
        } else {
            Ok(None)
        }
    } else {
        //判断锁文件
        match lock_info.file_lock_pid_run {
            Some(true) => panic!("无法为包文件上写入锁，正在被其他进程持有。"),
            Some(false) => panic!(
                r#"[警告]包文件未正常解锁，但相关进程(pid:{})可能已停止。
                    如果你认为可以继续，可以删除锁文件：{:?} 强制解锁"#,
                lock_info.file_lock_pid.expect(""),
                write_lock_path
            ),
            None => Ok(Some(write_lock_file(write_lock_path)?)),
        }
    }
}

//设置写入_文件锁
fn write_lock_file(write_lock_path: &PathBuf) -> Result<File, Error> {
    let pid = std::process::id();
    let mut write_lock = File::create(write_lock_path)?;
    write_lock.write(pid.to_le_bytes().as_slice())?;
    write_lock.sync_all()?;
    write_lock.lock()?;
    println!("已为包文件上写入锁");
    Ok(write_lock)
}

//创建新包文件
pub fn create_new_file<P: AsRef<Path>>(pack_path: &P) -> Result<WBFPManager, Error> {
    create_new_file2(pack_path, DEF_COW, DEF_S_DATA_FILE)
}

//创建新包文件
pub fn create_new_file2<P: AsRef<Path>>(
    pack_path: &P,
    cow: bool,
    s_data_file: bool,
) -> Result<WBFPManager, Error> {
    //判断文件是否存在
    match pack_path.as_ref().try_exists() {
        Ok(true) => Err(Error::new(ErrorKind::Other, "文件可能已存在，无法创建！")),
        Ok(false) => create_file2(pack_path, cow, s_data_file),
        Err(_) => create_file2(pack_path, cow, s_data_file),
    }
}

pub fn create_file<P: AsRef<Path>>(pack_path: &P) -> Result<WBFPManager, Error> {
    create_file2(pack_path, DEF_COW, DEF_S_DATA_FILE)
}

pub fn create_file2<P: AsRef<Path>>(
    pack_path: &P,
    cow: bool,
    s_data_file: bool,
) -> Result<WBFPManager, Error> {
    let mut write_lock_path =
        String::from(pack_path.as_ref().to_str().expect("无法将路径转换成文本"));
    write_lock_path.push_str(".lock");
    let write_lock_path = PathBuf::from(write_lock_path);
    let write_lock_file = write_lock(false, &write_lock_path)?;
    //创建包文件
    let pack_file = File::create(pack_path)?;
    //创建包文件数据文件

    let pack_data_file;
    let pack_data_file_b;
    if s_data_file {
        let mut data_path =
            String::from(pack_path.as_ref().to_str().expect("无法将路径转换成文件"));
        data_path.push_str(".json");
        pack_data_file = Some(File::create(&data_path)?);
        data_path.push_str(".b");
        pack_data_file_b = Some(File::create(data_path)?)
    } else {
        pack_data_file = None;
        pack_data_file_b = None;
    }

    Ok(create2(
        pack_path,
        cow,
        s_data_file,
        pack_file,
        pack_data_file,
        pack_data_file_b,
        write_lock_file,
    ))
}

//创建新包
pub fn create<P: AsRef<Path>>(pack_path: &P, pack_file: File) -> WBFPManager {
    create2(
        pack_path,
        DEF_COW,
        DEF_S_DATA_FILE,
        pack_file,
        None,
        None,
        None,
    )
}

//创建新包
pub fn create2<P: AsRef<Path>>(
    pack_path: &P,
    cow: bool,
    s_data_file: bool,
    pack_file: File,
    json_data_file: Option<File>,
    json_data_file_b: Option<File>,
    write_lock_file: Option<File>,
) -> WBFPManager {
    WBFPManager::new(
        pack_path,
        WBFilesPackData {
            attribute: Attribute {
                data_version: DataVersion {
                    value: DATA_VERSION,
                    compatible: DATA_VERSION_COMPATIBLE,
                },
                cow,
                s_data_file,
                empty_data_pos: Vec::new(),
            },
            pack_files_list: PackFilesList {
                files_list: HashMap::new(),
                data_length: 0,
                file_count: 0,
                dir_count: 0,
            },
        },
        pack_file,
        json_data_file,
        json_data_file_b,
        write_lock_file,
    )
}

//Test===
static TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/ok";
static TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/err";

fn _remove_test_pack_files<P: AsRef<Path>>(path: &P) {
    let pack_path = path.as_ref().to_str().unwrap().to_string();
    _ = fs::remove_file(&pack_path);
    let mut pack_json_path = pack_path.clone();
    pack_json_path.push_str(".json");
    _ = fs::remove_file(&pack_json_path);
    pack_json_path.push_str(".b");
    _ = fs::remove_file(pack_json_path);
    let mut pack_lock_path = pack_path.clone();
    pack_lock_path.push_str(".lock");
    _ = fs::remove_file(pack_lock_path);
}
//OK===
//创建文件测试
#[test]
fn test_create_new_pack_file() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    //测试文件
    let pack_file = pack_dir.join("pack");
    _remove_test_pack_files(&pack_file);
    //开始创建
    {
        create_new_file(&pack_file).expect("无法创建文件");
    } //使用作用域实现自动释放
    _remove_test_pack_files(&pack_file);
}

/*#[test]
fn test_create_new_pack_file_and_create_file() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    //测试文件
    let pack_file = pack_dir.join("pack");
    _remove_test_pack_files(&pack_file);
    //开始创建
    {
        let pack = create_new_file(&pack_file).expect("无法创建文件");
        pack.create_file_s()
    } //使用作用域实现自动释放
    _remove_test_pack_files(&pack_file);
}*/

//创建包文件同时创建虚拟目录
#[test]
fn test_create_new_pack_file_and_create_dir() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_OK_DIR_PATH);
    pack_dir.push_str("/create_new_file_and_create_dir");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    //测试文件
    let pack_file = pack_dir.join("pack");
    _remove_test_pack_files(&pack_file);
    //开始创建
    {
        let mut pack = create_new_file(&pack_file).expect("无法创建文件");
        let name = "Test/Test2".to_string();
        let modified_time = 0;
        pack.create_dir(&name, modified_time).unwrap();
        assert_eq!(pack.file_is_dir(&name), Some(true))
    } //使用作用域实现自动释放
    _remove_test_pack_files(&pack_file);
}

//ERR===
//创建文件测试_应失败
#[test]
#[should_panic(expected = "文件可能已存在，无法创建！")]
fn test_create_new_pack_file_err() {
    //测试目录
    let mut pack_dir = String::from(TEST_TEMP_ERR_DIR_PATH);
    pack_dir.push_str("/create_new_file");
    let pack_dir: &Path = pack_dir.as_ref();
    fs::create_dir_all(pack_dir).unwrap();
    let pack_file = pack_dir.join("pack");
    _remove_test_pack_files(&pack_file);
    //
    let r = {
        let mut pack = create_new_file(&pack_file).expect("无法创建文件");
        //给包文件上锁，使其无法创建
        pack.write_lock().expect("无法给包文件上锁");
        //当上锁时，无法创建是正确的。
        create_new_file(&pack_file)
    };
    if let Err(err) = r {
        _remove_test_pack_files(&pack_file);
        panic!("{}", err)
    }
}
