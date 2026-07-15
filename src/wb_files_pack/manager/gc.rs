use crate::wb_files_pack::error::{PackFileError, Result};
use crate::wb_files_pack::DATA_BLOCK_LEN;

use super::WBFPManager;

impl WBFPManager {
    pub(super) fn get_file_pos(&mut self, length: u64) -> Result<(u64, u64)> {
        const DATA_BLOCK_LEN_U64: u64 = DATA_BLOCK_LEN as u64;
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| PackFileError::Lock(format!("无法获得包文件锁, err: {err}")))?;
        let length = if length.is_multiple_of(DATA_BLOCK_LEN_U64) {
            length
        } else {
            let length = length / DATA_BLOCK_LEN_U64 + 1;
            length * DATA_BLOCK_LEN_U64
        };
        Ok(pack_file.get_file_pos(length))
    }

    fn _file_gc_add(&mut self, gc_pos_list: Vec<(u64, u64)>) -> Result<()> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| PackFileError::Lock(format!("无法获得包文件锁, err: {err}")))?;
        pack_file.file_gc_add(gc_pos_list);
        Ok(())
    }

    pub(super) fn file_gc(&mut self) -> Result<()> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| PackFileError::Lock(format!("无法获得包文件锁, err: {err}")))?;
        pack_file.file_gc();
        drop(pack_file);
        self.save_empty_data_list()
    }

    pub(super) fn manifest_file_gc(&mut self) -> Result<()> {
        if let Some(file) = self.manifest.file_mut() {
            file.file_gc();
            self.save_manifest_empty_data_list()
        } else {
            Ok(())
        }
    }
}
