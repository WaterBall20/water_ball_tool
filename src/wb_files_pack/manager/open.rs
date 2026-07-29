use crate::wb_files_pack::pack_io::{
    PackIO, FILE_HEADER_BLOCK_LEN, FILE_HEADER_BOOL_DATA_INDEX,
    FILE_HEADER_DATA_LENGTH, FILE_HEADER_DATA_LENGTH_INDEX,
    FILE_HEADER_DATA_LENGTH_LENGTH, FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX, FILE_HEADER_TYPE_NAME,
    FILE_HEADER_VERSION, FILE_HEADER_VERSION_INDEX,
};
use crate::wb_files_pack::{
    Attribute, DataPosList, ManifestDataBlock, ManifestDataBlockTrait, PackStruct,
    WBFilesPackManifest,
};
use crate::wb_files_pack::error::{PackFileError, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use super::WBFPManager;

impl WBFPManager {
    pub(crate) fn init_new_pack(&mut self) -> Result<()> {
        let mut file_header_buf = vec![0; FILE_HEADER_BLOCK_LEN];
        for (index, value) in FILE_HEADER_TYPE_NAME.iter().enumerate() {
            file_header_buf[index] = *value;
        }
        for (index, value) in FILE_HEADER_VERSION.iter().enumerate() {
            file_header_buf[FILE_HEADER_VERSION_INDEX + index] = *value;
        }
        let mut header_tag: u8 = 0;
        if self.cow {
            header_tag |= 0b1000_0000;
        }
        if self.separate_manifest {
            header_tag |= 0b0100_0000;
        }
        file_header_buf[FILE_HEADER_BOOL_DATA_INDEX] = header_tag;
        for (index, value) in FILE_HEADER_BLOCK_LEN.to_le_bytes().iter().enumerate() {
            file_header_buf[FILE_HEADER_DATA_LENGTH_INDEX + index] = *value;
        }
        for (index, byte) in self
            .manifest
            .attribute_mut()
            .get_block_data()?
            .0
            .into_iter()
            .enumerate()
        {
            file_header_buf[FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX + index] = byte;
        }
        file_header_buf.resize(FILE_HEADER_BLOCK_LEN, 0);
        {
            let pack_file = self.pack_file.clone();
            let mut pack_file = pack_file
                .lock()
                .map_err(|err| PackFileError::Lock(format!("无法获得包文件锁, err: {err}")))?;
            pack_file
                .write_all(&file_header_buf)?;
            pack_file
                .set_len(FILE_HEADER_BLOCK_LEN as u64)?;
        }
        self.save_empty_data_list()?;
        if self.separate_manifest {
            self.save_manifest_empty_data_list()?;
        }
        self.manifest.root_struct_mut().mark_dirty();
        self.save_root_pack_struct()?;
        Ok(())
    }

    pub(crate) fn open_pack_file<P: AsRef<Path>>(
        pack_path: &P,
        pack_file: Arc<Mutex<PackIO>>,
    ) -> Result<WBFPManager> {
        const HEADER_TYPE_LEN: usize = FILE_HEADER_TYPE_NAME.len();
        let m_pack_file_arc = pack_file.clone();
        let mut m_pack_file = m_pack_file_arc
            .lock()
            .map_err(|err| PackFileError::Lock(format!("获得包文件IO锁错误, err: {err}")))?;
        let pack_path = pack_path
            .as_ref()
            .to_str()
            .ok_or(PackFileError::Format("无法将路径转换成文本".into()))?
            .to_string();
        let mut header_block_data = vec![0; FILE_HEADER_BLOCK_LEN];
        let header_block_r_len = m_pack_file.read(&mut header_block_data)?;
        if header_block_r_len < FILE_HEADER_DATA_LENGTH {
            return Err(PackFileError::Format("无法读取完整的文件头".into()));
        }
        let header = &header_block_data[..FILE_HEADER_DATA_LENGTH];
        if header[..HEADER_TYPE_LEN] != FILE_HEADER_TYPE_NAME {
            return Err(PackFileError::Format("文件类型不是水球包文件".into()));
        }
        if header[HEADER_TYPE_LEN..HEADER_TYPE_LEN + 2] != FILE_HEADER_VERSION {
            return Err(PackFileError::Version(
                "文件格式版本不一致，对于文件格式，版本必须一致".into(),
            ));
        }
        let bool_data = header[FILE_HEADER_BOOL_DATA_INDEX];
        let separate_manifest = ((bool_data << 1) >> 7) == 1;
        let pack_len = {
            let arr: [u8; 8] = header[FILE_HEADER_DATA_LENGTH_INDEX
                ..(FILE_HEADER_DATA_LENGTH_INDEX + FILE_HEADER_DATA_LENGTH_LENGTH)]
                .try_into()
                .map_err(|_| PackFileError::Format("文件头长度字段长度固定".into()))?;
            u64::from_le_bytes(arr)
        };
        m_pack_file.len = pack_len;
        let attribute_data =
            &header_block_data[FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX..FILE_HEADER_BLOCK_LEN];
        let attribute_data = ManifestDataBlock::from_block_data_new(
            attribute_data.to_vec(),
            FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX as u64,
        )?;
        let attribute = Attribute::load(attribute_data)?;
        let mut write_lock_file_path = pack_path.clone();
        write_lock_file_path.push_str(".lock");
        let write_lock_file = Self::write_lock_file(&PathBuf::from(write_lock_file_path))?;
        if separate_manifest {
            Self::open_pack_file_with_separate_manifest(
                pack_file,
                &mut m_pack_file,
                &pack_path,
                separate_manifest,
                attribute,
                write_lock_file,
            )
        } else {
            let empty_pos_data_block =
                m_pack_file.manifest_data_block_read(attribute.empty_data_pos_list_pos())?;
            let empty_pos_data = empty_pos_data_block
                .get_this_data()?
                .to_vec();
            m_pack_file.empty_data_list =
                DataPosList::load(&empty_pos_data, Some(empty_pos_data_block));
            let root_struct_block_data =
                m_pack_file.manifest_data_block_read(attribute.root_struct_pos())?;
            let root_struct = PackStruct::load(root_struct_block_data)?;
            let manifest = WBFilesPackManifest::new(attribute, root_struct, None);
            drop(m_pack_file);
            drop(m_pack_file_arc);
            WBFPManager::new(
                pack_path,
                manifest,
                pack_file,
                separate_manifest,
                Some(write_lock_file),
            )
        }
    }

    pub(super) fn open_pack_file_with_separate_manifest(
        pack_file: Arc<Mutex<PackIO>>,
        m_pack_file: &mut MutexGuard<PackIO>,
        pack_path: &String,
        separate_manifest: bool,
        attribute: Attribute,
        write_lock_file: File,
    ) -> Result<WBFPManager> {
        let mut manifest_path = pack_path.clone();
        manifest_path.push_str(".wbm");
        let manifest_file = File::options().read(true).write(true).open(manifest_path)?;
        let mut manifest_file = PackIO::new2(manifest_file, attribute.manifest_file_len());
        let empty_pos_data_block =
            manifest_file.manifest_data_block_read(attribute.empty_data_pos_list_pos())?;
        let empty_pos_data = empty_pos_data_block
            .get_this_data()?
            .to_vec();
        m_pack_file.empty_data_list =
            DataPosList::load(&empty_pos_data, Some(empty_pos_data_block));
        let manifest_empty_pos_data_block =
            manifest_file.manifest_data_block_read(attribute.manifest_empty_data_pos_list_pos())?;
        let manifest_empty_pos_data = manifest_empty_pos_data_block
            .get_this_data()?
            .to_vec();
        manifest_file.empty_data_list = DataPosList::load(
            &manifest_empty_pos_data,
            Some(manifest_empty_pos_data_block),
        );
        let root_struct_block_data =
            manifest_file.manifest_data_block_read(attribute.root_struct_pos())?;
        let root_struct = PackStruct::load(root_struct_block_data)?;
        let manifest = WBFilesPackManifest::new(attribute, root_struct, Some(manifest_file));
        WBFPManager::new(
            pack_path,
            manifest,
            pack_file,
            separate_manifest,
            Some(write_lock_file),
        )
    }

    pub(crate) fn create_pack_file<P: AsRef<Path>>(
        path: &P,
        pack_file: Arc<Mutex<PackIO>>,
        cow: bool,
        separate_manifest: bool,
        create_new: bool,
    ) -> Result<WBFPManager> {
        let mut write_lock_path = path
            .as_ref()
            .to_str()
            .ok_or(PackFileError::Format("无法将路径转换成文本".into()))?
            .to_string();
        write_lock_path.push_str(".lock");
        let write_lock_path = PathBuf::from(write_lock_path);
        let write_lock_file = Self::write_lock(false, &write_lock_path)?;
        let manifest_file = if separate_manifest {
            let mut manifest_path =
                String::from(path.as_ref().to_str().ok_or(PackFileError::Format("无法将路径转换成文本".into()))?);
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
        Self::create_pack(
            path,
            cow,
            pack_file,
            separate_manifest,
            manifest_file,
            write_lock_file,
        )
    }

    pub(super) fn create_pack<P: AsRef<Path>>(
        pack_path: &P,
        cow: bool,
        pack_file: Arc<Mutex<PackIO>>,
        separate_manifest: bool,
        manifest_file: Option<PackIO>,
        write_lock_file: Option<File>,
    ) -> Result<WBFPManager> {
        let mut attribute = Attribute::default();
        attribute.set_cow(cow);
        WBFPManager::new(
            pack_path,
            WBFilesPackManifest::new(attribute, PackStruct::default(), manifest_file),
            pack_file,
            separate_manifest,
            write_lock_file,
        )
    }
}
