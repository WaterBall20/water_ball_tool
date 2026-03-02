/*
开始时间：26/2/11 15：51
 */
//use crate::wb_files_pack::manager::file::PackFileWR;
use crate::wb_files_pack::*;
use rand::RngExt;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::{fs, io};
use tracing::{debug, error, info, warn};

//pub mod file;

#[cfg(test)]
mod test;

/*
#[cfg(debug_assertions)]
#[cfg(test)]
mod new_test;*/

pub struct WBFPManager {
    // 清单实例
    manifest: WBFilesPackManifest,
    // 包文件实例
    pack_file: File,
    // 启用写时复制
    cow: bool,
    // 清单分离
    s_manifest_file: bool,
    // 当前包文件大小
    pack_file_length: u64,
    // 根结构位置
    root_struct_pos: u64,
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
    //总写入大小
    all_write_len: u64,
    //上次总写入的长度
    last_all_write_len: u64,
    //运行时总创建文件数量
    all_cr_file_count: u64,
    //上次创建总创建文件数量
    last_all_cr_file_count: u64,
    //GC数据列表
    gc_data_pos_list: DataPosList,
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
            all_write_len: 0,
            last_all_write_len: 0,
            all_cr_file_count: 0,
            last_all_cr_file_count: 0,
            gc_data_pos_list: DataPosList::default(),
        }
    }
}
impl WBFPManager {
    //创建实例
    fn new<P: AsRef<Path>>(
        pack_path: P,
        manifest: WBFilesPackManifest,
        pack_file: File,
        s_manifest_file: bool,
        write_lock_file: Option<File>,
    ) -> WBFPManager {
        Self::new2(
            pack_path,
            manifest,
            pack_file,
            s_manifest_file,
            write_lock_file,
            0,
            0,
        )
    }

    fn new2<P: AsRef<Path>>(
        pack_path: P,
        manifest: WBFilesPackManifest,
        pack_file: File,
        s_manifest_file: bool,
        write_lock_file: Option<File>,
        root_struct_pos: u64,
        pack_file_length: u64,
    ) -> WBFPManager {
        let cow = manifest.attribute().cow();
        let mut write_lock_path =
            String::from(pack_path.as_ref().to_str().expect("无法将转换路径成文本"));
        write_lock_path.push_str(".lock");
        let write_lock_path = Path::new(&write_lock_path).to_path_buf();
        WBFPManager {
            manifest,
            pack_file,
            cow,
            s_manifest_file,
            root_struct_pos,
            pack_file_length,
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
        if self.cow {
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

        //保存清单属性
        self.save_manifest_attribute().expect("写入清单属性失败");
        //self.save_json_data().expect("无法保存索引数据");

        self.write_unlock().expect("解除文件锁失败");
    }

    //读取===

    //包文件读取
    fn pack_file_read_root(&self, data: &mut [u8]) -> io::Result<()> {
        let mut file = &self.pack_file;
        file.read_exact(data)?;
        Ok(())
    }

    //写入===

    //创建文件

    //创建目录

    //将路径转换为Vec
    fn create_path_vec<P: AsRef<Path>>(path: P) -> Vec<String> {
        let path = path.as_ref();
        let path = if let Ok(r) = path
            .strip_prefix("./")
            .or_else(|_| path.strip_prefix("."))
            .or_else(|_| path.strip_prefix(".\\\\"))
        {
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

    //垃圾回收提交 TODO:未使用方法
    fn file_gc_add(&mut self, gc_pos_list: Vec<(u64, u64)>) -> io::Result<()> {
        for pos in gc_pos_list {
            //直接添加
            self.run_data.gc_data_pos_list.list.push(pos)
        }
        Ok(())
    }
    //清单文件垃圾回收提交 TODO:未使用方法
    fn manifest_file_gc_add(&mut self, gc_pos_list: Vec<(u64, u64)>) -> io::Result<()> {
        for pos in gc_pos_list {
            //直接添加
            self.manifest.run_data.gc_data_pos_list.list.push(pos)
        }
        Ok(())
    }
    //垃圾回收
    fn file_gc(&mut self) {
        Self::from_gc(
            &mut self.run_data.gc_data_pos_list,
            &mut self.manifest.empty_data_list,
        );
    }

    //清单文件垃圾回收
    fn manifest_file_gc(&mut self) -> io::Result<()> {
        if let Some(to_list) = &mut self.manifest.this_empty_data_list {
            Self::from_gc(&mut self.manifest.run_data.gc_data_pos_list, to_list);
            Ok(())
        } else {
            Err(Error::other("没有清单文件空数据列表实例"))
        }
    }

    fn from_gc(gc_data_pos_list: &mut DataPosList, data_pos_list: &mut DataPosList) {
        //准备：排序
        let pos_gc_list = &gc_data_pos_list.list;
        let pos_list = &mut data_pos_list.list;
        'gc_for: for (gc_pos, gc_len) in pos_gc_list {
            //排序插入
            let mut j = 0;
            while j < pos_list.len() {
                let (pos, _) = pos_list.get(j).unwrap();
                //插入判断
                if gc_pos < pos {
                    //如果位置在前
                    pos_list.insert(j, (*gc_pos, *gc_len));
                    continue 'gc_for;
                }
                j += 1;
            }
            pos_list.push((*gc_pos, *gc_len));
        }
        //清空缓存
        gc_data_pos_list.list.clear();

        //合并功能
        //当前索引
        let mut index = 0;
        //直接使用无限循环，内部判断
        loop {
            //下一个索引内容
            let (next_pos, next_len) = match pos_list.get(index + 1) {
                Some(v) => v.clone(), //必须复制
                None => break,
            };
            //当前索引内容
            let (this_pos, this_len) = match pos_list.get_mut(index) {
                Some(v) => v,
                None => break,
            };
            let this_end_pos = *this_pos + *this_len;
            //检查，判断当前位置加当前长度是否等于下一个位置
            if this_end_pos == next_pos {
                //合并，将下一个占用的大小加到当前大小
                *this_len += next_pos + next_len;
                pos_list.remove(1);
            } else {
                //否则什么都不做，并附加索引
                index += 1;
            }
        }
    }

    //获取可用的文件位置
    fn get_file_pos(&mut self, length: u64) -> Vec<(u64, u64)> {
        let mut add_pos: Vec<(u64, u64)> = Vec::new();
        let mut l_add_len = 0;
        //优先使用空隙
        let empty_data_pos = &mut self.manifest.empty_data_list.list;
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
            add_pos.push((self.pack_file_length, this_add_len))
        }
        add_pos
    }

    //获取连续的空间
    fn get_file_pos_l(&mut self, length: u64) -> (u64, u64) {
        //优先使用空隙
        let empty_data_pos = &mut self.manifest.empty_data_list.list;
        //优先使用空数据，但必须完整一块
        let index = 0;
        while !empty_data_pos.is_empty() {
            //从第一个开始
            let (pos, len) = empty_data_pos.get_mut(index).unwrap();
            //判断是否能占用完
            //剩余大小
            return if length == *len {
                //能占用完，但必须等于
                let r = (*pos, *len);
                empty_data_pos.remove(0);
                r
            } else if *len > length {
                //不能则切出
                let r = (*pos, *len - length);
                //修改，位置加大小使其向后移动，长度减大小使其边界不变
                *pos += length;
                *len -= length;
                r
            } else {
                continue;
            };
        }
        //扩容处理
        (self.pack_file_length, length)
    }
    //获取连续的清单空间
    fn get_manifest_file_pos_l(&mut self, length: u64) -> io::Result<(u64, u64)> {
        //尝试获取空数据列表

        if let Some(empty_data_pos) = &mut self.manifest.this_empty_data_list {
            let empty_data_pos = &mut empty_data_pos.list;
            //优先使用空数据，但必须完整一块
            let index = 0;
            while !empty_data_pos.is_empty() {
                //从第一个开始
                let (pos, len) = empty_data_pos.get_mut(index).unwrap();
                //判断是否能占用完
                //剩余大小
                return Ok(if length == *len {
                    //能占用完，但必须等于
                    let r = (*pos, *len);
                    empty_data_pos.remove(0);
                    r
                } else if *len > length {
                    //不能则切出
                    let r = (*pos, *len - length);
                    //修改，位置加大小使其向后移动，长度减大小使其边界不变
                    *pos += length;
                    *len -= length;
                    r
                } else {
                    continue;
                });
            }
            //扩容处理
            Ok((self.manifest.file_len, length))
        } else {
            Err(Error::other("清单文件空数据列表不存在"))
        }
    }

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
        self.set_pack_file_pos(FILE_HEADER_DATA_LENGTH_POS)?;
        //写入数据
        self.pack_file_write_root(self.pack_file_length.to_le_bytes().as_slice())?;
        Ok(())
    }
    //保存实例的清单
    fn save_manifest(&mut self) -> io::Result<()> {
        fn s_save(pack_struct: PackStruct) {}
        let root_struct = &self.manifest;
        todo!()
    }
    //保存结构
    /*fn save_pack_struct_and_metadata(&mut self) -> io::Result<()> {
        let root_struct_pos = self.root_struct_pos;
        let mut root_struct = &self.manifest.root_struct;
        self.s_pack_struct(&mut root_struct, root_struct_pos)
    }*/
    fn s_pack_struct(&mut self, pack_struct: &mut PackStruct, pack_struct_pos: u64) -> io::Result<
        ()> {
        //
        //旧数据大小
        let gc_len = pack_struct.data_len;
        //获取新数据
        let data = pack_struct.get_bytes_vec();
        //获取新位置
        let (new_pos, new_len) = self.get_file_pos_l(data.len() as u64);
        //更改文件指针位置
        self.set_pack_file_pos(new_pos)?;
        //写入
        self.pack_file_write_root(&data)?;
        //GC
        todo!()
    } //递归

    //保存属性
    fn save_manifest_attribute(&mut self) -> io::Result<()> {
        //属性
        let attribute = &mut self.manifest.attribute;
        //转换数据
        let data = attribute.get_bytes_vec();
        //写入数据
        //设置文件指针位置,从文件头后面写
        self.set_pack_file_pos(FILE_HEADER_LENGTH)?;
        //写入数据
        self.pack_file_write_root(&data)?;
        Ok(())
    }

    //保存空数据位置列表
    fn save_empty_data_pos_list(&mut self) -> io::Result<()> {
        //保存前GC处理
        self.file_gc();
        //当前文件位置
        let this_pos = self.manifest.attribute.empty_data_pos_list_pos;
        //数据大小
        let this_len = self.manifest.empty_data_list.data_len;
        //判断是否分离
        if self.s_manifest_file {
            //转换数据
            let data = self.manifest.empty_data_list.get_bytes_vec();
            //获取新空间
            let (new_pos, new_len) = self.get_manifest_file_pos_l(data.len() as u64)?;
            assert_eq!(data.len() as u64, new_len);
            //设置文件位置
            self.set_manifest_file_pos(new_pos)?;
            //写入
            self.manifest_file_write_root(&data)?;
            //更改指针
            self.manifest.attribute.empty_data_pos_list_pos = new_pos;
            Ok(())
        } else {
            //转换数据
            let data = self
                .manifest
                .empty_data_list
                .get_bytes_vec2(Some((this_pos, this_len)));
            //获取新空间
            let (new_pos, new_len) = self.get_file_pos_l(data.len() as u64);
            assert_eq!(data.len() as u64, new_len);
            //设置文件位置
            self.set_pack_file_pos(new_pos)?;
            //写入
            self.pack_file_write_root(&data)?;
            //GC提交
            self.file_gc_add(vec![(this_pos, this_pos)])?;
            //更改指针
            self.manifest.attribute.empty_data_pos_list_pos = new_pos;
            //GC处理
            self.file_gc();
            Ok(())
        }
    }
    //保存

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
        self.set_pack_file_len(self.pack_file_length + length)
    }

    //设置包文件文件地址
    fn set_pack_file_pos(&self, pos: u64) -> io::Result<()> {
        if self.run_data.pack_file_pos != pos {
            let mut file = &self.pack_file;
            file.seek(SeekFrom::Start(pos))?;
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
        (&mut self.pack_file).set_len(len)?;
        self.pack_file_length = len;
        Ok(())
    }

    //设置清单文件文件地址
    fn set_manifest_file_pos(&self, pos: u64) -> io::Result<()> {
        if let Some(file) = &self.manifest.file {
            let mut file = file;
            if self.run_data.pack_file_pos != pos {
                file.seek(SeekFrom::Start(pos))?;
            }
            Ok(())
        } else {
            Err(Error::other("清单文件实例不存在"))
        }
    }

    //清单文件写入
    fn manifest_file_write_root(&mut self, data: &[u8]) -> io::Result<()> {
        if let Some(file) = &mut self.manifest.file {
            file.write_all(data)?;
            let len = data.len() as u64;
            self.run_data.pack_file_pos += len;
            Ok(())
        } else {
            Err(Error::other("清单文件实例不存在"))
        }
    }

    //设置清单文件大小
    fn set_manifest_file_len(&mut self, len: u64) -> io::Result<()> {
        if let Some(file) = &mut self.manifest.file {
            file.set_len(len)?;
            Ok(())
        } else {
            Err(Error::other("清单文件实例不存在"))
        }
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
        manifest_path.push_str(".wbm");
        Some(File::create(&manifest_path)?)
    } else {
        None
    };

    Ok(create2(
        pack_path,
        cow,
        pack_file,
        s_manifest_file,
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
        pack_file,
        DEFAULT_S_DATA_FILE,
        None,
        None,
    )
}

fn create2<P: AsRef<Path>>(
    pack_path: &P,
    cow: bool,
    pack_file: File,
    s_manifest_file: bool,
    manifest_file: Option<File>,
    write_lock_file: Option<File>,
) -> WBFPManager {
    WBFPManager::new(
        pack_path,
        WBFilesPackManifest {
            attribute: Attribute {
                cow,
                ..Attribute::default()
            },
            root_struct: PackStruct::default(),
            empty_data_list: DataPosList::default(),
            this_empty_data_list: if s_manifest_file {
                Some(DataPosList::default())
            } else {
                None
            },
            this_empty_data_list_pos: 0,
            file: manifest_file,
            file_len: 0,
            run_data: WBFilesPackManifestRun {
                file_pos: 0,
                gc_data_pos_list: DataPosList::default(),
            },
        },
        pack_file,
        s_manifest_file,
        write_lock_file,
    )
}
