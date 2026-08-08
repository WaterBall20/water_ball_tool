use crate::wb_files_pack::error::{PackFileError, Result};

use super::WBFPManager;

impl WBFPManager {
    /// 已知大小文件的初始分配：按 128B（DATA_BLOCK_LEN）精确对齐分配，
    /// 不多占空间。与清单数据分配使用同一对齐规则（PackIO::get_file_pos）。
    ///
    /// Known-size file initial allocation: exact 128B (DATA_BLOCK_LEN) alignment,
    /// no over-allocation. Same alignment rule as manifest data allocation.
    pub(super) fn get_file_pos(&mut self, length: u64) -> Result<(u64, u64)> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| PackFileError::Lock(format!("无法获得包文件锁, err: {err}")))?;
        Ok(pack_file.get_file_pos(length))
    }

    /// 未知大小文件的初始分配：长度按 4MiB（DATA_DATA_BLOCK_LEN）对齐，
    /// 优先复用空闲碎片（单块 → 紧贴连续 → 从大到小拼接），多余空间由
    /// 垃圾回收回收。清单数据分配不经过此方法（仍为 128B 对齐）。
    ///
    /// Unknown-size file initial allocation: aligned to 4MiB (DATA_DATA_BLOCK_LEN),
    /// reusing free fragments first (single block → contiguous → biggest-first
    /// stitching); excess space is reclaimed by GC. Manifest data does not use it.
    pub(super) fn get_data_file_pos(
        &mut self,
        length: u64,
        current_seg: usize,
        max_seg: usize,
    ) -> Result<Vec<(u64, u64)>> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| PackFileError::Lock(format!("无法获得包文件锁, err: {err}")))?;
        Ok(pack_file.get_data_file_pos_multi(length, current_seg, max_seg)?)
    }

    pub(crate) fn file_gc_add(&mut self, gc_pos_list: Vec<(u64, u64)>) -> Result<()> {
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
        pack_file.file_gc()?;
        drop(pack_file);
        self.save_empty_data_list()
    }

    pub(super) fn manifest_file_gc(&mut self) -> Result<()> {
        if let Some(file) = self.manifest.file_mut() {
            file.file_gc()?;
            self.save_manifest_empty_data_list()
        } else {
            Ok(())
        }
    }
}
