/// 水球包文件模块 / Water Ball Files Pack module
///
/// 提供容器文件格式的创建、读写、管理功能。
/// Provides container file format creation, read/write, and management functionality.
pub mod manager;

mod data;
mod pack_io;
#[cfg(test)]
mod test;
/// 线程安全分配器模块 / Thread-safe allocator module
pub mod allocator;
mod net_server;
pub mod error;

pub use error::{ PackFileError, Result };

// 重导出公共数据类型 / Re-export public data types
pub use data::{
    Attribute,
    DataPosList,
    OverwriteStrategy,
    PackFileMetadata,
    PackFileMetadataRun,
    PackFileMetadataType,
    PackStruct,
    PackStructItem,
    PackStructItemType,
    WBFilesPackManifest,
};
pub(crate) use data::{
    ManifestDataBlock,
    ManifestDataBlockTrait,
    DATA_BLOCK_LEN,
    DATA_DATA_BLOCK_LEN,
    MANIFEST_ATTRIBUTE_BLOCK_LEN,
};
pub use data::{ MANIFEST_VERSION, MANIFEST_VERSION_COMPATIBLE };
