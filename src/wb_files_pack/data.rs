use super::error::{PackFileError, Result};
use super::manager::DEFAULT_COW;
use super::pack_io::file_handle::PackFileHandle;
use std::collections::HashMap;
use std::sync::{Mutex, Weak};

/// 当前清单文件格式版本 / Current manifest file format version
pub const MANIFEST_VERSION: u16 = 10;

/// 当前清单文件兼容的最低版本 / Minimum compatible manifest file version
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
pub(crate) const DATA_BLOCK_LEN: usize = 128;

pub(crate) const DATA_DATA_BLOCK_LEN: u64 = 4 * 1024 * 1024;

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
    MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_INDEX +
        MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_LEN;
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

const MANIFEST_ATTRIBUTE_LEN: usize =
    MANIFEST_ATTRIBUTE_VERSION_LEN +
        MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_LEN +
        MANIFEST_ATTRIBUTE_BOOL_DATA_LEN +
        MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_LEN +
        MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_LEN +
        MANIFEST_ATTRIBUTE_MANIFEST_FILE_LEN_LEN +
        MANIFEST_ATTRIBUTE_ROOT_STRUCT_POS_LEN +
        MANIFEST_ATTRIBUTE_FILE_COUNT_LEN +
        MANIFEST_ATTRIBUTE_DIR_COUNT_LEN +
        MANIFEST_ATTRIBUTE_DATA_LEN_LEN;

pub(crate) const MANIFEST_ATTRIBUTE_BLOCK_LEN: usize =
    ManifestDataBlock::get_block_len_us(MANIFEST_ATTRIBUTE_LEN);

//清单数据实际占用大小
pub(crate) const MANIFEST_DATA_BLOCK_DATA_LEN_LEN: usize = 8;
//清单数据块版本号
pub(crate) const MANIFEST_DATA_BLOCK_DATA_VER_INDEX: usize = MANIFEST_DATA_BLOCK_DATA_LEN_LEN;
pub(crate) const MANIFEST_DATA_BLOCK_DATA_VER_LEN: usize = 4;
//数据哈希
const MANIFEST_DATA_BLOCK_DATA_HASH_INDEX: usize =
    MANIFEST_DATA_BLOCK_DATA_VER_INDEX + MANIFEST_DATA_BLOCK_DATA_VER_LEN;
const MANIFEST_DATA_BLOCK_DATA_HASH_LEN: usize = 8;

//清单数据块
#[derive(Default, Debug, PartialEq, Clone)]
pub(crate) struct ManifestDataBlock {
    file_pos: u64,
    block_data: Vec<u8>,
    is_a_data: bool,
    data_len: u64,
    hash_value: Vec<u8>,
}

impl ManifestDataBlock {
    pub(crate) fn file_pos(&self) -> u64 {
        self.file_pos
    }

    pub(crate) fn set_file_pos(&mut self, pos: u64) {
        self.file_pos = pos;
    }

    pub(crate) fn from_block_data_new(block_data: Vec<u8>, file_pos: u64) -> Result<Self> {
        if block_data.len().is_multiple_of(DATA_BLOCK_LEN) {
            let (is_a_data, data) = Self::get_data2(&block_data)?;
            let data_len = data.len() as u64;
            let hash_value = Self::get_hash(&block_data)?.to_vec();
            Ok(Self {
                file_pos,
                block_data,
                is_a_data,
                data_len,
                hash_value,
            })
        } else {
            Err(PackFileError::Format("大小不符合数据块对齐要求".into()))
        }
    }

    pub(crate) fn get_block_data(&self) -> &[u8] {
        &self.block_data
    }

    #[cfg(test)]
    pub(crate) fn block_data(&self) -> &[u8] {
        &self.block_data
    }

    #[cfg(test)]
    pub(crate) fn block_data_mut(&mut self) -> &mut Vec<u8> {
        &mut self.block_data
    }

    pub(crate) fn get_this_data(&self) -> Result<&[u8]> {
        let data = Self::get_data(&self.block_data)?;
        if data.is_empty() {
            Err(PackFileError::Format("没有数据".into()))
        } else {
            Ok(data)
        }
    }

    pub(crate) fn get_this_block_len_u64(&self) -> u64 {
        self.block_data.len() as u64
    }

    pub(crate) fn get_this_block_len_us(&self) -> usize {
        self.block_data.len()
    }

    pub(crate) fn update(&mut self, data: &[u8]) -> bool {
        let old_len = self.get_this_block_len_us();
        let block_len = Self::get_block_len_us(data.len());
        if block_len == old_len {
            self.is_a_data = Self::save_data_to_block_data(
                data,
                &mut self.block_data,
                block_len,
            ).unwrap();
            self.data_len = data.len() as u64;
            self.hash_value = Self::get_hash(&self.block_data).unwrap().to_vec();
            false
        } else {
            self.block_data.resize(block_len, 0);
            self.block_data.fill(0);
            Self::save_data_to_block_data_new(data, &mut self.block_data, block_len);
            self.is_a_data = true;
            self.data_len = data.len() as u64;
            self.hash_value = Self::get_hash(&self.block_data).unwrap().to_vec();
            true
        }
    }

    pub(crate) fn get_block_len(data: &[u8]) -> Result<u64> {
        if data.len() < MANIFEST_DATA_BLOCK_DATA_LEN_LEN {
            Err(PackFileError::Format("提供的数据块数据不完整".into()))
        } else {
            let data_len = usize::try_from(Self::get_data_len(data)).unwrap();
            Ok(Self::get_block_len_us(data_len) as u64)
        }
    }

    pub(crate) const fn get_block_len_us(data_len: usize) -> usize {
        let block_ab_len =
            data_len +
                MANIFEST_DATA_BLOCK_DATA_LEN_LEN +
                MANIFEST_DATA_BLOCK_DATA_VER_LEN * 2 +
                MANIFEST_DATA_BLOCK_DATA_HASH_LEN;
        let block_len = block_ab_len * 2;
        let block_ab_len = block_len.div_ceil(DATA_BLOCK_LEN);
        let block_len = block_ab_len * DATA_BLOCK_LEN;
        assert!(block_len.is_multiple_of(DATA_BLOCK_LEN));
        block_len
    }

    pub(crate) fn next_ver(ver: u32) -> u32 {
        if ver == u32::MAX { 1 } else { ver + 1 }
    }

    pub(crate) fn ver_is_older(a: u32, b: u32) -> bool {
        a != b && b.wrapping_sub(a) < a.wrapping_sub(b)
    }

    pub(crate) fn get_ver(data: &[u8]) -> Result<u32> {
        let ver = u32::from_le_bytes(
            data[
                MANIFEST_DATA_BLOCK_DATA_VER_INDEX..MANIFEST_DATA_BLOCK_DATA_VER_INDEX +
                    MANIFEST_DATA_BLOCK_DATA_VER_LEN
                ]
                .try_into()
                .unwrap()
        );
        let end_ver = u32::from_le_bytes(
            data[data.len() - MANIFEST_DATA_BLOCK_DATA_VER_LEN..].try_into().unwrap()
        );
        if ver == end_ver && ver != 0 {
            Ok(ver)
        } else {
            Err(PackFileError::Integrity("快速完整性验证失败".into()))?
        }
    }

    pub(crate) fn this_data_hash_x(&self) -> Result<bool> {
        Ok(Self::data_hash_x(self.get_this_data()?, &self.hash_value))
    }

    fn data_hash_x(data: &[u8], hash: &[u8]) -> bool {
        let binding = blake3::hash(data);
        let in_hash = binding.as_bytes();
        let in_hash = &in_hash[..MANIFEST_DATA_BLOCK_DATA_HASH_LEN];
        hash == in_hash
    }

    fn get_data_len(data: &[u8]) -> u64 {
        u64::from_le_bytes(data[..MANIFEST_DATA_BLOCK_DATA_LEN_LEN].try_into().unwrap())
    }

    fn get_data(data: &[u8]) -> Result<&[u8]> {
        Ok(Self::get_data2(data)?.1)
    }

    fn get_data2(data: &[u8]) -> Result<(bool, &[u8])> {
        if data.len().is_multiple_of(DATA_BLOCK_LEN) {
            let ab_block_data_len = data.len() / 2;
            let a_data = &data[..ab_block_data_len];
            let b_data = &data[ab_block_data_len..];
            let a_ver = Self::get_ver(a_data);
            let a_err = a_ver.is_err();
            let b_ver = Self::get_ver(b_data);
            let b_err = b_ver.is_err();
            let mut read_a = true;

            if ab_block_data_len == 0 {
                std::hint::black_box(());
            }

            if a_err {
                if b_err {
                    return Err(PackFileError::Integrity("解析错误，快速验证失败".into()))?;
                }
                read_a = false;
            } else if b_err {
                read_a = true;
            } else if Self::ver_is_older(a_ver?, b_ver?) {
                read_a = false;
            }
            Ok((
                read_a,
                if read_a {
                    let a_data_len = usize::try_from(Self::get_data_len(a_data)).unwrap();
                    &a_data
                        [
                        MANIFEST_DATA_BLOCK_DATA_LEN_LEN +
                            MANIFEST_DATA_BLOCK_DATA_VER_LEN +
                            MANIFEST_DATA_BLOCK_DATA_HASH_LEN..MANIFEST_DATA_BLOCK_DATA_LEN_LEN +
                            MANIFEST_DATA_BLOCK_DATA_VER_LEN +
                            MANIFEST_DATA_BLOCK_DATA_HASH_LEN +
                            a_data_len
                        ]
                } else {
                    let b_data_len = usize::try_from(Self::get_data_len(b_data)).unwrap();
                    &b_data
                        [
                        MANIFEST_DATA_BLOCK_DATA_LEN_LEN +
                            MANIFEST_DATA_BLOCK_DATA_VER_LEN +
                            MANIFEST_DATA_BLOCK_DATA_HASH_LEN..MANIFEST_DATA_BLOCK_DATA_LEN_LEN +
                            MANIFEST_DATA_BLOCK_DATA_VER_LEN +
                            MANIFEST_DATA_BLOCK_DATA_HASH_LEN +
                            b_data_len
                        ]
                },
            ))
        } else {
            Err(PackFileError::Format("提供的数据未对齐，数据可能不完整".into()))?
        }
    }

    fn save_data_to_block_data(
        data: &[u8],
        block_data: &mut [u8],
        block_len: usize,
    ) -> Result<bool> {
        let block_len_2 = block_len / 2;

        let a_block_data = &block_data[..block_len_2];
        let b_block_data = &block_data[block_len_2..];
        let a_data_ver = Self::get_ver(a_block_data);
        let a_err = a_data_ver.is_err();
        let b_data_ver = Self::get_ver(b_block_data);
        let b_err = b_data_ver.is_err();
        //
        if !a_err && !b_err {
            let a_data_ver = a_data_ver?;
            let b_data_ver = b_data_ver?;
            if Self::ver_is_older(a_data_ver, b_data_ver) {
                Self::save_data_to_ab_block_data(
                    data,
                    &mut block_data[..block_len_2],
                    Self::next_ver(b_data_ver),
                );
                Ok(true)
            } else {
                Self::save_data_to_ab_block_data(
                    data,
                    &mut block_data[block_len_2..],
                    Self::next_ver(a_data_ver),
                );
                Ok(false)
            }
        } else if a_err {
            let new_ver = Self::next_ver(b_data_ver?);
            Self::save_data_to_ab_block_data(data, &mut block_data[..block_len_2], new_ver);
            Ok(true)
        } else {
            let new_ver = Self::next_ver(a_data_ver?);
            Self::save_data_to_ab_block_data(data, &mut block_data[block_len_2..], new_ver);
            Ok(false)
        }
    }

    fn save_data_to_block_data_new(data: &[u8], block_data: &mut [u8], block_len: usize) {
        Self::save_data_to_ab_block_data(data, &mut block_data[..block_len / 2], 1);
    }

    fn save_data_to_ab_block_data(data: &[u8], ab_block_data: &mut [u8], var: u32) {
        let data_len_data = (data.len() as u64).to_le_bytes();
        for (index, value) in data_len_data.iter().enumerate() {
            ab_block_data[index] = *value;
        }
        for (index, value) in var.to_le_bytes().iter().enumerate() {
            ab_block_data[MANIFEST_DATA_BLOCK_DATA_VER_INDEX + index] = *value;
            ab_block_data[ab_block_data.len() - MANIFEST_DATA_BLOCK_DATA_VER_LEN + index] = *value;
        }
        let hash = blake3::hash(data);
        let hash = &hash.as_bytes()[0..MANIFEST_DATA_BLOCK_DATA_HASH_LEN];
        for (index, value) in hash.iter().enumerate() {
            ab_block_data[MANIFEST_DATA_BLOCK_DATA_HASH_INDEX + index] = *value;
        }
        let data_index =
            MANIFEST_DATA_BLOCK_DATA_LEN_LEN +
                MANIFEST_DATA_BLOCK_DATA_VER_LEN +
                MANIFEST_DATA_BLOCK_DATA_HASH_LEN;
        for (index, value) in data.iter().enumerate() {
            let data_index = data_index + index;
            assert!(
                data_index < ab_block_data.len() - MANIFEST_DATA_BLOCK_DATA_VER_LEN,
                "数据超出块容量 / data exceeds block capacity"
            );
            ab_block_data[data_index] = *value;
        }
    }

    fn get_hash(block_data: &[u8]) -> Result<&[u8]> {
        if Self::get_data2(block_data)?.0 {
            Ok(
                &block_data
                    [
                    MANIFEST_DATA_BLOCK_DATA_HASH_INDEX..MANIFEST_DATA_BLOCK_DATA_HASH_INDEX +
                        MANIFEST_DATA_BLOCK_DATA_HASH_LEN
                    ]
            )
        } else {
            let b_data_index = block_data.len() / 2;
            Ok(
                &block_data
                    [
                    b_data_index + MANIFEST_DATA_BLOCK_DATA_HASH_INDEX..b_data_index +
                        MANIFEST_DATA_BLOCK_DATA_HASH_INDEX +
                        MANIFEST_DATA_BLOCK_DATA_HASH_LEN
                    ]
            )
        }
    }
}

pub(crate) trait ManifestDataBlockTrait {
    fn to_bytes_vec(&self) -> Vec<u8>;
    fn data_block_mut(&mut self) -> &mut ManifestDataBlock;
    fn get_block_data(&mut self) -> (Vec<u8>, bool) {
        let update = self.to_bytes_vec();
        let data_block = self.data_block_mut();
        let new_block = data_block.update(&update);
        (data_block.get_block_data().to_vec(), new_block)
    }
}

/// 清单属性 / Manifest attribute
///
/// 存储包文件的全局属性信息，包括版本、文件计数、数据位置等。
/// Stores global attribute information for the pack file, including version, file counts, data positions, etc.
#[derive(Debug, Clone)]
pub struct Attribute {
    /// 格式版本 / Format version
    version: u16,
    /// 最低兼容版本 / Minimum compatible version
    version_compatible: u16,
    /// 是否启用写时复制 / Whether copy-on-write is enabled
    cow: bool,
    /// 空数据位置列表的文件偏移 / File offset of the empty data position list
    empty_data_pos_list_pos: u64,
    /// 清单空数据位置列表的文件偏移 / File offset of the manifest empty data position list
    manifest_empty_data_pos_list_pos: u64,
    /// 清单文件长度 / Manifest file length
    manifest_file_len: u64,
    /// 根目录结构的文件偏移 / File offset of the root struct
    root_struct_pos: u64,
    /// 文件总数 / Total file count
    file_count: u64,
    /// 目录总数 / Total directory count
    dir_count: u64,
    /// 数据总长度（字节）/ Total data length (bytes)
    data_len: u64,
    /// 数据块 / Data block
    data_block: ManifestDataBlock,
    /// 是否脏（未写入）/ Whether dirty (not yet written)
    dirty: bool,
}

impl PartialEq for Attribute {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version &&
            self.version_compatible == other.version_compatible &&
            self.cow == other.cow &&
            self.empty_data_pos_list_pos == other.empty_data_pos_list_pos &&
            self.manifest_empty_data_pos_list_pos == other.manifest_empty_data_pos_list_pos &&
            self.manifest_file_len == other.manifest_file_len &&
            self.root_struct_pos == other.root_struct_pos &&
            self.file_count == other.file_count &&
            self.dir_count == other.dir_count &&
            self.data_len == other.data_len &&
            self.data_block == other.data_block
    }
}

impl Default for Attribute {
    fn default() -> Self {
        Self {
            version: MANIFEST_VERSION,
            version_compatible: MANIFEST_VERSION_COMPATIBLE,
            cow: DEFAULT_COW,
            empty_data_pos_list_pos: 0,
            manifest_empty_data_pos_list_pos: 0,
            manifest_file_len: 0,
            root_struct_pos: 0,
            file_count: 0,
            dir_count: 0,
            data_len: 0,
            data_block: ManifestDataBlock::default(),
            dirty: false,
        }
    }
}

impl Attribute {
    /// 返回包文件的格式版本 / Returns the format version of the pack file
    #[must_use]
    pub fn version(&self) -> u16 {
        self.version
    }

    /// 返回包文件的最低兼容版本 / Returns the minimum compatible version
    #[must_use]
    pub fn version_compatible(&self) -> u16 {
        self.version_compatible
    }

    /// 返回是否启用写时复制 / Returns whether copy-on-write is enabled
    #[must_use]
    pub fn cow(&self) -> bool {
        self.cow
    }

    /// 返回包中文件总数 / Returns total file count in the pack
    #[must_use]
    pub fn file_count(&self) -> u64 {
        self.file_count
    }

    /// 返回包中目录总数 / Returns total directory count in the pack
    #[must_use]
    pub fn dir_count(&self) -> u64 {
        self.dir_count
    }

    /// 返回包中数据的总长度（字节）/ Returns total data length in the pack (bytes)
    #[must_use]
    pub fn data_len(&self) -> u64 {
        self.data_len
    }

    pub(crate) fn empty_data_pos_list_pos(&self) -> u64 {
        self.empty_data_pos_list_pos
    }

    pub(crate) fn root_struct_pos(&self) -> u64 {
        self.root_struct_pos
    }

    pub(crate) fn manifest_empty_data_pos_list_pos(&self) -> u64 {
        self.manifest_empty_data_pos_list_pos
    }

    pub(crate) fn manifest_file_len(&self) -> u64 {
        self.manifest_file_len
    }

    pub(crate) fn set_root_struct_pos(&mut self, pos: u64) {
        self.root_struct_pos = pos;
        self.dirty = true;
    }

    pub(crate) fn add_file_count(&mut self, delta: u64) {
        self.file_count += delta;
        self.dirty = true;
    }

    pub(crate) fn add_dir_count(&mut self, delta: u64) {
        self.dir_count += delta;
        self.dirty = true;
    }

    pub(crate) fn add_data_len(&mut self, delta: u64) {
        self.data_len += delta;
        self.dirty = true;
    }

    pub(crate) fn set_empty_data_pos_list_pos(&mut self, pos: u64) {
        self.empty_data_pos_list_pos = pos;
        self.dirty = true;
    }

    pub(crate) fn set_cow(&mut self, cow: bool) {
        self.cow = cow;
        self.dirty = true;
    }

    #[cfg(test)]
    pub(crate) fn set_version(&mut self, version: u16) {
        self.version = version;
        self.dirty = true;
    }

    #[cfg(test)]
    pub(crate) fn set_version_compatible(&mut self, version_compatible: u16) {
        self.version_compatible = version_compatible;
        self.dirty = true;
    }

    pub(crate) fn set_manifest_empty_data_pos_list_pos(&mut self, pos: u64) {
        self.manifest_empty_data_pos_list_pos = pos;
        self.dirty = true;
    }

    #[cfg(test)]
    pub(crate) fn set_manifest_file_len(&mut self, len: u64) {
        self.manifest_file_len = len;
        self.dirty = true;
    }

    pub(crate) fn load(data_block: ManifestDataBlock) -> Result<Self> {
        let data = data_block
            .get_this_data()
            .map_err(|err| PackFileError::Format(format!(r"无法获取属性数据, err: {err}")))?;
        let version = u16::from_le_bytes(
            data[..MANIFEST_ATTRIBUTE_VERSION_LEN].try_into().unwrap()
        );
        let version_compatible = u16::from_le_bytes(
            data[
                MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_INDEX..MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_INDEX +
                    MANIFEST_ATTRIBUTE_VERSION_COMPATIBLE_LEN
                ]
                .try_into()
                .unwrap()
        );
        if version != MANIFEST_VERSION {
            if version < MANIFEST_VERSION_COMPATIBLE {
                Err(PackFileError::Version("版本过低，无法解析".into()))?;
            } else if version_compatible > MANIFEST_VERSION {
                Err(PackFileError::Version("版本过高，无法解析".into()))?;
            }
        }
        let bool_data = &data[MANIFEST_ATTRIBUTE_BOOL_DATA_INDEX];
        let cow = (bool_data >> 7) == 1;
        let empty_data_pos_list_pos = u64::from_le_bytes(
            data[
                MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_INDEX..MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_INDEX +
                    MANIFEST_ATTRIBUTE_EMPTY_DATA_POS_LEN
                ]
                .try_into()
                .unwrap()
        );
        let manifest_empty_data_pos_list_pos = u64::from_le_bytes(
            data[
                MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_INDEX..MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_INDEX +
                    MANIFEST_ATTRIBUTE_MANIFEST_EMPTY_DATA_POS_LEN
                ]
                .try_into()
                .unwrap()
        );
        let manifest_file_len = u64::from_le_bytes(
            data[
                MANIFEST_ATTRIBUTE_MANIFEST_FILE_LEN_INDEX..MANIFEST_ATTRIBUTE_MANIFEST_FILE_LEN_INDEX +
                    MANIFEST_ATTRIBUTE_MANIFEST_FILE_LEN_LEN
                ]
                .try_into()
                .unwrap()
        );
        let root_struct_pos = u64::from_le_bytes(
            data[
                MANIFEST_ATTRIBUTE_ROOT_STRUCT_POS_INDEX..MANIFEST_ATTRIBUTE_ROOT_STRUCT_POS_INDEX +
                    MANIFEST_ATTRIBUTE_ROOT_STRUCT_POS_LEN
                ]
                .try_into()
                .unwrap()
        );
        let file_count = u64::from_le_bytes(
            data[
                MANIFEST_ATTRIBUTE_FILE_COUNT_INDEX..MANIFEST_ATTRIBUTE_FILE_COUNT_INDEX +
                    MANIFEST_ATTRIBUTE_FILE_COUNT_LEN
                ]
                .try_into()
                .unwrap()
        );
        let dir_count = u64::from_le_bytes(
            data[
                MANIFEST_ATTRIBUTE_DIR_COUNT_INDEX..MANIFEST_ATTRIBUTE_DIR_COUNT_INDEX +
                    MANIFEST_ATTRIBUTE_DIR_COUNT_LEN
                ]
                .try_into()
                .unwrap()
        );
        let data_len = u64::from_le_bytes(
            data[
                MANIFEST_ATTRIBUTE_DATA_LEN_INDEX..MANIFEST_ATTRIBUTE_DATA_LEN_INDEX +
                    MANIFEST_ATTRIBUTE_DATA_LEN_LEN
                ]
                .try_into()
                .unwrap()
        );
        Ok(Self {
            version,
            version_compatible,
            cow,
            file_count,
            dir_count,
            data_len,
            empty_data_pos_list_pos,
            manifest_empty_data_pos_list_pos,
            manifest_file_len,
            root_struct_pos,
            data_block,
            dirty: false,
        })
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub(crate) fn clear_dirty(&mut self) {
        self.dirty = false;
    }
}

impl ManifestDataBlockTrait for Attribute {
    fn to_bytes_vec(&self) -> Vec<u8> {
        let mut data = Vec::new();
        for to_le_byte in self.version.to_le_bytes() {
            data.push(to_le_byte);
        }
        for to_le_byte in self.version_compatible.to_le_bytes() {
            data.push(to_le_byte);
        }
        let mut bool_data = 0;
        if self.cow {
            bool_data |= 0b1000_0000;
        }
        data.push(bool_data);
        for to_le_byte in self.empty_data_pos_list_pos.to_le_bytes() {
            data.push(to_le_byte);
        }
        for to_le_byte in self.manifest_empty_data_pos_list_pos.to_le_bytes() {
            data.push(to_le_byte);
        }
        for to_le_byte in self.manifest_file_len.to_le_bytes() {
            data.push(to_le_byte);
        }
        for to_le_byte in self.root_struct_pos.to_le_bytes() {
            data.push(to_le_byte);
        }
        for to_le_byte in self.file_count.to_le_bytes() {
            data.push(to_le_byte);
        }
        for to_le_byte in self.dir_count.to_le_bytes() {
            data.push(to_le_byte);
        }
        for to_le_byte in self.data_len.to_le_bytes() {
            data.push(to_le_byte);
        }
        assert!(!data.is_empty(), "输出数据为空，但不能为空");
        data
    }

    fn data_block_mut(&mut self) -> &mut ManifestDataBlock {
        &mut self.data_block
    }
}

const DATA_POS_LIST_COUNT_LEN: usize = 8;

#[derive(Debug, Default, PartialEq, Clone)]
/// 数据位置列表 / Data position list
///
/// 管理文件中的空闲块位置列表，用于空间分配和回收。
/// Manages the list of free block positions in the file, used for space allocation and recycling.
pub struct DataPosList {
    /// 数据块 / Data block
    data_block: Option<ManifestDataBlock>,
    /// 位置列表（起始位置, 长度）/ Position list (start position, length)
    list: Vec<(u64, u64)>,
}

impl DataPosList {
    pub(crate) fn new(list: Vec<(u64, u64)>) -> Self {
        Self {
            data_block: None,
            list,
        }
    }

    pub(crate) fn list(&self) -> &Vec<(u64, u64)> {
        &self.list
    }

    pub(crate) fn list_mut(&mut self) -> &mut Vec<(u64, u64)> {
        &mut self.list
    }

    pub(crate) fn set_data_block(&mut self, db: Option<ManifestDataBlock>) {
        self.data_block = db;
    }

    pub(crate) fn get_data_block_mut(&mut self) -> Option<&mut ManifestDataBlock> {
        if let Some(v) = &mut self.data_block { Some(v) } else { None }
    }

    pub(crate) fn load(data: &[u8], data_block: Option<ManifestDataBlock>) -> Self {
        let count = usize::from_le_bytes(data[..DATA_POS_LIST_COUNT_LEN].try_into().unwrap());
        let data_len = count
            .checked_mul(DATA_POS_LIST_ITEM_LEN)
            .and_then(|v| v.checked_add(DATA_POS_LIST_COUNT_LEN))
            .unwrap_or(data.len());
        let data_pos_list_data = &data[DATA_POS_LIST_COUNT_LEN..data_len];
        let mut list = Vec::with_capacity(count);
        while list.len() < count {
            let index = list.len();
            let pos = u64::from_le_bytes(
                data_pos_list_data[
                    index * DATA_POS_LIST_ITEM_LEN..index * DATA_POS_LIST_ITEM_LEN +
                        DATA_POS_LIST_ITEM_POS_LEN
                    ]
                    .try_into()
                    .unwrap()
            );
            let len = u64::from_le_bytes(
                data_pos_list_data[
                    index * DATA_POS_LIST_ITEM_LEN + DATA_POS_LIST_ITEM_POS_LEN..index *
                        DATA_POS_LIST_ITEM_LEN +
                        DATA_POS_LIST_ITEM_LEN
                    ]
                    .try_into()
                    .unwrap()
            );
            list.push((pos, len));
        }
        Self { data_block, list }
    }

    pub(crate) fn to_bytes_vec(&self) -> Vec<u8> {
        self.to_bytes_vec2(None)
    }

    pub(crate) fn to_bytes_vec2(&self, this_gc_pos: Option<(u64, u64)>) -> Vec<u8> {
        let list_count = match this_gc_pos {
            Some(_) => self.list.len() + 1,
            None => self.list.len(),
        };
        let mut data = Vec::with_capacity(
            DATA_POS_LIST_COUNT_LEN + list_count * DATA_POS_LIST_ITEM_LEN
        );
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
        if let Some((pos, len)) = this_gc_pos {
            for to_le_byte in pos.to_le_bytes() {
                data.push(to_le_byte);
            }
            for to_le_byte in len.to_le_bytes() {
                data.push(to_le_byte);
            }
        }
        assert!(!data.is_empty(), "输出数据为空，但不能为空");
        data
    }

    pub(crate) fn get_block_data(&mut self) -> Option<(Vec<u8>, bool)> {
        if self.data_block.is_some() {
            let up_data = self.to_bytes_vec();
            if let Some(data_block) = &mut self.data_block {
                let new_block = data_block.update(&up_data);
                Some((data_block.get_block_data().to_vec(), new_block))
            } else {
                None
            }
        } else {
            None
        }
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

/// 包目录结构项 / Pack struct item
///
/// 表示包文件目录树中的一个节点，可以是文件或子目录。
/// Represents a node in the pack file directory tree, either a file or a subdirectory.
#[derive(Debug, PartialEq, Clone)]
pub struct PackStructItem {
    /// 项名称 / Item name
    name: String,
    /// 元数据在清单文件中的位置 / Metadata position in the manifest file
    metadata_file_pos: u64,
    /// 项类型（文件或目录）/ Item type (File or Dir)
    item_type: PackStructItemType,
    /// 元数据运行状态 / Metadata run state
    metadata: PackFileMetadataRun,
}

impl PackStructItem {
    pub(crate) fn new(
        name: String,
        item_type: PackStructItemType,
        metadata_file_pos: u64,
        metadata: PackFileMetadataRun,
    ) -> Self {
        Self {
            name,
            metadata_file_pos,
            item_type,
            metadata,
        }
    }

    pub(crate) fn new_empty_dir(name: &str, metadata: PackFileMetadata) -> Self {
        let metadata = PackFileMetadataRun::Loaded(Box::from(metadata));
        Self {
            name: name.to_string(),
            metadata_file_pos: 0,
            item_type: PackStructItemType::Dir {
                struct_file_pos: 0,
                pack_struct: Some(PackStruct::default()),
            },
            metadata,
        }
    }

    /// 返回该项的名称 / Returns the name of this item
    #[must_use]
    pub fn name(&self) -> &String {
        &self.name
    }

    /// 返回该项的类型（文件或目录）/ Returns the item type (File or Dir)
    #[must_use]
    pub fn item_type(&self) -> &PackStructItemType {
        &self.item_type
    }

    /// 返回该项的元数据运行状态 / Returns the metadata run for this item
    #[must_use]
    pub fn metadata(&self) -> &PackFileMetadataRun {
        &self.metadata
    }

    pub(crate) fn metadata_file_pos(&self) -> u64 {
        self.metadata_file_pos
    }

    pub(crate) fn set_metadata_file_pos(&mut self, pos: u64) {
        self.metadata_file_pos = pos;
    }

    pub(crate) fn metadata_mut(&mut self) -> &mut PackFileMetadataRun {
        &mut self.metadata
    }

    pub(crate) fn item_type_mut(&mut self) -> &mut PackStructItemType {
        &mut self.item_type
    }

    pub(crate) fn load(data: &[u8]) -> Result<Self> {
        let type_value = &data[PACK_STRUCT_ITEM_TYPE_INDEX];
        let name_len = u16::from_le_bytes(
            data[
                PACK_STRUCT_ITEM_NAME_LEN_INDEX..PACK_STRUCT_ITEM_NAME_LEN_INDEX +
                    PACK_STRUCT_ITEM_NAME_LEN_LEN
                ]
                .try_into()
                .unwrap()
        );
        let name_end_pos = PACK_STRUCT_ITEM_NAME_INDEX + (name_len as usize);
        let name = String::from_utf8(
            data[PACK_STRUCT_ITEM_NAME_INDEX..name_end_pos].to_vec()
        ).unwrap();
        let metadata_file_pos = u64::from_le_bytes(
            data[name_end_pos..name_end_pos + PACK_STRUCT_ITEM_METADATA_FILE_POS_LEN]
                .try_into()
                .unwrap()
        );
        let type_data_start_pos = name_end_pos + PACK_STRUCT_ITEM_METADATA_FILE_POS_LEN;
        let type_data = &data[type_data_start_pos..];
        let item_type = match type_value {
            0 =>
                PackStructItemType::File {
                    handle: None,
                },
            1 =>
                PackStructItemType::Dir {
                    struct_file_pos: u64::from_le_bytes(
                        type_data[..PACK_STRUCT_DIR_STRUCT_FILE_POS_LEN].try_into().unwrap()
                    ),
                    pack_struct: None,
                },
            _ => Err(PackFileError::Format(format!("未知类型:{type_value}")))?,
        };
        Ok(Self {
            name,
            metadata_file_pos,
            item_type,
            metadata: PackFileMetadataRun::NoLoad,
        })
    }

    pub(crate) fn to_bytes_vec(&self) -> Vec<u8> {
        let name_vec = self.name.as_bytes().to_vec();
        let name_len = name_vec.len();
        let type_data = match &self.item_type {
            PackStructItemType::File { .. } => (0, Vec::new()),
            PackStructItemType::Dir { struct_file_pos, .. } =>
                (
                    1,
                    {
                        let mut data = Vec::with_capacity(PACK_STRUCT_DIR_STRUCT_FILE_POS_LEN);
                        for to_le_byte in struct_file_pos.to_le_bytes() {
                            data.push(to_le_byte);
                        }
                        assert!(!data.is_empty(), "输出的数据为空， 但不能为空");
                        data
                    },
                ),
        };
        let data_len =
            PACK_STRUCT_ITEM_LEN_LEN +
                PACK_STRUCT_ITEM_TYPE_LEN +
                PACK_STRUCT_ITEM_NAME_LEN_LEN +
                name_len +
                PACK_STRUCT_ITEM_METADATA_FILE_POS_LEN +
                type_data.1.len();
        let mut data = Vec::with_capacity(data_len);
        for to_le_byte in data_len.to_le_bytes() {
            data.push(to_le_byte);
        }
        data.push(type_data.0);
        for to_le_byte in u16::try_from(name_len).unwrap().to_le_bytes() {
            data.push(to_le_byte);
        }
        for name_b in name_vec {
            data.push(name_b);
        }
        for to_le_byte in self.metadata_file_pos.to_le_bytes() {
            data.push(to_le_byte);
        }
        for type_datum in type_data.1 {
            data.push(type_datum);
        }
        assert!(!data.is_empty(), "输出的数据为空， 但不能为空");
        data
    }
}

const PACK_STRUCT_DIR_STRUCT_FILE_POS_LEN: usize = 8;

/// 包目录结构项类型 / Pack struct item type
#[derive(Debug, Clone)]
pub enum PackStructItemType {
    /// 文件类型 / File type
    File {
        handle: Option<Weak<Mutex<PackFileHandle>>>,
    },
    /// 目录类型 / Directory type
    Dir {
        /// 子目录结构在文件中的位置 / Position of the subdirectory struct in the file
        struct_file_pos: u64,
        /// 子目录结构（已加载时）/ Subdirectory struct (when loaded)
        pack_struct: Option<PackStruct>,
    },
}

impl PartialEq for PackStructItemType {
    fn eq(&self, other: &Self) -> bool {
        match self {
            Self::File { .. } => matches!(other, Self::File { .. }),
            Self::Dir { struct_file_pos, pack_struct } => {
                if
                let Self::Dir {
                    struct_file_pos: o_struct_file_pos,
                    pack_struct: o_pack_struct,
                } = other
                {
                    struct_file_pos == o_struct_file_pos && pack_struct == o_pack_struct
                } else {
                    false
                }
            }
        }
    }
}

/// 包目录结构 / Pack struct
///
/// 代表包文件中的一个目录节点，包含子文件和子目录的映射表。
/// Represents a directory node in the pack file, containing a map of child files and subdirectories.
#[derive(Clone, Debug)]
pub struct PackStruct {
    /// 子项映射表（名称 → 项）/ Children map (name → item)
    items: HashMap<String, PackStructItem>,
    /// 数据块 / Data block
    data_block: ManifestDataBlock,
    /// 是否脏（未写入）/ Whether dirty (not yet written)
    dirty: bool,
}

impl Default for PackStruct {
    fn default() -> Self {
        Self {
            items: HashMap::default(),
            data_block: ManifestDataBlock::default(),
            dirty: false,
        }
    }
}

impl PartialEq for PackStruct {
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items && self.data_block == other.data_block
    }
}

impl PackStruct {
    /// 返回目录中的子项映射表 / Returns the items in this pack struct
    ///
    /// key 是文件/目录名, value 是对应的结构项。
    /// Key is the file/directory name, value is the corresponding struct item.
    #[must_use]
    pub fn items(&self) -> &HashMap<String, PackStructItem> {
        &self.items
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub(crate) fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub(crate) fn add_item(&mut self, name: String, item: PackStructItem) {
        self.items.insert(name, item);
        self.dirty = true;
    }

    pub(crate) fn remove_item(&mut self, name: &str) -> Option<PackStructItem> {
        let result = self.items.remove(name);
        if result.is_some() {
            self.dirty = true;
        }
        result
    }

    pub(crate) fn get_item(&self, name: &str) -> Option<&PackStructItem> {
        self.items.get(name)
    }

    pub(crate) fn get_item_mut(&mut self, name: &str) -> Option<&mut PackStructItem> {
        self.items.get_mut(name)
    }

    pub(crate) fn set_item_metadata_file_pos(&mut self, name: &str, pos: u64) -> Result<()> {
        if let Some(item) = self.items.get_mut(name) {
            item.metadata_file_pos = pos;
            self.dirty = true;
            Ok(())
        } else {
            Err(PackFileError::NotFound(format!("结构项 \"{name}\" 不存在")))
        }
    }

    pub(crate) fn set_item_struct_file_pos(&mut self, name: &str, pos: u64) -> Result<()> {
        if let Some(item) = self.items.get_mut(name) {
            match &mut item.item_type {
                PackStructItemType::Dir { struct_file_pos, .. } => {
                    *struct_file_pos = pos;
                    self.dirty = true;
                    Ok(())
                }
                PackStructItemType::File { .. } => {
                    Err(PackFileError::Format(format!(r#"结构项 "{name}" 是文件不是目录"#)))
                }
            }
        } else {
            Err(PackFileError::NotFound(format!("结构项 \"{name}\" 不存在")))
        }
    }

    pub(crate) fn load_item_metadata(
        &mut self,
        name: &str,
        metadata: PackFileMetadata,
    ) -> Result<()> {
        if let Some(item) = self.items.get_mut(name) {
            item.metadata = PackFileMetadataRun::Loaded(Box::from(metadata));
            Ok(())
        } else {
            Err(PackFileError::NotFound(format!("结构项 \"{name}\" 不存在")))
        }
    }

    pub(crate) fn unlock_item_metadata(
        &mut self,
        name: &str,
        metadata: PackFileMetadata,
    ) -> Result<()> {
        if let Some(item) = self.items.get_mut(name) {
            item.metadata.unlock(metadata);
            Ok(())
        } else {
            Err(PackFileError::NotFound(format!("结构项 \"{name}\" 不存在")))
        }
    }

    pub(crate) fn try_lock_item_metadata(&mut self, name: &str) -> Result<PackFileMetadata> {
        if let Some(item) = self.items.get_mut(name) {
            item.metadata.try_lock()
        } else {
            Err(PackFileError::NotFound(format!("结构项 \"{name}\" 不存在")))
        }
    }

    pub(crate) fn take_item_metadata(
        &mut self,
        name: &str,
    ) -> Result<Option<PackFileMetadata>> {
        if let Some(item) = self.items.get_mut(name) {
            Ok(item.metadata.take(PackFileMetadataRun::NoLoad))
        } else {
            Err(PackFileError::NotFound(format!("结构项 \"{name}\" 不存在")))
        }
    }

    pub(crate) fn set_item_pack_struct(&mut self, name: &str, ps: PackStruct) -> Result<()> {
        if let Some(item) = self.items.get_mut(name) {
            match &mut item.item_type {
                PackStructItemType::Dir { pack_struct, .. } => {
                    *pack_struct = Some(ps);
                    Ok(())
                }
                PackStructItemType::File { .. } => {
                    Err(PackFileError::Format(format!(r#"结构项 "{name}" 是文件不是目录"#)))
                }
            }
        } else {
            Err(PackFileError::NotFound(format!("结构项 \"{name}\" 不存在")))
        }
    }

    pub(crate) fn load(data_block: ManifestDataBlock) -> Result<Self> {
        let empty_data = Vec::new();
        let data = data_block.get_this_data().unwrap_or(&empty_data);
        let data_len = data.len();

        let mut read_len = 0;
        let all_read_len = data_len;
        let mut items = HashMap::new();
        while read_len < all_read_len {
            let item_data_len = usize::from_le_bytes(
                data[read_len..read_len + PACK_STRUCT_ITEM_LEN_LEN].try_into().unwrap()
            );
            let item = PackStructItem::load(&data[read_len..read_len + item_data_len])?;
            items.insert(item.name.clone(), item);
            read_len += item_data_len;
        }
        Ok(Self {
            items,
            data_block,
            dirty: false,
        })
    }

    pub(crate) fn get_block_data(&mut self) -> (Vec<u8>, bool) {
        let update = self.to_bytes_vec();
        let data_block = &mut self.data_block;
        let new_block = data_block.update(&update);
        (data_block.get_block_data().to_vec(), new_block)
    }
}

impl ManifestDataBlockTrait for PackStruct {
    fn to_bytes_vec(&self) -> Vec<u8> {
        let mut items_data = Vec::new();
        for item in &self.items {
            let item = item.1;
            let item_vec = item.to_bytes_vec();
            for item_data in item_vec {
                items_data.push(item_data);
            }
        }
        assert_eq!(
            self.items.is_empty(),
            items_data.is_empty(),
            "输出的数据为空，但存在数据，不应为空"
        );
        items_data
    }

    fn data_block_mut(&mut self) -> &mut ManifestDataBlock {
        &mut self.data_block
    }
}

/// 文件元数据运行状态 / File metadata run state
///
/// 状态机管理元数据的加载、锁定和释放。
/// State machine managing metadata loading, locking, and releasing.
#[derive(Clone, Debug, PartialEq)]
pub enum PackFileMetadataRun {
    /// 不存在元数据
    None,
    /// 未加载 / Not loaded
    NoLoad,
    /// 已加载 / Loaded
    Loaded(Box<PackFileMetadata>),
    /// 已锁定（正在写入）/ Locked (writing in progress)
    Locked,
}

impl PackFileMetadataRun {
    pub(crate) fn take(&mut self, this_type: Self) -> Option<PackFileMetadata> {
        match std::mem::replace(self, this_type) {
            Self::Loaded(m) => Some(*m),
            _ => None,
        }
    }

    pub(crate) fn try_lock(&mut self) -> Result<PackFileMetadata> {
        match self {
            Self::Loaded(_) => Ok(self.take(Self::Locked).expect("行为异常")),
            Self::NoLoad => Err(PackFileError::State("实例没有被加载".into())),
            Self::Locked => Err(PackFileError::Lock("无法获得锁，已被锁定".into())),
            Self::None => Err(PackFileError::State("无法对没有元数据的类型获得锁".into())),
        }
    }

    pub(crate) fn unlock(&mut self, metadata: PackFileMetadata) {
        if self == &Self::Locked {
            self.take(PackFileMetadataRun::Loaded(Box::from(metadata)));
        }
    }

    fn _try_metadata_drop(&mut self) -> Result<()> {
        match self {
            Self::Loaded(_) => {
                drop(self.take(PackFileMetadataRun::NoLoad).expect("行为异常"));
                Ok(())
            }
            Self::Locked => Err(PackFileError::Lock("元数据正在被锁定".into())),
            _ => Ok(()),
        }
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

//文件===
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

//目录===
//数据格式字段
//文件数量
const PACK_METADATA_DIR_FILE_COUNT_LEN: usize = 8;
//目录数量
const PACK_METADATA_DIR_DIR_COUNT_INDEX: usize = PACK_METADATA_DIR_FILE_COUNT_LEN;
const PACK_METADATA_DIR_DIR_COUNT_LEN: usize = 8;

/// 文件元数据 / File metadata
///
/// 存储包内文件的元信息，包括大小、修改时间、类型及哈希等。
/// Stores metadata for a file inside the pack, including size, modification time, type, and hash.
#[derive(Debug, Clone)]
pub struct PackFileMetadata {
    /// 数据块 / Data block
    data_block: ManifestDataBlock,
    /// 是否启用写时复制 / Whether copy-on-write is enabled
    cow: bool,
    /// 文件数据长度（字节）/ File data length (bytes)
    len: u64,
    /// 最后修改时间（毫秒）/ Last modified time (milliseconds)
    modified: u128,
    /// 文件类型（文件或目录）/ File type (File or Dir)
    file_type: PackFileMetadataType,
    /// 是否脏（未写入）/ Whether dirty (not yet written)
    dirty: bool,
}

impl PartialEq for PackFileMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.data_block == other.data_block &&
            self.cow == other.cow &&
            self.len == other.len &&
            self.modified == other.modified &&
            self.file_type == other.file_type
    }
}

impl PackFileMetadata {
    pub(crate) fn new(
        cow: bool,
        len: u64,
        modified: u128,
        file_type: PackFileMetadataType,
    ) -> Self {
        Self {
            data_block: ManifestDataBlock::default(),
            cow,
            len,
            modified,
            file_type,
            dirty: false,
        }
    }

    pub(crate) fn new_empty_dir(cow: bool) -> Self {
        Self {
            data_block: ManifestDataBlock::default(),
            cow,
            len: 0,
            modified: 0,
            file_type: PackFileMetadataType::Dir {
                file_count: 0,
                dir_count: 0,
            },
            dirty: false,
        }
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub(crate) fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// 返回是否启用写时复制 / Returns whether copy-on-write is enabled
    #[must_use]
    pub fn cow(&self) -> bool {
        self.cow
    }

    /// 返回文件是否为空（长度为 0）/ Returns whether the file is empty (length 0)
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 返回文件数据长度（字节）/ Returns the file data length in bytes
    #[must_use]
    pub fn len(&self) -> u64 {
        self.len
    }

    /// 返回最后修改时间（毫秒）/ Returns last modified time in milliseconds
    #[must_use]
    pub fn modified(&self) -> u128 {
        self.modified
    }

    /// 返回文件元数据类型（文件或目录）/ Returns the metadata type (File or Dir)
    #[must_use]
    pub fn file_type(&self) -> &PackFileMetadataType {
        &self.file_type
    }

    pub(crate) fn file_type_mut(&mut self) -> &mut PackFileMetadataType {
        &mut self.file_type
    }

    pub(crate) fn add_len(&mut self, delta: u64) {
        if delta > 0 {
            self.len += delta;
            self.dirty = true;
        }
    }

    pub(crate) fn set_len(&mut self, len: u64) {
        self.len = len;
        self.dirty = true;
    }

    pub(crate) fn add_file_count(&mut self, delta: u64) {
        if delta > 0 && let PackFileMetadataType::Dir { file_count, .. } = &mut self.file_type {
            *file_count += delta;
            self.dirty = true;
        }
    }

    pub(crate) fn add_dir_count(&mut self, delta: u64) {
        if delta > 0 && let PackFileMetadataType::Dir { dir_count, .. } = &mut self.file_type {
            *dir_count += delta;
            self.dirty = true;
        }
    }

    pub(crate) fn set_hash_value(&mut self, value: Vec<u8>) {
        if let PackFileMetadataType::File { hash_value, .. } = &mut self.file_type {
            *hash_value = value;
            self.dirty = true;
        }
    }

    pub(crate) fn load(data_block: ManifestDataBlock) -> Result<Self> {
        let data = data_block.get_this_data()?;
        let this_type = data[0];
        if this_type > 1 {
            std::hint::black_box(());
        }
        let bool_data = data[PACK_FILE_METADATA_BOOL_DATA_INDEX];
        let cow = (bool_data >> 7) == 1;
        let len = u64::from_le_bytes(
            data[PACK_FILE_METADATA_LEN_INDEX..PACK_FILE_METADATA_MODIFIED_INDEX]
                .try_into()
                .unwrap()
        );
        let modified = u128::from_le_bytes(
            data[PACK_FILE_METADATA_MODIFIED_INDEX..PACK_FILE_METADATA_TYPE_DATA_INDEX]
                .try_into()
                .unwrap()
        );
        let type_data = &data[PACK_FILE_METADATA_TYPE_DATA_INDEX..];
        let file_type = match this_type {
            0 => {
                if !data_block.this_data_hash_x()? {
                    Err(PackFileError::Integrity("文件的哈希验证未通过".into()))?;
                }
                let hash_type = type_data[PACK_METADATA_FILE_HASH_TYPE_INDEX];
                let hash_len = type_data[PACK_METADATA_FILE_HASH_LEN_INDEX];
                let hash_value =
                    type_data[
                        PACK_METADATA_FILE_HASH_INDEX..PACK_METADATA_FILE_HASH_INDEX +
                            (hash_len as usize)
                        ].to_vec();
                let data_pos_list_count_index = PACK_METADATA_FILE_HASH_INDEX + (hash_len as usize);
                let data_pos_list = DataPosList::load(
                    &type_data[data_pos_list_count_index..],
                    None,
                );
                PackFileMetadataType::File {
                    hash_type,
                    hash_value,
                    data_pos_list,
                }
            }
            1 => {
                let file_count = u64::from_le_bytes(
                    type_data[..PACK_METADATA_DIR_DIR_COUNT_INDEX].try_into().unwrap()
                );
                let dir_count = u64::from_le_bytes(
                    type_data[
                        PACK_METADATA_DIR_DIR_COUNT_INDEX..PACK_METADATA_DIR_DIR_COUNT_INDEX +
                            PACK_METADATA_DIR_DIR_COUNT_LEN
                        ]
                        .try_into()
                        .unwrap()
                );
                PackFileMetadataType::Dir {
                    file_count,
                    dir_count,
                }
            }
            _ => Err(PackFileError::Format(format!("未知类型:{this_type}")))?,
        };

        Ok(Self {
            data_block,
            cow,
            len,
            modified,
            file_type,
            dirty: false,
        })
    }
}

impl ManifestDataBlockTrait for PackFileMetadata {
    fn to_bytes_vec(&self) -> Vec<u8> {
        let type_data = match &self.file_type {
            PackFileMetadataType::File { hash_type, hash_value, data_pos_list } => {
                let data_pos_list_data = data_pos_list.to_bytes_vec();
                let mut data = Vec::with_capacity(
                    PACK_METADATA_FILE_HASH_TYPE_LEN +
                        PACK_METADATA_FILE_HASH_LEN_LEN +
                        hash_value.len() +
                        data_pos_list_data.len()
                );
                data.push(*hash_type);
                let hash_len = u8::try_from(hash_value.len()).expect("哈希值长度值过大");
                data.push(hash_len);
                for hash in hash_value {
                    data.push(*hash);
                }
                for data_pos_list_data in data_pos_list_data {
                    data.push(data_pos_list_data);
                }
                data
            }
            PackFileMetadataType::Dir { file_count, dir_count } => {
                let mut data = Vec::with_capacity(
                    PACK_METADATA_DIR_FILE_COUNT_LEN + PACK_METADATA_DIR_DIR_COUNT_LEN
                );
                for to_le_byte in file_count.to_le_bytes() {
                    data.push(to_le_byte);
                }
                for to_le_byte in dir_count.to_le_bytes() {
                    data.push(to_le_byte);
                }
                data
            }
        };
        let mut data = Vec::with_capacity(
            PACK_FILE_METADATA_TYPE_LEN +
                PACK_FILE_METADATA_BOOL_DATA_LEN +
                PACK_FILE_METADATA_MODIFIED_LEM +
                type_data.len()
        );
        data.push(self.file_type.to_u8_type());
        let mut bool_data = 0;
        if self.cow {
            bool_data |= 0b1000_0000;
        }
        data.push(bool_data);
        for to_le_byte in self.len.to_le_bytes() {
            data.push(to_le_byte);
        }
        for to_le_byte in self.modified.to_le_bytes() {
            data.push(to_le_byte);
        }
        for type_datum in type_data {
            data.push(type_datum);
        }
        assert!(!data.is_empty(), "输出的数据为空， 但不能为空");
        data
    }

    fn data_block_mut(&mut self) -> &mut ManifestDataBlock {
        &mut self.data_block
    }
}

/// 文件元数据类型 / File metadata type
#[derive(PartialEq, Debug, Clone)]
pub enum PackFileMetadataType {
    /// 文件 / file
    File {
        /// 哈希类型（1 = Blake3）/ Hash type (1 = Blake3)
        hash_type: u8,
        /// 哈希值 / Hash value
        hash_value: Vec<u8>,
        /// 数据位置列表 / Data position list
        data_pos_list: DataPosList,
    },
    /// 目录 / Directory
    Dir {
        /// 子文件数 / Child file count
        file_count: u64,
        /// 子目录数 / Child directory count
        dir_count: u64,
    },
}

impl PackFileMetadataType {
    fn to_u8_type(&self) -> u8 {
        match self {
            Self::File { .. } => 0,
            Self::Dir { .. } => 1,
        }
    }
}

//清单数据运行数据
/// 清单数据运行数据（预留）/ Manifest data run data (reserved)
#[derive(Default, Debug)]
pub struct WBFilesPackManifestRun {}

/// 清单数据结构 / Manifest data structure
///
/// 包文件的清单，包含属性、根目录结构和文件 IO 句柄。
/// The manifest of the pack file, containing attributes, root struct, and file IO handle.
#[derive(Debug)]
pub struct WBFilesPackManifest {
    /// 清单属性 / Manifest attribute
    attribute: Attribute,
    /// 根目录结构 / Root struct
    root_struct: PackStruct,
    /// 清单文件 IO / Manifest file IO
    file: Option<crate::wb_files_pack::pack_io::PackIO>,
    /// 运行数据 / Run data
    _run_data: WBFilesPackManifestRun,
}

impl WBFilesPackManifest {
    /// 返回清单的属性信息 / Returns the manifest attribute
    #[must_use]
    pub fn attribute(&self) -> &Attribute {
        &self.attribute
    }

    pub(crate) fn attribute_mut(&mut self) -> &mut Attribute {
        &mut self.attribute
    }

    /// 返回清单的根目录结构 / Returns the root pack struct
    #[must_use]
    pub fn root_struct(&self) -> &PackStruct {
        &self.root_struct
    }

    pub(crate) fn root_struct_mut(&mut self) -> &mut PackStruct {
        &mut self.root_struct
    }

    pub(crate) fn file(&self) -> &Option<crate::wb_files_pack::pack_io::PackIO> {
        &self.file
    }

    pub(crate) fn file_mut(&mut self) -> &mut Option<crate::wb_files_pack::pack_io::PackIO> {
        &mut self.file
    }

    pub(crate) fn new(
        attribute: Attribute,
        root_struct: PackStruct,
        file: Option<crate::wb_files_pack::pack_io::PackIO>,
    ) -> Self {
        Self {
            attribute,
            root_struct,
            file,
            _run_data: WBFilesPackManifestRun::default(),
        }
    }
}

#[cfg(test)]
#[test]
fn get_block_len_us() {
    assert_eq!(
        ManifestDataBlock::get_block_len_us(
            DATA_BLOCK_LEN / 2 -
                MANIFEST_DATA_BLOCK_DATA_LEN_LEN -
                MANIFEST_DATA_BLOCK_DATA_VER_LEN -
                MANIFEST_DATA_BLOCK_DATA_HASH_LEN +
                1
        ),
        DATA_BLOCK_LEN * 2
    );
}
