/*
开始时间：26/2/11 15：51
 */
pub mod manager;
#[cfg(test)]
mod test;

use std::collections::HashMap;
use std::io;
use std::io::Error;

//文件头===
//文件头-文件名:WPFilesPack
pub static FILE_HEADER_TYPE_NAME: [u8; 11] = [
    0x57u8, 0x42, 0x46, 0x69, 0x6c, 0x65, 0x73, 0x50, 0x61, 0x63, 0x6b,
];
//文件头版本
static FILE_HEADER_VERSION: [u8; 2] = [0, 2];

//文件头标签位长度
static FILE_HEADER_TAG_LENGTH: usize = 1;

//文件头清单数据属性起始位置的位置
static FILE_HEADER_MANIFEST_ATTRIBUTE_POS_POS: u64 =
    (FILE_HEADER_TYPE_NAME.len() + FILE_HEADER_VERSION.len() + FILE_HEADER_TAG_LENGTH) as u64;
//文件头清单数据属性起始位置数据段长度
static FILE_HEADER_MANIFEST_ATTRIBUTE_POS_POS_LENGTH: u64 = 8;
//文件头清单数据根结构起始位置的位置
static FILE_HEADER_MANIFEST_ROOT_STATIC_POS_POS: u64 =
    FILE_HEADER_MANIFEST_ATTRIBUTE_POS_POS + FILE_HEADER_MANIFEST_ATTRIBUTE_POS_POS_LENGTH;

//文件头清单数据跟结构起始位置的位置长度
static FILE_HEADER_MANIFEST_ROOT_STATIC_POS_POS_LENGTH: u64 = 8;

//文件头清单数据的属性位置和根结构位置段长度
static FILE_HEADER_MANIFEST_ATTRIBUTE_AND_STATIC_LENGTH: u64 =
    FILE_HEADER_MANIFEST_ATTRIBUTE_POS_POS_LENGTH + FILE_HEADER_MANIFEST_ROOT_STATIC_POS_POS_LENGTH;

//文件头数据长度位置
static FILE_HEADER_DATA_LENGTH_POS: u64 =
    FILE_HEADER_MANIFEST_ATTRIBUTE_POS_POS + FILE_HEADER_MANIFEST_ATTRIBUTE_AND_STATIC_LENGTH;

//文件头数据长度长度
static FILE_HEADER_DATA_LENGTH_LENGTH: u64 = 8;
//文件头长度
static FILE_HEADER_LENGTH: u64 = FILE_HEADER_MANIFEST_ATTRIBUTE_POS_POS
    + FILE_HEADER_MANIFEST_ATTRIBUTE_AND_STATIC_LENGTH
    + FILE_HEADER_DATA_LENGTH_LENGTH;

//默认写实复制
pub static DEFAULT_COW: bool = false;

//默认分离数据为单独文件
pub static DEFAULT_S_DATA_FILE: bool = true;
//当前解析器版本
pub static MANIFEST_VERSION: u16 = 10;

//当前解析器兼任版本
pub static MANIFEST_VERSION_COMPATIBLE: u16 = 10;
//数据格式
//已分配数据列表项
//位置
static DATA_POS_LIST_ITEM_POS_LEN: usize = 8;
//长度
static DATA_POS_LIST_ITEM_LEN_LEN: usize = 8;
//总大小
static DATA_POS_LIST_ITEM_LEN: usize = DATA_POS_LIST_ITEM_POS_LEN + DATA_POS_LIST_ITEM_LEN_LEN;

#[derive(Default, Debug)]
pub struct WBFilesPacManifest {
    //属性
    attribute: Attribute,
    //根结构
    root_struct: PackStruct,
} //包文件数据

impl WBFilesPacManifest {
    pub fn attribute(&self) -> &Attribute {
        &self.attribute
    }

    pub fn root_struct(&self) -> &PackStruct {
        &self.root_struct
    }
}

//格式版本
static MANIFEST_ATTRIBUTE_VERSION_LEN: usize = 2;
//格式兼容版本
static MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_INDEX: usize = MANIFEST_ATTRIBUTE_VERSION_LEN;
static MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_LEN: usize = 2;
//布尔数据
static MANIFEST_ATTRIBUTE_BOOL_DATA_INDEX: usize =
    MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_INDEX + MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_LEN;
static MANIFEST_ATTRIBUTE_BOOL_DATA_LEN: usize = 1;
//空数据列表的文件指针位置
static MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_INDEX: usize =
    MANIFEST_ATTRIBUTE_BOOL_DATA_INDEX + MANIFEST_ATTRIBUTE_BOOL_DATA_LEN;
static MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_LEN: usize = 8;
//所有文件数
static MANIFEST_ATTRIBUTE_FILE_COUNT_INDEX: usize =
    MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_INDEX + MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_LEN;
static MANIFEST_ATTRIBUTE_FILE_COUNT_LEN: usize = 8;
//所有目录数
static MANIFEST_ATTRIBUTE_DIR_COUNT_INDEX: usize =
    MANIFEST_ATTRIBUTE_FILE_COUNT_INDEX + MANIFEST_ATTRIBUTE_FILE_COUNT_LEN;
static MANIFEST_ATTRIBUTE_DIR_COUNT_LEN: usize = 8;

static MANIFEST_ATTRIBUTE_LEN: usize = MANIFEST_ATTRIBUTE_VERSION_LEN
    + MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_LEN
    + MANIFEST_ATTRIBUTE_BOOL_DATA_LEN
    + MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_LEN
    + MANIFEST_ATTRIBUTE_FILE_COUNT_LEN
    + MANIFEST_ATTRIBUTE_DIR_COUNT_LEN;
#[derive(Default, Debug)]
pub struct Attribute {
    //格式版本
    version: u16,
    //格式兼容版本
    version_compatible: u16,
    //写时复制
    cow: bool,
    //空数据列表的文件指针位置
    empty_data_pos_list_pos: u64,
    //文件数，不含目录
    file_count: u64,
    //目录数
    dir_count: u64,
} //包文件属性
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
                    Err(Error::other("版本过低，无法解析"))?
                } else {
                    //版本过高
                    Err(Error::other("版本过高，无法解析"))?
                }
            }
            //布尔数据
            let bool_data = &data[MANIFEST_ATTRIBUTE_BOOL_DATA_INDEX];
            let cow = bool_data >> 7 == 1;
            //空数据列表文件指针位置
            let empty_data_pos_list_pos = u64::from_le_bytes(
                data[MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_INDEX
                    ..MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_INDEX
                    + MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_LEN]
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
            Ok(Self {
                version,
                version_compatible,
                cow,
                empty_data_pos_list_pos,
                file_count,
                dir_count,
            })
        }
    }

    fn get_bytes_vec(&mut self, empty_data_pos_s_pos: u64) -> Vec<u8> {
        let mut data = Vec::new();
        //格式版本
        for to_le_byte in self.version.to_le_bytes() {
            data.push(to_le_byte)
        }
        //格式兼容版本
        for to_le_byte in self.version_compatible.to_le_bytes() {
            data.push(to_le_byte)
        }
        //布尔数据
        let mut bool_data = 0;
        //写时复制
        if self.cow {
            bool_data |= 0b10000000;
        }
        data.push(bool_data);
        //空数据列表的文件指针位置
        for to_le_byte in empty_data_pos_s_pos.to_le_bytes() {
            data.push(to_le_byte)
        }
        //所有文件数
        for to_le_byte in self.file_count.to_le_bytes() {
            data.push(to_le_byte)
        }
        //所有目录数
        for to_le_byte in self.dir_count.to_le_bytes() {
            data.push(to_le_byte)
        }
        data
    }
}

static DATA_POS_LIST_COUNT_LEN: usize = 8;

#[derive(Debug, Default, PartialEq)]
struct DataPosList {
    data_len: u64,
    list: Vec<(u64, u64)>,
}
impl DataPosList {
    fn list(&self) -> &Vec<(u64, u64)> {
        &self.list
    }

    fn load(data: &[u8]) -> io::Result<Self> {
        //数量
        let count = usize::from_le_bytes(data[..DATA_POS_LIST_COUNT_LEN].try_into().unwrap());
        let data_len = count * DATA_POS_LIST_ITEM_LEN + DATA_POS_LIST_COUNT_LEN;
        let data_pos_list_data = &data[DATA_POS_LIST_COUNT_LEN..data_len];
        let mut list = Vec::with_capacity(count);
        while list.len() < count {
            let index = list.len();
            let pos = u64::from_le_bytes(
                data_pos_list_data[(index * DATA_POS_LIST_ITEM_LEN)
                    ..(index * DATA_POS_LIST_ITEM_LEN) + DATA_POS_LIST_ITEM_POS_LEN]
                    .try_into()
                    .unwrap(),
            );
            let len = u64::from_le_bytes(
                data_pos_list_data[(index * DATA_POS_LIST_ITEM_LEN) + DATA_POS_LIST_ITEM_POS_LEN
                    ..(index * DATA_POS_LIST_ITEM_LEN) + DATA_POS_LIST_ITEM_LEN]
                    .try_into()
                    .unwrap(),
            );
            list.push((pos, len))
        }
        Ok(Self {
            data_len: data_len as u64,
            list,
        })
    }

    fn get_bytes_vec(&mut self) -> Vec<u8> {
        self.get_bytes_vec2(None)
    }

    fn get_bytes_vec2(&mut self, this_gc_pos: Option<(u64, u64)>) -> Vec<u8> {
        let mut data = Vec::new();
        let list_count = match this_gc_pos {
            Some(_) => self.list.len() + 1,
            None => self.list.len()
        };
        for to_le_byte in (list_count as u64).to_le_bytes() {
            data.push(to_le_byte)
        }
        for (pos, len) in &self.list {
            for to_le_byte in pos.to_le_bytes() {
                data.push(to_le_byte)
            }
            for to_le_byte in len.to_le_bytes() {
                data.push(to_le_byte)
            }
        }
        //更新大小
        self.data_len = data.len() as u64;
        data
    }
}

static PACK_STRUCT_DATA_LEN_LEN: usize = 8;

#[derive(Default, Debug, PartialEq)]
pub struct PackStruct {
    //此结构的文件指针位置
    this_file_pos: u64,
    //数据长度
    data_len: u64,
    //结构项列表
    items: HashMap<String, PackStructItem>,
} //包结构
impl PackStruct {
    pub fn items(&self) -> &HashMap<String, PackStructItem> {
        &self.items
    }

    fn load(this_file_pos: u64, data: &[u8]) -> io::Result<Self> {
        if data.len() < 8 {
            Err(Error::other("提供的数据大小不够"))
        } else {
            let data_len =
                usize::from_le_bytes(data[..PACK_STRUCT_DATA_LEN_LEN].try_into().unwrap());
            if data.len() < data_len {
                Err(Error::other("提供的数据大小不够"))
            } else {
                //已读取大小
                let mut read_len = PACK_STRUCT_DATA_LEN_LEN;
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
                    this_file_pos,
                    data_len: data_len as u64,
                    items,
                })
            }
        }
    }

    fn get_bytes_vec(&mut self) -> Vec<u8> {
        let mut data = Vec::new();
        let mut items_data = Vec::new();
        //获取项数据
        for item in &self.items {
            let item = item.1;
            let item_vec = item.to_bytes_vec();
            for item_data in item_vec {
                items_data.push(item_data)
            }
        }
        //数据长度
        let data_len = PACK_STRUCT_ITEM_LEN_LEN + items_data.len();
        let data_len = data_len as u64;
        for to_le_byte in data_len.to_le_bytes() {
            data.push(to_le_byte)
        }
        //结构项
        for items_datum in items_data {
            data.push(items_datum)
        }
        //更新大小
        self.data_len = data_len;
        data
    }
}
//长度
static PACK_STRUCT_ITEM_LEN_LEN: usize = 8;
//类型
static PACK_STRUCT_ITEM_TYPE_INDEX: usize = PACK_STRUCT_ITEM_LEN_LEN;
static PACK_STRUCT_ITEM_TYPE_LEN: usize = 1;
//名称长度
static PACK_STRUCT_ITEM_NAME_LEN_INDEX: usize =
    PACK_STRUCT_ITEM_TYPE_INDEX + PACK_STRUCT_ITEM_TYPE_LEN;
static PACK_STRUCT_ITEM_NAME_LEN_LEN: usize = 2;
//名称
static PACK_STRUCT_ITEM_NAME_INDEX: usize =
    PACK_STRUCT_ITEM_NAME_LEN_INDEX + PACK_STRUCT_ITEM_NAME_LEN_LEN;
//虚拟文件元数据的文件指针位置
static PACK_STRUCT_ITEM_METADATA_FILE_POS_LEN: usize = 8;

#[derive(Default, Debug, PartialEq)]
pub struct PackStructItem {
    //名称
    name: String,
    //元数据文件指针位置
    metadata_file_pos: u64,
    //结构项类型
    item_type: PackStructItemType,
} //包结构项
impl PackStructItem {
    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn item_type(&self) -> &PackStructItemType {
        &self.item_type
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
        let name_end_pos = PACK_STRUCT_ITEM_NAME_INDEX + name_len as usize;
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
            data.push(to_le_byte)
        }
        //类型
        data.push(type_data.0);
        //名称长度
        for to_le_byte in (name_len as u16).to_le_bytes() {
            data.push(to_le_byte)
        }
        //名称
        for name_b in name_vec {
            data.push(name_b)
        }
        //虚拟文件元数据的文件指针位置
        for to_le_byte in self.metadata_file_pos.to_le_bytes() {
            data.push(to_le_byte)
        }
        //类型特有数据
        for type_datum in type_data.1 {
            data.push(type_datum)
        }
        data
    }
}

#[derive(Default, Debug, PartialEq)]
pub enum PackStructItemType {
    #[default]
    File,
    Dir(PackStructItemDir),
} //结构项类型

static PACK_STRUCT_DIR_STRUCT_FILE_POS_LEN: usize = 8;

#[derive(Default, Debug, PartialEq)]
pub struct PackStructItemDir {
    //结构的文件指针位置
    struct_file_pos: u64,
    //运行时缓存结构
    pack_struct: Option<PackStruct>,
} //结构项目录类型特有数据
impl PackStructItemDir {
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
            data.push(to_le_byte)
        }
        data
    }
}

//数据格式常量
//数据长度
static PACK_FILE_METADATA_DATA_LEN_LEN: usize = 8;
//类型
static PACK_FILE_METADATA_TYPE_INDEX: usize = PACK_FILE_METADATA_DATA_LEN_LEN;
static PACK_FILE_METADATA_TYPE_LEN: usize = 1;
static PACK_FILE_METADATA_BOOL_DATA_INDEX: usize =
    PACK_FILE_METADATA_TYPE_INDEX + PACK_FILE_METADATA_TYPE_LEN;
//布尔数据
static PACK_FILE_METADATA_BOOL_DATA_LEN: usize = 1;
static PACK_FILE_METADATA_LEN_INDEX: usize =
    PACK_FILE_METADATA_BOOL_DATA_INDEX + PACK_FILE_METADATA_BOOL_DATA_LEN;
//大小
static PACK_FILE_METADATA_LEN_LEN: usize = 8;
static PACK_FILE_METADATA_MODIFIED_INDEX: usize =
    PACK_FILE_METADATA_LEN_INDEX + PACK_FILE_METADATA_LEN_LEN;
//修改时间
static PACK_FILE_METADATA_MODIFIED_LEM: usize = 16;
//类型数据
static PACK_FILE_METADATA_TYPE_DATA_INDEX: usize =
    PACK_FILE_METADATA_MODIFIED_INDEX + PACK_FILE_METADATA_MODIFIED_LEM;

#[derive(PartialEq, Debug)]
pub struct PackFileMetadata {
    //数据长度
    data_len: u64,
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

    fn load(data: &[u8]) -> io::Result<Self> {
        let data_len =
            u64::from_le_bytes(data[..PACK_FILE_METADATA_DATA_LEN_LEN].try_into().unwrap());

        //类型
        let this_type = data[PACK_FILE_METADATA_TYPE_INDEX];
        //布尔值
        let bool_data = data[PACK_FILE_METADATA_BOOL_DATA_INDEX];
        //写时复制
        let cow = bool_data >> 7 == 1;
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
            0 => PackFileMetadataType::File(PackFileMetadataFile::load(type_data)?),
            1 => PackFileMetadataType::Dir(PackFileMetadataDir::load(type_data)?),
            _ => Err(Error::other("未知类型"))?,
        };

        Ok(Self {
            data_len,
            cow,
            modified,
            len,
            file_type,
        })
    }

    fn get_bytes_vec(&mut self) -> Vec<u8> {
        let mut data = Vec::new();
        //类型及其数据
        let type_data = match &mut self.file_type {
            PackFileMetadataType::File(file) => (0, file.get_bytes_vec()),
            PackFileMetadataType::Dir(dir) => (1, dir.to_bytes_vec()),
        };
        //数据长度
        let data_len = PACK_FILE_METADATA_DATA_LEN_LEN
            + PACK_FILE_METADATA_TYPE_LEN
            + PACK_FILE_METADATA_BOOL_DATA_LEN
            + PACK_FILE_METADATA_LEN_LEN
            + PACK_FILE_METADATA_MODIFIED_LEM
            + type_data.1.len();
        let data_len = data_len as u64;
        for to_le_byte in data_len.to_le_bytes() {
            data.push(to_le_byte);
        }
        //类型
        data.push(type_data.0);
        //布尔数据
        let mut bool_data = 0;
        if self.cow {
            bool_data |= 0b10000000;
        }
        data.push(bool_data);
        //长度
        for to_le_byte in self.len.to_le_bytes() {
            data.push(to_le_byte)
        }
        //修改时间
        for to_le_byte in self.modified.to_le_bytes() {
            data.push(to_le_byte)
        }
        //类型数据
        for type_datum in type_data.1 {
            data.push(type_datum)
        }
        //更新大小
        self.data_len = data_len;
        data
    }
}

#[derive(PartialEq, Debug)]
pub enum PackFileMetadataType {
    File(PackFileMetadataFile),
    Dir(PackFileMetadataDir),
} //包文件元数据类型

//数据格式常量
//哈希算法值类型
static PACK_METADATA_FILE_HASH_TYPE_INDEX: usize = 0;
static PACK_METADATA_FILE_HASH_TYPE_LEN: usize = 1;
//哈希值长度
static PACK_METADATA_FILE_HASH_LEN_INDEX: usize = 1;
static PACK_METADATA_FILE_HASH_LEN_LEN: usize = 1;
//哈希值
static PACK_METADATA_FILE_HASH_INDEX: usize =
    PACK_METADATA_FILE_HASH_TYPE_LEN + PACK_METADATA_FILE_HASH_LEN_LEN;
//已分配数据列表数量
static PACK_METADATA_FILE_DATA_POS_S_COUNT_LEN: usize = 8;

#[derive(Default, PartialEq, Debug)]
pub struct PackFileMetadataFile {
    //哈希算法类型
    hash_type: u8,
    //哈希值
    hash: Vec<u8>,
    //已分配数据集合
    data_pos_list: DataPosList,
} //包文件元数据文件
impl PackFileMetadataFile {
    fn load(data: &[u8]) -> io::Result<Self> {
        let hash_type = data[PACK_METADATA_FILE_HASH_TYPE_INDEX];
        let hash_len = data[PACK_METADATA_FILE_HASH_LEN_INDEX];
        let hash = data
            [PACK_METADATA_FILE_HASH_INDEX..PACK_METADATA_FILE_HASH_INDEX + hash_len as usize]
            .to_vec();
        let data_pos_list_count_index = PACK_METADATA_FILE_HASH_INDEX + hash_len as usize;
        let data_pos_list = DataPosList::load(&data[data_pos_list_count_index..])?;
        Ok(Self {
            hash_type,
            hash,
            data_pos_list,
        })
    }
    fn get_bytes_vec(&mut self) -> Vec<u8> {
        let mut data = Vec::new();
        //哈希算法类型
        data.push(self.hash_type);
        //哈希值长度
        let hash_len = self.hash.len() as u8;
        data.push(hash_len);
        //哈希
        for hash in &self.hash {
            data.push(*hash)
        }
        //已分配数据列表
        for data_pos_list_data in self.data_pos_list.get_bytes_vec() {
            data.push(data_pos_list_data);
        }
        data
    }
}

//数据格式字段
//文件数量
static PACK_METADATA_DIR_FILE_COUNT_LEN: usize = 8;
//目录数量
static PACK_METADATA_DIR_DIR_COUNT_INDEX: usize = PACK_METADATA_DIR_FILE_COUNT_LEN;
static PACK_METADATA_DIR_DIR_COUNT_LEN: usize = 8;

#[derive(Default, PartialEq, Debug)]
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
            data.push(to_le_byte)
        }
        //dir_count
        for to_le_byte in self.dir_count.to_le_bytes() {
            data.push(to_le_byte)
        }
        data
    }
}
