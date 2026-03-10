/*
开始时间：26/2/11 15：51
 */
pub mod manager;

#[cfg(test)]
mod test;

use crate::wb_files_pack::manager::WBFPManager;
use crate::wb_files_pack::manager::file::PackFileWR;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::{Error, ErrorKind};

//当前解析器版本
pub const MANIFEST_VERSION: u16 = 10;

//当前解析器兼任版本
pub const MANIFEST_VERSION_COMPATIBLE: u16 = 10;
//数据格式
//已分配数据列表项
//位置
const DATA_POS_LIST_ITEM_POS_LEN: usize = 8;
//长度
const DATA_POS_LIST_ITEM_LEN_LEN: usize = 8;
//总大小
const DATA_POS_LIST_ITEM_LEN: usize = DATA_POS_LIST_ITEM_POS_LEN + DATA_POS_LIST_ITEM_LEN_LEN;
//数据块大小
const DATA_BLOCK_LEN: usize = 4096;

#[derive(Debug)]
pub struct WBFilesPackManifest {
    //属性
    attribute: Attribute,
    //根结构
    root_struct: PackStruct,
    //清单文件实例
    file: Option<File>,
    //空数据列表
    empty_data_list: DataPosList,
    //清单空数据列表
    this_empty_data_list: Option<DataPosList>,
    //运行时数据
    run_data: WBFilesPackManifestRun,
} //包文件数据

impl WBFilesPackManifest {
    pub fn attribute(&self) -> &Attribute {
        &self.attribute
    }

    pub fn root_struct(&self) -> &PackStruct {
        &self.root_struct
    }
}

//清单数据运行数据
#[derive(Debug)]
struct WBFilesPackManifestRun {
    //清单文件位置
    file_pos: u64,
    //GC数据列表
    gc_data_pos_list: DataPosList,
}

//格式版本
const MANIFEST_ATTRIBUTE_VERSION_LEN: usize = 2;
//格式兼容版本
const MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_INDEX: usize = MANIFEST_ATTRIBUTE_VERSION_LEN;
const MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_LEN: usize = 2;
//布尔数据
const MANIFEST_ATTRIBUTE_BOOL_DATA_INDEX: usize =
    MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_INDEX + MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_LEN;
const MANIFEST_ATTRIBUTE_BOOL_DATA_LEN: usize = 1;
//空数据列表的文件指针位置
const MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_INDEX: usize =
    MANIFEST_ATTRIBUTE_BOOL_DATA_INDEX + MANIFEST_ATTRIBUTE_BOOL_DATA_LEN;
const MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_LEN: usize = 8;
//清单..
const MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_INDEX: usize =
    MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_INDEX + MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_LEN;
const MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_LEN: usize = 8;
//清单文件大小
const MANIFEST_ATTRIBUTE_MANIFEST_FILE_LEN_INDEX: usize =
    MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_INDEX
        + MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_LEN;
const MANIFEST_ATTRIBUTE_MANIFEST_FILE_LEN_LEN: usize = 8;
//根结构的文件指针位置
const MANIFEST_ATTRIBUTE_ROOT_STRUCT_POS_INDEX: usize =
    MANIFEST_ATTRIBUTE_MANIFEST_FILE_LEN_INDEX + MANIFEST_ATTRIBUTE_MANIFEST_FILE_LEN_LEN;
const MANIFEST_ATTRIBUTE_ROOT_STRUCT_POS_LEN: usize = 8;
//所有文件数
const MANIFEST_ATTRIBUTE_FILE_COUNT_INDEX: usize =
    MANIFEST_ATTRIBUTE_ROOT_STRUCT_POS_INDEX + MANIFEST_ATTRIBUTE_ROOT_STRUCT_POS_LEN;
const MANIFEST_ATTRIBUTE_FILE_COUNT_LEN: usize = 8;
//所有目录数
const MANIFEST_ATTRIBUTE_DIR_COUNT_INDEX: usize =
    MANIFEST_ATTRIBUTE_FILE_COUNT_INDEX + MANIFEST_ATTRIBUTE_FILE_COUNT_LEN;
const MANIFEST_ATTRIBUTE_DIR_COUNT_LEN: usize = 8;
//数据大小
const MANIFEST_ATTRIBUTE_DATA_LEN_INDEX: usize =
    MANIFEST_ATTRIBUTE_DIR_COUNT_INDEX + MANIFEST_ATTRIBUTE_DIR_COUNT_LEN;
const MANIFEST_ATTRIBUTE_DATA_LEN_LEN: usize = 8;

const MANIFEST_ATTRIBUTE_LEN: usize = MANIFEST_ATTRIBUTE_VERSION_LEN
    + MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_LEN
    + MANIFEST_ATTRIBUTE_BOOL_DATA_LEN
    + MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_LEN
    + MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_LEN
    + MANIFEST_ATTRIBUTE_MANIFEST_FILE_LEN_LEN
    + MANIFEST_ATTRIBUTE_ROOT_STRUCT_POS_LEN
    + MANIFEST_ATTRIBUTE_FILE_COUNT_LEN
    + MANIFEST_ATTRIBUTE_DIR_COUNT_LEN
    + MANIFEST_ATTRIBUTE_DATA_LEN_LEN;
#[derive(Debug, PartialEq)]
pub struct Attribute {
    //格式版本
    version: u16,
    //格式兼容版本
    version_compatible: u16,
    //写时复制
    cow: bool,
    //文件数，不含目录
    file_count: u64,
    //目录数
    dir_count: u64,
    //数据大小
    data_len: u64,
    //空数据列表的文件指针位置
    empty_data_pos_list_pos: u64,
    //清单..
    manifest_empty_data_pos_list_pos: u64,
    //清单文件大小
    manifest_file_len: u64,
    // 根结构位置
    root_struct_pos: u64,
} //包文件属性
impl Default for Attribute {
    fn default() -> Self {
        Self {
            version: MANIFEST_VERSION,
            version_compatible: MANIFEST_VERSION_COMPATIBLE,
            cow: manager::DEFAULT_COW,
            file_count: 0,
            dir_count: 0,
            data_len: 0,
            empty_data_pos_list_pos: 0,
            manifest_empty_data_pos_list_pos: 0,
            manifest_file_len: 0,
            root_struct_pos: 0,
        }
    }
}
impl Attribute {
    pub fn version(&self) -> u16 {
        self.version
    }

    pub fn version_compatible(&self) -> u16 {
        self.version_compatible
    }

    pub fn cow(&self) -> bool {
        self.cow
    }

    pub fn file_count(&self) -> u64 {
        self.file_count
    }

    pub fn dir_count(&self) -> u64 {
        self.dir_count
    }

    pub fn data_len(&self) -> u64 {
        self.data_len
    }

    fn load(data: &[u8]) -> io::Result<Self> {
        //大小检查
        if data.len() < MANIFEST_ATTRIBUTE_LEN {
            Err(Error::other("提供的数据大小不够"))
        } else {
            //格式版本
            let version =
                u16::from_le_bytes(data[..MANIFEST_ATTRIBUTE_VERSION_LEN].try_into().unwrap());
            //格式兼容版本
            let version_compatible = u16::from_le_bytes(
                data[MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_INDEX
                    ..MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_INDEX
                        + MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_LEN]
                    .try_into()
                    .unwrap(),
            );
            //版本兼容性判断
            if version != MANIFEST_VERSION {
                if version < MANIFEST_VERSION_COMPATIBLE {
                    //版本过低
                    Err(Error::other("版本过低，无法解析"))?;
                } else if version_compatible > MANIFEST_VERSION {
                    //版本过高
                    Err(Error::other("版本过高，无法解析"))?;
                }
            }
            //布尔数据
            let bool_data = &data[MANIFEST_ATTRIBUTE_BOOL_DATA_INDEX];
            let cow = (bool_data >> 7) == 1;
            //空数据列表文件指针位置
            let empty_data_pos_list_pos = u64::from_le_bytes(
                data[MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_INDEX
                    ..MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_INDEX
                        + MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_LEN]
                    .try_into()
                    .unwrap(),
            );
            //清单空数据列表文件指针位置
            let manifest_empty_data_pos_list_pos = u64::from_le_bytes(
                data[MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_INDEX
                    ..MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_INDEX
                        + MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_LEN]
                    .try_into()
                    .unwrap(),
            );
            //清单文件大小
            let manifest_file_len = u64::from_le_bytes(
                data[MANIFEST_ATTRIBUTE_MANIFEST_FILE_LEN_INDEX
                    ..MANIFEST_ATTRIBUTE_MANIFEST_FILE_LEN_INDEX
                        + MANIFEST_ATTRIBUTE_MANIFEST_FILE_LEN_LEN]
                    .try_into()
                    .unwrap(),
            );
            //根结构文件指针位置
            let root_struct_pos = u64::from_le_bytes(
                data[MANIFEST_ATTRIBUTE_ROOT_STRUCT_POS_INDEX
                    ..MANIFEST_ATTRIBUTE_ROOT_STRUCT_POS_INDEX
                        + MANIFEST_ATTRIBUTE_ROOT_STRUCT_POS_LEN]
                    .try_into()
                    .unwrap(),
            );
            //所有文件数
            let file_count = u64::from_le_bytes(
                data[MANIFEST_ATTRIBUTE_FILE_COUNT_INDEX
                    ..MANIFEST_ATTRIBUTE_FILE_COUNT_INDEX + MANIFEST_ATTRIBUTE_FILE_COUNT_LEN]
                    .try_into()
                    .unwrap(),
            );
            //所有目录数
            let dir_count = u64::from_le_bytes(
                data[MANIFEST_ATTRIBUTE_DIR_COUNT_INDEX
                    ..MANIFEST_ATTRIBUTE_DIR_COUNT_INDEX + MANIFEST_ATTRIBUTE_DIR_COUNT_LEN]
                    .try_into()
                    .unwrap(),
            );
            //数据大小
            let data_len = u64::from_le_bytes(
                data[MANIFEST_ATTRIBUTE_DATA_LEN_INDEX
                    ..MANIFEST_ATTRIBUTE_DATA_LEN_INDEX + MANIFEST_ATTRIBUTE_DATA_LEN_LEN]
                    .try_into()
                    .unwrap(),
            );
            Ok(Self {
                version,
                version_compatible,
                cow,
                empty_data_pos_list_pos,
                manifest_empty_data_pos_list_pos,
                manifest_file_len,
                root_struct_pos,
                file_count,
                dir_count,
                data_len,
            })
        }
    }

    fn to_bytes_vec(&self) -> Vec<u8> {
        let mut data = Vec::new();
        //格式版本
        for to_le_byte in self.version.to_le_bytes() {
            data.push(to_le_byte);
        }
        //格式兼容版本
        for to_le_byte in self.version_compatible.to_le_bytes() {
            data.push(to_le_byte);
        }
        //布尔数据
        let mut bool_data = 0;
        //写时复制
        if self.cow {
            bool_data |= 0b1000_0000;
        }
        data.push(bool_data);
        //空数据列表的文件指针位置
        for to_le_byte in self.empty_data_pos_list_pos.to_le_bytes() {
            data.push(to_le_byte);
        }
        //清单空数据列表的文件指针位置
        for to_le_byte in self.manifest_empty_data_pos_list_pos.to_le_bytes() {
            data.push(to_le_byte);
        }
        //清单文件大小
        for to_le_byte in self.manifest_file_len.to_le_bytes() {
            data.push(to_le_byte);
        }
        //根结构的文件指针位置
        for to_le_byte in self.root_struct_pos.to_le_bytes() {
            data.push(to_le_byte);
        }
        //所有文件数
        for to_le_byte in self.file_count.to_le_bytes() {
            data.push(to_le_byte);
        }
        //所有目录数
        for to_le_byte in self.dir_count.to_le_bytes() {
            data.push(to_le_byte);
        }
        //数据大小
        for to_le_byte in self.data_len.to_le_bytes() {
            data.push(to_le_byte);
        }
        data
    }
}

const DATA_POS_LIST_COUNT_LEN: usize = 8;

#[derive(Debug, Default, PartialEq, Clone)]
struct DataPosList {
    data_block: Option<ManifestDataBlock>,
    list: Vec<(u64, u64)>,
}
impl DataPosList {
    fn get_data_block_mut(&mut self) -> Option<&mut ManifestDataBlock> {
        if let Some(v) = &mut self.data_block {
            Some(v)
        } else {
            None
        }
    }

    fn list(&self) -> &Vec<(u64, u64)> {
        &self.list
    }

    fn load(data: &[u8], data_block: Option<ManifestDataBlock>) -> Self {
        //数量
        let count = usize::from_le_bytes(data[..DATA_POS_LIST_COUNT_LEN].try_into().unwrap());
        let data_len = count * DATA_POS_LIST_ITEM_LEN + DATA_POS_LIST_COUNT_LEN;
        let data_pos_list_data = &data[DATA_POS_LIST_COUNT_LEN..data_len];
        let mut list = Vec::with_capacity(count);
        while list.len() < count {
            let index = list.len();
            let pos = u64::from_le_bytes(
                data_pos_list_data[index * DATA_POS_LIST_ITEM_LEN
                    ..index * DATA_POS_LIST_ITEM_LEN + DATA_POS_LIST_ITEM_POS_LEN]
                    .try_into()
                    .unwrap(),
            );
            let len = u64::from_le_bytes(
                data_pos_list_data[index * DATA_POS_LIST_ITEM_LEN + DATA_POS_LIST_ITEM_POS_LEN
                    ..index * DATA_POS_LIST_ITEM_LEN + DATA_POS_LIST_ITEM_LEN]
                    .try_into()
                    .unwrap(),
            );
            list.push((pos, len));
        }
        Self { data_block, list }
    }

    fn to_bytes_vec(&self) -> Vec<u8> {
        self.to_bytes_vec2(None)
    }

    fn to_bytes_vec2(&self, this_gc_pos: Option<(u64, u64)>) -> Vec<u8> {
        let mut data = Vec::new();
        let list_count = match this_gc_pos {
            Some(_) => self.list.len() + 1,
            None => self.list.len(),
        };
        for to_le_byte in (list_count as u64).to_le_bytes() {
            data.push(to_le_byte);
        }
        for (pos, len) in &self.list {
            for to_le_byte in pos.to_le_bytes() {
                data.push(to_le_byte);
            }
            for to_le_byte in len.to_le_bytes() {
                data.push(to_le_byte);
            }
        }
        //附加自身GC
        if let Some((pos, len)) = this_gc_pos {
            for to_le_byte in pos.to_le_bytes() {
                data.push(to_le_byte);
            }
            for to_le_byte in len.to_le_bytes() {
                data.push(to_le_byte);
            }
        }
        data
    }

    fn get_block_data(&mut self) -> Option<(Vec<u8>, bool)> {
        let up_data = self.to_bytes_vec();
        if let Some(data_block) = &mut self.data_block {
            let new_block = data_block.update(&up_data);
            Some((data_block.get_block_data().to_vec(), new_block))
        } else {
            None
        }
    }
}

#[derive(Default, Debug, PartialEq, Clone)]
pub struct PackStruct {
    //结构项列表
    items: HashMap<String, PackStructItem>,
    //运行时数据
    run_data: PackStructRun,
} //包结构
#[derive(Default, Debug, PartialEq, Clone)]
struct PackStructRun {
    //数据块
    data_block: ManifestDataBlock,
}
impl PackStructRun {
    fn get_data_block(&mut self) -> &mut ManifestDataBlock {
        &mut self.data_block
    }
}
impl PackStruct {
    fn l_clone(&self) -> Self {
        let mut l_clone_items = HashMap::new();
        for (item_name, item) in &self.items {
            l_clone_items.insert(item_name.clone(), item.l_clone());
        }
        Self {
            items: l_clone_items,
            run_data: self.run_data.clone(),
        }
    }

    pub fn items(&self) -> &HashMap<String, PackStructItem> {
        &self.items
    }

    fn load(data_block: ManifestDataBlock) -> io::Result<Self> {
        let data = data_block.get_this_data();
        let data_len = data.len();

        //已读取大小
        let mut read_len = 0;
        let all_read_len = data_len;
        let mut items = HashMap::new();
        while read_len < all_read_len {
            let item_data_len = usize::from_le_bytes(
                data[read_len..read_len + PACK_STRUCT_ITEM_LEN_LEN]
                    .try_into()
                    .unwrap(),
            );
            let item = PackStructItem::load(&data[read_len..read_len + item_data_len])?;
            items.insert(item.name.clone(), item);
            read_len += item_data_len;
        }
        Ok(Self {
            items,
            run_data: PackStructRun { data_block },
        })
    }

    fn to_bytes_vec(&self) -> Vec<u8> {
        let mut data = Vec::new();
        let mut items_data = Vec::new();
        //获取项数据
        for item in &self.items {
            let item = item.1;
            let item_vec = item.to_bytes_vec();
            for item_data in item_vec {
                items_data.push(item_data);
            }
        }
        //结构项
        for items_datum in items_data {
            data.push(items_datum);
        }
        data
    }

    fn get_block_data(&mut self) -> (Vec<u8>, bool) {
        let update = self.to_bytes_vec();
        let data_block = &mut self.run_data.data_block;
        let new_block = data_block.update(&update);
        (data_block.get_block_data().to_vec(), new_block)
    }
}
//长度
const PACK_STRUCT_ITEM_LEN_LEN: usize = 8;
//类型
const PACK_STRUCT_ITEM_TYPE_INDEX: usize = PACK_STRUCT_ITEM_LEN_LEN;
const PACK_STRUCT_ITEM_TYPE_LEN: usize = 1;
//名称长度
const PACK_STRUCT_ITEM_NAME_LEN_INDEX: usize =
    PACK_STRUCT_ITEM_TYPE_INDEX + PACK_STRUCT_ITEM_TYPE_LEN;
const PACK_STRUCT_ITEM_NAME_LEN_LEN: usize = 2;
//名称
const PACK_STRUCT_ITEM_NAME_INDEX: usize =
    PACK_STRUCT_ITEM_NAME_LEN_INDEX + PACK_STRUCT_ITEM_NAME_LEN_LEN;
//虚拟文件元数据的文件指针位置
const PACK_STRUCT_ITEM_METADATA_FILE_POS_LEN: usize = 8;

#[derive(Debug, PartialEq, Clone)]
pub struct PackStructItem {
    //名称
    name: String,
    //元数据文件指针位置
    metadata_file_pos: u64,
    //结构项类型
    item_type: PackStructItemType,
    //元数据
    pack_file_metadata: Option<PackFileMetadata>,
} //包结构项
impl PackStructItem {
    fn new_empty_dir(name: &str, pack_file_metadata: Option<PackFileMetadata>) -> Self {
        Self {
            name: name.to_string(),
            metadata_file_pos: 0,
            item_type: PackStructItemType::Dir(PackStructItemDir {
                struct_file_pos: 0,
                pack_struct: Some(PackStruct::default()),
            }),
            pack_file_metadata,
        }
    }

    fn get_empty() -> Self {
        Self {
            name: String::new(),
            metadata_file_pos: 0,
            item_type: PackStructItemType::File,
            pack_file_metadata: None,
        }
    }

    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn item_type(&self) -> &PackStructItemType {
        &self.item_type
    }

    pub fn metadata(&self) -> &Option<PackFileMetadata> {
        &self.pack_file_metadata
    }

    fn l_clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            metadata_file_pos: self.metadata_file_pos,
            item_type: self.item_type.l_clone(),
            pack_file_metadata: None,
        }
    }

    fn load(data: &[u8]) -> io::Result<Self> {
        //类型
        let type_value = &data[PACK_STRUCT_ITEM_TYPE_INDEX];
        //名称长度
        let name_len = u16::from_le_bytes(
            data[PACK_STRUCT_ITEM_NAME_LEN_INDEX
                ..PACK_STRUCT_ITEM_NAME_LEN_INDEX + PACK_STRUCT_ITEM_NAME_LEN_LEN]
                .try_into()
                .unwrap(),
        );
        let name_end_pos = PACK_STRUCT_ITEM_NAME_INDEX + (name_len as usize);
        //名称
        let name =
            String::from_utf8(data[PACK_STRUCT_ITEM_NAME_INDEX..name_end_pos].to_vec()).unwrap();
        let metadata_file_pos = u64::from_le_bytes(
            data[name_end_pos..name_end_pos + PACK_STRUCT_ITEM_METADATA_FILE_POS_LEN]
                .try_into()
                .unwrap(),
        );
        let type_data_start_pos = name_end_pos + PACK_STRUCT_ITEM_METADATA_FILE_POS_LEN;

        let item_type = match type_value {
            0 => PackStructItemType::File,
            1 => PackStructItemType::Dir(PackStructItemDir::load(&data[type_data_start_pos..])),
            _ => Err(Error::other("未知类型"))?,
        };
        Ok(Self {
            name,
            metadata_file_pos,
            item_type,
            pack_file_metadata: None,
        })
    }

    fn to_bytes_vec(&self) -> Vec<u8> {
        let mut data = Vec::new();
        //名称
        let name_vec = self.name.as_bytes().to_vec();
        //名称长度
        let name_len = name_vec.len();
        //类型数据
        let type_data = match &self.item_type {
            PackStructItemType::File => (0, Vec::new()),
            PackStructItemType::Dir(dir) => (1, dir.to_bytes_vec()),
        };
        //长度
        let data_len = PACK_STRUCT_ITEM_LEN_LEN
            + PACK_STRUCT_ITEM_TYPE_LEN
            + PACK_STRUCT_ITEM_NAME_LEN_LEN
            + name_len
            + PACK_STRUCT_ITEM_METADATA_FILE_POS_LEN
            + type_data.1.len();
        for to_le_byte in data_len.to_le_bytes() {
            data.push(to_le_byte);
        }
        //类型
        data.push(type_data.0);
        //名称长度
        for to_le_byte in u16::try_from(name_len).unwrap().to_le_bytes() {
            data.push(to_le_byte);
        }
        //名称
        for name_b in name_vec {
            data.push(name_b);
        }
        //虚拟文件元数据的文件指针位置
        for to_le_byte in self.metadata_file_pos.to_le_bytes() {
            data.push(to_le_byte);
        }
        //类型特有数据
        for type_datum in type_data.1 {
            data.push(type_datum);
        }
        data
    }
}

#[derive(Default, Debug, PartialEq, Clone)]
pub enum PackStructItemType {
    #[default]
    File,
    Dir(PackStructItemDir),
} //结构项类型
impl PackStructItemType {
    fn l_clone(&self) -> Self {
        match self {
            PackStructItemType::File => PackStructItemType::File,
            PackStructItemType::Dir(dir) => PackStructItemType::Dir(dir.l_clone()),
        }
    }
}

const PACK_STRUCT_DIR_STRUCT_FILE_POS_LEN: usize = 8;

#[derive(Default, Debug, PartialEq, Clone)]
pub struct PackStructItemDir {
    //结构的文件指针位置
    struct_file_pos: u64,
    //运行时缓存结构
    pack_struct: Option<PackStruct>,
} //结构项目录类型特有数据
impl PackStructItemDir {
    fn l_clone(&self) -> Self {
        Self {
            struct_file_pos: self.struct_file_pos,
            pack_struct: None,
        }
    }

    pub fn pack_struct(&self) -> &Option<PackStruct> {
        &self.pack_struct
    }
    fn load(data: &[u8]) -> Self {
        Self {
            struct_file_pos: u64::from_le_bytes(
                data[..PACK_STRUCT_DIR_STRUCT_FILE_POS_LEN]
                    .try_into()
                    .unwrap(),
            ),
            pack_struct: None,
        }
    }

    fn to_bytes_vec(&self) -> Vec<u8> {
        let mut data = Vec::new();
        //结构文件指针位置
        for to_le_byte in self.struct_file_pos.to_le_bytes() {
            data.push(to_le_byte);
        }
        data
    }
}

//数据格式常量
//数据长度

const PACK_FILE_METADATA_TYPE_LEN: usize = 1;
const PACK_FILE_METADATA_BOOL_DATA_INDEX: usize = PACK_FILE_METADATA_TYPE_LEN;
//布尔数据
const PACK_FILE_METADATA_BOOL_DATA_LEN: usize = 1;
const PACK_FILE_METADATA_LEN_INDEX: usize =
    PACK_FILE_METADATA_BOOL_DATA_INDEX + PACK_FILE_METADATA_BOOL_DATA_LEN;
//大小
const PACK_FILE_METADATA_LEN_LEN: usize = 8;
const PACK_FILE_METADATA_MODIFIED_INDEX: usize =
    PACK_FILE_METADATA_LEN_INDEX + PACK_FILE_METADATA_LEN_LEN;
//修改时间
const PACK_FILE_METADATA_MODIFIED_LEM: usize = 16;
//类型数据
const PACK_FILE_METADATA_TYPE_DATA_INDEX: usize =
    PACK_FILE_METADATA_MODIFIED_INDEX + PACK_FILE_METADATA_MODIFIED_LEM;

#[derive(PartialEq, Debug, Clone)]
pub struct PackFileMetadata {
    //数据块
    data_block: ManifestDataBlock,
    //写时复制
    cow: bool,
    //长度
    len: u64,
    //修改时间
    modified: u128,
    //文件类型
    file_type: PackFileMetadataType,
} //包文件元数据
impl PackFileMetadata {
    fn new_empty_dir(cow: bool) -> Self {
        Self {
            data_block: ManifestDataBlock::default(),
            cow,
            len: 0,
            modified: 0,
            file_type: PackFileMetadataType::Dir(PackFileMetadataDir::default()),
        }
    }

    fn get_empty() -> Self {
        Self {
            data_block: ManifestDataBlock::default(),
            cow: false,
            len: 0,
            modified: 0,
            file_type: PackFileMetadataType::Dir(PackFileMetadataDir::default()),
        }
    }

    pub fn cow(&self) -> bool {
        self.cow
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn modified(&self) -> u128 {
        self.modified
    }

    pub fn file_type(&self) -> &PackFileMetadataType {
        &self.file_type
    }

    pub fn get_rw<'a>(&self, wbfp_manager: &'a mut WBFPManager) -> io::Result<PackFileWR<'a>> {
        if let PackFileMetadataType::File(file) = &self.file_type {
            Ok(PackFileWR::new(wbfp_manager, file.data_pos_list.clone()))
        } else {
            Err(Error::new(ErrorKind::IsADirectory, "类型不是文件"))
        }
    }

    fn load(data_block: ManifestDataBlock) -> io::Result<Self> {
        let data = data_block.get_this_data();
        //类型
        let this_type = data[0];
        //布尔值
        let bool_data = data[PACK_FILE_METADATA_BOOL_DATA_INDEX];
        //写时复制
        let cow = (bool_data >> 7) == 1;
        //长度
        let len = u64::from_le_bytes(
            data[PACK_FILE_METADATA_LEN_INDEX..PACK_FILE_METADATA_MODIFIED_INDEX]
                .try_into()
                .unwrap(),
        );
        //修改时间
        let modified = u128::from_le_bytes(
            data[PACK_FILE_METADATA_MODIFIED_INDEX..PACK_FILE_METADATA_TYPE_DATA_INDEX]
                .try_into()
                .unwrap(),
        );
        let type_data = &data[PACK_FILE_METADATA_TYPE_DATA_INDEX..];
        let file_type = match this_type {
            0 => PackFileMetadataType::File(PackFileMetadataFile::load(type_data)),
            1 => PackFileMetadataType::Dir(PackFileMetadataDir::load(type_data)?),
            _ => Err(Error::other("未知类型"))?,
        };

        Ok(Self {
            data_block,
            cow,
            len,
            modified,
            file_type,
        })
    }

    fn to_bytes_vec(&self) -> Vec<u8> {
        let mut data = Vec::new();
        //类型及其数据
        let type_data = match &self.file_type {
            PackFileMetadataType::File(file) => (0, file.to_bytes_vec()),
            PackFileMetadataType::Dir(dir) => (1, dir.to_bytes_vec()),
        };
        //类型
        data.push(type_data.0);
        //布尔数据
        let mut bool_data = 0;
        if self.cow {
            bool_data |= 0b1000_0000;
        }
        data.push(bool_data);
        //长度
        for to_le_byte in self.len.to_le_bytes() {
            data.push(to_le_byte);
        }
        //修改时间
        for to_le_byte in self.modified.to_le_bytes() {
            data.push(to_le_byte);
        }
        //类型数据
        for type_datum in type_data.1 {
            data.push(type_datum);
        }
        //更新大小
        data
    }

    fn get_block_data(&mut self) -> (Vec<u8>, bool) {
        let up_data = self.to_bytes_vec();
        let data_block = &mut self.data_block;
        let new_block = data_block.update(&up_data);
        (data_block.get_block_data().to_vec(), new_block)
    }
}

#[derive(PartialEq, Debug, Clone)]
pub enum PackFileMetadataType {
    File(PackFileMetadataFile),
    Dir(PackFileMetadataDir),
} //包文件元数据类型

//数据格式常量
//哈希算法值类型
const PACK_METADATA_FILE_HASH_TYPE_INDEX: usize = 0;
const PACK_METADATA_FILE_HASH_TYPE_LEN: usize = 1;
//哈希值长度
const PACK_METADATA_FILE_HASH_LEN_INDEX: usize = 1;
const PACK_METADATA_FILE_HASH_LEN_LEN: usize = 1;
//哈希值
const PACK_METADATA_FILE_HASH_INDEX: usize =
    PACK_METADATA_FILE_HASH_TYPE_LEN + PACK_METADATA_FILE_HASH_LEN_LEN;

#[derive(Default, PartialEq, Debug, Clone)]
pub struct PackFileMetadataFile {
    //哈希算法类型
    hash_type: u8,
    //哈希值
    hash: Vec<u8>,
    //已分配数据集合
    data_pos_list: DataPosList,
} //包文件元数据文件
impl PackFileMetadataFile {
    fn load(data: &[u8]) -> Self {
        let hash_type = data[PACK_METADATA_FILE_HASH_TYPE_INDEX];
        let hash_len = data[PACK_METADATA_FILE_HASH_LEN_INDEX];
        let hash = data
            [PACK_METADATA_FILE_HASH_INDEX..PACK_METADATA_FILE_HASH_INDEX + (hash_len as usize)]
            .to_vec();
        let data_pos_list_count_index = PACK_METADATA_FILE_HASH_INDEX + (hash_len as usize);
        let data_pos_list = DataPosList::load(&data[data_pos_list_count_index..], None);
        Self {
            hash_type,
            hash,
            data_pos_list,
        }
    }
    fn to_bytes_vec(&self) -> Vec<u8> {
        let mut data = Vec::new();
        //哈希算法类型
        data.push(self.hash_type);
        //哈希值长度
        let hash_len = u8::try_from(self.hash.len()).expect("哈希值长度值过大");
        data.push(hash_len);
        //哈希
        for hash in &self.hash {
            data.push(*hash);
        }
        //已分配数据列表
        for data_pos_list_data in self.data_pos_list.to_bytes_vec() {
            data.push(data_pos_list_data);
        }
        data
    }
}

//数据格式字段
//文件数量
const PACK_METADATA_DIR_FILE_COUNT_LEN: usize = 8;
//目录数量
const PACK_METADATA_DIR_DIR_COUNT_INDEX: usize = PACK_METADATA_DIR_FILE_COUNT_LEN;
const PACK_METADATA_DIR_DIR_COUNT_LEN: usize = 8;

#[derive(Default, PartialEq, Debug, Clone)]
pub struct PackFileMetadataDir {
    file_count: u64,
    dir_count: u64,
} //包文件目录数量
impl PackFileMetadataDir {
    pub fn file_count(&self) -> u64 {
        self.file_count
    }

    pub fn dir_count(&self) -> u64 {
        self.dir_count
    }

    fn load(data: &[u8]) -> io::Result<Self> {
        let file_count = u64::from_le_bytes(
            data[..PACK_METADATA_DIR_DIR_COUNT_INDEX]
                .try_into()
                .unwrap(),
        );
        let dir_count = u64::from_le_bytes(
            data[PACK_METADATA_DIR_DIR_COUNT_INDEX
                ..PACK_METADATA_DIR_DIR_COUNT_INDEX + PACK_METADATA_DIR_DIR_COUNT_LEN]
                .try_into()
                .unwrap(),
        );

        Ok(Self {
            file_count,
            dir_count,
        })
    }
    fn to_bytes_vec(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(8 + 8);
        //file_count
        for to_le_byte in self.file_count.to_le_bytes() {
            data.push(to_le_byte);
        }
        //dir_count
        for to_le_byte in self.dir_count.to_le_bytes() {
            data.push(to_le_byte);
        }
        data
    }
}

//清单数据块实际占用大小
const MANIFEST_DATA_BLOCK_DATA_LEN_LEN: usize = 8;
//清单数据块版本号占用
const MANIFEST_DATA_BLOCK_DATA_VER_INDEX: usize = MANIFEST_DATA_BLOCK_DATA_LEN_LEN;
const MANIFEST_DATA_BLOCK_DATA_VER_LEN: usize = 4;

//清单数据块
/*
清单数据块是4k对其的，
清单数据块采用A/B机制尽量实现原子化更新和备份，
由于A/B机制，所以内部分成两块，每块的前面版本和末尾的版本应是对应的，如果不对应则说明数据很可能损坏。
数据块格式：4字节32位占用大小+4字节32位版本
*/
#[derive(Default, Debug, PartialEq)]
struct ManifestDataBlock {
    file_pos: u64,
    run_block_data: Vec<u8>,
}
impl Clone for ManifestDataBlock {
    fn clone(&self) -> Self {
        Self {
            file_pos: self.file_pos,
            run_block_data: Vec::new(),
        }
    }
}

impl ManifestDataBlock {
    fn from_data_new(data: &[u8], file_pos: u64) -> io::Result<Self> {
        let block_len = Self::get_block_len_us(data.len());
        let mut run_block_data = vec![0; block_len];
        Self::save_data_to_block_data_new(data, &mut run_block_data)?;
        Ok(Self {
            file_pos,
            run_block_data,
        })
    }

    fn from_block_data_new(block_data: Vec<u8>, file_pos: u64) -> io::Result<Self> {
        if block_data.len().is_multiple_of(DATA_BLOCK_LEN) {
            Ok(Self {
                file_pos,
                run_block_data: block_data,
            })
        } else {
            Err(Error::other("大小不符合要求"))
        }
    }

    fn get_block_data(&self) -> &[u8] {
        &self.run_block_data
    }

    fn get_this_data_len(&self) -> u64 {
        Self::get_data_len(&self.run_block_data)
    }

    fn get_data_len(data: &[u8]) -> u64 {
        u64::from_le_bytes(data[..MANIFEST_DATA_BLOCK_DATA_LEN_LEN].try_into().unwrap())
    }

    fn get_this_block_len_u64(&self) -> u64 {
        self.run_block_data.len() as u64
    }

    fn get_this_block_len_us(&self) -> usize {
        self.run_block_data.len()
    }

    fn get_block_len(data: &[u8]) -> io::Result<u64> {
        if data.len() < MANIFEST_DATA_BLOCK_DATA_LEN_LEN {
            Err(Error::other("提供的数据不完整"))
        } else {
            //实际占用大小
            let data_len = usize::try_from(Self::get_data_len(data)).unwrap()
                + MANIFEST_DATA_BLOCK_DATA_LEN_LEN
                + MANIFEST_DATA_BLOCK_DATA_VER_LEN;
            //2倍4k对齐大小计算
            Ok(Self::get_block_len_us(data_len) as u64)
        }
    }

    fn get_this_ver(&self) -> u32 {
        let block_len_2 = self.get_this_block_len_us() / 2;
        let a_ver = Self::get_ver(&self.run_block_data[..block_len_2]).unwrap();
        let b_ver = Self::get_ver(&self.run_block_data[block_len_2..]).unwrap();
        if a_ver > b_ver { a_ver } else { b_ver }
    }

    fn get_ver(data: &[u8]) -> io::Result<u32> {
        let ver = u32::from_le_bytes(
            data[MANIFEST_DATA_BLOCK_DATA_VER_INDEX
                ..MANIFEST_DATA_BLOCK_DATA_VER_INDEX + MANIFEST_DATA_BLOCK_DATA_VER_LEN]
                .try_into()
                .unwrap(),
        );
        let end_ver = u32::from_le_bytes(
            data[data.len() - MANIFEST_DATA_BLOCK_DATA_VER_LEN..]
                .try_into()
                .unwrap(),
        );
        if ver == end_ver {
            Ok(ver)
        } else {
            Err(Error::other("快速完整性验证失败"))
        }
    }

    fn get_this_data(&self) -> &[u8] {
        Self::get_data(&self.run_block_data).unwrap()
    }

    fn get_data(data: &[u8]) -> io::Result<&[u8]> {
        //对齐判断
        if data.len().is_multiple_of(DATA_BLOCK_LEN) {
            let ab_block_data_len = data.len() / 2;
            //AB数据分开
            let a_data = &data[..ab_block_data_len];
            let b_data = &data[ab_block_data_len..];
            //获取版本并判断最终读取的是A还是B
            //完整性判断
            let a_ver = Self::get_ver(a_data);
            let a_err = a_ver.is_err();
            let b_ver = Self::get_ver(b_data);
            let b_err = b_ver.is_err();
            //版本判断
            let mut read_a = true;
            if a_err {
                //完全破碎判断
                if b_err {
                    return Err(Error::other("解析错误，数据损坏"));
                }
                //如果损坏则读取B
                read_a = false;
            } else if a_ver? < b_ver? {
                //如果A没有损坏了就判断版本
                read_a = false;
            }
            Ok(if read_a {
                let a_data_len = usize::try_from(Self::get_data_len(a_data)).unwrap();
                &a_data[MANIFEST_DATA_BLOCK_DATA_LEN_LEN + MANIFEST_DATA_BLOCK_DATA_VER_LEN
                    ..MANIFEST_DATA_BLOCK_DATA_LEN_LEN
                        + MANIFEST_DATA_BLOCK_DATA_VER_LEN
                        + a_data_len]
            } else {
                let b_data_len = usize::try_from(Self::get_data_len(b_data)).unwrap();
                &b_data[MANIFEST_DATA_BLOCK_DATA_LEN_LEN + MANIFEST_DATA_BLOCK_DATA_VER_LEN
                    ..MANIFEST_DATA_BLOCK_DATA_LEN_LEN
                        + MANIFEST_DATA_BLOCK_DATA_VER_LEN
                        + b_data_len]
            })
        } else {
            Err(Error::other("提供的数据未对齐"))
        }
    }

    fn update(&mut self, data: &[u8]) -> bool {
        let block_len = Self::get_block_len_us(data.len());
        let this_len = self.get_this_block_len_us();
        if block_len == this_len {
            Self::save_data_to_block_data2(data, &mut self.run_block_data, block_len).unwrap();
            false
        } else {
            let new_block_add_len = block_len.cast_signed() - this_len.cast_signed();
            if new_block_add_len > 0 {
                //附加
                for _ in 0..new_block_add_len {
                    self.run_block_data.push(0);
                }
            } else {
                //减少
                let new_block_add_len = -new_block_add_len;
                for _ in 0..new_block_add_len {
                    self.run_block_data.pop();
                }
            }
            Self::save_data_to_block_data_new2(data, &mut self.run_block_data, block_len);
            true
        }
    }

    fn save_data_to_block_data(data: &[u8], block_data: &mut [u8]) -> io::Result<()> {
        //大小检查
        let block_len = Self::get_block_len_us(data.len());
        if block_data.len() == block_len {
            Self::save_data_to_block_data2(data, block_data, block_len)
        } else {
            Err(Error::other("提供的缓冲区的大小和需要的块大小不一致"))
        }
    }

    fn save_data_to_block_data2(
        data: &[u8],
        block_data: &mut [u8],
        block_len: usize,
    ) -> io::Result<()> {
        let block_len_2 = block_len / 2;

        let a_block_data = &block_data[..block_len_2];
        let b_block_data = &block_data[block_len_2..];
        //获取A/B版本
        let a_data_ver = Self::get_ver(a_block_data);
        let a_err = a_data_ver.is_err();
        let b_data_ver = Self::get_ver(b_block_data);
        let b_err = b_data_ver.is_err();
        //
        if !a_err && !b_err {
            let a_data_ver = a_data_ver?;
            let b_data_ver = b_data_ver?;
            if a_data_ver < b_data_ver {
                //更新A
                Self::save_data_to_ab_block_data(
                    data,
                    &mut block_data[..block_len_2],
                    b_data_ver + 1,
                );
                Ok(())
            } else {
                //更新B
                Self::save_data_to_ab_block_data(
                    data,
                    &mut block_data[block_len_2..],
                    a_data_ver + 1,
                );
                Ok(())
            }
        } else if a_err {
            //更新A
            Self::save_data_to_ab_block_data(data, &mut block_data[..block_len_2], b_data_ver? + 1);
            Ok(())
        } else {
            //更新B
            Self::save_data_to_ab_block_data(data, &mut block_data[block_len_2..], a_data_ver? + 1);
            Ok(())
        }
    }

    fn save_data_to_block_data_new(data: &[u8], block_data: &mut [u8]) -> io::Result<()> {
        //大小检查
        let block_len = Self::get_block_len_us(data.len());
        if block_data.len() == block_len {
            Self::save_data_to_block_data_new2(data, block_data, block_len);
            Ok(())
        } else {
            Err(Error::other("提供的块数据缓冲区大小和所需大小不一致"))
        }
    }
    fn save_data_to_block_data_new2(data: &[u8], block_data: &mut [u8], block_len: usize) {
        Self::save_data_to_ab_block_data(data, &mut block_data[..block_len / 2], 1);
    }

    fn save_data_to_ab_block_data(data: &[u8], ab_block_data: &mut [u8], var: u32) {
        let data_len_data = (data.len() as u64).to_le_bytes();
        //数据大小
        for (index, value) in data_len_data.iter().enumerate() {
            ab_block_data[index] = *value;
        }
        //版本
        for (index, value) in var.to_le_bytes().iter().enumerate() {
            ab_block_data[MANIFEST_DATA_BLOCK_DATA_VER_INDEX + index] = *value;
            ab_block_data[ab_block_data.len() - MANIFEST_DATA_BLOCK_DATA_VER_LEN + index] = *value;
        }
        //数据
        for (index, value) in data.iter().enumerate() {
            let data_index =
                MANIFEST_DATA_BLOCK_DATA_LEN_LEN + MANIFEST_DATA_BLOCK_DATA_VER_LEN + index;
            if data_index == ab_block_data.len() {
                panic!("逻辑错误")
            }
            ab_block_data[data_index] = *value;
        }
    }

    fn get_block_len_us(data_len: usize) -> usize {
        let block_ab_len =
            data_len + MANIFEST_DATA_BLOCK_DATA_LEN_LEN + MANIFEST_DATA_BLOCK_DATA_VER_LEN;
        let block_len = block_ab_len * 2;
        let block_ab_len = block_len / DATA_BLOCK_LEN;
        let block_len = block_ab_len + 1;
        block_len * DATA_BLOCK_LEN
    }

    fn get_block_len_u64(data_len: u64) -> u64 {
        Self::get_block_len_us(usize::try_from(data_len).unwrap()) as u64
    }
}
#[test]
fn get_block_len_us() {
    assert_eq!(
        ManifestDataBlock::get_block_len_us(
            DATA_BLOCK_LEN / 2
                - MANIFEST_DATA_BLOCK_DATA_LEN_LEN
                - MANIFEST_DATA_BLOCK_DATA_VER_LEN
                + 1
        ),
        DATA_BLOCK_LEN * 2
    )
}
