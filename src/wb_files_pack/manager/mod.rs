use crate::wb_files_pack::manager::file::PackFileWR;
/*
开始时间：26/2/11 15：51
 */
//use crate::wb_files_pack::manager::file::PackFileWR;
use crate::wb_files_pack::{DATA_BLOCK_LEN, WBFilesPackManifest, DataPosList, PackStructItem, PackStruct, PackStructItemType, PackFileMetadataType, PackFileMetadata, ManifestDataBlock, PackFileMetadataFile, PackFileMetadataDir, PackStructItemDir, Attribute, WBFilesPackManifestRun};
use std::fs::File;
use std::io::{ Error, ErrorKind, Read, Seek, SeekFrom, Write };
use std::path::{ Path, PathBuf };
use std::{ fs, io };
use tracing:: debug ;

pub mod file;

#[cfg(test)]
mod test;

/*
#[cfg(debug_assertions)]
#[cfg(test)]
mod new_test;*/

//文件头===
//文件头-文件名:WPFilesPack
pub static FILE_HEADER_TYPE_NAME: [u8; 11] = [
    0x57u8, 0x42, 0x46, 0x69, 0x6c, 0x65, 0x73, 0x50, 0x61, 0x63, 0x6b,
];
//文件头版本
static FILE_HEADER_VERSION: [u8; 2] = [0, 2];

//文件头标签位长度
static FILE_HEADER_BOOL_DATA_LENGTH: usize = 1;

//文件头清单数据属性起始位置的位置
static FILE_HEADER_MANIFEST_ATTRIBUTE_POS_POS: u64 = (FILE_HEADER_TYPE_NAME.len() +
    FILE_HEADER_VERSION.len() +
    FILE_HEADER_BOOL_DATA_LENGTH) as u64;
//文件头清单数据属性起始位置数据段长度
static FILE_HEADER_MANIFEST_ATTRIBUTE_POS_POS_LENGTH: u64 = 8;

//文件头数据长度位置
static FILE_HEADER_DATA_LENGTH_POS: u64 =
    FILE_HEADER_MANIFEST_ATTRIBUTE_POS_POS + FILE_HEADER_MANIFEST_ATTRIBUTE_POS_POS_LENGTH;

//文件头数据长度长度
static FILE_HEADER_DATA_LENGTH_LENGTH: u64 = 8;
//文件头长度
static FILE_HEADER_DATA_LENGTH: u64 =
    FILE_HEADER_MANIFEST_ATTRIBUTE_POS_POS + FILE_HEADER_DATA_LENGTH_LENGTH;

//文件头块长度
static FILE_HEADER_BLOCK_LEN: usize = DATA_BLOCK_LEN;
//默认写实复制
pub static DEFAULT_COW: bool = false;

//默认分离数据为单独文件
pub static DEFAULT_S_DATA_FILE: bool = true;

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
        write_lock_file: Option<File>
    ) -> Self {
        Self {
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
        write_lock_file: Option<File>
    ) -> Self {
        Self::new2(pack_path, manifest, pack_file, s_manifest_file, write_lock_file, 0, 0)
    }

    fn new2<P: AsRef<Path>>(
        pack_path: P,
        manifest: WBFilesPackManifest,
        pack_file: File,
        s_manifest_file: bool,
        write_lock_file: Option<File>,
        root_struct_pos: u64,
        pack_file_length: u64
    ) -> Self {
        let cow = manifest.attribute().cow();
        let mut write_lock_path = String::from(
            pack_path.as_ref().to_str().expect("无法将转换路径成文本")
        );
        write_lock_path.push_str(".lock");
        let write_lock_path = Path::new(&write_lock_path).to_path_buf();
        Self {
            manifest,
            pack_file,
            cow,
            s_manifest_file,
            pack_file_length,
            run_data: WBFPManagerRun::new(pack_path, write_lock_path, write_lock_file),
        }
    }

    //初始化新包
    fn init_new_pack(&mut self) {
        //文件头缓存
        let mut file_header_buf = Vec::with_capacity(DATA_BLOCK_LEN);
        //写出文件头
        //类型名称
        for value in FILE_HEADER_TYPE_NAME {
            file_header_buf.push(value);
        }
        //写入文件版本
        for value in FILE_HEADER_VERSION {
            file_header_buf.push(value);
        }

        //写入标签===
        //文件头标签，二进制位:
        //|   0   |     1    |
        //|写实复制|清单文件分离|
        let mut header_tag: u8 = 0;
        if self.cow {
            header_tag |= 0b1000_0000;
        }
        if self.s_manifest_file {
            header_tag |= 0b0100_0000;
        }
        file_header_buf.push(header_tag);
        //===

        self.pack_file_write_root(&file_header_buf).expect("无法写入文件头");

        //设置文件大小
        self.set_pack_file_len(FILE_HEADER_BLOCK_LEN as u64).expect("无法设置文件大小");

        //初始化并保存空数据列表
        self.init_save_empty_data_list().expect("初始化和保存空数据列表失败");

        //初始化并保存根结构
        //self.save_pack_struct_and_metadata().expect("保存根结构失败");
        self.init_save_root_pack_struct().expect("初始化和保存根结构失败");

        //初始化并保存清单属性
        self.save_manifest_attribute().expect("写入清单属性失败");

        //初始化并保存大小
        self.save_pack_length().expect("无法保存有效大小属性");
    }

    fn init_save_root_pack_struct(&mut self) -> io::Result<()> {
        let block_data = self.manifest.root_struct.get_block_data().0;
        if self.s_manifest_file {
            let (pos, _) = self.get_manifest_file_pos(DATA_BLOCK_LEN as u64)?;
            self.set_manifest_file_pos(pos)?;
            self.manifest_file_write_root(&block_data)?;
            self.manifest.attribute.root_struct_pos = pos;
        } else {
            let (pos, _) = self.get_file_pos(DATA_BLOCK_LEN as u64);
            self.set_pack_file_pos_write(pos)?;
            self.pack_file_write_root(&block_data)?;
            self.manifest.attribute.root_struct_pos = pos;
        }
        Ok(())
    }

    fn init_save_empty_data_list(&mut self) -> io::Result<()> {
        let block_data = self.manifest.empty_data_list.update_get_block_data().unwrap().0;
        if self.s_manifest_file {
            let (pos, _) = self.get_manifest_file_pos(DATA_BLOCK_LEN as u64)?;
            self.set_manifest_file_pos(pos)?;
            self.manifest_file_write_root(&block_data)?;
            self.manifest.attribute.empty_data_pos_list_pos = pos;
        } else {
            let (pos, _) = self.get_file_pos(DATA_BLOCK_LEN as u64);
            self.set_pack_file_pos_write(pos)?;
            self.pack_file_write_root(&block_data)?;
            self.manifest.attribute.empty_data_pos_list_pos = pos;
        }
        Ok(())
    }

    //读取===

    //包文件读取
    fn pack_file_read_root(&self, data: &mut [u8]) -> io::Result<()> {
        let mut file = &self.pack_file;
        file.read_exact(data)?;
        Ok(())
    }

    fn get_pack_struct_item<P: AsRef<Path>>(&self, path: P) -> io::Result<&PackStructItem> {
        todo!()
    }

    pub fn get_dir<P: AsRef<Path>>(&self, path: P) -> io::Result<&PackStruct> {
        let mut pack_struct = &self.manifest.root_struct;
        let path_list = Self::path_to_string_vec(path);
        let mut path_list_iter = path_list.iter();
        let mut name = path_list_iter.next();
        while let Some(this_name) = name {
            if pack_struct.items.contains_key(this_name) {
                let this_pack_struct = if
                    let PackStructItemType::Dir(dir) = &pack_struct.items
                        .get(this_name)
                        .unwrap().item_type
                {
                    if let Some(pack_struct) = &dir.pack_struct {
                        pack_struct
                    } else {
                        //TODO:需要实现结构加载
                        panic!("暂未实现结构加载")
                    }
                } else {
                    return Err(Error::new(ErrorKind::NotADirectory, "路径存在非目录"));
                };
                let next_name = path_list_iter.next();
                if next_name.is_some() {
                    name = next_name;
                    pack_struct = this_pack_struct;
                } else {
                    return Ok(this_pack_struct);
                }
            } else {
                return Err(Error::new(ErrorKind::DirectoryNotEmpty, "目录不存在"));
            }
        }
        Err(Error::other("未知错误"))
    }

    //写入===

    //创建文件

    pub fn create_file_new<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64
    ) -> io::Result<PackFileWR<'_>> {
        let cow = self.cow;
        self.create_file_new2(path, modified, len, cow)
    }

    fn s_create_file_new<>(
        s_pack_struct: &mut PackStruct,
        dir_path_list: &[String],
        dir_path_list_index: usize,
        file_name: &str,
        pack_struct_item: PackStructItem
    ) -> io::Result<(PackStructItem, MutDirAddReturn)> {
        let dir_name = &dir_path_list[dir_path_list_index];
        //判断目录是否存在
        if let Some(item) = s_pack_struct.items.get_mut(dir_name) {
            if let PackStructItemType::Dir(dir) = &mut item.item_type {
                if dir.pack_struct.is_none() {
                    //TODO:需要实现加载结构
                    todo!();
                }
                if let Some(pack_struct) = &mut dir.pack_struct {
                    //递归
                    let r = if dir_path_list_index + 1 < dir_path_list.len() {
                        let (p, r) = Self::s_create_file_new(
                            pack_struct,
                            dir_path_list,
                            dir_path_list_index + 1,
                            dir_name,
                            pack_struct_item
                        )?;
                        let dir_name = &dir_path_list[dir_path_list_index + 1];
                        pack_struct.items.insert(dir_name.clone(), p);
                        r
                    } else {
                        if let Some(v) = pack_struct.items.get(file_name) {
                            return match &v.item_type {
                                PackStructItemType::Dir(_) =>
                                    Err(Error::new(ErrorKind::IsADirectory, "存在同名目录")),
                                PackStructItemType::File =>
                                    Err(Error::new(ErrorKind::AlreadyExists, "文件已存在")),
                            };
                        }
                        pack_struct.items.insert(file_name.to_string(), pack_struct_item);
                        MutDirAddReturn {
                            dir_count: 0,
                            file_count: 1,
                            length: 0,
                        }
                    };
                    //更新元数据
                    if item.pack_file_metadata.is_none() {
                        //TODO:需要实现加载元数据
                        todo!();
                    }
                    if let Some(metadata) = &mut item.pack_file_metadata {
                        if let PackFileMetadataType::Dir(dir) = &mut metadata.file_type {
                            dir.dir_count += r.dir_count;
                            Ok((
                                PackStructItem::get_empty(),
                                MutDirAddReturn {
                                    dir_count: r.dir_count,
                                    file_count: 0,
                                    length: 0,
                                },
                            ))
                        } else {
                            panic!("存在逻辑错误")
                        }
                    } else {
                        panic!("元数据没有被加载")
                    }
                } else {
                    panic!("结构没有被加载")
                }
            } else {
                Err(Error::new(ErrorKind::NotADirectory, "提供的目录已存在非目录的文件"))
            }
        } else {
            Err(Error::new(ErrorKind::DirectoryNotEmpty, "目录不存在"))
        }
    }

    fn create_file_new2<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
        cow: bool
    ) -> io::Result<PackFileWR<'_>> {
        let path_list = Self::path_to_string_vec(path);
        let data_pos_list = DataPosList { data_block: None, list: vec![self.get_file_pos(len)] };
        let wr_data_pos_list = data_pos_list.clone();
        let pack_file_metadata = PackFileMetadata {
            modified,
            len,
            cow,
            data_block: ManifestDataBlock::default(),
            file_type: PackFileMetadataType::File(PackFileMetadataFile {
                hash_type: 0,
                hash: Vec::new(),
                data_pos_list,
            }),
        };
        let pack_struct_item = PackStructItem {
            name: path_list[0].clone(),
            item_type: PackStructItemType::File,
            metadata_file_pos: 0,
            pack_file_metadata: Some(pack_file_metadata),
        };
        if path_list.len() > 1 {
            //尝试创建目录
            self.create_dir_all2(&path_list[..path_list.len() - 1])?;
            Self::s_create_file_new(
                &mut self.manifest.root_struct,
                &path_list[..&path_list.len() - 1],
                0,
                &path_list[path_list.len() - 1],
                pack_struct_item
            )?;
        } else {
            self.manifest.root_struct.items.insert(path_list[0].clone(), pack_struct_item);
        }
        self.save_pack_struct_and_metadata()?;
        Ok(PackFileWR::new(self, wr_data_pos_list))
    }

    //创建目录(结构)
    pub fn create_dir_all<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        self.create_dir_all2(&Self::path_to_string_vec(path))
    }

    fn s_create_dir_all(
        s_pack_struct: &mut PackStruct,
        path_list: &[String],
        path_list_index: usize,
        cow: bool
    ) -> io::Result<(PackStructItem, MutDirAddReturn)> {
        let name = &path_list[path_list_index];
        //判断目录是否存在
        if let Some(item) = s_pack_struct.items.get_mut(name) {
            if let PackStructItemType::Dir(dir) = &mut item.item_type {
                if dir.pack_struct.is_none() {
                    //TODO:需要实现加载结构
                    todo!();
                }
                if let Some(pack_struct) = &mut dir.pack_struct {
                    //递归
                    let r = if path_list_index + 1 < path_list.len() {
                        let (p, r) = Self::s_create_dir_all(
                            pack_struct,
                            path_list,
                            path_list_index + 1,
                            cow
                        )?;
                        let name = &path_list[path_list_index + 1];
                        pack_struct.items.insert(name.clone(), p);
                        r
                    } else {
                        MutDirAddReturn {
                            dir_count: 0,
                            file_count: 0,
                            length: 0,
                        }
                    };
                    //更新元数据
                    if item.pack_file_metadata.is_none() {
                        //TODO:需要实现加载元数据
                        todo!();
                    }
                    if let Some(metadata) = &mut item.pack_file_metadata {
                        if let PackFileMetadataType::Dir(dir) = &mut metadata.file_type {
                            dir.dir_count += r.dir_count;
                            Ok((
                                item.clone(),
                                MutDirAddReturn {
                                    dir_count: r.dir_count,
                                    file_count: 0,
                                    length: 0,
                                },
                            ))
                        } else {
                            panic!("存在逻辑错误")
                        }
                    } else {
                        panic!("元数据没有被加载")
                    }
                } else {
                    panic!("结构没有被加载")
                }
            } else {
                Err(Error::new(ErrorKind::NotADirectory, "提供的目录已存在非目录的文件"))
            }
        } else {
            Self::create_dir_new(path_list, path_list_index, cow, name)
        }
    }

    fn create_dir_new(
        path_list: &[String],
        path_list_index: usize,
        cow: bool,
        name: &str
    ) -> io::Result<(PackStructItem, MutDirAddReturn)> {
        let mut pack_struct = PackStruct::default();
        //递归
        let r = if path_list_index + 1 < path_list.len() {
            let (p, r) = Self::s_create_dir_all(
                &mut pack_struct,
                path_list,
                path_list_index + 1,
                cow
            )?;
            let name = &path_list[path_list_index + 1];
            pack_struct.items.insert(name.clone(), p);
            r
        } else {
            MutDirAddReturn {
                dir_count: 0,
                file_count: 0,
                length: 0,
            }
        };
        let this_metadata = PackFileMetadata {
            len: 0,
            cow,
            data_block: ManifestDataBlock::default(),
            modified: 0,
            file_type: PackFileMetadataType::Dir(PackFileMetadataDir {
                file_count: 0, //创建目录不会产生普通文件
                dir_count: r.dir_count,
            }),
        };
        Ok((
            PackStructItem {
                name: name.to_string(),
                metadata_file_pos: 0,
                pack_file_metadata: Some(this_metadata),
                item_type: PackStructItemType::Dir(PackStructItemDir {
                    struct_file_pos: 0,
                    pack_struct: Some(pack_struct),
                }),
            },
            MutDirAddReturn {
                dir_count: r.dir_count + 1,
                file_count: 0,
                length: 0,
            },
        ))
    }

    fn create_dir_all2(&mut self, path_list: &[String]) -> io::Result<()> {
        let (p, r) = Self::s_create_dir_all(
            &mut self.manifest.root_struct,
            path_list,
            0,
            self.cow
        )?;
        let name = &path_list[0];
        self.manifest.root_struct.items.insert(name.clone(), p);
        self.manifest.attribute.dir_count += r.dir_count;
        self.save_pack_struct_and_metadata()?;
        Ok(())
    }

    //将路径转换为Vec
    fn path_to_string_vec<P: AsRef<Path>>(path: P) -> Vec<String> {
        let path = path.as_ref();
        let path = path
            .strip_prefix("./")
            .or_else(|_| path.strip_prefix("."))
            .or_else(|_| path.strip_prefix(".\\\\"))
            .map_or(path, |r| r);
        let mut path_list: Vec<String> = Vec::new();
        for item in path {
            path_list.push(String::from(item.to_str().expect("转换文本错误")));
        }
        path_list
    }

    //核心代码===

    //保存根结构
    fn save_root_pack_struct(&mut self) -> io::Result<()> {
        let old_pos = self.manifest.attribute.root_struct_pos;
        let root_struct = &mut self.manifest.root_struct;
        let old_block_len = root_struct.run_data.data_block.get_this_block_len_u64();
        let (block_data, new_block) = root_struct.get_block_data();
        let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_block_len)?;
        if old_pos != pos {
            self.manifest.attribute.root_struct_pos = pos;
        }
        Ok(())
    }

    //TODO:保存所有打开的结构及其元数据
    fn save_pack_struct_and_metadata(&mut self) -> io::Result<()> {
        fn m_save_pack_struct_and_metadata(
            manager: &mut WBFPManager,
            item_list: &mut Vec<&mut PackStructItem>
        ) -> io::Result<()> {
            fn m_save_metadata(
                manager: &mut WBFPManager,
                item: &mut PackStructItem
            ) -> io::Result<()> {
                if let Some(metadata) = &mut item.pack_file_metadata {
                    let data_block = &mut metadata.data_block;
                    let (new_block, pos) = manager.save_metadata_write(metadata)?;
                    //如果发生重新分配
                    if new_block {
                        item.metadata_file_pos = pos;
                    }
                }
                Ok(())
            }
            for item in item_list {
                if let PackStructItemType::Dir(dir) = &mut item.item_type {
                    //如果存在结构实例就更新子项
                    if let Some(pack_struct) = &mut dir.pack_struct {
                        //递归处理
                        let mut up_struct_items = Vec::new();
                        for item in pack_struct.items.values_mut() {
                            up_struct_items.push(item);
                        }
                        m_save_pack_struct_and_metadata(manager, &mut up_struct_items)?;
                        //保存结构
                        let (new_block, pos) = manager.save_pack_struct_write(pack_struct)?;
                        if new_block {
                            dir.struct_file_pos = pos;
                        }
                    }
                }
                m_save_metadata(manager, item)?;
            }
            Ok(())
        }

        let mut root_struct_item_name_list = Vec::new();
        let mut up_item_list = Vec::new();
        let mut up_struct_items = Vec::new();
        for name in self.manifest.root_struct.items.keys() {
            root_struct_item_name_list.push(name.clone());
        }
        //暂移
        for name in root_struct_item_name_list {
            if let Some(value) = self.manifest.root_struct.items.remove(&name) {
                up_item_list.push(value);
            }
        }
        //转换成引用
        for item in &mut up_item_list {
            up_struct_items.push(item);
        }
        m_save_pack_struct_and_metadata(self, &mut up_struct_items)?;
        //
        for item in up_item_list {
            self.manifest.root_struct.items.insert(item.name.clone(), item);
        }
        self.save_root_pack_struct()?;
        Ok(())
    }

    //保存结构
    fn save_pack_struct_write(&mut self, pack_struct: &mut PackStruct) -> io::Result<(bool, u64)> {
        let old_pos = pack_struct.run_data.data_block.block_file_pos;
        let old_block_len = pack_struct.run_data.data_block.get_this_block_len_u64();
        let (block_data, new_block) = pack_struct.get_block_data();
        let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_block_len)?;
        pack_struct.run_data.data_block.block_file_pos = pos;
        Ok((new_block, pos))
    }

    //保存元数据
    fn save_metadata_write(&mut self, metadata: &mut PackFileMetadata) -> io::Result<(bool, u64)> {
        let old_pos = metadata.data_block.block_file_pos;
        let old_block_len = metadata.data_block.get_this_block_len_u64();
        let (block_data, new_block) = metadata.get_block_data();
        let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_block_len)?;
        metadata.data_block.block_file_pos = pos;
        Ok((new_block, pos))
    }

    fn manifest_data_block_write(
        &mut self,
        block_data: &[u8],
        new_block: bool,
        old_pos: u64,
        old_block_len: u64
    ) -> io::Result<u64> {
        Ok(
            if new_block {
                if self.s_manifest_file {
                    let (new_pos, _) = self.get_manifest_file_pos(block_data.len() as u64)?;
                    self.set_manifest_file_pos(new_pos)?;
                    self.manifest_file_write_root(block_data)?;
                    self.file_gc_add(vec![(old_pos, old_block_len)]);
                    new_pos
                } else {
                    let (new_pos, _) = self.get_file_pos(block_data.len() as u64);
                    self.set_pack_file_pos_write(new_pos)?;
                    self.pack_file_write_root(block_data)?;
                    self.file_gc_add(vec![(old_pos, old_block_len)]);
                    new_pos
                }
            } else if self.s_manifest_file {
                self.set_manifest_file_pos(old_pos)?;
                self.manifest_file_write_root(block_data)?;
                old_pos
            } else {
                self.set_pack_file_pos_write(old_pos)?;
                self.pack_file_write_root(block_data)?;
                old_pos
            }
        )
    }

    //垃圾回收提交 TODO:未使用方法
    fn file_gc_add(&mut self, gc_pos_list: Vec<(u64, u64)>) {
        for pos in gc_pos_list {
            //直接添加
            self.run_data.gc_data_pos_list.list.push(pos);
        }
    }
    //清单文件垃圾回收提交 TODO:未使用方法
    fn manifest_file_gc_add(&mut self, gc_pos_list: Vec<(u64, u64)>) {
        for pos in gc_pos_list {
            //直接添加
            self.manifest.run_data.gc_data_pos_list.list.push(pos);
        }
    }
    //垃圾回收
    fn file_gc(&mut self) {
        Self::from_gc(&mut self.run_data.gc_data_pos_list, &mut self.manifest.empty_data_list);
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
        //如果有下一个则循环
        while let Some(v) = pos_list.get(index + 1) {
            let (next_pos, next_len) = *v;
            //当前索引内容
            if let Some((this_pos, this_len)) = pos_list.get_mut(index) {
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
    }

    //获取可用的文件位置
    fn get_file_pos(&mut self, length: u64) -> (u64, u64) {
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
    fn get_manifest_file_pos(&mut self, length: u64) -> io::Result<(u64, u64)> {
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
                return Ok(
                    if length == *len {
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
                    }
                );
            }
            //扩容处理
            Ok((self.manifest.file_len, length))
        } else {
            Err(Error::other("清单文件空数据列表不存在"))
        }
    }

    //保存所有数据
    fn save_and_up_all(&mut self) -> io::Result<()> {
        self.save_pack_length()?;
        //self.save_manifest()?;
        Ok(())
    }

    //保存数据长度
    fn save_pack_length(&mut self) -> io::Result<()> {
        //上锁
        self.write_lock()?;
        //修改包文件位置
        self.set_pack_file_pos_write(FILE_HEADER_DATA_LENGTH_POS)?;
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

    //保存属性
    fn save_manifest_attribute(&mut self) -> io::Result<()> {
        //属性
        let attribute = &mut self.manifest.attribute;
        //转换数据
        let data = attribute.to_bytes_vec();
        //写入数据
        //设置文件指针位置,从文件头后面写
        self.set_pack_file_pos_write(FILE_HEADER_DATA_LENGTH)?;
        //写入数据
        self.pack_file_write_root(&data)?;
        Ok(())
    }

    //保存空数据位置列表
    fn save_empty_data_pos_list(&mut self) -> io::Result<()> {
        todo!()
        /* //保存前GC处理
        self.file_gc();
        //当前文件位置
        let this_pos = self.manifest.attribute.empty_data_pos_list_pos;
        //数据大小
        let this_len = self.manifest.empty_data_list.data_len;
        //判断是否分离
        if self.s_manifest_file {
            //转换数据
            let data = self.manifest.empty_data_list.to_bytes_vec();
            //获取新空间
            let (new_pos, new_len) = self.get_manifest_file_pos(data.len() as u64)?;
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
            let data = self.manifest.empty_data_list.to_bytes_vec2(Some((this_pos, this_len)));
            //获取新空间
            let (new_pos, new_len) = self.get_file_pos(data.len() as u64);
            assert_eq!(data.len() as u64, new_len);
            //设置文件位置
            self.set_pack_file_pos_write(new_pos)?;
            //写入
            self.pack_file_write_root(&data)?;
            //GC提交
            self.file_gc_add(vec![(this_pos, this_pos)]);
            //更改指针
            self.manifest.attribute.empty_data_pos_list_pos = new_pos;
            //GC处理
            self.file_gc();
            Ok(())
        } */
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
        if !self.run_data.write_lock {
            let lock_file = write_lock(true, &self.run_data.write_lock_path)?;
            if let Some(lock_file) = lock_file {
                self.run_data.write_lock_file = Some(lock_file);
            }
            self.run_data.write_lock = true;
            self.pack_file.lock()?;
        }
        Ok(())
    }

    //解除写入锁
    fn write_unlock(&mut self) -> io::Result<()> {
        //锁文件路径
        let path = &self.run_data.write_lock_path;
        //获取锁信息
        let lock_info = self.write_lock_info();
        match lock_info.file_lock_type {
            PackLockType::File => {
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
            PackLockType::Dir =>
                Err(Error::new(ErrorKind::IsADirectory, "无法解锁，锁文件类型是目录")),
            PackLockType::Symlink => Err(Error::other("无法解锁，锁文件类型是符号链接")),
            PackLockType::None => Ok(()),
        }
    }

    //附加设置文件长度
    fn add_set_pack_len(&mut self, length: u64) -> io::Result<()> {
        self.set_pack_file_len(self.pack_file_length + length)
    }

    //更新文件大小
    fn up_pack_length(&mut self) {
        //判断是否需要设置
        if self.run_data.pack_file_pos > self.pack_file_length {
            self.pack_file_length = self.run_data.pack_file_pos;
        }
    }

    //更新清单文件大小
    fn up_manifest_length(&mut self) {
        //判断是否需要设置
        if self.manifest.run_data.file_pos > self.manifest.file_len {
            self.manifest.file_len = self.manifest.run_data.file_pos;
        }
    }

    //设置包文件文件地址
    fn set_pack_file_pos_read(&self, pos: u64) -> io::Result<()> {
        if self.run_data.pack_file_pos != pos {
            let mut file = &self.pack_file;
            file.seek(SeekFrom::Start(pos))?;
        }
        Ok(())
    }

    fn set_pack_file_pos_write(&mut self, pos: u64) -> io::Result<()> {
        self.set_pack_file_pos_read(pos)?;
        self.run_data.pack_file_pos = pos;
        Ok(())
    }

    //包文件写入
    fn pack_file_write_root(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_lock()?;
        let file = &mut self.pack_file;
        file.write_all(data)?;
        let len = data.len() as u64;
        self.run_data.pack_file_pos += len;
        self.up_pack_length();
        Ok(())
    }

    //设置包文件大小
    fn set_pack_file_len(&mut self, len: u64) -> io::Result<()> {
        self.pack_file.set_len(len)?;
        self.pack_file_length = len;
        self.up_pack_length();
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
            self.manifest.run_data.file_pos += len;
            self.up_manifest_length();
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
impl Drop for WBFPManager {
    fn drop(&mut self) {
        //确保文件完全写入
        _ = self.pack_file.sync_all();
        //释放缓存文件===
        //强制写入索引数据
        _ = self.save_and_up_all();
        //TODO:需要完成
        /* //如果索引文件分离
        if self.s_data_file {
            //写入两个索引文件，使其同步
            _ = self.save_json_data();
            //将B文件释放并删除
            let mut json_b_path = self.run_data.pack_path.clone();
            json_b_path.push_str(".json.b");
            //改动：移除自动删除代码，并改为用户提示
            info!(r#"包文件已安全保存，"{json_b_path}"是原子同步文件，用来确保安全，你可以删除。"#)
        } */
        //释放写入锁
        _ = self.write_unlock();
    }
}

struct MutDirAddReturn {
    length: u64,
    file_count: u64,
    dir_count: u64,
}

enum PackLockType {
    File,
    Dir,
    Symlink,
    None,
}

struct PackLockInfo {
    //运行时_锁状态
    run_lock: bool,
    //文件_锁类型
    file_lock_type: PackLockType,
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
    let mut file_lock_pid = None;
    let mut file_lock_pid_run = None;
    if path.try_exists().is_ok() {
        is_dir = path.is_dir();
        if path.is_file() {
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
                file_lock_pid_run = Some(is_process_running(pid));
            }
        }
    } else {
        is_dir = false;
    }
    let file_lock_type = if is_symlink {
        PackLockType::Symlink
    } else if is_dir {
        PackLockType::Dir
    } else {
        PackLockType::File
    };
    PackLockInfo {
        run_lock,
        file_lock_type,
        file_lock_pid,
        file_lock_pid_run,
    }
}

//设置写入锁
fn write_lock(run_lock: bool, write_lock_path: &PathBuf) -> Result<Option<File>, Error> {
    let lock_info = write_lock_info(run_lock, write_lock_path);
    if lock_info.run_lock {
        //若锁文件不存在就写入
        if let PackLockType::None = lock_info.file_lock_type {
            Ok(Some(write_lock_file(write_lock_path)?))
        } else {
            Ok(None)
        }
    } else {
        //判断锁文件
        match lock_info.file_lock_pid_run {
            Some(true) => panic!("无法为包文件上写入锁，正在被其他进程持有。"),
            Some(false) =>
                panic!(
                    r"包文件未正常解锁，但相关进程(pid:{})可能已停止。
                    如果你认为可以继续，可以删除锁文件：{:?} 强制解锁",
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
/* pub fn open_file<P: AsRef<Path>>(pack_path: &P) -> io::Result<WBFPManager> {
    fn load_manifest_attribute(data: &[u8]) -> io::Result<Attribute> {
        let attribute = Attribute::load(data)?;
        //兼容性判断
        //文件版本
        if attribute.version != MANIFEST_VERSION {
            warn!("Json数据版本不一致");
            //兼容性判断
            if attribute.version < MANIFEST_VERSION_COMPATIBLE {
                //版本低于解析器最低兼容版本
                error!("检查发现Json版本过低，低于实例最低兼容版本，拒绝创建包文件实例");
                return Err(Error::other("Json版本过低"));
            } else if attribute.version_compatible > MANIFEST_VERSION {
                //版本过高
                error!("检查发现Json版本过高，实例版本低于文件指定的兼容版本，拒绝创建包文件实例");
                return Err(Error::other("Json版本过高"));
            }
        }
        Ok(attribute)
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
    //读取布尔数据位
    let bool_data = header[FILE_HEADER_BOOL_DATA_LENGTH];
    //写时复制 TODO:未使用变量
    let _cow = bool_data >> 7 == 1;
    let s_data_file = bool_data >> 6 == 1;
    let pack_len = u64::from_le_bytes(
        header[FILE_HEADER_DATA_LENGTH_POS as usize
            ..(FILE_HEADER_DATA_LENGTH_POS + FILE_HEADER_DATA_LENGTH_LENGTH) as usize]
            .try_into()
            .unwrap(),
    );
    //如果分离数据文件
    if s_data_file {
        //获取属性
        let mut attribute_data = [0u8; MANIFEST_ATTRIBUTE_LEN];
        if pack_file.read(&mut attribute_data)? < MANIFEST_ATTRIBUTE_LEN {
            return Err(Error::other("无法完整的读取清单属性，文件可能破损"));
        }
        let attribute = load_manifest_attribute(&attribute_data)?;
        //空数据列表===
        let mut data = Vec::new();
        //文件指针位置
        let empty_pos = attribute.empty_data_pos_list_pos;
        //获取数量数据以确定大小
        let mut pos_count_data = [0u8; DATA_POS_LIST_COUNT_LEN];
        //设置文件位置
        pack_file.seek(SeekFrom::Start(empty_pos))?;
        //读取
        if pack_file.read(&mut pos_count_data)? < DATA_POS_LIST_COUNT_LEN {
            error!("无法读取空数据列表数据，将会导致游离数据")
        } else {
            let pos_count = u64::from_le_bytes(pos_count_data.try_into().unwrap());
            //复制数量数据到用于解析的动态数组
            for pos_count_datum in pos_count_data {
                data.push(pos_count_datum)
            }
            //列表项数据大小
            let empty_pos_items_len = pos_count as usize * DATA_POS_LIST_ITEM_LEN;
            //列表项数据动态数组
            let mut empty_pos_items_data = Vec::with_capacity(empty_pos_items_len);
            //读取
            pack_file.take(empty_pos_items_len as u64).read_to_end(&mut empty_pos_items_data)?;
            //合并数据
            for empty_pos_items_datum in empty_pos_items_data {
                data.push(empty_pos_items_datum)
            }
            //读取数据2
        }
        todo!()
    } else {
        todo!()
    }
} */

//创建===

//创建新包文件
pub fn create_new_file<P: AsRef<Path>>(pack_path: &P) -> io::Result<WBFPManager> {
    create_new_file2(pack_path, DEFAULT_COW, DEFAULT_S_DATA_FILE)
}

//创建新包文件
pub fn create_new_file2<P: AsRef<Path>>(
    pack_path: &P,
    cow: bool,
    s_data_file: bool
) -> Result<WBFPManager, Error> {
    //判断文件是否存在
    let mut path = (match pack_path.as_ref().try_exists() {
        Ok(true) => Err(Error::other("文件可能已存在，无法创建！")),
        Ok(false) | Err(_) => create_file2(pack_path, cow, s_data_file, true),
    })?;
    path.init_new_pack();
    Ok(path)
}

//创建新包文件,
pub fn create_file2<P: AsRef<Path>>(
    pack_path: &P,
    cow: bool,
    s_manifest_file: bool,
    create_new: bool
) -> Result<WBFPManager, Error> {
    let mut write_lock_path = pack_path
        .as_ref()
        .to_str()
        .expect("无法将路径转换成文本")
        .to_string();
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
        let mut manifest_path = String::from(
            pack_path.as_ref().to_str().expect("无法将路径转换成文件")
        );
        manifest_path.push_str(".wbm");
        Some(File::create(&manifest_path)?)
    } else {
        None
    };

    Ok(create2(pack_path, cow, pack_file, s_manifest_file, manifest_file, write_lock_file))
}

//创建新包实例===

//TODO:未使用函数
fn _create<P: AsRef<Path>>(pack_path: &P, pack_file: File) -> WBFPManager {
    create2(pack_path, DEFAULT_COW, pack_file, DEFAULT_S_DATA_FILE, None, None)
}

fn create2<P: AsRef<Path>>(
    pack_path: &P,
    cow: bool,
    pack_file: File,
    s_manifest_file: bool,
    manifest_file: Option<File>,
    write_lock_file: Option<File>
) -> WBFPManager {
    WBFPManager::new(
        pack_path,
        WBFilesPackManifest {
            attribute: Attribute {
                cow,
                ..Attribute::default()
            },
            root_struct: PackStruct::default(),
            empty_data_list: DataPosList {
                data_block: Some(ManifestDataBlock::default()),
                list: Vec::new(),
            },
            this_empty_data_list: if s_manifest_file {
                Some(DataPosList::default())
            } else {
                None
            },
            file: manifest_file,
            file_len: 0,
            run_data: WBFilesPackManifestRun {
                file_pos: 0,
                gc_data_pos_list: DataPosList::default(),
            },
        },
        pack_file,
        s_manifest_file,
        write_lock_file
    )
}
