/*
开始时间：26/2/11 15：51
 */
use crate::wb_files_pack::manager::file::PackFileWR;
use crate::wb_files_pack::*;
use rand::RngExt;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::{fs, io};
use tracing::{debug, error, info, warn};

pub mod file;

#[cfg(test)]
mod test;

#[cfg(debug_assertions)]
#[cfg(test)]
mod new_test;

//内部缓冲区大小
//TODO:未使用的字段
static _FILE_BUF_LEN: usize = 1024 * 1024 * 8;

pub struct WBFPManager {
    manifest: WBFilesPacManifest,
    //包文件实例
    pack_file: File,
    //包数据文件实例
    manifest_file: Option<File>,
    //启用写时复制
    //TODO:未实现功能
    _cow: bool,
    //清单分离
    s_manifest_file: bool,
    //清单数据位置
    manifest_file_pos: u64,
    //当前包文件数据长度
    pack_file_data_length: u64,
    //运行时数据结构体
    pub(super) run_data: WBFPManagerRun,
} //水球包文件管理器
pub(super) struct WBFPManagerRun {
    //包文件路径
    pack_path: String,
    //写入锁
    write_lock: bool,
    //写入锁路径
    write_lock_path: PathBuf,
    //锁文件对象实例
    write_lock_file: Option<File>,
    //包文件位置
    pack_file_pos: u64,
    //id
    pub(super) id: u32,
    //总写入大小
    all_write_len: u64,
    //上次总写入的长度
    last_all_write_len: u64,
    //运行时总创建文件数量
    all_cr_file_count: u64,
    //上次创建总创建文件数量
    last_all_cr_file_count: u64,
    //写入缓冲区
    //write_buf: Vec<u8>,
    //写入缓冲区占用大小
    //write_buf_len: u64,
    //读取缓冲区
    //read_buf:Vec<u8>,
    //读取缓冲区占用大小
    //read_buf_len: u64,
} //运行时数据结构体
impl WBFPManagerRun {
    fn new<P: AsRef<Path>>(
        pack_path: P,
        write_lock_path: PathBuf,
        write_lock_file: Option<File>,
    ) -> WBFPManagerRun {
        WBFPManagerRun {
            pack_path: String::from(pack_path.as_ref().to_str().expect("无法将路径转换成文本")),
            write_lock: false,
            write_lock_path,
            write_lock_file,
            pack_file_pos: 0,
            id: rand::rng().random_range(0..100_000_000),
            all_write_len: 0,
            last_all_write_len: 0,
            all_cr_file_count: 0,
            last_all_cr_file_count: 0,
        }
    }
}
impl WBFPManager {
    //创建实例
    fn new<P: AsRef<Path>>(
        pack_path: P,
        s_manifest_file: bool,
        manifest: WBFilesPacManifest,
        pack_file: File,
        manifest_file: Option<File>,
        write_lock_file: Option<File>,
    ) -> WBFPManager {
        Self::new2(
            pack_path,
            s_manifest_file,
            manifest,
            pack_file,
            manifest_file,
            write_lock_file,
            0,
            0,
        )
    }

    fn new2<P: AsRef<Path>>(
        pack_path: P,
        s_manifest_file: bool,
        manifest: WBFilesPacManifest,
        pack_file: File,
        manifest_file: Option<File>,
        write_lock_file: Option<File>,
        manifest_file_pos: u64,
        pack_file_data_length: u64,
    ) -> WBFPManager {
        let cow = manifest.attribute().cow();
        let mut write_lock_path =
            String::from(pack_path.as_ref().to_str().expect("无法将转换路径成文本"));
        write_lock_path.push_str(".lock");
        let write_lock_path = Path::new(&write_lock_path).to_path_buf();
        WBFPManager {
            manifest,
            pack_file,
            manifest_file,
            _cow: cow,
            s_manifest_file,
            manifest_file_pos,
            pack_file_data_length,
            run_data: WBFPManagerRun::new(pack_path, write_lock_path, write_lock_file),
        }
    }

    //初始化新包
    fn init_new_pack(&mut self) {
        //写出文件头
        //类型名称
        self.pack_file_write_root(FILE_HEADER_TYPE_NAME.as_slice())
            .unwrap();
        //写入文件版本
        self.pack_file_write_root(FILE_HEADER_VERSION.as_slice())
            .unwrap();

        //写入标签===
        //文件头标签，二进制位:
        //|   0   |     1    |
        //|写实复制|清单文件分离|
        let mut header_tag: u8 = 0;
        if self._cow {
            header_tag |= 0b10000000
        }
        if self.s_manifest_file {
            header_tag |= 0b01000000
        }
        self.pack_file_write_root([header_tag].as_slice()).unwrap();
        //===

        //设置文件大小
        self.set_pack_file_len(FILE_HEADER_LENGTH)
            .expect("无法设置文件大小");


        //self.save_json_data().expect("无法保存索引数据");

        self.write_unlock().expect("解除文件锁失败");
    }

    //读取===

    /*pub fn get_files_list(&self) -> &PackFilesList {
        &self.pack_data.pack_files_list
    }

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
        let mut info_list = &self.pack_data.pack_files_list.files_list;
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

    //包文件读取
    fn pack_file_read_root(&self, data: &mut [u8]) -> io::Result<()> {
        //自我注意：这里必须是引用，除非想一次性写入。
        let mut file = &self.pack_file;
        file.read_exact(data)?;
        Ok(())
    }*/

    //写入===

    /*pub(crate) fn get_files_list_mut(&mut self) -> &mut PackFilesList {
        &mut self.pack_data.pack_files_list
    }

    //获取目录信息可变借用，带类型检查 TODO:未使用方法
    fn _get_dir_file_info_mut<P: AsRef<Path>>(&mut self, path: &P) -> Option<&mut PackFileInfo> {
        self._get_dir_file_info_mut2(&Self::create_path_vec(path))
    }

    //获取目录信息可变借用，带类型检查 TODO:未使用方法
    fn _get_dir_file_info_mut2(&mut self, path_list: &[String]) -> Option<&mut PackFileInfo> {
        match self.get_file_info_mut2(path_list) {
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

    //获取文件信息可变借用，仅文件，带类型检查 TODO:未使用方法
    fn _get_file_file_info_mut<P: AsRef<Path>>(&mut self, path: &P) -> Option<&mut PackFileInfo> {
        self._get_file_file_info_mut2(&Self::create_path_vec(path))
    }

    //获取文件信息可变借用，仅文件，带类型检查 TODO:未使用方法
    fn _get_file_file_info_mut2(&mut self, path_list: &[String]) -> Option<&mut PackFileInfo> {
        match self.get_file_info_mut2(path_list) {
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

    //获取文件信息的可变借用: TODO:未使用方法
    fn _get_file_indo_mut<P: AsRef<Path>>(&mut self, path: P) -> Option<&mut PackFileInfo> {
        self.get_file_info_mut2(&Self::create_path_vec(path))
    }
    fn get_file_info_mut2(&mut self, path_list: &[String]) -> Option<&mut PackFileInfo> {
        fn get_file_indo_s<'a>(
            path_list: &[String],
            path_list_index: usize,
            s_info_list: &'a mut HashMap<String, PackFileInfo>,
        ) -> Option<&'a mut PackFileInfo> {
            if let Some(name) = path_list.get(path_list_index) {
                //判断是否为最后一个
                match s_info_list.get_mut(name) {
                    Some(info) => {
                        if path_list_index + 1 == path_list.len() {
                            //是就直接返回
                            Some(info)
                        } else {
                            //不是就递归
                            //判断是否存在目录数据
                            if let PackFileKind::Dir(dir) = &mut info.file_kind {
                                get_file_indo_s(
                                    path_list,
                                    path_list_index + 1,
                                    dir.files_list_mut(),
                                )
                            } else {
                                None
                            }
                        }
                    }
                    None => None,
                }
            } else {
                None
            }
        }

        let info_list = self.get_files_list_mut().files_list_mut();
        get_file_indo_s(path_list, 0, info_list)
    }
    //获取文件读写器
    pub fn get_file_rw<P: AsRef<Path>>(&self, path: &P) -> io::Result<PackFileWR> {
        self.get_file_rw2(&Self::create_path_vec(path))
    }
    pub fn get_file_rw2(&self, path_list: &[String]) -> io::Result<PackFileWR> {
        let info = self
            .get_file_file_info2(path_list)
            .ok_or(Error::other(""))?;
        info.get_rw(self)
    }

    //更新目录大小和数量
    fn up_dir_len_and_count2(
        &mut self,
        path_list: &[String],
        add_length: u64,
        add_file_count: u64,
        add_dir_count: u64,
    ) -> io::Result<()> {
        //父目录的可变借用
        let mut s_info_list = self.get_files_list_mut().files_list_mut();
        //循环更新
        for name in path_list {
            match s_info_list.get_mut(name) {
                Some(info) => {
                    if let PackFileKind::Dir(dir) = &mut info.file_kind {
                        //更新
                        info.length += add_length;
                        dir.file_count += add_file_count;
                        dir.dir_count += add_dir_count;
                        //修改变量
                        s_info_list = dir.files_list_mut();
                    } else {
                        return Err(Error::new(ErrorKind::NotADirectory, "文件存在，但不是目录"));
                    }
                }
                None => return Err(Error::new(ErrorKind::NotADirectory, "目录不存在")),
            }
        }
        Ok(())
    }

    //创建文件
    pub fn create_file_new<P: AsRef<Path>>(
        &mut self,
        path: &P,
        modified_time: u128,
        length: u64,
    ) -> io::Result<PackFileWR> {
        self.create_file_new2(&Self::create_path_vec(path), modified_time, length)
    }
    fn create_file_new2(
        &mut self,
        path_list: &[String],
        modified_time: u128,
        length: u64,
    ) -> io::Result<PackFileWR> {
        fn m_create_new_file(
            modified_time: u128,
            length: u64,
            data_pos: Vec<(u64, u64)>,
            run_id: u32,
            file_name: &String,
            info_list: &mut HashMap<String, PackFileInfo>,
        ) -> io::Result<PackFileWR> {
            //检查文件是否存在
            if info_list.contains_key(file_name) {
                Err(Error::other("文件已存在"))
            } else {
                //创建文件
                let file_info = PackFileInfo {
                    name: file_name.clone(),
                    length,
                    is_dir: false,
                    modified_time,
                    file_kind: PackFileKind::File(PackFile {
                        data_pos: data_pos.clone(),
                        hash: String::new(),
                    }),
                };
                info_list.insert(file_name.clone(), file_info);
                //更新大小和数量
                Ok(PackFileWR::new(run_id, data_pos))
            }
        }
        let data_pos = self.get_file_pos(length);
        let run_id = self.run_data.id;
        //判断是否为根目录
        if path_list.len() > 1 {
            let dir_path_list = &path_list[..path_list.len() - 1];
            //尝试获取文件
            let dir_file_info = match self.get_file_info_mut2(dir_path_list) {
                Some(info) => info,
                None => {
                    //不存在就创建
                    self.create_dir2(dir_path_list, modified_time)?;
                    //获取已创建的文件，如果失败就panic
                    match self.get_file_info_mut2(dir_path_list) {
                        Some(r) => r,
                        None => return Err(Error::other("创建虚拟目录失败")),
                    }
                }
            };
            //创建文件
            if let PackFileKind::Dir(dir) = &mut dir_file_info.file_kind {
                let file_name = &path_list[path_list.len() - 1];
                let info_list = dir.files_list_mut();
                let r = m_create_new_file(
                    modified_time,
                    length,
                    data_pos,
                    run_id,
                    file_name,
                    info_list,
                )?;
                self.up_dir_len_and_count2(&path_list[..path_list.len() - 1], length, 1, 0)?;
                self.add_run_all_cr_file_count()?;
                self.add_pack_len(length)?;
                Ok(r)
            } else {
                Err(Error::new(
                    ErrorKind::NotADirectory,
                    "虚拟路径上存在同名文件，需要目录但实际为文件",
                ))
            }
            //判断路径是否为空
        } else if !path_list.is_empty() {
            //根目录处理
            let r = m_create_new_file(
                modified_time,
                length,
                data_pos,
                run_id,
                &path_list[0],
                &mut self.pack_data.pack_files_list.files_list,
            )?;
            //更新大小
            self.pack_data.pack_files_list.data_length += length;
            self.pack_data.pack_files_list.file_count += 1;
            self.add_run_all_cr_file_count()?;
            self.add_pack_len(length)?;
            Ok(r)
        } else {
            panic!("路径为空")
        }
    }*/

    /*//创建目录
    pub fn create_dir<P: AsRef<Path>>(&mut self, path: &P, modified_time: u128) -> io::Result<()> {
        self.create_dir2(&Self::create_path_vec(path), modified_time)
    }
    fn create_dir2(&mut self, path_list: &[String], modified_time: u128) -> io::Result<()> {
        fn get_dir(
            dir: &mut PackDir,
            modified_time: u128,
            path_list: &[String],
            path_list_index: usize,
        ) -> io::Result<MutDirReturn> {
            let info_list = dir.files_list_mut();
            //递归，带边界检查
            let r = if path_list_index + 1 < path_list.len() {
                create_dir_s(modified_time, path_list, path_list_index + 1, info_list)?
            } else {
                MutDirReturn {
                    add_length: 0,
                    add_dir_count: 0,
                    add_file_count: 0,
                }
            };
            //附加
            dir.file_count += r.add_file_count;
            dir.dir_count += r.add_dir_count;
            Ok(MutDirReturn {
                add_length: 0,
                add_file_count: dir.file_count,
                add_dir_count: dir.dir_count,
            })
        }

        fn create_new_dir(
            name: &str,
            modified_time: u128,
            path_list: &[String],
            path_list_index: usize,
            s_info_list: &mut HashMap<String, PackFileInfo>,
        ) -> io::Result<MutDirReturn> {
            //不存在则创建
            let mut files_list = HashMap::new();
            //递归，带边界检查
            let r = if path_list_index + 1 < path_list.len() {
                create_dir_s(
                    modified_time,
                    path_list,
                    path_list_index + 1,
                    &mut files_list,
                )?
            } else {
                MutDirReturn {
                    add_length: 0,
                    add_file_count: 0,
                    add_dir_count: 0,
                }
            };
            let new_dir_file_info = PackFileInfo {
                name: name.to_string(),
                length: 0,
                modified_time,
                is_dir: true,
                file_kind: PackFileKind::Dir(PackDir {
                    file_count: r.add_file_count,
                    dir_count: r.add_dir_count,
                    files_list,
                }),
            };
            s_info_list.insert(name.to_string(), new_dir_file_info);
            Ok(MutDirReturn {
                add_length: r.add_length,
                add_dir_count: r.add_dir_count + 1,
                add_file_count: r.add_file_count,
            })
        }

        fn create_dir_s(
            modified_time: u128,
            path_list: &[String],
            path_list_index: usize,
            s_info_list: &mut HashMap<String, PackFileInfo>,
        ) -> io::Result<MutDirReturn> {
            //获取文件名
            let name = path_list.get(path_list_index).unwrap();
            //判断目录是否存在
            match s_info_list.get_mut(name) {
                Some(info) => {
                    //存在则获取
                    match &mut info.file_kind {
                        PackFileKind::Dir(dir) => {
                            get_dir(dir, modified_time, path_list, path_list_index)
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
                    create_new_dir(name, modified_time, path_list, path_list_index, s_info_list)
                }
            }
        }

        //
        let pack_files_list = &mut self.pack_data.pack_files_list;
        let r = create_dir_s(modified_time, path_list, 0, &mut pack_files_list.files_list)?;
        pack_files_list.file_count += r.add_file_count;
        pack_files_list.dir_count += r.add_dir_count;
        Ok(())
    }*/

    //将路径转换为Vec
    fn create_path_vec<P: AsRef<Path>>(path: P) -> Vec<String> {
        let path = path.as_ref();
        let path = if let Ok(r) = path.strip_prefix("./").or_else(|_| path.strip_prefix(".")) {
            r
        } else {
            path
        };
        let mut path_list: Vec<String> = Vec::new();
        for item in path {
            path_list.push(String::from(item.to_str().expect("转换文本错误")));
        }
        path_list
    }

    //核心代码===

    /*//垃圾回收 TODO:未使用方法
    fn _file_gc(&mut self, gc_pos_s: Vec<(u64, u64)>) -> io::Result<()> {
        let pos_s = &mut self.pack_data.attribute.empty_data_pos;
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
        let empty_data_pos = &mut self.pack_data.attribute.empty_data_pos;
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
            add_pos.push((self.pack_file_data_length, this_add_len))
        }
        add_pos
    }*/

    //保存所有数据
    fn save_and_up_all(&mut self) -> io::Result<()> {
        self.up_data_length()?;
        self.save_manifest()?;
        Ok(())
    }

    //保存数据长度
    fn up_data_length(&mut self) -> io::Result<()> {
        //上锁
        self.write_lock()?;
        //修改包文件位置
        self.set_pack_file_pos2(FILE_HEADER_DATA_LENGTH_POS, true)?;
        //写入数据
        self.pack_file_write_root(self.pack_file_data_length.to_le_bytes().as_slice())?;
        Ok(())
    }
    //保存所有打开实例的清单
    fn save_manifest(&mut self) -> io::Result<()> {
        fn s_save(pack_struct: PackStruct) {}
        let root_struct = &self.manifest;
        todo!()
    }

    //写入数据文件
    /*fn save_json_data(&mut self) -> io::Result<()> {
        //数据分离：写入数据文件a
        fn write_json_data2_root_a(write_file: &mut Option<File>, buf: &[u8]) -> io::Result<usize> {
            if let Some(write_file) = write_file {
                //清空内容
                write_file.set_len(0)?;
                //重设位置
                write_file.seek(SeekFrom::Start(0))?;
                //写入
                write_file.write(buf)
            } else {
                Err(Error::other("数据文件a实例不存在"))
            }
        }
        //数据分离：写入数据文件b
        fn write_json_data2_root_b(write_file: &mut Option<File>, buf: &[u8]) -> io::Result<usize> {
            if let Some(write_file) = write_file {
                //清空内容
                write_file.set_len(0)?;
                //重设位置
                write_file.seek(SeekFrom::Start(0))?;
                //写入
                write_file.write(buf)
            } else {
                Err(Error::other("数据文件b实例不存在"))
            }
        }

        //设置写入锁
        self.write_lock()?;
        //判断是否分离
        if self.s_data_file {
            //Json数据转换
            let json_data = serde_json::to_vec_pretty(&self.pack_data)?;
            //写入判断
            if self.run_data.is_write_json_data_file_b {
                //B
                write_json_data2_root_b(&mut self.json_data_file_b, json_data.as_slice())?;
            } else {
                write_json_data2_root_a(&mut self.json_data_file, json_data.as_slice())?;
            }
            //翻转标志位
            self.run_data.is_write_json_data_file_b = !self.run_data.is_write_json_data_file_b;
            Ok(())
        } else {
            //Json数据转换
            let json_data = serde_json::to_vec(&self.pack_data)?;
            //获取包文件长度，并设为数据起始位置
            self.json_data_start_pos = self.pack_file_data_length;
            //设备包文件位置
            self.set_pack_file_pos(self.json_data_start_pos)?;
            //写入数据到包文件
            self.pack_file_write_root(json_data.as_slice())?;
            //立刻写入
            self.pack_file.sync_all()?;
            //设置结束位置
            self.json_data_end_pos = self.json_data_start_pos + json_data.len() as u64;
            //更新JSON数据的起始和结束位置
            //更改文件偏移量
            self.set_pack_file_pos2(FILE_HEADER_MANIFEST_DATA_START_POS_POS, true)?;
            //写入起始位置
            self.pack_file_write_root(self.json_data_start_pos.to_le_bytes().as_slice())?;
            //写入结束位置
            self.pack_file_write_root(self.json_data_end_pos.to_le_bytes().as_slice())?;
            Ok(())
        }
    }*/

    //写入锁信息
    fn write_lock_info(&self) -> PackLockInfo {
        let run_lock = self.run_data.write_lock;
        let path = &self.run_data.write_lock_path;
        write_lock_info(run_lock, path)
    }

    //设置写入锁
    fn write_lock(&mut self) -> io::Result<()> {
        let lock_file = write_lock(true, &self.run_data.write_lock_path)?;
        if let Some(lock_file) = lock_file {
            self.run_data.write_lock_file = Some(lock_file);
        }
        self.run_data.write_lock = true;
        self.pack_file.lock()?;
        Ok(())
    }

    //解除写入锁
    fn write_unlock(&mut self) -> io::Result<()> {
        //锁文件路径
        let path = &self.run_data.write_lock_path;
        //获取锁信息
        let lock_info = self.write_lock_info();
        if lock_info.file_lock {
            if lock_info.file_lock_is_symlink {
                return Err(Error::other("无法解锁，锁文件类型是符号链接"));
            }
            if lock_info.file_lock_is_dir {
                Err(Error::new(
                    ErrorKind::IsADirectory,
                    "无法解锁，锁文件类型是目录",
                ))
            } else {
                //释放文件句柄
                if let Some(lock_file) = self.run_data.write_lock_file.take() {
                    lock_file.unlock()?;
                    debug!("释放写入锁");
                    drop(lock_file);
                    fs::remove_file(path)?;
                }
                self.run_data.write_lock = false;
                self.pack_file.unlock()?;
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    //附加文件长度
    fn add_pack_len(&mut self, length: u64) -> io::Result<()> {
        //上锁
        self.write_lock()?;
        //附加长度
        self.pack_file_data_length += length;
        //更改文件大小
        self.set_pack_file_len(FILE_HEADER_LENGTH + self.pack_file_data_length)?;
        Ok(())
    }

    //设置包文件文件地址，不包括文件头，即相对于文件头末尾
    fn set_pack_file_pos(&self, pos: u64) -> io::Result<()> {
        self.set_pack_file_pos2(pos, false)
    }

    //设置文件大小，是否从文件头算，true ,表示从文件头开始算
    fn set_pack_file_pos2(&self, pos: u64, header_len: bool) -> io::Result<()> {
        if self.run_data.pack_file_pos != pos {
            let mut file = &self.pack_file;
            file.seek(SeekFrom::Start(if header_len {
                pos
            } else {
                FILE_HEADER_LENGTH + pos
            }))?;
            //self.run_data.pack_file_pos = pos;
        }
        Ok(())
    }

    //包文件写入
    fn pack_file_write_root(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_lock()?;
        //自我注意：这里必须是引用，除非想一次性写入。
        let file = &mut self.pack_file;
        file.write_all(data)?;
        let len = data.len() as u64;
        self.run_data.pack_file_pos += len;
        Ok(())
    }

    //设置包文件大小
    fn set_pack_file_len(&mut self, len: u64) -> io::Result<()> {
        self.write_lock()?;
        let file = &mut self.pack_file;
        file.set_len(len)
    }
}
/*impl Drop for WBFPManager {
    fn drop(&mut self) {
        //确保文件完全写入
        _ = self.pack_file.sync_all();
        //释放缓存文件===
        //强制写入索引数据
        _ = self.save_and_up_all();
        //如果索引文件分离
        if self.s_data_file {
            //写入两个索引文件，使其同步
            _ = self.save_json_data();
            //将B文件释放并删除
            let mut json_b_path = self.run_data.pack_path.clone();
            json_b_path.push_str(".json.b");
            //改动：移除自动删除代码，并改为用户提示
            info!(r#"包文件已安全保存，"{json_b_path}"是原子同步文件，用来确保安全，你可以删除。"#)
        }
        //释放写入锁
        _ = self.write_unlock();
    }
}*/

struct MutDirReturn {
    //
    add_length: u64,
    add_file_count: u64,
    add_dir_count: u64,
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
    //判断进程是否存在
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
                //获取锁存储的pid
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
    //写入当前进程pid
    write_lock.write_all(pid.to_le_bytes().as_slice())?;
    write_lock.sync_all()?;
    write_lock.lock()?;
    debug!("已为包文件上写入锁");
    Ok(write_lock)
}
//打开
/*pub fn open_file<P: AsRef<Path>>(pack_path: &P) -> io::Result<WBFPManager> {
    fn load_json_data(data: &[u8]) -> io::Result<WBFilesPacManifest> {
        let pack: WBFilesPacManifest = serde_json::from_slice(data)?;
        //兼容性判断
        //文件版本
        let ver = pack.attribute.data_version.value;
        //文件兼容版本
        let com_ver = pack.attribute.data_version.compatible;
        if ver != DATA_VERSION {
            warn!("Json数据版本不一致");
            //兼容性判断
            if ver < DATA_VERSION_COMPATIBLE {
                //版本低于解析器最低兼容版本
                error!("检查发现Json版本过低，低于实例最低兼容版本，拒绝创建包文件实例");
                return Err(Error::other("Json版本过低"));
            } else if com_ver > DATA_VERSION {
                //版本过高
                error!("检查发现Json版本过高，实例版本低于文件指定的兼容版本，拒绝创建包文件实例");
                return Err(Error::other("Json版本过高"));
            }
        }
        Ok(pack)
    }

    //打开水球包文件
    let mut pack_file = File::options().read(true).write(true).open(pack_path)?;
    //读取完整的文件头
    let mut header = [0u8; FILE_HEADER_LENGTH as usize];
    let header_r_len = pack_file.read(&mut header)?;
    if (header_r_len as u64) < FILE_HEADER_LENGTH {
        return Err(Error::other("无法读取完整的文件头"));
    }
    //判断文件类型
    const HEADER_TYPE_LEN: usize = FILE_HEADER_TYPE_NAME.len();
    let he_type = &header[..HEADER_TYPE_LEN];
    if he_type != FILE_HEADER_TYPE_NAME {
        return Err(Error::other("文件类型不是水球包文件"));
    }
    //判断版本是否一致
    let he_ver = &header[HEADER_TYPE_LEN..HEADER_TYPE_LEN + 2];
    if he_ver != FILE_HEADER_VERSION {
        return Err(Error::other(
            "文件格式版本不一致，对于文件格式，版本必须一致",
        ));
    }
    //读取标志位
    let he_tag = header[HEADER_TYPE_LEN + 2..FILE_HEADER_MANIFEST_DATA_START_POS_POS as usize][0];
    //写时复制 TODO:未使用变量
    let _cow = he_tag >> 7 == 1;
    let s_data_file = he_tag >> 6 == 1;
    let pack_data_len = u64::from_le_bytes(
        header[FILE_HEADER_DATA_LENGTH_POS as usize
            ..(FILE_HEADER_DATA_LENGTH_POS + FILE_HEADER_DATA_LENGTH_LENGTH) as usize]
            .try_into()
            .unwrap(),
    );
    //如果分离数据文件
    if s_data_file {
        let mut json_path_str = pack_path.as_ref().to_str().unwrap().to_string();
        json_path_str.push_str(".json");
        let json_path = PathBuf::from(&json_path_str);
        //获取当前已存在的数据
        let json_data = fs::read(&json_path)?;
        let pack_data = load_json_data(&json_data).map_or_else(
            |_| {
                //如果错误就尝试读取B文件
                json_path_str.push_str(".b");
                let json_b_path = PathBuf::from(json_path_str);
                //判断文件是否存在
                if json_b_path.is_file() {
                    //获取数据
                    let json_data = fs::read(&json_b_path)?;
                    Ok(load_json_data(&json_data)?)
                } else if json_b_path.is_dir() {
                    Err(Error::other(
                        "无法读取[包文件]数据文件A，自动尝试通过同步文件B恢复，但目前是目录",
                    ))
                } else if json_b_path.is_symlink() {
                    Err(Error::other(
                        "无法获取[包文件]数据文件A，自动尝试通过同步文件B恢复，目前是符号链接，但链接已断",
                    ))
                } else {
                    Err(Error::new(
                        ErrorKind::NotFound,
                        "无法获取[包文件]数据文件A，自动尝试通过同步文件B恢复，但文件不存在或拒绝访问",
                    ))
                }
            },
            Ok,
        )?;
        let json_file = File::options().read(true).write(true).open(json_path)?;
        //创建实例
        Ok(WBFPManager::new2(
            pack_path,
            pack_data,
            pack_file,
            Some(json_file),
            None,
            None,
            0,
            0,
            pack_data_len,
        ))
    } else {
        //获取起始位置和结束位置
        let json_data_start_pos = u64::from_le_bytes(
            *<&[u8; 8]>::try_from(
                &header[FILE_HEADER_MANIFEST_DATA_START_POS_POS as usize
                    ..FILE_HEADER_MANIFEST_DATA_START_POS_POS as usize + 8],
            )
                .unwrap(),
        );
        let json_data_end_pos = u64::from_le_bytes(
            *<&[u8; 8]>::try_from(
                &header[FILE_HEADER_MANIFEST_DATA_START_POS_POS as usize + 8
                    ..FILE_HEADER_MANIFEST_DATA_START_POS_POS as usize + 16],
            )
                .unwrap(),
        );
        let json_data_len = json_data_end_pos - json_data_start_pos;

        //读取数据
        //设置文件位置
        pack_file.seek(SeekFrom::Start(json_data_start_pos + FILE_HEADER_LENGTH))?;
        //缓存
        let mut json_data: Vec<u8> = Vec::with_capacity(json_data_len as usize);
        //限制大小读取
        (&mut pack_file)
            .take(json_data_len)
            .read_to_end(&mut json_data)?;

        if json_data_len as usize != json_data.len() {
            return Err(Error::other("读取到的Json数据不完整"));
        }
        let pack_data = load_json_data(&json_data)?;
        Ok(WBFPManager::new2(
            pack_path,
            pack_data,
            pack_file,
            None,
            None,
            None,
            json_data_start_pos,
            json_data_end_pos,
            pack_data_len,
        ))
    }
}*/

//创建===

//创建新包文件
pub fn create_new_file<P: AsRef<Path>>(pack_path: &P) -> io::Result<WBFPManager> {
    create_new_file2(pack_path, DEFAULT_COW, DEFAULT_S_DATA_FILE)
}

//创建新包文件
pub fn create_new_file2<P: AsRef<Path>>(
    pack_path: &P,
    cow: bool,
    s_data_file: bool,
) -> Result<WBFPManager, Error> {
    //判断文件是否存在
    let mut path = match pack_path.as_ref().try_exists() {
        Ok(true) => Err(Error::other("文件可能已存在，无法创建！")),
        Ok(false) => create_file2(pack_path, cow, s_data_file, true),
        Err(_) => create_file2(pack_path, cow, s_data_file, true),
    }?;
    path.init_new_pack();
    Ok(path)
}

//创建新包文件,
pub fn create_file2<P: AsRef<Path>>(
    pack_path: &P,
    cow: bool,
    s_manifest_file: bool,
    create_new: bool,
) -> Result<WBFPManager, Error> {
    let mut write_lock_path =
        String::from(pack_path.as_ref().to_str().expect("无法将路径转换成文本"));
    write_lock_path.push_str(".lock");
    let write_lock_path = PathBuf::from(write_lock_path);
    let write_lock_file = write_lock(false, &write_lock_path)?;
    //创建包文件文件
    let pack_file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .create_new(create_new)
        .open(pack_path)?;
    //创建包文件数据文件

    let manifest_file = if s_manifest_file {
        let mut manifest_path =
            String::from(pack_path.as_ref().to_str().expect("无法将路径转换成文件"));
        manifest_path.push_str(".json");
        Some(File::create(&manifest_path)?)
    } else {
        None
    };

    Ok(create2(
        pack_path,
        cow,
        s_manifest_file,
        pack_file,
        manifest_file,
        write_lock_file,
    ))
}

//创建新包实例===

//TODO:未使用函数
fn _create<P: AsRef<Path>>(pack_path: &P, pack_file: File) -> WBFPManager {
    create2(
        pack_path,
        DEFAULT_COW,
        DEFAULT_S_DATA_FILE,
        pack_file,
        None,
        None,
    )
}

fn create2<P: AsRef<Path>>(
    pack_path: &P,
    cow: bool,
    s_manifest_file: bool,
    pack_file: File,
    manifest_file: Option<File>,
    write_lock_file: Option<File>,
) -> WBFPManager {
    WBFPManager::new(
        pack_path,
        s_manifest_file,
        WBFilesPacManifest {
            attribute: Attribute {
                cow,
                ..Attribute::default()
            },
            ..WBFilesPacManifest::default()
        },
        pack_file,
        manifest_file,
        write_lock_file,
    )
}
