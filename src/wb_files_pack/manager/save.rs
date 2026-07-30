use crate::wb_files_pack::error::{PackFileError, Result};
use crate::wb_files_pack::pack_io::{
    FILE_HEADER_DATA_LENGTH_INDEX, FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX,
};
use crate::wb_files_pack::{
    DATA_BLOCK_LEN, ManifestDataBlock, ManifestDataBlockTrait, PackFileMetadata, PackStruct,
};
use std::io::Write;

use super::WBFPManager;

impl WBFPManager {
    pub(super) fn throttled_save(&mut self) -> Result<()> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| PackFileError::Lock(format!("无法获得包文件锁, err: {err}")))?;
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

    pub(crate) fn save_all(&mut self) -> Result<()> {
        self.file_gc()?;
        self.manifest_file_gc()?;
        self.save_root_pack_struct()?;
        self.save_pack_length()?;
        Ok(())
    }

    pub(super) fn save_empty_data_list(&mut self) -> Result<()> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| PackFileError::Lock(format!("无法获得包文件锁, err: {err}")))?;
        let old_pos = self.manifest.attribute().empty_data_pos_list_pos();
        let old_len = pack_file
            .empty_data_list
            .get_data_block_mut()
            .ok_or(PackFileError::State("空数据列表未加载".into()))?
            .get_this_block_len_u64();
        let (block_data, new_block) = pack_file
            .empty_data_list
            .get_block_data()
            .ok_or(PackFileError::State("空数据列表无法获取数据".into()))?;
        drop(pack_file);
        let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_len)?;
        if new_block {
            self.manifest
                .attribute_mut()
                .set_empty_data_pos_list_pos(pos);
        }
        Ok(())
    }

    pub(super) fn save_manifest_empty_data_list(&mut self) -> Result<()> {
        let old_pos = self.manifest.attribute().manifest_empty_data_pos_list_pos();
        if let Some(file) = self.manifest.file_mut() {
            let old_len = file
                .empty_data_list
                .get_data_block_mut()
                .ok_or(PackFileError::State("分离清单的空数据列表未加载".into()))?
                .get_this_block_len_u64();
            let (block_data, new_block) =
                file.empty_data_list
                    .get_block_data()
                    .ok_or(PackFileError::State(
                        "分离清单的空数据列表无法获取数据".into(),
                    ))?;
            let pos = self.manifest_data_block_write(&block_data, new_block, old_pos, old_len)?;
            if new_block {
                self.manifest
                    .attribute_mut()
                    .set_manifest_empty_data_pos_list_pos(pos);
            }
        }
        Ok(())
    }

    pub(super) fn save_root_pack_struct(&mut self) -> Result<()> {
        let old_pos = self.manifest.attribute().root_struct_pos();
        let (need_save, old_block_len, block_data, new_block) = {
            let root_struct = self.manifest.root_struct_mut();
            if root_struct.is_dirty() {
                let old_block_len = root_struct.data_block_mut().get_this_block_len_u64();
                let (block_data, new_block) = root_struct.get_block_data()?;
                (true, old_block_len, block_data, new_block)
            } else {
                (false, 0, Vec::new(), false)
            }
        };
        if need_save {
            let pos =
                self.manifest_data_block_write(&block_data, new_block, old_pos, old_block_len)?;
            self.manifest.attribute_mut().set_root_struct_pos(pos);
            self.manifest.root_struct_mut().clear_dirty();
        }
        self.save_manifest_attribute()?;
        Ok(())
    }

    pub(super) fn save_manifest_attribute(&mut self) -> Result<()> {
        let attribute = self.manifest.attribute_mut();
        if !attribute.is_dirty() {
            return Ok(());
        }
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| PackFileError::Lock(format!("无法获得包文件锁, err: {err}")))?;
        let data = attribute.get_block_data()?.0;
        pack_file.set_pos_write(FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX as u64)?;
        pack_file.write_all(&data)?;
        attribute.clear_dirty();
        Ok(())
    }

    pub(super) fn save_pack_length(&mut self) -> Result<()> {
        self.this_write_lock()?;
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| PackFileError::Lock(format!("无法获得包文件锁, err: {err}")))?;
        pack_file.sync_file_length();
        pack_file.set_pos_write(FILE_HEADER_DATA_LENGTH_INDEX as u64)?;
        let pack_len = pack_file.len;
        pack_file.write_all(pack_len.to_le_bytes().as_slice())?;
        Ok(())
    }

    pub(super) fn save_pack_struct_write(
        &mut self,
        pack_struct: &mut PackStruct,
    ) -> Result<(bool, u64)> {
        let current_pos = pack_struct.data_block_mut().file_pos();
        if !pack_struct.is_dirty() {
            return Ok((false, current_pos));
        }
        let old_block_len = pack_struct.data_block_mut().get_this_block_len_u64();
        let (block_data, new_block) = pack_struct.get_block_data()?;
        let pos =
            self.manifest_data_block_write(&block_data, new_block, current_pos, old_block_len)?;
        pack_struct.data_block_mut().set_file_pos(pos);
        pack_struct.clear_dirty();
        Ok((new_block, pos))
    }

    pub(super) fn save_metadata_write(
        &mut self,
        metadata: &mut PackFileMetadata,
    ) -> Result<(bool, u64)> {
        let current_pos = metadata.data_block_mut().file_pos();
        if !metadata.is_dirty() {
            return Ok((false, current_pos));
        }
        let old_block_len = metadata.data_block_mut().get_this_block_len_u64();
        let (block_data, new_block) = metadata.get_block_data()?;
        let pos =
            self.manifest_data_block_write(&block_data, new_block, current_pos, old_block_len)?;
        metadata.data_block_mut().set_file_pos(pos);
        metadata.clear_dirty();
        Ok((new_block, pos))
    }

    pub(super) fn manifest_data_block_read(&self, file_pos: u64) -> Result<ManifestDataBlock> {
        if !self.separate_manifest {
            let pack_file = self.pack_file.clone();
            let pack_file = pack_file
                .lock()
                .map_err(|err| PackFileError::Lock(format!("无法获得包文件锁, err: {err}")))?;
            pack_file.manifest_data_block_read(file_pos)
        } else if let Some(manifest_file) = self.manifest.file() {
            manifest_file.manifest_data_block_read(file_pos)
        } else {
            Err(PackFileError::State(
                "已启用清单分离文件，但清单文件实例不存在".into(),
            ))?
        }
    }

    pub(super) fn manifest_data_block_write(
        &mut self,
        block_data: &[u8],
        new_block: bool,
        old_pos: u64,
        old_block_len: u64,
    ) -> Result<u64> {
        if self.separate_manifest {
            if let Some(file) = self.manifest.file_mut() {
                file.manifest_data_block_write(block_data, new_block, old_pos, old_block_len)
            } else {
                Err(PackFileError::State(
                    "已启用清单分离文件，但清单文件实例不存在".into(),
                ))
            }
        } else {
            let pack_file = self.pack_file.clone();
            let mut pack_file = pack_file
                .lock()
                .map_err(|err| PackFileError::Lock(format!("无法获得包文件锁, err: {err}")))?;
            pack_file.manifest_data_block_write(block_data, new_block, old_pos, old_block_len)
        }
    }
}
