/*
开始时间：26/2/11 15：51
 */
use crate::tools::PathTool;
use crate::wb_files_pack::pack_io::{
    PackIO, FILE_HEADER_BLOCK_LEN, FILE_HEADER_BOOL_DATA_INDEX,
    FILE_HEADER_DATA_LENGTH, FILE_HEADER_DATA_LENGTH_INDEX,
    FILE_HEADER_DATA_LENGTH_LENGTH, FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX, FILE_HEADER_TYPE_NAME,
    FILE_HEADER_VERSION, FILE_HEADER_VERSION_INDEX,
};
use crate::wb_files_pack::{
    Attribute, DataPosList, ManifestDataBlock, ManifestDataBlockTrait, PackFileMetadata,
    PackFileMetadataRun, PackFileMetadataType, PackStruct, PackStructItem,
    PackStructItemType, WBFilesPackManifest, WBFilesPackManifestRun, DATA_BLOCK_LEN, DATA_DATA_BLOCK_LEN,
};
use core::slice::Iter;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;
use std::vec::IntoIter;
use std::{fs, io};
use tracing::info;

#[cfg(test)]
mod test;

/*
#[cfg(debug_assertions)] {
#[cfg(test)]
mod new_test;*/

//默认写实复制
pub const DEFAULT_COW: bool = false;

//默认分离数据为单独文件
pub const DEFAULT_S_MANIFEST_FILE: bool = true;
//默认哈希算法
pub const DEFAULT_HASH_TYPE: u8 = 1;

pub(crate) struct WBFPManager {
    // 清单实例
    manifest: WBFilesPackManifest,
    // 包文件实例
    pack_file: Arc<Mutex<PackIO>>,
    // 启用写时复制
    cow: bool,
    // 清单分离
    s_manifest_file: bool,
    //运行时数据结构体
    run_data: WBFPManagerRun,
} //水球包文件管理器
struct WBFPManagerRun {
    //写入锁
    write_lock: bool,
    //写入锁路径
    write_lock_path: PathBuf,
    //锁文件对象实例
    write_lock_file: Option<File>,
} //运行时数据结构体
impl WBFPManagerRun {
    fn new(write_lock_path: PathBuf, write_lock_file: Option<File>) -> Self {
        Self {
            write_lock: false,
            write_lock_path,
            write_lock_file,
        }
    }
}
impl WBFPManager {
    //创建实例
    fn new<P: AsRef<Path>>(
        pack_path: P,
        manifest: WBFilesPackManifest,
        pack_file: Arc<Mutex<PackIO>>,
        s_manifest_file: bool,
        write_lock_file: Option<File>,
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
            run_data: WBFPManagerRun::new(write_lock_path, write_lock_file),
        }
    }

    //初始化新包
    pub(crate) fn init_new_pack(&mut self) -> io::Result<()> {
        //文件头缓存
        let mut file_header_buf = vec![0; FILE_HEADER_BLOCK_LEN];
        //写出文件头
        //类型名称
        for (index, value) in FILE_HEADER_TYPE_NAME.iter().enumerate() {
            file_header_buf[index] = *value;
        }
        //写入文件版本
        for (index, value) in FILE_HEADER_VERSION.iter().enumerate() {
            file_header_buf[FILE_HEADER_VERSION_INDEX + index] = *value;
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
        file_header_buf[FILE_HEADER_BOOL_DATA_INDEX] = header_tag;
        //===

        //文件大小
        for (index, value) in FILE_HEADER_BLOCK_LEN.to_le_bytes().iter().enumerate() {
            file_header_buf[FILE_HEADER_DATA_LENGTH_INDEX + index] = *value;
        }
        //清单属性
        for (index, byte) in self
            .manifest
            .attribute
            .get_block_data()
            .0
            .into_iter()
            .enumerate()
        {
            file_header_buf[FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX + index] = byte;
        }
        //填充所有剩余空间
        file_header_buf.resize(FILE_HEADER_BLOCK_LEN, 0);

        {
            let pack_file = self.pack_file.clone();
            let mut pack_file = pack_file
                .lock()
                .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
            pack_file
                .write_all(&file_header_buf)
                .expect("无法写入文件头");

            //设置文件大小
            pack_file
                .set_len(FILE_HEADER_BLOCK_LEN as u64)
                .map_err(|err| Error::other(format!("无法设置文件大小, err: {err}")))?;
        }

        //初始化并保存空数据列表
        self.save_empty_data_list()
            .map_err(|err| Error::other(format!("初始化和保存空数据列表失败, err:{err}")))?;

        if self.s_manifest_file {
            self.save_manifest_empty_data_list()
                .map_err(|err| Error::other(format!("初始化和保存空数据列表失败, err: {err}")))?;
        }
        //初始化并保存根结构
        self.save_root_pack_struct()
            .map_err(|err| Error::other(format!("初始化和保存根结构失败, err: {err}")))?;
        Ok(())
    }
}

//读取===
impl WBFPManager /* 读取 */ {
    pub(crate) fn file_is_some<P: AsRef<Path>>(&mut self, path: P) -> bool {
        self.get_pack_struct_item(path).is_ok()
    }

    //获取属性数据
    pub(crate) fn get_manifest_attribute(&self) -> &Attribute {
        self.manifest.attribute()
    }

    //获取根结构列表
    pub(crate) fn get_root_struct_items(&self) -> &HashMap<String, PackStructItem> {
        &self.manifest.root_struct.items
    }

    //获取根结构的名称列表
    pub(crate) fn get_root_struct_item_name_list(&self) -> Vec<String> {
        let mut name_list = Vec::with_capacity(self.manifest.root_struct.items.len());
        for name in self.manifest.root_struct.items.keys() {
            name_list.push(name.clone());
        }
        name_list
    }

    //获取结构项文件文件名列表
    pub(crate) fn get_struct_item_name_list<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> io::Result<Vec<String>> {
        let item = self.get_pack_struct_item(&path)?;
        match &item.item_type {
            PackStructItemType::Dir { pack_struct, .. } => {
                if let Some(pack_struct) = pack_struct {
                    let mut name_list = Vec::with_capacity(pack_struct.items.len());
                    for name in pack_struct.items.keys() {
                        name_list.push(name.clone());
                    }
                    Ok(name_list)
                } else {
                    Err(Error::other(format!(
                        r#"虚拟路径"{}"的结构实例没有被加载"#,
                        path.as_ref().display()
                    )))
                }
            }
            PackStructItemType::File => Err(Error::new(
                ErrorKind::NotADirectory,
                format!(r#"虚拟路径"{}"是文件不是目录"#, path.as_ref().display()),
            )),
        }
    }

    //获取目录结构项列表
    pub(crate) fn get_dir_pack_struct_items<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> io::Result<&HashMap<String, PackStructItem>> {
        self.load_pack_struct_metadata_path(&path)?;
        let path_pack_struct_item = self.get_pack_struct_item_dir(&path)?;
        match &path_pack_struct_item.item_type {
            PackStructItemType::Dir { pack_struct, .. } => {
                if let Some(pack_struct) = pack_struct {
                    Ok(&pack_struct.items)
                } else {
                    Err(Error::other(format!(
                        r#"虚拟路径"{}"的结构实例没有被加载"#,
                        path.as_ref().display()
                    )))
                }
            }
            PackStructItemType::File => Err(Error::new(
                ErrorKind::NotADirectory,
                format!(r#"提供的路径"{}"是文件不是目录"#, path.as_ref().display()),
            )),
        }
    }

    //获取目录结构项
    pub(crate) fn get_pack_struct_item_dir<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> io::Result<&PackStructItem> {
        let path_list = PathTool::path_to_string_vec(path);
        self.load_pack_struct_metadata_path2(&path_list, true)?;
        self.get_pack_struct_item2(&path_list)
    }

    //获取结构项
    pub(crate) fn get_pack_struct_item<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> io::Result<&PackStructItem> {
        let path_list = PathTool::path_to_string_vec(path);
        self.load_pack_struct_metadata_path2(&path_list, false)?;
        self.get_pack_struct_item2(&path_list)
    }

    fn get_pack_struct_item2(&self, path_list: &[String]) -> io::Result<&PackStructItem> {
        if path_list.len() > 1 {
            let mut name_list = path_list.iter();
            let mut s_name = name_list.next();
            let mut s_pack_struct = &self.manifest.root_struct;
            let mut s_path = PathBuf::new();
            while let Some(name) = s_name {
                let this_path = s_path.join(name);
                if let Some(item) = s_pack_struct.items.get(name) {
                    if let Some(next_name) = name_list.next() {
                        match &item.item_type {
                            PackStructItemType::Dir { pack_struct, .. } => {
                                if let Some(pack_struct) = pack_struct {
                                    s_pack_struct = pack_struct;
                                    s_path = this_path;
                                    s_name = Some(next_name);
                                } else {
                                    Err(Error::other(format!(
                                        r#"虚拟路径"{}"实例没有加载"#,
                                        this_path.display()
                                    )))?;
                                }
                            }
                            PackStructItemType::File => {
                                Err(Error::new(
                                    ErrorKind::NotADirectory,
                                    format!(r#"虚拟路径"{}"是文件不是目录"#, this_path.display()),
                                ))?;
                            }
                        }
                    } else {
                        return Ok(item);
                    }
                } else {
                    Err(Error::new(
                        ErrorKind::NotFound,
                        format!(r#"虚拟路径目录"{}"的结构项不存在"#, this_path.display()),
                    ))?;
                }
            }
            Err(Error::other(format!("未找到路径{path_list:?}")))
        } else if let Some(v) = self.manifest.root_struct.items.get(&path_list[0]) {
            Ok(v)
        } else if !path_list.is_empty() {
            Err(Error::new(
                ErrorKind::NotFound,
                format!(r#"虚拟路径"{}"的结构项不存在"#, path_list[0]),
            ))
        } else {
            Err(Error::other("提供了无效或空的路径"))
        }
    }

    pub(crate) fn load_all_data(&mut self, no_err: bool) -> io::Result<()> {
        fn m_load_all_data(
            wbfp_manager: &mut WBFPManager,
            pack_struct_items: &mut HashMap<String, PackStructItem>,
            no_err: bool,
            s_path: &Path,
        ) -> io::Result<()> {
            for item in pack_struct_items.values_mut() {
                if let PackStructItemType::Dir {
                    struct_file_pos,
                    pack_struct,
                } = &mut item.item_type
                    && pack_struct.is_none()
                {
                    let mut this_pack_struct = wbfp_manager.load_pack_struct(*struct_file_pos)?;
                    if let Err(err) = m_load_all_data(
                        wbfp_manager,
                        &mut this_pack_struct.items,
                        no_err,
                        &s_path.join(&item.name),
                    ) && !no_err
                    {
                        Err(err)?;
                    }
                    *pack_struct = Some(this_pack_struct);
                }
                //加载元数据
                if let PackFileMetadataRun::NoLoad = item.metadata {
                    let this_path = s_path.join(&item.name);
                    item.metadata = PackFileMetadataRun::Loaded(
                        match wbfp_manager.load_pack_file_metadata(item.metadata_file_pos) {
                            Ok(v) => Box::new(v),
                            Err(err) => Err(Error::other(format!(
                                r#"虚拟路径"{}"的元数据无法加载, err:{err}"#,
                                this_path.display()
                            )))?,
                        },
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
        if let Err(err) = m_load_all_data(self, &mut this_root_items, no_err, &PathBuf::new()) {
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

    pub(crate) fn load_pack_struct_metadata_path<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> io::Result<()> {
        self.load_pack_struct_metadata_path2(&PathTool::path_to_string_vec(path), false)
    }
    fn load_pack_struct_metadata_path2(
        &mut self,
        path_list: &[String],
        is_dir: bool,
    ) -> io::Result<()> {
        let mut path_list = path_list.iter();
        let two_name = path_list.next().unwrap();
        //暂移
        let two_pack_struct = if let Some(mut struct_item) =
            self.manifest.root_struct.items.remove(two_name)
        {
            match &mut struct_item.item_type {
                PackStructItemType::Dir {
                    struct_file_pos,
                    pack_struct,
                } => {
                    if pack_struct.is_none() {
                        *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                    }
                    if let Some(pack_struct) = pack_struct {
                        //递归
                        if let Err(err) = self.s_load_pack_struct_metadata_path(
                            pack_struct,
                            &mut path_list,
                            is_dir,
                            &PathBuf::from(two_name),
                        ) {
                            self.manifest
                                .root_struct
                                .items
                                .insert(two_name.clone(), struct_item);
                            return Err(err);
                        }
                        if let PackFileMetadataRun::NoLoad = struct_item.metadata {
                            struct_item.metadata = PackFileMetadataRun::Loaded(
                                match self.load_pack_file_metadata(struct_item.metadata_file_pos) {
                                    Ok(v) => Box::from(v),
                                    Err(err) => Err(Error::other(format!(
                                        r#"虚拟路径"{two_name}"的元数据加载失败, err:{err}"#
                                    )))?,
                                },
                            );
                        }
                        struct_item
                    } else {
                        Err(Error::other(format!(
                            r#"虚拟路径"{two_name}"的结构实例没有被加载"#
                        )))?
                    }
                }
                PackStructItemType::File => {
                    //如果只有二级，且不限定目录
                    if is_dir || path_list.next().is_some() {
                        Err(Error::new(
                            ErrorKind::NotADirectory,
                            format!(r#"虚拟路径"{two_name}"是文件不是目录"#),
                        ))?
                    } else {
                        if let PackFileMetadataRun::NoLoad = struct_item.metadata {
                            struct_item.metadata = PackFileMetadataRun::Loaded(
                                match self.load_pack_file_metadata(struct_item.metadata_file_pos) {
                                    Ok(v) => Box::from(v),
                                    Err(err) => Err(Error::other(format!(
                                        r#"虚拟路径"{two_name}"的元数据无法加载, err:{err}"#
                                    )))?,
                                },
                            );
                        }
                        struct_item
                    }
                }
            }
        } else {
            Err(Error::new(
                ErrorKind::NotFound,
                format!(r#"虚拟路径"{two_name}"的结构项不存在"#),
            ))?
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
        s_path: &Path,
    ) -> io::Result<()> {
        if let Some(this_name) = path_list.next() {
            let this_path = s_path.join(this_name);
            if let Some(item) = s_pack_struct.items.get_mut(this_name) {
                match &mut item.item_type {
                    PackStructItemType::Dir {
                        struct_file_pos,
                        pack_struct,
                    } => {
                        if pack_struct.is_none() {
                            *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                        }
                        if let Some(pack_struct) = pack_struct {
                            self.s_load_pack_struct_metadata_path(
                                pack_struct,
                                path_list,
                                is_dir,
                                &this_path,
                            )?;
                            //加载元数据
                            self.load_metadata_to_item(&this_path, item)?;
                            Ok(())
                        } else {
                            Err(Error::other(format!(
                                r#"虚拟路径"{}"的结构实例没有被加载"#,
                                this_path.display()
                            )))
                        }
                    }
                    PackStructItemType::File => {
                        //判断是否只有目录且是否存在下一个路径
                        if is_dir || path_list.next().is_some() {
                            Err(Error::new(
                                ErrorKind::NotADirectory,
                                format!(r#"虚拟路径"{}"是文件不是目录"#, this_path.display()),
                            ))
                        } else {
                            //最后一个就加载元数据
                            self.load_metadata_to_item(&this_path, item)?;
                            Ok(())
                        }
                    }
                }
            } else {
                Err(Error::new(
                    ErrorKind::NotFound,
                    format!(r#"虚拟路径"{}"的结构项不存在"#, this_path.display()),
                ))?
            }
        } else {
            Ok(())
        }
    }

    fn load_metadata_to_item(
        &mut self,
        this_path: &Path,
        item: &mut PackStructItem,
    ) -> Result<(), Error> {
        if let PackFileMetadataRun::NoLoad = item.metadata {
            item.metadata = PackFileMetadataRun::Loaded(
                match self.load_pack_file_metadata(item.metadata_file_pos) {
                    Ok(v) => Box::from(v),
                    Err(err) => Err(Error::other(format!(
                        r#"虚拟路径"{}"的元数据无法加载, err: {err}"#,
                        this_path.display()
                    )))?,
                },
            );
        }
        Ok(())
    }

    pub(crate) fn get_dir<P: AsRef<Path>>(&mut self, path: P) -> io::Result<&PackStruct> {
        let path_list = PathTool::path_to_string_vec(path);
        if path_list.is_empty() {
            Err(Error::other("提供了无效或空的路径"))
        } else {
            self.load_pack_struct_metadata_path2(&path_list, true)?;
            let mut pack_struct = &self.manifest.root_struct;
            let mut path_list_iter = path_list.iter();
            let mut name = path_list_iter.next();
            let mut path = PathBuf::new();
            while let Some(this_name) = name {
                let this_path = path.join(this_name);
                if let Some(item) = pack_struct.items.get(this_name) {
                    let this_pack_struct = match &item.item_type {
                        PackStructItemType::Dir { pack_struct, .. } => {
                            if let Some(pack_struct) = pack_struct {
                                pack_struct
                            } else {
                                Err(Error::other(format!(
                                    r#"虚拟路径"{}"的结构没有被加载"#,
                                    this_path.display()
                                )))?
                            }
                        }
                        PackStructItemType::File => Err(Error::new(
                            ErrorKind::NotADirectory,
                            format!(r#"虚拟路径"{}"是文件不是目录"#, this_path.display()),
                        ))?,
                    };
                    //下循环
                    let next_name = path_list_iter.next();
                    if next_name.is_some() {
                        name = next_name;
                        pack_struct = this_pack_struct;
                        path = this_path;
                    } else {
                        return Ok(this_pack_struct);
                    }
                } else {
                    Err(Error::new(
                        ErrorKind::NotFound,
                        format!(r#"虚拟路径"{}不存在""#, this_path.display()),
                    ))?;
                }
            }
            Err(Error::other(format!(r#"未找到路径"{}""#, path.display())))
        }
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
}
//写入===
impl WBFPManager /* 写入 */ {
    //元数据更新
    pub(crate) fn file_metadata_update(
        &mut self,
        path_list: Vec<String>,
        mut metadata: PackFileMetadata,
    ) -> io::Result<()> {
        let mut path_list = path_list.into_iter();
        let two_name = path_list.next().ok_or(Error::other("路径为空"))?;
        //暂移二级
        if let Some(mut struct_item) = self.manifest.root_struct.items.remove(&two_name) {
            match &mut struct_item.item_type {
                PackStructItemType::Dir {
                    struct_file_pos,
                    pack_struct,
                } => {
                    if pack_struct.is_none() {
                        *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                    }
                    if let Some(pack_struct) = pack_struct {
                        let r = self.file_metadata_update_inner(
                            path_list,
                            &PathBuf::from(&two_name),
                            &mut pack_struct.items,
                            metadata,
                        )?;
                        let (new_pos, pos) = self.save_pack_struct_write(pack_struct)?;
                        if new_pos {
                            *struct_file_pos = pos;
                        }
                        self.save_metadata(&mut struct_item, &r)?;
                        self.manifest
                            .root_struct
                            .items
                            .insert(two_name, struct_item);
                        self.manifest.attribute.file_count += r.file_count;
                        self.manifest.attribute.data_len += r.length;
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
                        struct_item.metadata.unlock(metadata);
                        self.manifest
                            .root_struct
                            .items
                            .insert(two_name, struct_item);
                    } else {
                        Err(Error::new(
                            ErrorKind::NotADirectory,
                            format!(r#"虚拟路径"{two_name}"是文件不是目录"#),
                        ))?;
                    }
                }
            }
        } else if path_list.next().is_none() {
            //写入元数据
            let (_, metadata_file_pos) =
                self.save_metadata_write(&mut metadata)
                    .or(Err(Error::other(format!(
                        r#"无法保存文件"{two_name}"的元数据"#
                    ))))?;
            //创建文件
            let len = metadata.len;
            let pack_struct_item = PackStructItem {
                name: two_name.clone(),
                item_type: PackStructItemType::File,
                metadata_file_pos,
                metadata: PackFileMetadataRun::Loaded(Box::from(metadata)),
            };
            self.manifest
                .root_struct
                .items
                .insert(two_name, pack_struct_item);
            self.manifest.attribute.file_count += 1;
            self.manifest.attribute.data_len += len;
        } else {
            Err(Error::new(
                ErrorKind::NotFound,
                format!(r#"虚拟路径"{two_name}"不存在"#),
            ))?;
        }
        self.save_root_pack_struct()
    }

    fn save_metadata(
        &mut self,
        struct_item: &mut PackStructItem,
        r: &DirFileAddReturn,
    ) -> Result<(), Error> {
        if let PackFileMetadataRun::Loaded(metadata) = &mut struct_item.metadata {
            metadata.len += r.length;
            if let PackFileMetadataType::Dir { file_count, .. } = &mut metadata.file_type {
                *file_count += r.file_count;
            }
            let (new_pos, pos) = self.save_metadata_write(metadata)?;
            if new_pos {
                struct_item.metadata_file_pos = pos;
            }
        }
        Ok(())
    }

    fn file_metadata_update_inner(
        &mut self,
        mut path_list: IntoIter<String>,
        s_path: &Path,
        pack_struct_items: &mut HashMap<String, PackStructItem>,
        mut metadata: PackFileMetadata,
    ) -> io::Result<DirFileAddReturn> {
        if let Some(name) = path_list.next() {
            let this_path = s_path.join(&name);
            if let Some(item) = pack_struct_items.get_mut(&name) {
                match &mut item.item_type {
                    PackStructItemType::Dir {
                        struct_file_pos,
                        pack_struct,
                    } => {
                        if pack_struct.is_none() {
                            *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                        }
                        if let Some(pack_struct) = pack_struct {
                            let r = self.file_metadata_update_inner(
                                path_list,
                                &PathBuf::from(name),
                                &mut pack_struct.items,
                                metadata,
                            )?;
                            let (new_pos, pos) = self.save_pack_struct_write(pack_struct)?;
                            if new_pos {
                                *struct_file_pos = pos;
                            }
                            self.save_metadata(item, &r)?;
                            Ok(r)
                        } else {
                            panic!("逻辑错误")
                        }
                    }
                    PackStructItemType::File => {
                        if path_list.next().is_none() {
                            //更新元数据
                            let (new_pos, pos) = self.save_metadata_write(&mut metadata)?;
                            if new_pos {
                                item.metadata_file_pos = pos;
                            }
                            item.metadata.unlock(metadata);
                            Ok(DirFileAddReturn {
                                length: 0,
                                file_count: 0,
                                dir_count: 0,
                            })
                        } else {
                            Err(Error::new(
                                ErrorKind::NotADirectory,
                                format!(r#"虚拟路径"{}"是文件不是目录"#, this_path.display()),
                            ))
                        }
                    }
                }
            } else if path_list.next().is_none() {
                //写入元数据
                let (_, metadata_file_pos) =
                    self.save_metadata_write(&mut metadata)
                        .or(Err(Error::other(format!(
                            r#"无法保存文件"{}"的元数据"#,
                            this_path.display()
                        ))))?;
                //创建文件
                let len = metadata.len;
                let pack_struct_item = PackStructItem {
                    name: name.clone(),
                    item_type: PackStructItemType::File,
                    metadata_file_pos,
                    metadata: PackFileMetadataRun::Loaded(Box::from(metadata)),
                };
                pack_struct_items.insert(name, pack_struct_item);
                Ok(DirFileAddReturn {
                    length: len,
                    file_count: 1,
                    dir_count: 0,
                })
            } else {
                Err(Error::new(
                    ErrorKind::NotFound,
                    format!(r#"虚拟路径"{}"不存在"#, this_path.display()),
                ))
            }
        } else {
            panic!("逻辑错误")
        }
    }
    pub(crate) fn file_metadata_lock(
        &mut self,
        path_list: &[String],
    ) -> io::Result<PackFileMetadata> {
        let mut path_list = path_list.iter();
        let two_pack_struct_name = path_list.next().ok_or(Error::other("路径为空"))?;
        let file_metadata = if let Some(mut pack_struct_item) =
            self.manifest.root_struct.items.remove(two_pack_struct_name)
        {
            match &mut pack_struct_item.item_type {
                //
                PackStructItemType::Dir {
                    struct_file_pos,
                    pack_struct,
                } => {
                    if pack_struct.is_none() {
                        *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                    }
                    if let Some(pack_struct) = pack_struct {
                        let r = self.file_metadata_lock_inner(
                            &mut path_list,
                            &PathBuf::from(two_pack_struct_name),
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
                    //
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
                            format!(r#"虚拟路径"{two_pack_struct_name}是文件不是目录""#),
                        ))?
                    }
                }
            }
        } else {
            Err(Error::new(ErrorKind::NotFound, "文件或目录不存在"))?
        };
        Ok(file_metadata)
    }
    fn file_metadata_lock_inner(
        &mut self,
        path_list: &mut Iter<String>,
        s_path: &Path,
        pack_struct_items: &mut HashMap<String, PackStructItem>,
    ) -> io::Result<PackFileMetadata> {
        if let Some(name) = path_list.next() {
            let this_path = s_path.join(name);
            if let Some(item) = pack_struct_items.get_mut(name) {
                match &mut item.item_type {
                    PackStructItemType::Dir {
                        struct_file_pos,
                        pack_struct,
                    } => {
                        if pack_struct.is_none() {
                            *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                        }
                        if let Some(pack_struct) = pack_struct {
                            self.file_metadata_lock_inner(
                                path_list,
                                &this_path,
                                &mut pack_struct.items,
                            )
                        } else {
                            panic!("逻辑错误")
                        }
                    }
                    PackStructItemType::File => {
                        if path_list.next().is_none() {
                            self.load_metadata_to_item(&this_path, item)?;
                            item.metadata.try_lock()
                        } else {
                            Err(Error::new(
                                ErrorKind::NotADirectory,
                                format!(r#"虚拟路径"{}"是文件不是目录"#, this_path.display()),
                            ))?
                        }
                    }
                }
            } else {
                Err(Error::new(
                    ErrorKind::NotFound,
                    format!(r#"虚拟路径"{}"不存在"#, this_path.display()),
                ))?
            }
        } else {
            panic!("逻辑错误")
        }
    }

    //创建文件

    pub(crate) fn create_file_no_len<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> io::Result<(Vec<String>, PackFileMetadata)> {
        self.create_file(
            path,
            if let Ok(d) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                d.as_millis()
            } else {
                0
            },
            DATA_DATA_BLOCK_LEN,
            self.cow,
            DEFAULT_HASH_TYPE,
        )
    }

    pub(crate) fn create_file2<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
    ) -> io::Result<(Vec<String>, PackFileMetadata)> {
        self.create_file(path, modified, len, self.cow, DEFAULT_HASH_TYPE)
    }

    pub(crate) fn create_file<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
        cow: bool,
        hash_type: u8,
    ) -> io::Result<(Vec<String>, PackFileMetadata)> {
        let path_list = PathTool::path_to_string_vec(&path);
        if self.file_is_some(&path) {
            Err(Error::other(format!(
                r#"虚拟路径"{}"文件或目录已存在"#,
                path.as_ref().display()
            )))?;
        }
        if path_list.len() > 1 {
            self.create_dir_all2(&path_list[..path_list.len() - 1])?;
        }
        let metadata = PackFileMetadata {
            cow,
            len,
            modified,
            data_block: ManifestDataBlock::default(),
            file_type: PackFileMetadataType::File {
                hash_type,
                hash_value: Vec::new(),
                data_pos_list: DataPosList {
                    data_block: None,
                    list: vec![self.get_file_pos(len)?],
                },
            },
        };

        Ok((path_list, metadata))
    }

    //创建目录
    fn create_dir_all_inner(
        &mut self,
        s_pack_struct: &mut PackStruct,
        path_list: &mut Iter<String>,
        cow: bool,
        s_path: &Path,
    ) -> io::Result<DirFileAddReturn> {
        if let Some(name) = path_list.next() {
            let this_path = s_path.join(name);
            //判断目录是否存在
            if let Some(item) = s_pack_struct.items.get_mut(name) {
                //加载元数据
                self.load_metadata_to_item(&this_path, item)?;
                match &mut item.item_type {
                    PackStructItemType::Dir {
                        struct_file_pos,
                        pack_struct,
                    } => {
                        if pack_struct.is_none() {
                            //加载实例
                            *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                        }
                        if let Some(pack_struct) = pack_struct {
                            //递归
                            let r =
                                self.create_dir_all_inner(pack_struct, path_list, cow, &this_path)?;
                            if r.dir_count != 0 || r.file_count != 0 {
                                let (new_block, pos) = self.save_pack_struct_write(pack_struct)?;

                                if new_block {
                                    *struct_file_pos = pos;
                                }
                            }
                            match &mut item.metadata {
                                PackFileMetadataRun::Loaded(metadata) => {
                                    if let PackFileMetadataType::Dir {
                                        file_count,
                                        dir_count,
                                    } = &mut metadata.file_type
                                    {
                                        *dir_count += r.dir_count;
                                        *file_count += r.file_count;
                                        metadata.len += r.length;
                                        //保存元数据和结构
                                        if r.dir_count != 0 || r.file_count != 0 || r.length != 0 {
                                            let (new_block, pos) =
                                                self.save_metadata_write(metadata)?;

                                            if new_block {
                                                item.metadata_file_pos = pos;
                                            }
                                        }
                                        Ok(DirFileAddReturn {
                                            dir_count: r.dir_count,
                                            file_count: r.file_count,
                                            length: r.length,
                                        })
                                    } else {
                                        panic!("存在逻辑错误")
                                    }
                                }
                                PackFileMetadataRun::Locked => Err(Error::other("元数据被锁定")),
                                PackFileMetadataRun::NoLoad => panic!("元数据没有被加载"),
                                PackFileMetadataRun::None => panic!("逻辑错误：目录的元数据为空"),
                            }
                        } else {
                            panic!(r#"虚拟路径"{}"的结构没有被加载"#, this_path.display())
                        }
                    }
                    PackStructItemType::File => Err(Error::new(
                        ErrorKind::NotADirectory,
                        format!("虚拟路径{}存在同名文件", this_path.display()),
                    )),
                }
            } else {
                //创建

                let mut item =
                    PackStructItem::new_empty_dir(name, PackFileMetadata::new_empty_dir(cow));
                //递归
                let r = if let PackStructItemType::Dir {
                    struct_file_pos,
                    pack_struct,
                } = &mut item.item_type
                    && let Some(pack_struct) = pack_struct
                {
                    let r = self.create_dir_all_inner(pack_struct, path_list, cow, &this_path)?;
                    if let PackFileMetadataRun::Loaded(metadata) = &mut item.metadata
                        && let PackFileMetadataType::Dir {
                        file_count,
                        dir_count,
                    } = &mut metadata.file_type
                    {
                        *dir_count += r.dir_count;
                        *file_count += r.file_count;
                        metadata.len += r.length;
                        //保存元数据
                        let (_, pos) = self.save_metadata_write(metadata)?;

                        item.metadata_file_pos = pos;
                    } else {
                        panic!("逻辑错误");
                    }
                    //保存结构
                    let (_, pos) = self.save_pack_struct_write(pack_struct)?;

                    *struct_file_pos = pos;
                    r
                } else {
                    panic!("逻辑错误")
                };
                s_pack_struct.items.insert(name.clone(), item);
                Ok(DirFileAddReturn {
                    length: r.length,
                    file_count: r.file_count,
                    dir_count: r.dir_count + 1,
                })
            }
        } else {
            Ok(DirFileAddReturn {
                length: 0,
                file_count: 0,
                dir_count: 0,
            })
        }
    }

    pub(crate) fn create_dir_all<P: AsRef<Path>>(&mut self, path: &P) -> io::Result<()> {
        self.create_dir_all2(&PathTool::path_to_string_vec(path))
    }

    pub(crate) fn create_dir_all2(&mut self, path_list: &[String]) -> io::Result<()> {
        let mut path_list = path_list.iter();
        //移动或创建二级目录实例，通过二级递归，避免借用问题
        let root_struct = &mut self.manifest.root_struct;
        let two_name = path_list.next().ok_or(Error::other("路径为空"))?;
        let (mut two_pack_struct_item, mut two_r) = if let Some(mut two_item) =
            root_struct.items.remove(two_name)
        {
            //存在则暂时删除（移动）
            //类型判断
            match &mut two_item.item_type {
                PackStructItemType::Dir {
                    struct_file_pos,
                    pack_struct,
                } => {
                    //实例判断并尝试加载
                    if pack_struct.is_none() {
                        //加载结构
                        *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                    }
                }
                PackStructItemType::File => {
                    Err(Error::other(format!(
                        r#"虚拟路径"{two_name}"是文件不是目录"#
                    )))?;
                }
            }
            (
                two_item,
                DirFileAddReturn {
                    length: 0,
                    file_count: 0,
                    dir_count: 0,
                },
            )
        } else {
            (
                PackStructItem::new_empty_dir(two_name, PackFileMetadata::new_empty_dir(self.cow)),
                DirFileAddReturn {
                    length: 0,
                    file_count: 0,
                    dir_count: 1,
                },
            )
        };
        //子目录处理
        match &mut two_pack_struct_item.item_type {
            PackStructItemType::Dir {
                struct_file_pos,
                pack_struct,
            } => {
                if let Some(two_pack_struct) = pack_struct {
                    let r = self.create_dir_all_inner(
                        two_pack_struct,
                        &mut path_list,
                        self.cow,
                        two_name.as_ref(),
                    )?;
                    two_r.dir_count += r.dir_count;
                    two_r.file_count += r.file_count;
                    two_r.length += r.length;
                    //

                    let (new_block, pos) = self.save_pack_struct_write(two_pack_struct)?;

                    if new_block {
                        *struct_file_pos = pos;
                    }
                    if let PackFileMetadataRun::Loaded(metadata) =
                        &mut two_pack_struct_item.metadata
                    {
                        metadata.len += r.length;
                        if let PackFileMetadataType::Dir {
                            file_count,
                            dir_count,
                        } = &mut metadata.file_type
                        {
                            *file_count += r.file_count;
                            *dir_count += r.dir_count;
                        }
                        let (new_block, pos) = self.save_metadata_write(metadata)?;

                        if new_block {
                            two_pack_struct_item.metadata_file_pos = pos;
                        }
                    } else {
                        panic!("逻辑错误");
                    }
                    self.manifest
                        .root_struct
                        .items
                        .insert(two_name.clone(), two_pack_struct_item);
                } else {
                    self.manifest
                        .root_struct
                        .items
                        .insert(two_name.clone(), two_pack_struct_item);
                    Err(Error::other(format!(r#"虚拟路径"{two_name}"的结构不存在"#)))?;
                }
            }
            PackStructItemType::File => {
                self.manifest
                    .root_struct
                    .items
                    .insert(two_name.clone(), two_pack_struct_item);
                Err(Error::other(format!(
                    r#"虚拟路径"{two_name}"是文件不是目录"#
                )))?;
            }
        }
        self.manifest.attribute.dir_count += two_r.dir_count;
        self.manifest.attribute.file_count += two_r.file_count;
        self.manifest.attribute.data_len += two_r.length;
        {
            let pack_file = self.pack_file.clone();
            let mut pack_file = pack_file
                .lock()
                .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
            pack_file.run_data.all_cr_file_count += two_r.file_count + two_r.dir_count;
        }
        self.save_root_pack_struct()?;
        self.low_save_all()?;
        Ok(())
    }
}

//核心代码===
impl WBFPManager /* 核心 */ {
    //获取可用的文件位置
    fn get_file_pos(&mut self, length: u64) -> io::Result<(u64, u64)> {
        //块对齐
        const DATA_BLOCK_LEN_U64: u64 = DATA_BLOCK_LEN as u64;
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
        let length = if length.is_multiple_of(DATA_BLOCK_LEN_U64) {
            length
        } else {
            let length = length / DATA_BLOCK_LEN_U64 + 1;
            length * DATA_BLOCK_LEN_U64
        };
        Ok(pack_file.get_file_pos(length))
    }

    //GC===

    //垃圾回收提交
    fn _file_gc_add(&mut self, gc_pos_list: Vec<(u64, u64)>) -> io::Result<()> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
        pack_file.file_gc_add(gc_pos_list);
        Ok(())
    }
    //清单文件垃圾回收提交
    /*fn manifest_file_gc_add(&mut self, gc_pos_list: Vec<(u64, u64)>) {
        if let Some(file) = &mut self.manifest.file {
            file.file_gc_add(gc_pos_list);
        }
    }*/
    //垃圾回收
    fn file_gc(&mut self) -> io::Result<()> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
        pack_file.file_gc();
        drop(pack_file);
        self.save_empty_data_list()
    }

    //清单文件垃圾回收
    fn manifest_file_gc(&mut self) -> io::Result<()> {
        if let Some(file) = &mut self.manifest.file {
            file.file_gc();
            self.save_manifest_empty_data_list()
        } else {
            Ok(())
        }
    }

    //慢保存代码
    fn low_save_all(&mut self) -> io::Result<()> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
        if pack_file.run_data.all_write_len - pack_file.run_data.last_all_write_len
            > (DATA_BLOCK_LEN as u64) * 1024
            || pack_file.run_data.all_cr_file_count - pack_file.run_data.last_all_cr_file_count
            > 10_000
        {
            pack_file.run_data.last_all_write_len = pack_file.run_data.all_write_len;
            pack_file.run_data.last_all_cr_file_count = pack_file.run_data.all_cr_file_count;
            drop(pack_file);
            self.save_all()?;
        }
        Ok(())
    }

    //保存===

    //保存所有数据
    fn save_all(&mut self) -> io::Result<()> {
        self.file_gc()?;
        self.manifest_file_gc()?;
        self.save_root_pack_struct()?;
        self.save_manifest_attribute()?;
        self.save_pack_length()?;
        Ok(())
    }

    //保存空数据位置列表
    fn save_empty_data_list(&mut self) -> io::Result<()> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
        let old_pos = self.manifest.attribute.empty_data_pos_list_pos;
        let old_len = pack_file
            .empty_data_list
            .get_data_block_mut()
            .unwrap()
            .get_this_block_len_u64();
        let (block_data, new_block) = pack_file.empty_data_list.get_block_data().unwrap();
        drop(pack_file);
        let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_len)?;
        if new_block {
            self.manifest.attribute.empty_data_pos_list_pos = pos;
        }
        Ok(())
    }

    //保存清单空数据位置列表
    fn save_manifest_empty_data_list(&mut self) -> io::Result<()> {
        if let Some(file) = &mut self.manifest.file {
            let old_pos = self.manifest.attribute.manifest_empty_data_pos_list_pos;
            let old_len = file
                .empty_data_list
                .get_data_block_mut()
                .unwrap()
                .get_this_block_len_u64();
            let (block_data, new_block) = file.empty_data_list.get_block_data().unwrap();
            let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_len)?;
            if new_block {
                self.manifest.attribute.manifest_empty_data_pos_list_pos = pos;
            }
        }
        Ok(())
    }

    //保存根结构
    fn save_root_pack_struct(&mut self) -> io::Result<()> {
        let old_pos = self.manifest.attribute.root_struct_pos;
        let root_struct = &mut self.manifest.root_struct;
        let old_block_len = root_struct.data_block.get_this_block_len_u64();
        let (block_data, new_block) = root_struct.get_block_data();
        let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_block_len)?;
        self.manifest.attribute.root_struct_pos = pos;
        self.save_manifest_attribute()?;
        Ok(())
    }

    //保存属性
    fn save_manifest_attribute(&mut self) -> io::Result<()> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
        //属性
        let attribute = &mut self.manifest.attribute;
        //转换数据
        let data = attribute.get_block_data().0;
        //写入数据
        //设置文件指针位置,从文件头后面写
        pack_file.set_pos_write(FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX as u64)?;
        //写入数据
        pack_file.write_all(&data)?;
        Ok(())
    }

    //保存数据长度
    fn save_pack_length(&mut self) -> io::Result<()> {
        //上锁
        self.this_write_lock()?;
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
        pack_file.up_len();
        //修改包文件位置
        pack_file.set_pos_write(FILE_HEADER_DATA_LENGTH_INDEX as u64)?;
        //写入数据
        let pack_len = pack_file.len;
        pack_file.write_all(pack_len.to_le_bytes().as_slice())?;
        Ok(())
    }

    //保存结构
    fn save_pack_struct_write(&mut self, pack_struct: &mut PackStruct) -> io::Result<(bool, u64)> {
        let old_pos = pack_struct.data_block.file_pos;
        let old_block_len = pack_struct.data_block.get_this_block_len_u64();
        let (block_data, new_block) = pack_struct.get_block_data();
        let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_block_len)?;
        pack_struct.data_block.file_pos = pos;
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
        if !self.s_manifest_file {
            let pack_file = self.pack_file.clone();
            let pack_file = pack_file
                .lock()
                .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
            pack_file.manifest_data_block_read(file_pos)
        } else if let Some(manifest_file) = &self.manifest.file {
            manifest_file.manifest_data_block_read(file_pos)
        } else {
            Err(Error::other("已启用清单分离文件，但清单文件实例不存在"))?
        }
    }

    fn manifest_data_block_write(
        &mut self,
        block_data: &[u8],
        new_block: bool,
        old_pos: u64,
        old_block_len: u64,
    ) -> io::Result<u64> {
        if self.s_manifest_file {
            if let Some(file) = &mut self.manifest.file {
                file.manifest_data_block_write(block_data, new_block, old_pos, old_block_len)
            } else {
                Err(Error::other("已启用清单分离文件，但清单文件实例不存在"))
            }
        } else {
            let pack_file = self.pack_file.clone();
            let mut pack_file = pack_file
                .lock()
                .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
            pack_file.manifest_data_block_write(block_data, new_block, old_pos, old_block_len)
        }
    }

    //写入锁信息
    fn this_write_lock_info(&self) -> PackLockInfo {
        let run_lock = self.run_data.write_lock;
        let path = &self.run_data.write_lock_path;
        Self::write_lock_info(run_lock, path)
    }

    //设置写入锁
    pub(crate) fn this_write_lock(&mut self) -> io::Result<()> {
        if !self.run_data.write_lock {
            let pack_file = self.pack_file.clone();
            let mut pack_file = pack_file
                .lock()
                .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
            let lock_file = Self::write_lock(true, &self.run_data.write_lock_path)?;
            if let Some(lock_file) = lock_file {
                self.run_data.write_lock_file = Some(lock_file);
            }
            self.run_data.write_lock = true;
            pack_file.lock()?;
        }
        Ok(())
    }

    //解除写入锁
    fn write_unlock(&mut self) -> io::Result<()> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
        //锁文件路径
        let path = &self.run_data.write_lock_path;
        //获取锁信息
        let lock_info = self.this_write_lock_info();
        match lock_info.file_lock_type {
            PackLockType::File => {
                //释放文件句柄
                if let Some(lock_file) = self.run_data.write_lock_file.take() {
                    lock_file.unlock()?;
                    //debug!("释放写入锁");
                    drop(lock_file);
                    fs::remove_file(path)?;
                }
                self.run_data.write_lock = false;
                pack_file.unlock()?;
                Ok(())
            }
            PackLockType::Dir => Err(Error::new(
                ErrorKind::IsADirectory,
                "无法解锁，锁文件类型很可能已被其他程序修改成目录",
            ))?,
            PackLockType::Symlink => Err(Error::other(
                "无法解锁，锁文件类型很可能已被其他程序修改成符号链接",
            ))?,
            PackLockType::_None => Ok(()),
        }
    }
}
impl Drop for WBFPManager {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            //写入索引数据
            self.save_all().expect("保存数据错误");
            //释放写入锁
            self.write_unlock().expect("无法解除写入锁");
        }
    }
}

struct DirFileAddReturn {
    length: u64,
    file_count: u64,
    dir_count: u64,
}

enum PackLockType {
    File,
    Dir,
    Symlink,
    _None,
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
impl WBFPManager {
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
    fn write_lock(run_lock: bool, write_lock_path: &PathBuf) -> io::Result<Option<File>> {
        let lock_info = Self::write_lock_info(run_lock, write_lock_path);
        if lock_info.run_lock {
            //若锁文件不存在就写入
            if let PackLockType::_None = lock_info.file_lock_type {
                Ok(Some(Self::write_lock_file(write_lock_path)?))
            } else {
                Ok(None)
            }
        } else {
            //判断锁文件
            match lock_info.file_lock_pid_run {
                Some(true) => panic!("无法为包文件上写入锁，正在被其他进程持有。"),
                Some(false) => panic!(
                    r#"包文件未正常解锁，但相关进程(pid:{})可能已停止。
                    如果你认为可以继续，可以删除锁文件"{}"强制解锁"#,
                    lock_info.file_lock_pid.expect("pid参数不存在"),
                    write_lock_path.display()
                ),
                None => Ok(Some(Self::write_lock_file(write_lock_path)?)),
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
        info!("已为包文件上写入锁");
        Ok(write_lock)
    }
    //打开
    pub fn open_pack_file<P: AsRef<Path>>(
        pack_path: &P,
        pack_file: Arc<Mutex<PackIO>>,
    ) -> io::Result<WBFPManager> {
        const HEADER_TYPE_LEN: usize = FILE_HEADER_TYPE_NAME.len();
        let m_pack_file_arc = pack_file.clone();
        let mut m_pack_file = m_pack_file_arc
            .lock()
            .map_err(|err| Error::other(format!("获得包文件IO锁错误, err: {err}")))?;
        let pack_path = pack_path
            .as_ref()
            .to_str()
            .ok_or(Error::other("无法将路径转换成文本"))?
            .to_string();

        //读取完整的文件头块
        let mut header_block_data = vec![0; FILE_HEADER_BLOCK_LEN];
        let header_block_r_len = m_pack_file.read(&mut header_block_data)?;
        if header_block_r_len < FILE_HEADER_DATA_LENGTH {
            return Err(Error::other("无法读取完整的文件头"));
        }
        let header = &header_block_data[..FILE_HEADER_DATA_LENGTH];
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
        //let cow = (bool_data >> 7) == 1;
        let s_manifest_file = ((bool_data << 1) >> 7) == 1;
        let pack_len = u64::from_le_bytes(
            header[FILE_HEADER_DATA_LENGTH_INDEX
                ..(FILE_HEADER_DATA_LENGTH_INDEX + FILE_HEADER_DATA_LENGTH_LENGTH)]
                .try_into()
                .unwrap(),
        );
        m_pack_file.len = pack_len;
        //获取清单属性数据
        //获取属性
        let attribute_data =
            &header_block_data[FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX..FILE_HEADER_BLOCK_LEN];
        let attribute_data = ManifestDataBlock::from_block_data_new(
            attribute_data.to_vec(),
            FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX as u64,
        )
            .expect("无法解析数据块");
        let attribute = Attribute::load(attribute_data)?;
        //锁文件
        let mut write_lock_file_path = pack_path.clone();
        write_lock_file_path.push_str(".lock");
        let write_lock_file = Self::write_lock_file(&PathBuf::from(write_lock_file_path))?;
        //如果分离数据文件
        if s_manifest_file {
            Self::open_pack_file_s_manifest_file(
                pack_file,
                &mut m_pack_file,
                &pack_path,
                s_manifest_file,
                attribute,
                write_lock_file,
            )?
        } else {
            //空数据列表===
            let empty_pos_data_block =
                m_pack_file.manifest_data_block_read(attribute.empty_data_pos_list_pos)?;
            let empty_pos_data = empty_pos_data_block
                .get_this_data()
                .expect("读取空数据列表失败")
                .to_vec();
            m_pack_file.empty_data_list =
                DataPosList::load(&empty_pos_data, Some(empty_pos_data_block));
            //加载根结构
            let root_struct_block_data =
                m_pack_file.manifest_data_block_read(attribute.root_struct_pos)?;
            let root_struct = PackStruct::load(root_struct_block_data)?;
            let manifest = WBFilesPackManifest {
                attribute,
                root_struct,
                file: None,
                _run_data: WBFilesPackManifestRun::default(),
            };
            drop(m_pack_file);
            drop(m_pack_file_arc);
            Ok(WBFPManager::new(
                pack_path,
                manifest,
                pack_file,
                s_manifest_file,
                Some(write_lock_file),
            ))
        }
    }

    fn open_pack_file_s_manifest_file(
        pack_file: Arc<Mutex<PackIO>>,
        m_pack_file: &mut MutexGuard<PackIO>,
        pack_path: &String,
        s_manifest_file: bool,
        attribute: Attribute,
        write_lock_file: File,
    ) -> Result<Result<WBFPManager, Error>, Error> {
        //尝试加载分离数据文件
        let mut manifest_path = pack_path.clone();
        manifest_path.push_str(".wbm");
        let manifest_file = File::options().read(true).write(true).open(manifest_path)?;
        let mut manifest_file = PackIO::new2(manifest_file, attribute.manifest_file_len);
        //空数据列表===
        let empty_pos_data_block =
            manifest_file.manifest_data_block_read(attribute.empty_data_pos_list_pos)?;
        let empty_pos_data = empty_pos_data_block
            .get_this_data()
            .expect("读取空数据列表失败")
            .to_vec();
        m_pack_file.empty_data_list =
            DataPosList::load(&empty_pos_data, Some(empty_pos_data_block));
        //清单文件空数据
        let manifest_empty_pos_data_block =
            manifest_file.manifest_data_block_read(attribute.manifest_empty_data_pos_list_pos)?;
        let manifest_empty_pos_data = manifest_empty_pos_data_block
            .get_this_data()
            .expect("读取清单空数据列表失败")
            .to_vec();
        manifest_file.empty_data_list = DataPosList::load(
            &manifest_empty_pos_data,
            Some(manifest_empty_pos_data_block),
        );
        //加载根结构
        let root_struct_block_data =
            manifest_file.manifest_data_block_read(attribute.root_struct_pos)?;
        let root_struct = PackStruct::load(root_struct_block_data)?;
        let manifest = WBFilesPackManifest {
            attribute,
            root_struct,
            file: Some(manifest_file),
            _run_data: WBFilesPackManifestRun::default(),
        };
        Ok(Ok(WBFPManager::new(
            pack_path,
            manifest,
            pack_file,
            s_manifest_file,
            Some(write_lock_file),
        )))
    }
    //创建===

    //创建新包文件,
    pub(crate) fn create_pack_file<P: AsRef<Path>>(
        path: &P,
        pack_file: Arc<Mutex<PackIO>>,
        cow: bool,
        s_manifest_file: bool,
        create_new: bool,
    ) -> io::Result<WBFPManager> {
        let mut write_lock_path = path
            .as_ref()
            .to_str()
            .expect("无法将路径转换成文本")
            .to_string();
        write_lock_path.push_str(".lock");
        let write_lock_path = PathBuf::from(write_lock_path);
        let write_lock_file = Self::write_lock(false, &write_lock_path)?;

        let manifest_file = if s_manifest_file {
            let mut manifest_path =
                String::from(path.as_ref().to_str().expect("无法将路径转换成文件"));
            manifest_path.push_str(".wbm");
            let manifest_pack_file = File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .create_new(create_new)
                .open(manifest_path)?;
            Some(PackIO::new(manifest_pack_file))
        } else {
            None
        };

        Ok(Self::create_pack(
            path,
            cow,
            pack_file,
            s_manifest_file,
            manifest_file,
            write_lock_file,
        ))
    }

    //创建新包实例===

    fn create_pack<P: AsRef<Path>>(
        pack_path: &P,
        cow: bool,
        pack_file: Arc<Mutex<PackIO>>,
        s_manifest_file: bool,
        manifest_file: Option<PackIO>,
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
                file: manifest_file,
                _run_data: WBFilesPackManifestRun::default(),
            },
            pack_file,
            s_manifest_file,
            write_lock_file,
        )
    }
}
