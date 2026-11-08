/// 水球包文件模块 / Water Ball Files Pack module
///
/// 提供容器文件格式的创建、读写、管理功能。
/// Provides container file format creation, read/write, and management functionality.
pub mod manager;

mod data;
pub mod error;
/// 线程安全同步管理器模块 / Thread-safe synchronized manager module
pub mod manager_sync;
///网络服务
#[cfg(debug_assertions)]
mod net_server;
mod pack_io;
#[cfg(test)]
mod test;

pub use error::{PackFileError, Result};

//AI=== 只读包探测：不获取 .lock / read-only pack probe without acquiring the .lock
/// 包文件头探测结果 / pack header probe result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackProbe {
    /// 文件头格式版本 / file-header format version.
    pub format_version: u16,
}

/// 只读探测一个文件是否为水球包，不获取 `.lock`。
///
/// 只读取文件头中的 magic、格式版本和声明长度，不打开清单、不修改文件。
/// I/O 错误原样返回；不是水球包时返回 `Ok(None)`。
///
/// Probe whether a file is a WaterBall pack without acquiring `.lock`.
/// Only the header magic, format version and declared length are read; the
/// manifest is not opened and the file is never modified. I/O errors are
/// returned as-is; a non-pack file yields `Ok(None)`.
pub fn probe_pack(path: &std::path::Path) -> Result<Option<PackProbe>> {
    use std::io::Read;

    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() < pack_io::FILE_HEADER_DATA_LENGTH as u64 {
        return Ok(None);
    }
    let mut file = std::fs::File::open(path)?;
    let mut header = [0u8; pack_io::FILE_HEADER_DATA_LENGTH];
    if file.read_exact(&mut header).is_err() {
        return Ok(None);
    }
    let type_end = pack_io::FILE_HEADER_VERSION_INDEX;
    if header[..type_end] != pack_io::FILE_HEADER_TYPE_NAME {
        return Ok(None);
    }
    let version_end = type_end + pack_io::FILE_HEADER_VERSION.len();
    if header[type_end..version_end] != pack_io::FILE_HEADER_VERSION {
        return Ok(None);
    }
    let len_start = pack_io::FILE_HEADER_DATA_LENGTH_INDEX;
    let len_end = len_start + pack_io::FILE_HEADER_DATA_LENGTH_LENGTH;
    let pack_len = u64::from_le_bytes(
        header[len_start..len_end]
            .try_into()
            .map_err(|_| PackFileError::Format("文件头长度字段不完整".into()))?,
    );
    if pack_len < pack_io::FILE_HEADER_BLOCK_LEN as u64 {
        return Ok(None);
    }
    Ok(Some(PackProbe {
        format_version: u16::from_be_bytes(
            header[type_end..version_end]
                .try_into()
                .map_err(|_| PackFileError::Format("文件头版本字段不完整".into()))?,
        ),
    }))
}
//AI_END===

// 重导出公共数据类型 / Re-export public data types
pub use data::{
    Attribute, DataPosList, OverwriteStrategy, PackFileMetadata, PackFileMetadataRun,
    PackFileMetadataType, PackStruct, PackStructItem, PackStructItemType, WBFilesPackManifest,
};
pub(crate) use data::{
    DATA_BLOCK_LEN, DATA_DATA_BLOCK_LEN, MANIFEST_ATTRIBUTE_BLOCK_LEN, MAX_DATA_SEGMENTS,
    ManifestDataBlock, ManifestDataBlockTrait,
};
pub use data::{MANIFEST_VERSION, MANIFEST_VERSION_COMPATIBLE};
