use crate::wb_files_pack::manager::file::PackFileWR;
use core::slice::Iter;
/*
开始时间：26/2/11 15：51
 */
use crate::wb_files_pack::{
    Attribute, DataPosList, ManifestDataBlock, PackFileMetadata, PackFileMetadataFile,
    PackFileMetadataRun, PackFileMetadataType, PackStruct, PackStructItem, PackStructItemType,
    WBFilesPackManifest, WBFilesPackManifestRun, DATA_BLOCK_LEN, MANIFEST_ATTRIBUTE_LEN,
};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::vec::IntoIter;
use std::{fs, io};
use tracing::debug;

pub mod file;

#[cfg(test)]
mod test;

//文件头===
//文件头-文件名:WPFilesPack
pub const FILE_HEADER_TYPE_NAME: [u8; 11] = [
    0x57u8, 0x42, 0x46, 0x69, 0x6c, 0x65, 0x73, 0x50, 0x61, 0x63, 0x6b,
];
//文件头版本
const FILE_HEADER_VERSION_INDEX: usize = FILE_HEADER_TYPE_NAME.len();
const FILE_HEADER_VERSION: [u8; 2] = [0, 2];
//文件头标签位长度
const FILE_HEADER_BOOL_DATA_INDEX: usize = FILE_HEADER_VERSION_INDEX + FILE_HEADER_VERSION.len();
const FILE_HEADER_BOOL_DATA_LENGTH: usize = 1;

//文件头数据长度位置
const FILE_HEADER_DATA_LENGTH_INDEX: u64 =
    (FILE_HEADER_BOOL_DATA_INDEX + FILE_HEADER_BOOL_DATA_LENGTH) as u64;

//文件头数据长度长度
const FILE_HEADER_DATA_LENGTH_LENGTH: u64 = 8;

//文件头清单属性
const FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX: u64 =
    FILE_HEADER_DATA_LENGTH_INDEX + FILE_HEADER_DATA_LENGTH_LENGTH;

//文件头长度
const FILE_HEADER_DATA_LENGTH: u64 = ((FILE_HEADER_TYPE_NAME.len()
    + FILE_HEADER_VERSION.len()
    + FILE_HEADER_BOOL_DATA_LENGTH
    + MANIFEST_ATTRIBUTE_LEN) as u64)
    + FILE_HEADER_DATA_LENGTH_LENGTH;

//文件头块长度
const FILE_HEADER_BLOCK_LEN: usize = DATA_BLOCK_LEN;
//默认写实复制
pub const DEFAULT_COW: bool = false;

//默认分离数据为单独文件
pub const DEFAULT_S_DATA_FILE: bool = true;

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
    fn new(write_lock_path: PathBuf, write_lock_file: Option<File>) -> Self {
        Self {
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
    ) -> Self {
        Self::new2(
            pack_path,
            manifest,
            pack_file,
            s_manifest_file,
            write_lock_file,
            0,
        )
    }

    fn new2<P: AsRef<Path>>(
        pack_path: P,
        manifest: WBFilesPackManifest,
        pack_file: File,
        s_manifest_file: bool,
        write_lock_file: Option<File>,
        pack_file_length: u64,
    ) -> Self {
        let cow = manifest.attribute().cow();
        let mut write_lock_path =
            String::from(pack_path.as_ref().to_str().expect("无法将转换路径成文本"));
        write_lock_path.push_str(".lock");
        let write_lock_path = Path::new(&write_lock_path).to_path_buf();
        Self {
            manifest,
            pack_file,
            cow,
            s_manifest_file,
            pack_file_length,
            run_data: WBFPManagerRun::new(write_lock_path, write_lock_file),
        }
    }

    //初始化新包
    fn init_new_pack(&mut self) {
        //文件头缓存
        let mut file_header_buf = Vec::with_capacity(FILE_HEADER_BLOCK_LEN);
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

        //文件大小
        for to_le_byte in (FILE_HEADER_BLOCK_LEN as u64).to_le_bytes() {
            file_header_buf.push(to_le_byte);
        }
        //清单属性
        for byte in self.manifest.attribute.to_bytes_vec() {
            file_header_buf.push(byte);
        }
        //填充所有剩余空间
        file_header_buf.resize(FILE_HEADER_BLOCK_LEN, 0);

        self.pack_file_write(&file_header_buf)
            .expect("无法写入文件头");

        //设置文件大小
        self.set_pack_file_len(FILE_HEADER_BLOCK_LEN as u64)
            .expect("无法设置文件大小");

        //初始化并保存空数据列表
        self.init_save_empty_data_list()
            .expect("初始化和保存空数据列表失败");

        if self.s_manifest_file {
            self.init_save_manifest_empty_data_list()
                .expect("初始化和保存空数据列表失败");
        }
        //初始化并保存根结构
        self.init_save_root_pack_struct()
            .expect("初始化和保存根结构失败");
    }

    fn init_save_root_pack_struct(&mut self) -> io::Result<()> {
        let block_data = self.manifest.root_struct.get_block_data().0;
        if self.s_manifest_file {
            let (pos, _) = self.get_manifest_file_pos(DATA_BLOCK_LEN as u64)?;
            self.set_manifest_file_pos(pos)?;
            self.manifest_file_write(&block_data)?;
            self.manifest.attribute.root_struct_pos = pos;
        } else {
            let (pos, _) = self.get_file_pos(DATA_BLOCK_LEN as u64);
            self.set_pack_file_pos_write(pos)?;
            self.pack_file_write(&block_data)?;
            self.manifest.attribute.root_struct_pos = pos;
        }
        Ok(())
    }

    fn init_save_empty_data_list(&mut self) -> io::Result<()> {
        let block_data = self.manifest.empty_data_list.get_block_data().unwrap().0;
        if self.s_manifest_file {
            let (pos, _) = self.get_manifest_file_pos(DATA_BLOCK_LEN as u64)?;
            self.set_manifest_file_pos(pos)?;
            self.manifest_file_write(&block_data)?;
            self.manifest.attribute.empty_data_pos_list_pos = pos;
        } else {
            let (pos, _) = self.get_file_pos(DATA_BLOCK_LEN as u64);
            self.set_pack_file_pos_write(pos)?;
            self.pack_file_write(&block_data)?;
            self.manifest.attribute.empty_data_pos_list_pos = pos;
        }
        Ok(())
    }

    fn init_save_manifest_empty_data_list(&mut self) -> io::Result<()> {
        if let Some(data_list) = &mut self.manifest.this_empty_data_list {
            let block_data = data_list
                .get_block_data()
                .ok_or(Error::other("数据块实例不存在"))?
                .0;
            if self.s_manifest_file {
                let (pos, _) = self.get_manifest_file_pos(DATA_BLOCK_LEN as u64)?;
                self.set_manifest_file_pos(pos)?;
                self.manifest_file_write(&block_data)?;
                self.manifest.attribute.manifest_empty_data_pos_list_pos = pos;
            } else {
                let (pos, _) = self.get_file_pos(DATA_BLOCK_LEN as u64);
                self.set_pack_file_pos_write(pos)?;
                self.pack_file_write(&block_data)?;
                self.manifest.attribute.manifest_empty_data_pos_list_pos = pos;
            }
        }
        Ok(())
    }

    //读取===

    //获取属性数据
    pub fn get_manifest_attribute(&self) -> &Attribute {
        self.manifest.attribute()
    }

    //获取根结构列表
    pub fn get_root_struct_items(&self) -> &HashMap<String, PackStructItem> {
        &self.manifest.root_struct.items
    }

    //获取根结构的名称列表
    pub fn get_root_struct_item_name_list(&self) -> Vec<String> {
        let mut name_list = Vec::with_capacity(self.manifest.root_struct.items.len());
        for name in self.manifest.root_struct.items.keys() {
            name_list.push(name.clone());
        }
        name_list
    }

    //获取结构项文件文件名列表
    pub fn get_struct_item_name_list<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> io::Result<Vec<String>> {
        let item = self.get_pack_struct_item(path)?;
        if let PackStructItemType::Dir(dir) = &item.item_type {
            if let Some(pack_struct) = &dir.pack_struct {
                let mut name_list = Vec::with_capacity(pack_struct.items.len());
                for name in pack_struct.items.keys() {
                    name_list.push(name.clone());
                }
                Ok(name_list)
            } else {
                Err(Error::other("结构实例没有被加载"))
            }
        } else {
            Err(Error::new(ErrorKind::NotADirectory, "提供的路径不是目录"))
        }
    }

    //包文件读取
    fn pack_file_read(&self, data: &mut [u8]) -> io::Result<()> {
        let mut file = &self.pack_file;
        file.read_exact(data)?;
        Ok(())
    }

    //获取目录结构项列表
    pub fn get_dir_pack_struct_items<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> io::Result<&HashMap<String, PackStructItem>> {
        //
        self.load_pack_struct_metadata_path(&path)?;
        let path_pack_struct_item = self.get_pack_struct_item_dir(path)?;
        if let PackStructItemType::Dir(dir) = &path_pack_struct_item.item_type {
            if let Some(pack_struct) = &dir.pack_struct {
                Ok(&pack_struct.items)
            } else {
                Err(Error::other("实例没有加载"))
            }
        } else {
            Err(Error::new(ErrorKind::NotADirectory, "提供的路径不是目录"))
        }
    }

    //获取目录结构项
    pub fn get_pack_struct_item_dir<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> io::Result<&PackStructItem> {
        let path_list = Self::path_to_string_vec(path);
        self.load_pack_struct_metadata_path2(&path_list, true)?;
        self.get_pack_struct_item2(&path_list)
    }

    //获取结构项
    pub fn get_pack_struct_item<P: AsRef<Path>>(&mut self, path: P) -> io::Result<&PackStructItem> {
        let path_list = Self::path_to_string_vec(path);
        self.load_pack_struct_metadata_path2(&path_list, false)?;
        self.get_pack_struct_item2(&path_list)
    }

    fn get_pack_struct_item2(&self, path_list: &[String]) -> io::Result<&PackStructItem> {
        if path_list.len() > 1 {
            let mut name_list = path_list.iter();
            let mut this_name = name_list.next();
            let mut this_pack_struct = &self.manifest.root_struct;
            while let Some(name) = this_name {
                if let Some(item) = this_pack_struct.items.get(name) {
                    if let Some(next_name) = name_list.next() {
                        if let PackStructItemType::Dir(dir) = &item.item_type {
                            if let Some(pack_struct) = &dir.pack_struct {
                                this_pack_struct = pack_struct;
                                this_name = Some(next_name);
                            } else {
                                Err(Error::other("实例没有加载"))?;
                            }
                        } else {
                            Err(Error::new(ErrorKind::NotADirectory, "路径存在非目录"))?;
                        }
                    } else {
                        return Ok(item);
                    }
                } else {
                    Err(Error::new(ErrorKind::NotFound, "结构项不存在"))?;
                }
            }
            Err(Error::other("未知错误"))
        } else if let Some(v) = self.manifest.root_struct.items.get(&path_list[0]) {
            Ok(v)
        } else {
            Err(Error::new(ErrorKind::NotFound, "结构项不存在"))
        }
    }

    pub fn load_all_data(&mut self, no_err: bool) -> io::Result<()> {
        fn m_load_all_data(
            wbfp_manager: &mut WBFPManager,
            pack_struct_items: &mut HashMap<String, PackStructItem>,
            no_err: bool,
        ) -> io::Result<()> {
            for item in pack_struct_items.values_mut() {
                if let PackStructItemType::Dir(dir) = &mut item.item_type
                    && dir.pack_struct.is_none()
                {
                    let mut pack_struct = wbfp_manager.load_pack_struct(dir.struct_file_pos)?;
                    if let Err(err) = m_load_all_data(wbfp_manager, &mut pack_struct.items, no_err)
                        && !no_err
                    {
                        Err(err)?;
                    }
                    dir.pack_struct = Some(pack_struct);
                }
                //加载元数据
                if let PackFileMetadataRun::NoLoad = item.metadata {
                    item.metadata = PackFileMetadataRun::Loaded(
                        wbfp_manager.load_pack_file_metadata(item.metadata_file_pos)?,
                    );
                }
            }
            Ok(())
        }
        let mut this_root_items = HashMap::with_capacity(self.manifest.root_struct.items.len());
        let mut root_item_string = Vec::with_capacity(self.manifest.root_struct.items.len());
        //获取所有键
        for name in self.manifest.root_struct.items.keys() {
            root_item_string.push(name.clone());
        }
        //暂移所有
        for name in &root_item_string {
            this_root_items.insert(
                name.clone(),
                self.manifest.root_struct.items.remove(name).unwrap(),
            );
        }
        //处理
        if let Err(err) = m_load_all_data(self, &mut this_root_items, no_err) {
            //是否忽略错误
            if !no_err {
                Err(err)?;
            }
        }
        //返还
        for name in &root_item_string {
            self.manifest
                .root_struct
                .items
                .insert(name.clone(), this_root_items.remove(name).unwrap());
        }
        Ok(())
    }

    pub fn load_pack_struct_metadata_path<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        self.load_pack_struct_metadata_path2(&Self::path_to_string_vec(path), false)
    }
    fn load_pack_struct_metadata_path2(
        &mut self,
        path_list: &[String],
        is_dir: bool,
    ) -> io::Result<()> {
        let mut path_list = path_list.iter();
        let two_name = path_list.next().unwrap();
        //暂移
        let two_pack_struct =
            if let Some(mut struct_item) = self.manifest.root_struct.items.remove(two_name) {
                match &mut struct_item.item_type {
                    PackStructItemType::Dir(dir) => {
                        if dir.pack_struct.is_none() {
                            dir.pack_struct = Some(self.load_pack_struct(dir.struct_file_pos)?);
                        }
                        if let Some(pack_struct) = &mut dir.pack_struct {
                            //递归
                            if self.s_load_pack_struct_metadata_path(
                                pack_struct,
                                &mut path_list,
                                is_dir,
                            )? && let PackFileMetadataRun::NoLoad = struct_item.metadata
                            {
                                struct_item.metadata = PackFileMetadataRun::Loaded(
                                    self.load_pack_file_metadata(struct_item.metadata_file_pos)?,
                                );
                            }
                            struct_item
                        } else {
                            Err(Error::other("结构实例没有被加载")).unwrap()
                        }
                    }
                    PackStructItemType::File => {
                        //如果只有二级，且不限定目录
                        if is_dir || path_list.next().is_some() {
                            Err(Error::new(ErrorKind::NotADirectory, "路径存在非目录")).unwrap()
                        } else {
                            if let PackFileMetadataRun::NoLoad = struct_item.metadata {
                                struct_item.metadata = PackFileMetadataRun::Loaded(
                                    self.load_pack_file_metadata(struct_item.metadata_file_pos)?,
                                );
                            }
                            struct_item
                        }
                    }
                }
            } else {
                Err(Error::new(ErrorKind::NotFound, "结构项不存在")).unwrap()
            };

        //返还
        self.manifest
            .root_struct
            .items
            .insert(two_pack_struct.name.clone(), two_pack_struct);
        Ok(())
    }

    fn s_load_pack_struct_metadata_path(
        &mut self,
        s_pack_struct: &mut PackStruct,
        path_list: &mut Iter<String>,
        is_dir: bool,
    ) -> io::Result<bool> {
        if let Some(this_name) = path_list.next() {
            if let Some(item) = s_pack_struct.items.get_mut(this_name) {
                match &mut item.item_type {
                    PackStructItemType::Dir(dir) => {
                        if dir.pack_struct.is_none() {
                            dir.pack_struct = Some(self.load_pack_struct(dir.struct_file_pos)?);
                        }
                        if let Some(pack_struct) = &mut dir.pack_struct {
                            if self.s_load_pack_struct_metadata_path(
                                pack_struct,
                                path_list,
                                is_dir,
                            )? {
                                //加载元数据
                                if let PackFileMetadataRun::NoLoad = item.metadata {
                                    item.metadata = PackFileMetadataRun::Loaded(
                                        self.load_pack_file_metadata(item.metadata_file_pos)?,
                                    );
                                }
                                Ok(false)
                            } else {
                                Ok(true)
                            }
                        } else {
                            Err(Error::other("结构实例没有被加载")).unwrap()
                        }
                    }
                    PackStructItemType::File => {
                        //判断是否只有目录且是否存在下一个路径
                        if is_dir || path_list.next().is_some() {
                            Err(Error::new(ErrorKind::NotADirectory, "目录存在非目录")).unwrap()
                        } else {
                            //最后一个就加载元数据
                            if let PackFileMetadataRun::NoLoad = item.metadata {
                                item.metadata = PackFileMetadataRun::Loaded(
                                    self.load_pack_file_metadata(item.metadata_file_pos)?,
                                );
                            }
                            Ok(true)
                        }
                    }
                }
            } else {
                Err(Error::new(ErrorKind::NotFound, "结构项不存在")).unwrap()
            }
        } else {
            Ok(true)
        }
    }

    pub fn get_dir<P: AsRef<Path>>(&mut self, path: P) -> io::Result<&PackStruct> {
        let path_list = Self::path_to_string_vec(path);
        self.load_pack_struct_metadata_path2(&path_list, true)?;
        let mut pack_struct = &self.manifest.root_struct;
        let mut path_list_iter = path_list.iter();
        let mut name = path_list_iter.next();
        while let Some(this_name) = name {
            if pack_struct.items.contains_key(this_name) {
                let this_pack_struct = if let PackStructItemType::Dir(dir) =
                    &pack_struct.items.get(this_name).unwrap().item_type
                {
                    if let Some(pack_struct) = &dir.pack_struct {
                        pack_struct
                    } else {
                        panic!("结构没有被加载")
                    }
                } else {
                    return Err(Error::new(ErrorKind::NotADirectory, "路径存在非目录")).unwrap();
                };
                let next_name = path_list_iter.next();
                if next_name.is_some() {
                    name = next_name;
                    pack_struct = this_pack_struct;
                } else {
                    return Ok(this_pack_struct);
                }
            } else {
                return Err(Error::new(ErrorKind::DirectoryNotEmpty, "目录不存在")).unwrap();
            }
        }
        panic!("未知错误")
    }

    //写入===
    //元数据解锁和返还
    fn file_metadata_unlock(
        &mut self,
        path_list: Vec<String>,
        mut metadata: PackFileMetadata,
    ) -> io::Result<()> {
        fn s_unlock(
            wbfp_man: &mut WBFPManager,
            mut path_list: IntoIter<String>,
            s_path: PathBuf,
            pack_struct_items: &mut HashMap<String, PackStructItem>,
            mut metadata: PackFileMetadata,
        ) -> io::Result<()> {
            if let Some(name) = path_list.next() {
                let this_path = s_path.join(&name);
                if let Some(item) = pack_struct_items.get_mut(&name) {
                    match &mut item.item_type {
                        PackStructItemType::Dir(dir) => {
                            if dir.pack_struct.is_none() {
                                dir.pack_struct =
                                    Some(wbfp_man.load_pack_struct(dir.struct_file_pos)?);
                            }
                            if let Some(pack_struct) = &mut dir.pack_struct {
                                s_unlock(
                                    wbfp_man,
                                    path_list,
                                    PathBuf::from(name),
                                    &mut pack_struct.items,
                                    metadata,
                                )
                            } else {
                                panic!("逻辑错误")
                            }
                        }
                        PackStructItemType::File => {
                            if path_list.next().is_none() {
                                //更新元数据
                                let (new_pos, pos) = wbfp_man.save_metadata_write(&mut metadata)?;
                                if new_pos {
                                    item.metadata_file_pos = pos;
                                }
                                item.metadata.unlock();
                                Ok(())
                            } else {
                                Err(Error::new(
                                    ErrorKind::NotADirectory,
                                    format!("虚拟路径{this_path:?}是文件不是目录"),
                                ))
                            }
                        }
                    }
                } else {
                    Err(Error::new(ErrorKind::NotFound, "文件或目录不存在"))
                }
            } else {
                panic!("逻辑错误")
            }
        }
        let mut path_list = path_list.into_iter();
        let two_name = path_list.next().ok_or(Error::other("路径为空"))?;
        //暂移二级
        if let Some(mut struct_item) = self.manifest.root_struct.items.remove(&two_name) {
            match &mut struct_item.item_type {
                PackStructItemType::Dir(dir) => {
                    if dir.pack_struct.is_none() {
                        dir.pack_struct = Some(self.load_pack_struct(dir.struct_file_pos)?);
                    }
                    if let Some(pack_struct) = &mut dir.pack_struct {
                        s_unlock(
                            self,
                            path_list,
                            PathBuf::from(&two_name),
                            &mut pack_struct.items,
                            metadata,
                        )?;
                        self.manifest
                            .root_struct
                            .items
                            .insert(two_name, struct_item);
                        Ok(())
                    } else {
                        panic!("逻辑错误")
                    }
                }
                PackStructItemType::File => {
                    if path_list.next().is_none() {
                        //保存元数据
                        let (new_pos, pos) = self.save_metadata_write(&mut metadata)?;
                        if new_pos {
                            struct_item.metadata_file_pos = pos;
                        }
                        struct_item.metadata.unlock();
                        self.manifest
                            .root_struct
                            .items
                            .insert(two_name, struct_item);
                        Ok(())
                    } else {
                        Err(Error::new(
                            ErrorKind::NotADirectory,
                            format!(r#"虚拟路径"{two_name}"是文件不是目录"#),
                        ))
                    }
                }
            }
        } else {
            Err(Error::new(ErrorKind::NotFound, "目录或文件不存在")).unwrap()
        }
    }

    fn file_metadata_lock(&mut self, path_list: &Vec<String>) -> io::Result<PackFileMetadata> {
        fn s_load(
            wbfp_man: &mut WBFPManager,
            path_list: &mut Iter<String>,
            s_path: PathBuf,
            pack_struct_items: &mut HashMap<String, PackStructItem>,
        ) -> io::Result<PackFileMetadata> {
            if let Some(name) = path_list.next() {
                let this_path = s_path.join(name);
                if let Some(item) = pack_struct_items.get_mut(name) {
                    match &mut item.item_type {
                        PackStructItemType::Dir(dir) => {
                            if dir.pack_struct.is_none() {
                                dir.pack_struct =
                                    Some(wbfp_man.load_pack_struct(dir.struct_file_pos)?);
                            }
                            if let Some(pack_struct) = &mut dir.pack_struct {
                                s_load(wbfp_man, path_list, this_path, &mut pack_struct.items)
                            } else {
                                panic!("逻辑错误")
                            }
                        }
                        PackStructItemType::File => {
                            if path_list.next().is_none() {
                                if let PackFileMetadataRun::NoLoad = item.metadata {
                                    item.metadata = PackFileMetadataRun::Loaded(
                                        wbfp_man.load_pack_file_metadata(item.metadata_file_pos)?,
                                    );
                                }
                                item.metadata.try_lock()
                            } else {
                                Err(Error::new(
                                    ErrorKind::NotADirectory,
                                    format!("虚拟路径{this_path:?}是文件不是目录"),
                                ))?
                            }
                        }
                    }
                } else {
                    Err(Error::new(
                        ErrorKind::NotFound,
                        format!(r#"虚拟路径"{this_path:?}"文件不存在"#),
                    ))?
                }
            } else {
                panic!("逻辑错误")
            }
        }
        let mut path_list = path_list.iter();
        let two_pack_struct_name = path_list.next().ok_or(Error::other("路径为空"))?;
        let file_metadata = if let Some(mut pack_struct_item) =
            self.manifest.root_struct.items.remove(two_pack_struct_name)
        {
            match &mut pack_struct_item.item_type {
                //
                PackStructItemType::Dir(dir) => {
                    if dir.pack_struct.is_none() {
                        dir.pack_struct = Some(self.load_pack_struct(dir.struct_file_pos)?);
                    }
                    if let Some(pack_struct) = &mut dir.pack_struct {
                        let r = s_load(
                            self,
                            &mut path_list,
                            PathBuf::from(two_pack_struct_name),
                            &mut pack_struct.items,
                        )?;
                        self.manifest
                            .root_struct
                            .items
                            .insert(two_pack_struct_name.clone(), pack_struct_item);
                        r
                    } else {
                        panic!("逻辑错误");
                    }
                }
                PackStructItemType::File => {
                    if path_list.next().is_none() {
                        let r = pack_struct_item.metadata.try_lock()?;
                        self.manifest
                            .root_struct
                            .items
                            .insert(two_pack_struct_name.clone(), pack_struct_item);
                        r
                    } else {
                        self.manifest
                            .root_struct
                            .items
                            .insert(two_pack_struct_name.clone(), pack_struct_item);
                        Err(Error::new(
                            ErrorKind::NotADirectory,
                            format!(r#"路径"{two_pack_struct_name}是文件不是目录""#),
                        ))
                            .unwrap()
                    }
                }
            }
        } else {
            Err(Error::new(ErrorKind::NotFound, "文件或目录不存在"))?
        };
        Ok(file_metadata)
    }

    pub fn get_file_rw<P: AsRef<Path>>(&mut self, path: P) -> io::Result<PackFileWR<'_>> {
        let path_list = Self::path_to_string_vec(path);
        let metadata = self.file_metadata_lock(&path_list)?;
        Ok(PackFileWR::new(self, path_list, metadata))
    }

    //创建文件
    pub fn create_file_new_wr<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
    ) -> io::Result<PackFileWR<'_>> {
        let metadata = self.create_file_new(&path, modified, len, true)?.unwrap();
        Ok(PackFileWR::new(
            self,
            Self::path_to_string_vec(path),
            metadata,
        ))
    }

    pub fn create_file_new<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
        metadata_lock: bool,
    ) -> io::Result<Option<PackFileMetadata>> {
        let cow = self.cow;
        self.create_file_new2(path, modified, len, cow, metadata_lock)
    }

    pub fn create_file_new2_wr<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
        cow: bool,
        metadata_lock: bool,
    ) -> io::Result<PackFileWR<'_>> {
        let metadata = self
            .create_file_new2(&path, modified, len, cow, true)?
            .unwrap();
        Ok(PackFileWR::new(
            self,
            Self::path_to_string_vec(path),
            metadata,
        ))
    }

    fn create_file_new2<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
        cow: bool,
        metadata_lock: bool,
    ) -> io::Result<Option<PackFileMetadata>> {
        let path_list = Self::path_to_string_vec(path);
        let data_pos_list = DataPosList {
            data_block: None,
            list: vec![self.get_file_pos(len)],
        };
        let mut pack_file_metadata = PackFileMetadata {
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
        //保存元数据
        let (_, metadata_file_pos) = self.save_metadata_write(&mut pack_file_metadata).unwrap();
        let mut pack_struct_item = PackStructItem {
            name: path_list[path_list.len() - 1].clone(),
            item_type: PackStructItemType::File,
            metadata_file_pos,
            metadata: PackFileMetadataRun::Loaded(pack_file_metadata),
        };
        //尝试锁定
        let r_metadata = if metadata_lock {
            Some(pack_struct_item.metadata.try_lock()?)
        } else {
            None
        };
        if path_list.len() > 1 {
            //创建目录和文件
            self.create_dir_all_and_file(
                &mut path_list[..path_list.len() - 1].iter(),
                Some(pack_struct_item),
                len,
            )?;
        } else {
            self.manifest
                .root_struct
                .items
                .insert(path_list[0].clone(), pack_struct_item);
            self.manifest.attribute.file_count += 1;
            self.manifest.attribute.data_len += len;
            self.save_root_pack_struct()?;
        }
        Ok(r_metadata)
    }

    //创建目录(结构)
    pub fn create_dir_all<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        self.create_dir_all_and_file(&mut Self::path_to_string_vec(path).iter(), None, 0)
    }

    fn s_create_dir_all_and_file(
        &mut self,
        s_pack_struct: &mut PackStruct,
        path_list: &mut Iter<String>,
        cow: bool,
        mut file_struct_item: Option<PackStructItem>,
        length: u64,
    ) -> io::Result<MutDirAddReturn> {
        if let Some(name) = path_list.next() {
            //判断目录是否存在
            if let Some(item) = s_pack_struct.items.get_mut(name) {
                if let PackStructItemType::Dir(dir) = &mut item.item_type {
                    if dir.pack_struct.is_none() {
                        //加载实例
                        dir.pack_struct = Some(self.load_pack_struct(dir.struct_file_pos)?);
                    }
                    if let Some(pack_struct) = &mut dir.pack_struct {
                        //递归
                        let r = self.s_create_dir_all_and_file(
                            pack_struct,
                            path_list,
                            cow,
                            file_struct_item,
                            length,
                        )?;
                        //更新元数据
                        if let PackFileMetadataRun::NoLoad = item.metadata {
                            //加载元数据
                            item.metadata = PackFileMetadataRun::Loaded(
                                self.load_pack_file_metadata(item.metadata_file_pos)?,
                            );
                        }
                        if r.dir_count != 0 || r.file_count != 0 {
                            let (new_block, pos) = self.save_pack_struct_write(pack_struct)?;
                            if new_block {
                                dir.struct_file_pos = pos;
                            }
                        }
                        match &mut item.metadata {
                            PackFileMetadataRun::Loaded(metadata) => {
                                if let PackFileMetadataType::Dir(dir) = &mut metadata.file_type {
                                    dir.dir_count += r.dir_count;
                                    dir.file_count += r.file_count;
                                    metadata.len += r.length;
                                    //保存元数据和结构
                                    if r.dir_count != 0 || r.file_count != 0 || r.length != 0 {
                                        let (new_block, pos) =
                                            self.save_metadata_write(metadata)?;
                                        if new_block {
                                            item.metadata_file_pos = pos;
                                        }
                                    }
                                    Ok(MutDirAddReturn {
                                        dir_count: r.dir_count,
                                        file_count: r.file_count,
                                        length: r.length,
                                    })
                                } else {
                                    panic!("存在逻辑错误")
                                }
                            }
                            PackFileMetadataRun::Locked => {
                                Err(Error::other("元数据被锁定")).unwrap()
                            }
                            PackFileMetadataRun::NoLoad => panic!("元数据没有被加载"),
                            PackFileMetadataRun::None => panic!("逻辑错误：目录的元数据为空"),
                        }
                    } else {
                        panic!("结构没有被加载")
                    }
                } else {
                    Err(Error::new(
                        ErrorKind::NotADirectory,
                        "提供的目录已存在非目录的文件",
                    ))
                        .unwrap()
                }
            } else {
                //创建
                let mut item = PackStructItem::new_empty_dir(
                    name,
                    PackFileMetadataRun::Loaded(PackFileMetadata::new_empty_dir(cow)),
                );
                //递归
                let r = if let PackStructItemType::Dir(dir) = &mut item.item_type
                    && let Some(pack_struct) = &mut dir.pack_struct
                {
                    let r = self.s_create_dir_all_and_file(
                        pack_struct,
                        path_list,
                        cow,
                        file_struct_item,
                        length,
                    )?;
                    if let PackFileMetadataRun::Loaded(metadata) = &mut item.metadata
                        && let PackFileMetadataType::Dir(dir) = &mut metadata.file_type
                    {
                        dir.dir_count += r.dir_count;
                        dir.file_count += r.file_count;
                        metadata.len += r.length;
                        //保存元数据
                        let (new_block, pos) = self.save_metadata_write(metadata)?;
                        assert!(new_block);
                        item.metadata_file_pos = pos;
                    } else {
                        panic!("逻辑错误");
                    }
                    //保存结构
                    let (new_block, pos) = self.save_pack_struct_write(pack_struct)?;
                    assert!(new_block);
                    dir.struct_file_pos = pos;
                    r
                } else {
                    panic!("逻辑错误")
                };
                s_pack_struct.items.insert(name.clone(), item);
                Ok(MutDirAddReturn {
                    length: r.length,
                    file_count: r.file_count,
                    dir_count: r.dir_count + 1,
                })
            }
        } else {
            if let Some(file_struct_item) = file_struct_item.take() {
                s_pack_struct
                    .items
                    .insert(file_struct_item.name.clone(), file_struct_item);
                Ok(MutDirAddReturn {
                    dir_count: 0,
                    file_count: 1,
                    length,
                })
            } else {
                Ok(MutDirAddReturn::new_empty())
            }
        }
    }

    fn create_dir_all_and_file(
        &mut self,
        path_list: &mut Iter<String>,
        file_struct_item: Option<PackStructItem>,
        length: u64,
    ) -> io::Result<()> {
        //移动或创建二级目录实例，通过二级递归，避免借用问题
        let root_struct = &mut self.manifest.root_struct;
        let two_name = path_list.next().unwrap();
        let (mut two_pack_struct_item, mut two_r) =
            if let Some(mut two_item) = root_struct.items.remove(two_name) {
                //存在则暂时删除（移动）
                //类型判断
                if let PackStructItemType::Dir(dir) = &mut two_item.item_type {
                    //实例判断并尝试加载
                    if dir.pack_struct.is_none() {
                        //加载结构
                        dir.pack_struct = Some(self.load_pack_struct(dir.struct_file_pos)?);
                    }
                    if dir.pack_struct.is_none() {
                        Err(Error::other("无法加载实例"))?;
                    }
                } else {
                    panic!("提供的目录存在非目录");
                    Err(Error::other("提供的目录存在非目录"))?;
                }
                (two_item, MutDirAddReturn::new_empty())
            } else {
                (
                    PackStructItem::new_empty_dir(
                        two_name,
                        PackFileMetadataRun::Loaded(PackFileMetadata::new_empty_dir(self.cow)),
                    ),
                    MutDirAddReturn {
                        length: 0,
                        file_count: 0,
                        dir_count: 1,
                    },
                )
            };
        //子目录处理
        if let PackStructItemType::Dir(dir) = &mut two_pack_struct_item.item_type {
            if let Some(two_pack_struct) = &mut dir.pack_struct {
                let r = self.s_create_dir_all_and_file(
                    two_pack_struct,
                    path_list,
                    self.cow,
                    file_struct_item,
                    length,
                )?;
                two_r.dir_count += r.dir_count;
                two_r.file_count += r.file_count;
                two_r.length += r.length;
                //
                let files_count = r.file_count + r.dir_count;
                let files_count_bool = files_count != 0;
                if files_count_bool {
                    let (new_block, pos) = self.save_pack_struct_write(two_pack_struct)?;
                    if new_block {
                        dir.struct_file_pos = pos;
                    }
                }
                if files_count_bool || r.length != 0 {
                    if let PackFileMetadataRun::Loaded(metadata) =
                        &mut two_pack_struct_item.metadata
                    {
                        metadata.len += r.length;
                        if let PackFileMetadataType::Dir(dir) = &mut metadata.file_type {
                            dir.file_count += r.file_count;
                            dir.dir_count += r.dir_count;
                        }
                        let (new_block, pos) = self.save_metadata_write(metadata)?;
                        if new_block {
                            two_pack_struct_item.metadata_file_pos = pos;
                        }
                    } else {
                        panic!("逻辑错误");
                    }
                }
            } else {
                panic!("没有结构实例");
            }
        } else {
            panic!("类型错误，不是目录");
        }
        self.manifest
            .root_struct
            .items
            .insert(two_name.clone(), two_pack_struct_item);
        self.manifest.attribute.dir_count += two_r.dir_count;
        self.manifest.attribute.file_count += two_r.file_count;
        self.manifest.attribute.data_len += two_r.length;
        self.run_data.all_cr_file_count += two_r.file_count + two_r.dir_count;
        self.save_root_pack_struct()?;
        self.low_save_all()?;
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

    //慢保存代码
    fn low_save_all(&mut self) -> io::Result<()> {
        if self.run_data.all_write_len - self.run_data.last_all_write_len
            > (DATA_BLOCK_LEN as u64) * 1024
            || self.run_data.all_cr_file_count - self.run_data.last_all_cr_file_count > 10_000
        {
            self.run_data.last_all_write_len = self.run_data.all_write_len;
            self.run_data.last_all_cr_file_count = self.run_data.all_cr_file_count;
            self.save_all()?;
        }
        Ok(())
    }
    //加载结构
    fn load_pack_struct(&self, file_pos: u64) -> io::Result<PackStruct> {
        let data_block = self.manifest_data_block_read(file_pos)?;
        let pack_struct = PackStruct::load(data_block)?;
        Ok(pack_struct)
    }
    //加载元数据
    fn load_pack_file_metadata(&self, file_pos: u64) -> io::Result<PackFileMetadata> {
        let data_block = self.manifest_data_block_read(file_pos)?;
        let metadata = PackFileMetadata::load(data_block)?;
        Ok(metadata)
    }

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

    //保存结构
    fn save_pack_struct_write(&mut self, pack_struct: &mut PackStruct) -> io::Result<(bool, u64)> {
        let old_pos = pack_struct.run_data.data_block.file_pos;
        let old_block_len = pack_struct.run_data.data_block.get_this_block_len_u64();
        let (block_data, new_block) = pack_struct.get_block_data();
        let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_block_len)?;
        pack_struct.run_data.data_block.file_pos = pos;
        Ok((new_block, pos))
    }

    //保存元数据
    fn save_metadata_write(&mut self, metadata: &mut PackFileMetadata) -> io::Result<(bool, u64)> {
        let old_pos = metadata.data_block.file_pos;
        let old_block_len = metadata.data_block.get_this_block_len_u64();
        let (block_data, new_block) = metadata.get_block_data();
        let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_block_len)?;
        metadata.data_block.file_pos = pos;
        Ok((new_block, pos))
    }

    fn manifest_data_block_read(&self, file_pos: u64) -> io::Result<ManifestDataBlock> {
        let data_file = if !self.s_manifest_file {
            &self.pack_file
        } else if let Some(manifest_file) = &self.manifest.file {
            manifest_file
        } else {
            Err(Error::other("已启用清单分离文件，但清单文件实例不存在")).unwrap()
        };
        manifest_data_block_read(data_file, file_pos)
    }

    fn manifest_data_block_write(
        &mut self,
        block_data: &[u8],
        new_block: bool,
        old_pos: u64,
        old_block_len: u64,
    ) -> io::Result<u64> {
        Ok(if new_block {
            if self.s_manifest_file {
                let (new_pos, _) = self.get_manifest_file_pos(block_data.len() as u64)?;
                self.set_manifest_file_pos(new_pos)?;
                self.manifest_file_write(block_data)?;
                self.manifest_file_gc_add(vec![(old_pos, old_block_len)]);
                new_pos
            } else {
                let (new_pos, _) = self.get_file_pos(block_data.len() as u64);
                self.set_pack_file_pos_write(new_pos)?;
                self.pack_file_write(block_data)?;
                self.file_gc_add(vec![(old_pos, old_block_len)]);
                new_pos
            }
        } else if self.s_manifest_file {
            self.set_manifest_file_pos(old_pos)?;
            self.manifest_file_write(block_data)?;
            old_pos
        } else {
            self.set_pack_file_pos_write(old_pos)?;
            self.pack_file_write(block_data)?;
            old_pos
        })
    }

    //垃圾回收提交
    fn file_gc_add(&mut self, gc_pos_list: Vec<(u64, u64)>) {
        for pos in gc_pos_list {
            //直接添加
            if pos.0 != 0 && pos.1 != 0 {
                self.run_data.gc_data_pos_list.list.push(pos);
            }
        }
    }
    //清单文件垃圾回收提交
    fn manifest_file_gc_add(&mut self, gc_pos_list: Vec<(u64, u64)>) {
        for pos in gc_pos_list {
            //直接添加
            if pos.0 != 0 && pos.1 != 0 {
                self.manifest.run_data.gc_data_pos_list.list.push(pos);
            }
        }
    }
    //垃圾回收
    fn file_gc(&mut self) -> io::Result<()> {
        Self::from_gc(
            &mut self.run_data.gc_data_pos_list,
            &mut self.manifest.empty_data_list,
        );
        self.save_empty_data_pos_list()
    }

    //清单文件垃圾回收
    fn manifest_file_gc(&mut self) -> io::Result<()> {
        if let Some(to_list) = &mut self.manifest.this_empty_data_list {
            Self::from_gc(&mut self.manifest.run_data.gc_data_pos_list, to_list);
            self.save_manifest_empty_data_pos_list()
        } else {
            Ok(())
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
            if *gc_pos != 0 && *gc_len != 0 {
                pos_list.push((*gc_pos, *gc_len));
            }
        }
        //清空缓存
        gc_data_pos_list.list.clear();

        //合并功能
        //当前索引
        let mut index = 0;
        //如果有下一个则循环
        while let Some(v) = pos_list.get(index + 1) {
            let (next_pos, next_len) = *v; //移除(0,0)

            //当前索引内容
            if let Some((this_pos, this_len)) = pos_list.get_mut(index) {
                //移除(0,0)

                let this_end_pos = *this_pos + *this_len;
                //检查，判断当前位置加当前长度是否等于下一个位置
                if this_end_pos == next_pos {
                    //合并，将下一个占用的大小加到当前大小
                    *this_len += next_len;
                    pos_list.remove(index + 1);
                } else {
                    //否则什么都不做，并附加索引
                    index += 1;
                }
            }
        }
    }

    //获取可用的文件位置
    fn get_file_pos(&mut self, length: u64) -> (u64, u64) {
        //块对齐
        const DATA_BLOCK_LEN_U64: u64 = DATA_BLOCK_LEN as u64;
        let length = if length.is_multiple_of(DATA_BLOCK_LEN_U64) {
            length
        } else {
            let length = length / DATA_BLOCK_LEN_U64 + 1;
            length * DATA_BLOCK_LEN_U64
        };
        //优先使用空隙
        let empty_data_pos = &mut self.manifest.empty_data_list.list;
        let value = if let Some(value) = Self::get_pos_gc(length, empty_data_pos) {
            value
        } else {
            //扩容处理
            (self.pack_file_length, length)
        };
        //分配空间
        let this_end_pos = value.0 + value.1;
        if this_end_pos > self.pack_file_length {
            self.pack_file_length = this_end_pos;
        }
        value
    }

    fn get_pos_gc(length: u64, empty_data_pos: &mut Vec<(u64, u64)>) -> Option<(u64, u64)> {
        //优先使用空数据，但必须完整一块
        let mut index = 0;
        while !empty_data_pos.is_empty() {
            //从第一个开始
            let (pos, len) = if let Some(v) = empty_data_pos.get_mut(index) {
                v
            } else {
                break;
            };
            //判断是否能占用完
            //剩余大小
            return Some(if length == *len {
                //能占用完，但必须等于
                empty_data_pos.remove(index)
            } else if *len > length {
                //不能则切出
                let r = (*pos, *len - length);
                //修改，位置加大小使其向后移动，长度减大小使其边界不变
                *pos += length;
                *len -= length;
                r
            } else {
                index += 1;
                continue;
            });
        }
        None
    }

    //获取连续的清单空间
    fn get_manifest_file_pos(&mut self, length: u64) -> io::Result<(u64, u64)> {
        //尝试获取空数据列表
        if let Some(empty_data_pos) = &mut self.manifest.this_empty_data_list {
            let empty_data_pos = &mut empty_data_pos.list;
            //优先使用空数据，但必须完整一块
            let value = if let Some(value) = Self::get_pos_gc(length, empty_data_pos) {
                value
            } else {
                //扩容处理
                (self.manifest.attribute.manifest_file_len, length)
            };
            let this_end_pos = value.0 + value.1;
            if this_end_pos > self.manifest.attribute.manifest_file_len {
                self.manifest.attribute.manifest_file_len = this_end_pos;
            }
            Ok(value)
        } else {
            Err(Error::other("清单文件空数据列表不存在")).unwrap()
        }
    }

    //保存所有数据
    fn save_all(&mut self) -> io::Result<()> {
        self.save_root_pack_struct()?;
        self.file_gc()?;
        self.manifest_file_gc()?;
        self.save_manifest_attribute()?;
        self.save_pack_length()?;
        Ok(())
    }

    //保存数据长度
    fn save_pack_length(&mut self) -> io::Result<()> {
        self.up_pack_length();
        //上锁
        self.write_lock()?;
        //修改包文件位置
        self.set_pack_file_pos_write(FILE_HEADER_DATA_LENGTH_INDEX)?;
        //写入数据
        self.pack_file_write(self.pack_file_length.to_le_bytes().as_slice())?;
        Ok(())
    }

    //保存属性
    fn save_manifest_attribute(&mut self) -> io::Result<()> {
        //属性
        let attribute = &mut self.manifest.attribute;
        //转换数据
        let data = attribute.to_bytes_vec();
        //写入数据
        //设置文件指针位置,从文件头后面写
        self.set_pack_file_pos_write(FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX)?;
        //写入数据
        self.pack_file_write(&data)?;
        Ok(())
    }

    //保存空数据位置列表
    fn save_empty_data_pos_list(&mut self) -> io::Result<()> {
        let old_pos = self.manifest.attribute.empty_data_pos_list_pos;
        let old_len = self
            .manifest
            .empty_data_list
            .get_data_block_mut()
            .unwrap()
            .get_this_block_len_u64();
        let (block_data, new_block) = self.manifest.empty_data_list.get_block_data().unwrap();
        let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_len)?;
        if new_block {
            self.manifest.attribute.empty_data_pos_list_pos = pos;
        }
        Ok(())
    }
    //保存清单空数据位置列表
    fn save_manifest_empty_data_pos_list(&mut self) -> io::Result<()> {
        if let Some(empty_data_pos_list) = &mut self.manifest.this_empty_data_list {
            let old_pos = self.manifest.attribute.manifest_empty_data_pos_list_pos;
            let old_len = empty_data_pos_list
                .get_data_block_mut()
                .unwrap()
                .get_this_block_len_u64();
            let (block_data, new_block) = empty_data_pos_list.get_block_data().unwrap();
            let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_len)?;
            if new_block {
                self.manifest.attribute.manifest_empty_data_pos_list_pos = pos;
            }
        }
        Ok(())
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
            PackLockType::Dir => Err(Error::new(
                ErrorKind::IsADirectory,
                "无法解锁，锁文件类型是目录",
            ))
                .unwrap(),
            PackLockType::Symlink => Err(Error::other("无法解锁，锁文件类型是符号链接")).unwrap(),
            PackLockType::None => Ok(()),
        }
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
        if self.manifest.run_data.file_pos > self.manifest.attribute.manifest_file_len {
            self.manifest.attribute.manifest_file_len = self.manifest.run_data.file_pos;
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
    fn pack_file_write(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_lock()?;
        let file = &mut self.pack_file;
        file.write_all(data)?;
        file.flush()?;
        let len = data.len() as u64;
        self.run_data.pack_file_pos += len;
        self.run_data.all_write_len += len;
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
    fn set_manifest_file_pos(&mut self, pos: u64) -> io::Result<()> {
        if let Some(file) = &self.manifest.file {
            let mut file = file;
            if self.manifest.run_data.file_pos != pos {
                self.manifest.run_data.file_pos = pos;
                file.seek(SeekFrom::Start(pos))?;
            }
            Ok(())
        } else {
            Err(Error::other("清单文件实例不存在")).unwrap()
        }
    }

    //清单文件写入
    fn manifest_file_write(&mut self, data: &[u8]) -> io::Result<()> {
        if let Some(file) = &mut self.manifest.file {
            file.write_all(data)?;
            file.flush()?;
            let len = data.len() as u64;
            self.manifest.run_data.file_pos += len;
            self.up_manifest_length();
            self.low_save_all()?;
            Ok(())
        } else {
            Err(Error::other("清单文件实例不存在")).unwrap()
        }
    }

    //设置清单文件大小
    fn set_manifest_file_len(&mut self, len: u64) -> io::Result<()> {
        if let Some(file) = &mut self.manifest.file {
            file.set_len(len)?;
            self.manifest.attribute.manifest_file_len = len;
            Ok(())
        } else {
            Err(Error::other("清单文件实例不存在")).unwrap()
        }
    }
}
impl Drop for WBFPManager {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            //确保文件完全写入
            self.pack_file.flush().expect("文件写入错误");
            //写入索引数据
            self.save_all().expect("保存数据错误");
            //释放写入锁
            self.write_unlock().expect("无法解除写入锁");
        }
    }
}

struct MutDirAddReturn {
    length: u64,
    file_count: u64,
    dir_count: u64,
}
impl MutDirAddReturn {
    fn new_empty() -> Self {
        Self {
            length: 0,
            file_count: 0,
            dir_count: 0,
        }
    }
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
//获取清单块数据
fn manifest_data_block_read(data_file: &File, file_pos: u64) -> io::Result<ManifestDataBlock> {
    let mut file = data_file;
    let block_data_buf = vec![0; DATA_BLOCK_LEN];
    let mut block_data_buf = block_data_buf;
    //设置文件指针
    file.seek(SeekFrom::Start(file_pos))?;
    //读取
    file.read_exact(&mut block_data_buf)?;
    //分析是否需要再加载
    let block_len = ManifestDataBlock::get_block_len(&block_data_buf)?;
    let l_len = usize::try_from(block_len).unwrap() - DATA_BLOCK_LEN;
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
            Some(false) => panic!(
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
pub fn open_file<P: AsRef<Path>>(pack_path: &P) -> io::Result<WBFPManager> {
    const HEADER_TYPE_LEN: usize = FILE_HEADER_TYPE_NAME.len();
    let pack_path = pack_path
        .as_ref()
        .to_str()
        .ok_or(Error::other("无法将路径转换成文本"))?
        .to_string();

    //打开水球包文件
    let mut pack_file = File::options().read(true).write(true).open(&pack_path)?;
    //读取完整的文件头块
    let mut header_block_data = vec![0; DATA_BLOCK_LEN];
    let header_block_r_len = pack_file.read(&mut header_block_data)?;
    if header_block_r_len < DATA_BLOCK_LEN {
        return Err(Error::other("无法读取完整的文件头"));
    }
    let header = &header_block_data[..FILE_HEADER_DATA_LENGTH as usize];
    //判断文件类型
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
    let bool_data = header[FILE_HEADER_BOOL_DATA_INDEX];
    //写时复制 TODO:未使用变量
    let _cow = (bool_data >> 7) == 1;
    let s_manifest_file = ((bool_data << 1) >> 7) == 1;
    let pack_len = u64::from_le_bytes(
        header[FILE_HEADER_DATA_LENGTH_INDEX as usize
            ..(FILE_HEADER_DATA_LENGTH_INDEX + FILE_HEADER_DATA_LENGTH_LENGTH) as usize]
            .try_into()
            .unwrap(),
    );
    //获取清单属性数据
    //获取属性
    let attribute_data = &header[FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX as usize
        ..(FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX as usize) + MANIFEST_ATTRIBUTE_LEN];
    let attribute = Attribute::load(attribute_data)?;
    //锁文件
    let mut write_lock_file_path = pack_path.clone();
    write_lock_file_path.push_str(".lock");
    let write_lock_file = write_lock_file(&PathBuf::from(write_lock_file_path))?;
    //如果分离数据文件
    if s_manifest_file {
        //尝试加载分离数据文件
        let mut manifest_path = pack_path.clone();
        manifest_path.push_str(".wbm");
        let manifest_file = File::options().read(true).write(true).open(manifest_path)?;
        //空数据列表===
        let empty_pos_data_block =
            manifest_data_block_read(&manifest_file, attribute.empty_data_pos_list_pos)?;
        let empty_pos_data = empty_pos_data_block
            .get_this_data()
            .expect("读取空数据列表失败")
            .to_vec();
        let empty_data_list = DataPosList::load(&empty_pos_data, Some(empty_pos_data_block));
        //清单文件空数据
        let manifest_empty_pos_data_block =
            manifest_data_block_read(&manifest_file, attribute.manifest_empty_data_pos_list_pos)?;
        let manifest_empty_pos_data = manifest_empty_pos_data_block
            .get_this_data()
            .expect("读取清单空数据列表失败")
            .to_vec();
        let manifest_empty_data_pos_list = DataPosList::load(
            &manifest_empty_pos_data,
            Some(manifest_empty_pos_data_block),
        );
        //加载根结构
        let root_struct_block_data =
            manifest_data_block_read(&manifest_file, attribute.root_struct_pos)?;
        let root_struct = PackStruct::load(root_struct_block_data)?;
        let manifest = WBFilesPackManifest {
            attribute,
            this_empty_data_list: Some(manifest_empty_data_pos_list),
            root_struct,
            empty_data_list,
            file: Some(manifest_file),
            run_data: WBFilesPackManifestRun::default(),
        };
        Ok(WBFPManager::new2(
            pack_path,
            manifest,
            pack_file,
            s_manifest_file,
            Some(write_lock_file),
            pack_len,
        ))
    } else {
        //空数据列表===
        let empty_pos_data_block =
            manifest_data_block_read(&pack_file, attribute.empty_data_pos_list_pos)?;
        let empty_pos_data = empty_pos_data_block
            .get_this_data()
            .expect("读取空数据列表失败")
            .to_vec();
        let empty_data_list = DataPosList::load(&empty_pos_data, Some(empty_pos_data_block));
        //加载根结构
        let root_struct_block_data =
            manifest_data_block_read(&pack_file, attribute.root_struct_pos)?;
        let root_struct = PackStruct::load(root_struct_block_data)?;
        let manifest = WBFilesPackManifest {
            attribute,
            this_empty_data_list: None,
            root_struct,
            empty_data_list,
            file: None,
            run_data: WBFilesPackManifestRun::default(),
        };
        Ok(WBFPManager::new2(
            pack_path,
            manifest,
            pack_file,
            s_manifest_file,
            Some(write_lock_file),
            pack_len,
        ))
    }
}

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
    create_new: bool,
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
            empty_data_list: DataPosList {
                data_block: Some(ManifestDataBlock::default()),
                list: Vec::new(),
            },
            this_empty_data_list: if s_manifest_file {
                Some(DataPosList {
                    data_block: Some(ManifestDataBlock::default()),
                    list: Vec::new(),
                })
            } else {
                None
            },
            file: manifest_file,
            run_data: WBFilesPackManifestRun::default(),
        },
        pack_file,
        s_manifest_file,
        write_lock_file,
    )
}

//算法测试
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
    WBFPManager::from_gc(&mut gc_data_list, &mut empty_data_list);
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
}
