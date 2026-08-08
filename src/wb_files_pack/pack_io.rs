use crate::tools;
use crate::wb_files_pack::error::{PackFileError, Result};
use crate::wb_files_pack::{
    DATA_BLOCK_LEN, DATA_DATA_BLOCK_LEN, DataPosList, MANIFEST_ATTRIBUTE_BLOCK_LEN,
    ManifestDataBlock,
};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

/// 多段拼接结果 / Multi-segment stitching outcome
enum MultiSegError {
    /// 碎片不足→扩容兜底 / insufficient → extend-file fallback
    NotEnough,
    /// 段数超限→拒绝 / over limit → reject
    TooManySegments,
}

/// 虚拟文件读写器模块（`PackVirtualFile`）/ Virtual file reader-writer module (`PackVirtualFile`)
pub mod file;
pub mod file_handle;
pub mod file_hash;

//文件头===
//文件头-文件名:WPFilesPack
pub(in crate::wb_files_pack) const FILE_HEADER_TYPE_NAME: [u8; 11] = [
    0x57u8, 0x42, 0x46, 0x69, 0x6c, 0x65, 0x73, 0x50, 0x61, 0x63, 0x6b,
];
//文件头版本
pub(in crate::wb_files_pack) const FILE_HEADER_VERSION_INDEX: usize = FILE_HEADER_TYPE_NAME.len();
pub(in crate::wb_files_pack) const FILE_HEADER_VERSION: [u8; 2] = [0, 2];
//文件头标签位长度
pub(in crate::wb_files_pack) const FILE_HEADER_BOOL_DATA_INDEX: usize =
    FILE_HEADER_VERSION_INDEX + FILE_HEADER_VERSION.len();
pub(in crate::wb_files_pack) const FILE_HEADER_BOOL_DATA_LENGTH: usize = 1;

//文件头数据长度位置
pub(in crate::wb_files_pack) const FILE_HEADER_DATA_LENGTH_INDEX: usize =
    FILE_HEADER_BOOL_DATA_INDEX + FILE_HEADER_BOOL_DATA_LENGTH;

//文件头数据长度长度
pub(in crate::wb_files_pack) const FILE_HEADER_DATA_LENGTH_LENGTH: usize = 8;

//文件头长度
pub(in crate::wb_files_pack) const FILE_HEADER_DATA_LENGTH: usize = FILE_HEADER_TYPE_NAME.len()
    + FILE_HEADER_VERSION.len()
    + FILE_HEADER_BOOL_DATA_LENGTH
    + FILE_HEADER_DATA_LENGTH_LENGTH;

//文件头清单属性
pub(in crate::wb_files_pack) const FILE_HEADER_MANIFEST_ATTRIBUTE_INDEX: usize = DATA_BLOCK_LEN;

//文件头块长度
pub(in crate::wb_files_pack) const FILE_HEADER_BLOCK_LEN: usize =
    DATA_BLOCK_LEN + MANIFEST_ATTRIBUTE_BLOCK_LEN;

#[derive(Default, Debug)]
pub(crate) struct RunData {
    pos: u64,
    pub(crate) all_write_len: u64,
    //上次总写入的长度
    pub(crate) last_all_write_len: u64,
    //运行时总创建文件数量
    pub(crate) all_cr_file_count: u64,
    //上次创建总创建文件数量
    pub(crate) last_all_cr_file_count: u64,
    //GC数据列表
    gc_data_pos_list: DataPosList,
}

#[derive(Debug)]
pub(crate) struct PackIO {
    file: File,
    pub(crate) len: u64,
    //空数据列表
    pub(crate) empty_data_list: DataPosList,
    pub(crate) run_data: RunData,
}
impl PackIO {
    /// 创建新的 PackIO 实例。
    /// Create a new PackIO instance.
    pub(crate) fn new(file: File) -> Self {
        let mut empty_data_list = DataPosList::new(Vec::new());
        empty_data_list.set_data_block(Some(ManifestDataBlock::default()));
        Self {
            file,
            len: 0,
            empty_data_list,
            run_data: RunData::default(),
        }
    }

    /// 创建新的 PackIO 实例并指定文件长度。
    /// Create a new PackIO instance with a specified file length.
    pub(crate) fn new2(file: File, len: u64) -> Self {
        let mut empty_data_list = DataPosList::new(Vec::new());
        empty_data_list.set_data_block(Some(ManifestDataBlock::default()));
        Self {
            file,
            len,
            empty_data_list,
            run_data: RunData::default(),
        }
    }
}

impl Write for PackIO {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let file = &mut self.file;
        let len = file.write(buf)?;
        {
            let len = len as u64;
            self.run_data.pos += len;
            self.run_data.all_write_len += len;
        }
        self.sync_file_length();
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let file = &mut self.file;
        file.write_all(buf)?;
        let len = buf.len() as u64;
        self.run_data.pos += len;
        self.run_data.all_write_len += len;
        self.sync_file_length();
        Ok(())
    }
}
impl Read for PackIO {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.file.read_exact(buf)
    }
}

impl PackIO /*核心*/ {
    /// 提交垃圾回收项到暂存列表（稍后由 `file_gc` 处理排序和合并）。
    /// Submit garbage collection items to the staging list (sorted and merged later by `file_gc`).
    //垃圾回收提交
    pub(crate) fn file_gc_add(&mut self, gc_pos_list: Vec<(u64, u64)>) {
        for pos in gc_pos_list {
            //直接添加
            if pos.0 != 0 && pos.1 != 0 {
                self.run_data.gc_data_pos_list.list_mut().push(pos);
            }
        }
    }
    /// 执行垃圾回收：将暂存的 GC 项排序插入空闲列表，合并相邻块。
    /// Execute garbage collection: sort and insert staged GC items into the free list, merging adjacent blocks.
    ///
    /// 算法：先将暂存条目追加到空闲列表，然后整体排序（batch sort），
    /// 最后单次扫描合并相邻块。整体 O((K+M) log(K+M))，避免原插入排序的 O(K·M)。
    //垃圾回收
    pub(crate) fn file_gc(&mut self) -> Result<()> {
        //第1步：将暂存列表追加到空闲列表，然后整体按位置排序
        //Step 1: Append staged GC entries to the free list, then sort by position
        let pos_list = &mut self.empty_data_list.list_mut();
        pos_list.extend(self.run_data.gc_data_pos_list.list_mut().drain(..));
        pos_list.sort_unstable_by_key(|(pos, _)| *pos);
        //清空暂存缓存（drain 已清空）
        self.run_data.gc_data_pos_list.list_mut().clear();

        //第2步：单次扫描合并相邻块
        //Step 2: Single-pass merge adjacent blocks
        let mut i = 0;
        while i + 1 < pos_list.len() {
            let (cur_pos, cur_len) = pos_list[i];
            let (next_pos, next_len) = pos_list[i + 1];
            if cur_pos + cur_len == next_pos {
                pos_list[i].1 += next_len;
                pos_list.remove(i + 1);
                #[cfg(test)]
                debug_assert!(
                    pos_list[i].0.is_multiple_of(DATA_BLOCK_LEN as u64)
                        && pos_list[i].1.is_multiple_of(DATA_BLOCK_LEN as u64),
                    "GC 合并后块未对齐"
                );
                //不移动 i — 继续检查新的下一个是否也相邻
            } else {
                i += 1;
            }
        }
        Ok(())
    }

    /// 分配文件空间：优先从空闲列表复用，无可用空间时从文件末尾扩容。
    ///
    /// 返回 `(起始位置, 长度)`，长度已对齐到 `DATA_BLOCK_LEN`。
    ///
    /// Allocate file space: reuse from the free list if possible, otherwise extend the file.
    ///
    /// Returns `(start_position, length)`, with length aligned to `DATA_BLOCK_LEN`.
    //获取可用的文件位置
    pub(crate) fn get_file_pos(&mut self, length: u64) -> (u64, u64) {
        self.get_file_pos_aligned(length, DATA_BLOCK_LEN as u64)
    }

    /// 虚拟文件数据分配：长度按 `DATA_DATA_BLOCK_LEN`（4MiB）对齐，与清单数据
    /// 的 `DATA_BLOCK_LEN`（128B）对齐区分。优先单块 first-fit，其次紧贴连续
    /// 碎片链，再其次从大到小跨块拼接；均不改变 empty_data_list 顺序。
    /// 拼接段数受 max_seg 约束（虚拟文件数据段数上限），超限返回 Err 拒绝。
    ///
    /// Virtual-file data allocation: length aligned to `DATA_DATA_BLOCK_LEN` (4MiB),
    /// distinct from manifest `DATA_BLOCK_LEN` (128B) alignment. Prefers a single
    /// first-fit block, then a contiguous chain of adjacent free fragments, then
    /// biggest-first cross-block stitching; the free-list order is never changed.
    /// Stitched segment count is bounded by max_seg; exceeding it rejects with Err.
    pub(crate) fn get_data_file_pos_multi(
        &mut self,
        length: u64,
        current_seg: usize,
        max_seg: usize,
    ) -> Result<Vec<(u64, u64)>> {
        //块对齐
        let length = if length.is_multiple_of(DATA_DATA_BLOCK_LEN) {
            length
        } else {
            (length / DATA_DATA_BLOCK_LEN + 1) * DATA_DATA_BLOCK_LEN
        };
        let seg_budget = max_seg.saturating_sub(current_seg);
        if seg_budget == 0 {
            return Err(PackFileError::State(format!(
                "数据段数已达上限 {max_seg}，拒绝分配 / segment limit {max_seg} reached, allocation rejected"
            )));
        }
        let empty_data_pos = &mut *self.empty_data_list.list_mut();

        //Pass 1: 单块 first-fit
        if let Some(value) = Self::get_pos_gc(length, empty_data_pos) {
            self.update_alloc_len(&[value]);
            return Ok(vec![value]);
        }
        //Pass 2: 紧贴连续碎片链（物理相邻，合并为一段）
        if let Some(value) = Self::get_pos_contiguous(length, empty_data_pos) {
            self.update_alloc_len(&[value]);
            return Ok(vec![value]);
        }
        //Pass 3: 从大到小跨块拼接
        match Self::get_pos_biggest(length, empty_data_pos, seg_budget) {
            Ok(list) => {
                self.update_alloc_len(&list);
                Ok(list)
            }
            Err(MultiSegError::TooManySegments) => Err(PackFileError::State(format!(
                "碎片拼接所需段数超过上限 {max_seg}，拒绝分配 / stitching needs more than {max_seg} segments, allocation rejected"
            ))),
            Err(MultiSegError::NotEnough) => {
                //碎片总量不足：文件尾扩容兜底（单段）
                let value = (self.len, length);
                self.update_alloc_len(&[value]);
                Ok(vec![value])
            }
        }
    }

    /// 更新已记录的文件长度：取分配列表中最大的结束位置。
    /// Update the tracked file length to the max end of the given segments.
    fn update_alloc_len(&mut self, list: &[(u64, u64)]) {
        for &(pos, len) in list {
            let this_end_pos = pos + len;
            if this_end_pos > self.len {
                self.len = this_end_pos;
            }
        }
    }

    /// 紧贴连续链：仅当碎片物理相邻（前段末尾 == 后段起始）时合并为一段，
    /// 链总长满足请求则消耗链上全部碎片，返回 (链起点, 链长) 单段。
    /// 不改变列表顺序，仅按索引降序移除已消耗碎片。
    ///
    /// Contiguous chain: only physically adjacent free fragments (prev end ==
    /// next start) merge into one segment; if the chain covers the request,
    /// consume all fragments in it and return a single segment.
    fn get_pos_contiguous(
        length: u64,
        empty_data_pos: &mut Vec<(u64, u64)>,
    ) -> Option<(u64, u64)> {
        use std::collections::HashMap;
        let mut start_map: HashMap<u64, usize> = HashMap::with_capacity(empty_data_pos.len());
        let mut end_map: HashMap<u64, usize> = HashMap::with_capacity(empty_data_pos.len());
        for (i, &(pos, len)) in empty_data_pos.iter().enumerate() {
            start_map.insert(pos, i);
            end_map.insert(pos + len, i);
        }
        let mut used = vec![false; empty_data_pos.len()];
        for start in 0..empty_data_pos.len() {
            if used[start] {
                continue;
            }
            let (s_pos, s_len) = empty_data_pos[start];
            let mut lo = s_pos;
            let mut hi = s_pos + s_len;
            used[start] = true;
            let mut chain: Vec<usize> = vec![start];
            //向前延伸：找结束于 lo 的碎片
            loop {
                if let Some(&i) = end_map.get(&lo) {
                    if !used[i] {
                        lo = empty_data_pos[i].0;
                        used[i] = true;
                        chain.push(i);
                        continue;
                    }
                }
                break;
            }
            //向后延伸：找起始于 hi 的碎片
            loop {
                if let Some(&i) = start_map.get(&hi) {
                    if !used[i] {
                        hi = empty_data_pos[i].0 + empty_data_pos[i].1;
                        used[i] = true;
                        chain.push(i);
                        continue;
                    }
                }
                break;
            }
            if hi - lo >= length {
                chain.sort_unstable_by(|a, b| b.cmp(a));
                for i in chain {
                    empty_data_pos.remove(i);
                }
                return Some((lo, hi - lo));
            }
        }
        None
    }

    /// 从大到小跨块拼接：不改变列表顺序，每轮线性扫描选取当前最大碎片，
    /// 相邻碎片合并为一段；段数（合并后）超过预算立即返回 TooManySegments
    /// （此时列表未动，安全）。碎片总量不足返回 NotEnough（调用方走扩容）。
    ///
    /// Biggest-first stitching: list order never changes; each round linearly
    /// picks the current largest free fragment, merging adjacent ones; if the
    /// merged segment count exceeds the budget it returns TooManySegments
    /// immediately (list untouched, safe). Insufficient total free space
    /// returns NotEnough (caller falls back to extending the file).
    fn get_pos_biggest(
        length: u64,
        empty_data_pos: &mut Vec<(u64, u64)>,
        seg_budget: usize,
    ) -> std::result::Result<Vec<(u64, u64)>, MultiSegError> {
        use std::collections::HashSet;
        let total_free: u64 = empty_data_pos.iter().map(|&(_, len)| len).sum();
        if total_free < length {
            return Err(MultiSegError::NotEnough);
        }
        let mut chosen_idx: HashSet<usize> = HashSet::new();
        let mut chosen: Vec<(u64, u64)> = Vec::new();
        let mut acc = 0u64;
        loop {
            if acc >= length {
                break;
            }
            //选取当前最大碎片（排除已选）
            let mut best: Option<usize> = None;
            for (i, &(_, len)) in empty_data_pos.iter().enumerate() {
                if len == 0 || chosen_idx.contains(&i) {
                    continue;
                }
                if best.map_or(true, |bi| len > empty_data_pos[bi].1) {
                    best = Some(i);
                }
            }
            let Some(bi) = best else {
                break;
            };
            let (pos, len) = empty_data_pos[bi];
            //与已选段相邻则合并
            let mut merged = false;
            for c in chosen.iter_mut() {
                if c.0 + c.1 == pos {
                    c.1 += len;
                    merged = true;
                    break;
                }
                if pos + len == c.0 {
                    c.0 = pos;
                    c.1 += len;
                    merged = true;
                    break;
                }
            }
            if !merged {
                chosen.push((pos, len));
            }
            chosen_idx.insert(bi);
            acc += len;
            if chosen.len() > seg_budget {
                return Err(MultiSegError::TooManySegments);
            }
        }
        if acc < length {
            return Err(MultiSegError::NotEnough);
        }
        //移除已选碎片（索引降序，保持剩余顺序）
        let mut remove_idx: Vec<usize> = chosen_idx.into_iter().collect();
        remove_idx.sort_unstable_by(|a, b| b.cmp(a));
        for i in remove_idx {
            empty_data_pos.remove(i);
        }
        Ok(chosen)
    }

    fn get_file_pos_aligned(&mut self, length: u64, align: u64) -> (u64, u64) {
        //块对齐
        let length = if length.is_multiple_of(align) {
            length
        } else {
            let length = length / align + 1;
            length * align
        };
        let empty_data_pos = &mut *self.empty_data_list.list_mut();

        let value = if let Some(value) = Self::get_pos_gc(length, empty_data_pos) {
            value
        } else {
            //扩容处理
            (self.len, length)
        };
        //分配空间
        let this_end_pos = value.0 + value.1;
        if this_end_pos > self.len {
            self.len = this_end_pos;
        }
        value
    }

    fn get_pos_gc(length: u64, empty_data_pos: &mut Vec<(u64, u64)>) -> Option<(u64, u64)> {
        //优先使用空数据，但必须完整一块
        let mut index = 0;
        while !empty_data_pos.is_empty() {
            //从第一个开始
            let Some((pos, len)) = empty_data_pos.get_mut(index) else {
                break;
            };
            //判断是否能占用完
            //剩余大小
            return Some(match (*len).cast_signed() - length.cast_signed() {
                //能占用完，等于
                0 => empty_data_pos.remove(index),
                //可用，比较大
                1.. => {
                    //不能则切出
                    let r = (*pos, length);
                    //修改，位置加大小使其向后移动，长度减大小使其边界不变
                    *pos += length;
                    *len -= length;
                    r
                }
                //不够
                ..0 => {
                    index += 1;
                    continue;
                }
            });
        }
        None
    }

    /// 同步已记录的文件长度（当写入指针超出当前记录长度时更新）。
    /// Synchronize the tracked file length (updated when the write pointer exceeds the current recorded length).
    //更新文件大小
    pub(crate) fn sync_file_length(&mut self) {
        //判断是否需要设置
        if self.run_data.pos > self.len {
            self.len = self.run_data.pos;
        }
    }

    /// 设置底层文件的大小（截断或扩展）并同步内部长度记录。
    /// Set the underlying file size (truncate or extend) and sync the internal length record.
    //设置包文件大小
    pub(crate) fn set_len(&mut self, len: u64) -> Result<()> {
        self.file.set_len(len)?;
        self.len = len;
        self.sync_file_length();
        Ok(())
    }

    /// 同步底层文件数据到磁盘。
    /// Sync the underlying file data to disk.
    pub(crate) fn sync_data(&mut self) -> Result<()> {
        self.file.sync_data()?;
        Ok(())
    }
}

impl PackIO /*读*/ {
    /// 从指定文件位置读取清单数据块。
    /// Read a manifest data block from the given file position.
    pub(crate) fn manifest_data_block_read(&self, file_pos: u64) -> Result<ManifestDataBlock> {
        let mut file = &self.file;
        let block_data_buf = vec![0; DATA_BLOCK_LEN];
        let mut block_data_buf = block_data_buf;
        //设置文件指针
        file.seek(SeekFrom::Start(file_pos))?;
        //读取
        file.read_exact(&mut block_data_buf)?;
        //分析是否需要再加载
        let block_len = ManifestDataBlock::get_block_len(&block_data_buf).map_err(|err| {
            PackFileError::Format(format!("包文件IO属性数据块读取错误，err:{err:?}"))
        })?;
        if block_len > u64::from(u32::MAX) {
            Err(PackFileError::Format(format!(
                "解析的数据大小过大，可能是错误的:{}[{}]",
                tools::bytes_len_to_string(block_len),
                block_len
            )))?;
        }
        let l_len = usize::try_from(block_len).map_err(|_| {
            PackFileError::Format(format!("数据块长度 {block_len} 无法转换为 usize"))
        })? - DATA_BLOCK_LEN;
        let block_data = if l_len > 0 {
            let mut l_block_buf = vec![0; l_len];
            file.read_exact(&mut l_block_buf)?;
            //合并
            let mut block_data = Vec::with_capacity(usize::try_from(block_len).map_err(|_| {
                PackFileError::Format(format!("数据块长度 {block_len} 无法转换为 usize"))
            })?);
            for byte in block_data_buf {
                block_data.push(byte);
            }
            for byte in l_block_buf {
                block_data.push(byte);
            }
            block_data
        } else {
            block_data_buf
        };
        ManifestDataBlock::from_block_data_new(block_data, file_pos)
    }

    /// 设置文件读取指针位置。
    /// Set the file read pointer position.
    //设置文件地址
    pub(crate) fn set_pos_read(&self, pos: u64) -> Result<()> {
        let mut file = &self.file;
        file.seek(SeekFrom::Start(pos))?;
        Ok(())
    }
}

//写
impl PackIO /*写*/ {
    /// 释放底层文件的排他锁。
    /// Release the exclusive lock on the underlying file.
    pub(crate) fn unlock(&mut self) -> Result<()> {
        self.file.unlock()?;
        Ok(())
    }

    /// 获取底层文件的排他锁。
    /// Acquire the exclusive lock on the underlying file.
    pub(crate) fn lock(&mut self) -> Result<()> {
        self.file.lock()?;
        Ok(())
    }

    /// 设置文件写入指针位置并更新内部位置跟踪。
    /// Set the file write pointer position and update internal position tracking.
    //设置文件地址
    pub(crate) fn set_pos_write(&mut self, pos: u64) -> Result<()> {
        self.set_pos_read(pos)?;
        self.run_data.pos = pos;
        Ok(())
    }

    /// 将清单数据块写入文件。
    ///
    /// 如果 `new_block` 为 `true`（块大小发生变化），则在文件新位置写入
    /// 并将旧位置提交垃圾回收。返回实际写入的文件位置。
    ///
    /// Write a manifest data block to file.
    ///
    /// If `new_block` is `true` (block size changed), writes at a new file position
    /// and submits the old position for garbage collection. Returns the actual file position written.
    pub(crate) fn manifest_data_block_write(
        &mut self,
        block_data: &[u8],
        new_block: bool,
        old_pos: u64,
        old_block_len: u64,
    ) -> Result<u64> {
        Ok(if new_block {
            let (new_pos, _) = self.get_file_pos(block_data.len() as u64);
            self.set_pos_write(new_pos)?;
            self.write_all(block_data)?;
            self.file_gc_add(vec![(old_pos, old_block_len)]);
            new_pos
        } else {
            self.set_pos_write(old_pos)?;
            self.write_all(block_data)?;
            old_pos
        })
    }
}
