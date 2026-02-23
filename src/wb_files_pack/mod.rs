/*
开始时间：26/2/11 15：51
 */
pub mod manager;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
pub struct WBFilesPackData {
    //属性
    attribute: Attribute,
    //包文件索引列表
    pack_files_list: PackFilesList,
} //包文件数据
impl WBFilesPackData {
    pub fn attribute(&self) -> &Attribute {
        &self.attribute
    }

    pub fn pack_files_list(&self) -> &PackFilesList {
        &self.pack_files_list
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Attribute {
    data_version: DataVersion,
    //写时复制
    cow: bool,
    //分离数据列表
    s_data_file: bool,
    //中间空白数据位置，格式:[[位置,长度]]
    empty_data_pos: Vec<(u64, u64)>,
} //包文件属性
impl Attribute {
    pub fn data_version(&self) -> &DataVersion {
        &self.data_version
    }

    pub fn cow(&self) -> bool {
        self.cow
    }

    pub fn s_data_file(&self) -> bool {
        self.s_data_file
    }

    pub fn empty_data_pos(&self) -> &Vec<(u64, u64)> {
        &self.empty_data_pos
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DataVersion {
    //文件版本
    value: u32,
    //兼任版本
    compatible: u32,
} //版本数据信息
impl DataVersion {
    pub fn value(&self) -> u32 {
        self.value
    }

    pub fn compatible(&self) -> u32 {
        self.compatible
    }

    pub fn new(value: u32, compatible: u32) -> DataVersion {
        DataVersion { value, compatible }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PackFilesList {
    //列表
    files_list: HashMap<String, PackFileInfo>,
    //总数据长度
    data_length: u64,
    //所有文件数，不包括目录
    file_count: u32,
    //所有目录数
    dir_count: u32,
} //包文件列表
impl PackFilesList {
    pub fn files_list(&self) -> &HashMap<String, PackFileInfo> {
        &self.files_list
    }

    pub fn data_length(&self) -> u64 {
        self.data_length
    }

    pub fn file_count(&self) -> u32 {
        self.file_count
    }

    pub fn dir_count(&self) -> u32 {
        self.dir_count
    }

    pub fn add_file(&mut self, file_info: PackFileInfo) -> Option<PackFileInfo> {
        self.files_list.insert(file_info.name.clone(), file_info)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PackFileInfo {
    //通用数据
    //名称
    name: String,
    //长度
    length: u64,
    //是目录
    is_dir: bool,
    //修改时间
    modified_time: u128,
    //
    file_kind: PackFileKind,
} //包文件信息
impl PackFileInfo {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn length(&self) -> u64 {
        self.length
    }

    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    pub fn modified_time(&self) -> u128 {
        self.modified_time
    }

    pub fn file_kind(&self) -> &PackFileKind {
        &self.file_kind
    }
}
impl Clone for PackFileInfo {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            length: self.length,
            is_dir: self.is_dir,
            modified_time: self.modified_time,
            file_kind: self.file_kind.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum PackFileKind {
    File(PackFile),
    Dir(PackDir),
    None,
} //

impl PackFileKind {}
impl Clone for PackFileKind {
    fn clone(&self) -> Self {
        match self {
            PackFileKind::File(file) => PackFileKind::File(file.clone()),
            PackFileKind::Dir(_) => PackFileKind::None,
            PackFileKind::None => PackFileKind::None
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PackFile {
    //数据位置，格式：[[起始位置,长度]]
    data_pos: Vec<(u64, u64)>,
    //哈希
    hash: String,
} //包文件特有数据
impl PackFile {
    pub fn data_pos(&self) -> &Vec<(u64, u64)> {
        &self.data_pos
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }

    pub fn now(data_pos: Vec<(u64, u64)>, hash: String) -> PackFile {
        PackFile { data_pos, hash }
    }
}
impl Clone for PackFile {
    fn clone(&self) -> Self {
        Self {
            data_pos: self.data_pos.clone(),
            hash: self.hash.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PackDir {
    //列表
    files_list: HashMap<String, PackFileInfo>,
    //所有子文件，不包含目录
    file_count: u32,
    //所有目录
    dir_count: u32,
} //包目录特有数据
impl PackDir {
    pub fn files_list(&self) -> &HashMap<String, PackFileInfo> {
        &self.files_list
    }

    fn files_list_mut(&mut self) -> &mut HashMap<String, PackFileInfo> {
        &mut self.files_list
    }

    pub fn file_count(&self) -> u32 {
        self.file_count
    }

    pub fn dir_count(&self) -> u32 {
        self.dir_count
    }

    pub fn add_file(&mut self, file_info: PackFileInfo) -> Option<PackFileInfo> {
        self.files_list.insert(file_info.name.clone(), file_info)
    }
}
impl Clone for PackDir {
    fn clone(&self) -> Self {
        Self {
            files_list: self.files_list.clone(),
            file_count: self.file_count,
            dir_count: self.dir_count,
        }
    }
}