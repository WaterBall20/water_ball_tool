use crate::wb_files_pack::error::{PackFileError, Result};
use crate::wb_files_pack::{
    ManifestDataBlockTrait, OverwriteStrategy, PackFileMetadataRun, PackFileMetadataType,
    PackStruct, PackStructItemType,
};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::{DirFileAddReturn, WBFPManager};

impl WBFPManager {
    /// 删除虚拟文件 / Delete a virtual file
    ///
    /// 从包文件目录树中删除指定虚拟文件，释放其数据块到垃圾回收暂存列表。
    /// 等待下一次 throttled/save_all 时正式回收空间。
    ///
    /// Removes the specified virtual file from the pack file directory tree,
    /// freeing its data blocks to the garbage collection staging list.
    /// Actual space reclamation occurs during the next throttled save or save_all.
    pub(crate) fn delete_file(&mut self, path_list: &[String]) -> Result<()> {
        if path_list.is_empty() {
            return Err(PackFileError::Other(
                "不能删除根目录 / Cannot delete root directory".into(),
            ));
        }

        if path_list.len() == 1 {
            self.delete_file_root(path_list)
        } else {
            self.delete_file_nested(path_list)
        }
    }

    /// 递归删除目录及所有内容 / Recursively delete a directory and all contents
    pub(crate) fn delete_dir_all(&mut self, path_list: &[String]) -> Result<DirFileAddReturn> {
        if path_list.is_empty() {
            return Err(PackFileError::Other(
                "不能删除根目录 / Cannot delete root directory".into(),
            ));
        }

        if path_list.len() == 1 {
            self.delete_dir_all_root(path_list)
        } else {
            self.delete_dir_all_nested(path_list)
        }
    }

    /// 擦除虚拟文件（覆写数据后再删除）/ Erase a virtual file (overwrite data then delete)
    ///
    /// 在删除之前，使用指定策略覆写文件数据块和元数据块，
    /// 确保数据无法恢复。
    ///
    /// Before deletion, overwrites the file's data blocks and metadata block
    /// using the specified strategy, ensuring data is unrecoverable.
    pub(crate) fn erase_file(
        &mut self,
        path_list: &[String],
        strategy: OverwriteStrategy,
    ) -> Result<()> {
        if path_list.is_empty() {
            return Err(PackFileError::Other(
                "不能删除根目录 / Cannot delete root directory".into(),
            ));
        }

        if path_list.len() == 1 {
            self.erase_file_root(path_list, strategy)
        } else {
            self.erase_file_nested(path_list, strategy)
        }
    }

    /// 擦除目录及其所有内容 / Erase a directory and all contents
    ///
    /// 递归覆写每个文件的数据和元数据后，再执行删除。
    ///
    /// Recursively overwrites each file's data and metadata, then performs deletion.
    pub(crate) fn erase_dir_all(
        &mut self,
        path_list: &[String],
        strategy: OverwriteStrategy,
    ) -> Result<DirFileAddReturn> {
        if path_list.is_empty() {
            return Err(PackFileError::Other(
                "不能删除根目录 / Cannot delete root directory".into(),
            ));
        }

        if path_list.len() == 1 {
            self.erase_dir_all_root(path_list, strategy)
        } else {
            self.erase_dir_all_nested(path_list, strategy)
        }
    }

    // ==================== 根级别操作 / Root-Level Operations ====================

    /// 删除根目录下的文件 / Delete a file directly under root
    fn delete_file_root(&mut self, path_list: &[String]) -> Result<()> {
        let file_name = &path_list[0];
        let this_path = PathBuf::from(file_name);

        if self.manifest.root_struct().child_locked_count() != 0 {
            return Err(PackFileError::Other(
                "目录有被锁定的子项 / Directory has locked children".into(),
            ));
        }

        let mut item = self
            .manifest
            .root_struct_mut()
            .remove_item(file_name)
            .ok_or(PackFileError::NotFound(format!(
                r#"虚拟路径"{file_name}"的结构项不存在"#
            )))?;

        if !matches!(item.item_type(), PackStructItemType::File { .. }) {
            self.manifest
                .root_struct_mut()
                .add_item(file_name.clone(), item);
            return Err(PackFileError::Format(format!(
                r#"虚拟路径"{file_name}"不是文件"#
            )));
        }

        self.load_metadata_to_item(&this_path, &mut item)?;

        let metadata = item.metadata_mut().try_lock().map_err(|_| {
            self.manifest
                .root_struct_mut()
                .add_item(file_name.clone(), item);
            PackFileError::Lock("文件正在被使用 / File is in use".into())
        })?;

        let data_pos_list = match metadata.file_type() {
            PackFileMetadataType::File { data_pos_list, .. } => data_pos_list.list().clone(),
            _ => unreachable!(),
        };

        let file_len = metadata.len();

        self.file_gc_add(data_pos_list)?;
        self.manifest.attribute_mut().sub_file_count(1);
        self.manifest.attribute_mut().sub_data_len(file_len);
        self.save_root_pack_struct()?;
        self.throttled_save()?;

        Ok(())
    }

    /// 删除根目录下的目录及其全部内容 / Delete a directory and all contents under root

    /// 删除根目录下的目录及其全部内容 / Delete a directory and all contents under root
    fn delete_dir_all_root(&mut self, path_list: &[String]) -> Result<DirFileAddReturn> {
        let dir_name = &path_list[0];
        let this_path = PathBuf::from(dir_name);

        if self.manifest.root_struct().child_locked_count() != 0 {
            return Err(PackFileError::Other(
                "目录有被锁定的子项 / Directory has locked children".into(),
            ));
        }

        let mut item = self
            .manifest
            .root_struct_mut()
            .remove_item(dir_name)
            .ok_or(PackFileError::NotFound(format!(
                r#"虚拟路径"{dir_name}"的结构项不存在"#
            )))?;

        if !matches!(item.item_type(), PackStructItemType::Dir { .. }) {
            self.manifest
                .root_struct_mut()
                .add_item(dir_name.clone(), item);
            return Err(PackFileError::NotADirectory(format!(
                r#"虚拟路径"{dir_name}"是文件不是目录"#
            )));
        }

        let (r, _gc_list) = if let PackStructItemType::Dir {
            struct_file_pos,
            pack_struct,
        } = item.item_type_mut()
        {
            if pack_struct.is_none() {
                *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
            }
            let sub_ps = pack_struct
                .as_mut()
                .ok_or(PackFileError::State("子目录结构未加载".into()))?;
            let (r_inner, gc_list) = self.delete_dir_contents(sub_ps, &this_path)?;
            self.file_gc_add(gc_list)?;

            let mut r = r_inner;
            r.dir_count += 1;
            (r, Vec::<(u64, u64)>::new())
        } else {
            unreachable!()
        };

        self.manifest.attribute_mut().sub_file_count(r.file_count);
        self.manifest.attribute_mut().sub_dir_count(r.dir_count);
        self.manifest.attribute_mut().sub_data_len(r.length);
        self.save_root_pack_struct()?;
        self.throttled_save()?;

        Ok(r)
    }

    /// 擦除根目录下的文件 / Erase a file directly under root
    fn erase_file_root(&mut self, path_list: &[String], strategy: OverwriteStrategy) -> Result<()> {
        let file_name = &path_list[0];
        let this_path = PathBuf::from(file_name);

        if self.manifest.root_struct().child_locked_count() != 0 {
            return Err(PackFileError::Other(
                "目录有被锁定的子项 / Directory has locked children".into(),
            ));
        }

        let mut item = self
            .manifest
            .root_struct_mut()
            .remove_item(file_name)
            .ok_or(PackFileError::NotFound(format!(
                r#"虚拟路径"{file_name}"的结构项不存在"#
            )))?;

        if !matches!(item.item_type(), PackStructItemType::File { .. }) {
            self.manifest
                .root_struct_mut()
                .add_item(file_name.clone(), item);
            return Err(PackFileError::Format(format!(
                r#"虚拟路径"{file_name}"不是文件"#
            )));
        }

        self.load_metadata_to_item(&this_path, &mut item)?;

        let mut metadata = item.metadata_mut().try_lock().map_err(|_| {
            self.manifest
                .root_struct_mut()
                .add_item(file_name.clone(), item);
            PackFileError::Lock("文件正在被使用 / File is in use".into())
        })?;

        let data_pos_list = match metadata.file_type() {
            PackFileMetadataType::File { data_pos_list, .. } => data_pos_list.list().clone(),
            _ => unreachable!(),
        };

        let file_len = metadata.len();
        let metadata_file_pos = metadata.data_block_mut().file_pos();

        // 覆写每个数据块 / Overwrite each data block
        for &(pos, len) in &data_pos_list {
            self.overwrite_data_block(pos, len, strategy)?;
        }
        // 覆写元数据块 / Overwrite metadata block
        self.overwrite_manifest_block(metadata_file_pos, strategy)?;

        self.file_gc_add(data_pos_list)?;
        self.manifest.attribute_mut().sub_file_count(1);
        self.manifest.attribute_mut().sub_data_len(file_len);
        self.save_root_pack_struct()?;
        self.throttled_save()?;

        Ok(())
    }

    /// 擦除根目录下的目录及其全部内容 / Erase a directory and all contents under root
    fn erase_dir_all_root(
        &mut self,
        path_list: &[String],
        strategy: OverwriteStrategy,
    ) -> Result<DirFileAddReturn> {
        let dir_name = &path_list[0];
        let this_path = PathBuf::from(dir_name);

        if self.manifest.root_struct().child_locked_count() != 0 {
            return Err(PackFileError::Other(
                "目录有被锁定的子项 / Directory has locked children".into(),
            ));
        }

        let mut item = self
            .manifest
            .root_struct_mut()
            .remove_item(dir_name)
            .ok_or(PackFileError::NotFound(format!(
                r#"虚拟路径"{dir_name}"的结构项不存在"#
            )))?;

        if !matches!(item.item_type(), PackStructItemType::Dir { .. }) {
            self.manifest
                .root_struct_mut()
                .add_item(dir_name.clone(), item);
            return Err(PackFileError::NotADirectory(format!(
                r#"虚拟路径"{dir_name}"是文件不是目录"#
            )));
        }

        let (r, _gc_list) = if let PackStructItemType::Dir {
            struct_file_pos,
            pack_struct,
        } = item.item_type_mut()
        {
            if pack_struct.is_none() {
                *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
            }
            let sub_ps = pack_struct
                .as_mut()
                .ok_or(PackFileError::State("子目录结构未加载".into()))?;
            let (r_inner, gc_list) = self.erase_dir_contents(sub_ps, &this_path, strategy)?;

            // 覆写此目录的 PackStruct 数据块 / Overwrite this dir's PackStruct block
            self.overwrite_manifest_block(*struct_file_pos, strategy)?;

            self.file_gc_add(gc_list)?;

            let mut r = r_inner;
            r.dir_count += 1;
            (r, Vec::<(u64, u64)>::new())
        } else {
            unreachable!()
        };

        self.manifest.attribute_mut().sub_file_count(r.file_count);
        self.manifest.attribute_mut().sub_dir_count(r.dir_count);
        self.manifest.attribute_mut().sub_data_len(r.length);
        self.save_root_pack_struct()?;
        self.throttled_save()?;

        Ok(r)
    }

    // ==================== 嵌套路径操作 / Nested Path Operations ====================

    /// 在嵌套路径中删除文件 / Delete a file at a nested path
    fn delete_file_nested(&mut self, path_list: &[String]) -> Result<()> {
        let first_name = &path_list[0];
        let s_path = PathBuf::from(first_name);
        let mut first_item = self
            .manifest
            .root_struct_mut()
            .remove_item(first_name)
            .ok_or(PackFileError::NotFound(format!(
                r#"虚拟路径"{first_name}"的结构项不存在"#
            )))?;

        // 检查路径的第一个元素是否为目录 / Check first path component is a directory
        if !matches!(first_item.item_type(), PackStructItemType::Dir { .. }) {
            self.manifest
                .root_struct_mut()
                .add_item(first_name.clone(), first_item);
            return Err(PackFileError::NotADirectory(format!(
                r#"虚拟路径"{first_name}"是文件不是目录"#
            )));
        }

        let (r, gc_list) = {
            let (struct_file_pos, pack_struct) = if let PackStructItemType::Dir {
                struct_file_pos,
                pack_struct,
            } = first_item.item_type_mut()
            {
                (struct_file_pos, pack_struct)
            } else {
                unreachable!()
            };

            if pack_struct.is_none() {
                *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
            }
            let sub_ps = pack_struct
                .as_mut()
                .ok_or(PackFileError::State("子目录结构未加载".into()))?;

            let result = self.delete_file_inner(sub_ps, &path_list[1..], &s_path);
            let (r, gc_list) = match result {
                Ok(v) => v,
                Err(e) => {
                    self.manifest
                        .root_struct_mut()
                        .add_item(first_name.clone(), first_item);
                    return Err(e);
                }
            };

            // 保存子目录的结构 / Save sub-PackStruct
            let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
            if new_block {
                *struct_file_pos = pos;
            }

            // 更新目录元数据 / Update directory metadata
            self.load_metadata_to_item(&s_path, &mut first_item)?;
            if let PackFileMetadataRun::Loaded(metadata) = first_item.metadata_mut() {
                metadata.sub_file_count(r.file_count);
                metadata.sub_dir_count(r.dir_count);
                metadata.sub_len(r.length);
                let (new_block, pos) = self.save_metadata_write(metadata)?;
                if new_block {
                    first_item.set_metadata_file_pos(pos);
                }
            }

            (r, gc_list)
        };

        self.manifest
            .root_struct_mut()
            .add_item(first_name.clone(), first_item);
        self.file_gc_add(gc_list)?;
        self.manifest.attribute_mut().sub_file_count(r.file_count);
        self.manifest.attribute_mut().sub_data_len(r.length);
        self.save_root_pack_struct()?;
        self.throttled_save()?;

        Ok(())
    }

    /// 在嵌套路径中删除目录 / Delete a directory at a nested path
    fn delete_dir_all_nested(&mut self, path_list: &[String]) -> Result<DirFileAddReturn> {
        let first_name = &path_list[0];
        let s_path = PathBuf::from(first_name);
        let mut first_item = self
            .manifest
            .root_struct_mut()
            .remove_item(first_name)
            .ok_or(PackFileError::NotFound(format!(
                r#"虚拟路径"{first_name}"的结构项不存在"#
            )))?;

        if !matches!(first_item.item_type(), PackStructItemType::Dir { .. }) {
            self.manifest
                .root_struct_mut()
                .add_item(first_name.clone(), first_item);
            return Err(PackFileError::NotADirectory(format!(
                r#"虚拟路径"{first_name}"是文件不是目录"#
            )));
        }

        let (r, gc_list) = {
            let (struct_file_pos, pack_struct) = if let PackStructItemType::Dir {
                struct_file_pos,
                pack_struct,
            } = first_item.item_type_mut()
            {
                (struct_file_pos, pack_struct)
            } else {
                unreachable!()
            };

            if pack_struct.is_none() {
                *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
            }
            let sub_ps = pack_struct
                .as_mut()
                .ok_or(PackFileError::State("子目录结构未加载".into()))?;

            let result = self.delete_dir_inner(sub_ps, &path_list[1..], &s_path);
            let (r, gc_list) = match result {
                Ok(v) => v,
                Err(e) => {
                    self.manifest
                        .root_struct_mut()
                        .add_item(first_name.clone(), first_item);
                    return Err(e);
                }
            };

            let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
            if new_block {
                *struct_file_pos = pos;
            }

            self.load_metadata_to_item(&s_path, &mut first_item)?;
            if let PackFileMetadataRun::Loaded(metadata) = first_item.metadata_mut() {
                metadata.sub_file_count(r.file_count);
                metadata.sub_dir_count(r.dir_count);
                metadata.sub_len(r.length);
                let (new_block, pos) = self.save_metadata_write(metadata)?;
                if new_block {
                    first_item.set_metadata_file_pos(pos);
                }
            }

            (r, gc_list)
        };

        self.manifest
            .root_struct_mut()
            .add_item(first_name.clone(), first_item);
        self.file_gc_add(gc_list)?;
        self.manifest.attribute_mut().sub_file_count(r.file_count);
        self.manifest.attribute_mut().sub_dir_count(r.dir_count);
        self.manifest.attribute_mut().sub_data_len(r.length);
        self.save_root_pack_struct()?;
        self.throttled_save()?;

        Ok(r)
    }

    /// 在嵌套路径中擦除文件 / Erase a file at a nested path
    fn erase_file_nested(
        &mut self,
        path_list: &[String],
        strategy: OverwriteStrategy,
    ) -> Result<()> {
        let first_name = &path_list[0];
        let s_path = PathBuf::from(first_name);
        let mut first_item = self
            .manifest
            .root_struct_mut()
            .remove_item(first_name)
            .ok_or(PackFileError::NotFound(format!(
                r#"虚拟路径"{first_name}"的结构项不存在"#
            )))?;

        if !matches!(first_item.item_type(), PackStructItemType::Dir { .. }) {
            self.manifest
                .root_struct_mut()
                .add_item(first_name.clone(), first_item);
            return Err(PackFileError::NotADirectory(format!(
                r#"虚拟路径"{first_name}"是文件不是目录"#
            )));
        }

        let (r, gc_list) = {
            let (struct_file_pos, pack_struct) = if let PackStructItemType::Dir {
                struct_file_pos,
                pack_struct,
            } = first_item.item_type_mut()
            {
                (struct_file_pos, pack_struct)
            } else {
                unreachable!()
            };

            if pack_struct.is_none() {
                *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
            }
            let sub_ps = pack_struct
                .as_mut()
                .ok_or(PackFileError::State("子目录结构未加载".into()))?;

            let result = self.erase_file_inner(sub_ps, &path_list[1..], &s_path, strategy);
            let (r, gc_list) = match result {
                Ok(v) => v,
                Err(e) => {
                    self.manifest
                        .root_struct_mut()
                        .add_item(first_name.clone(), first_item);
                    return Err(e);
                }
            };

            let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
            if new_block {
                *struct_file_pos = pos;
            }

            self.load_metadata_to_item(&s_path, &mut first_item)?;
            if let PackFileMetadataRun::Loaded(metadata) = first_item.metadata_mut() {
                metadata.sub_file_count(r.file_count);
                metadata.sub_dir_count(r.dir_count);
                metadata.sub_len(r.length);
                let (new_block, pos) = self.save_metadata_write(metadata)?;
                if new_block {
                    first_item.set_metadata_file_pos(pos);
                }
            }

            (r, gc_list)
        };

        self.manifest
            .root_struct_mut()
            .add_item(first_name.clone(), first_item);
        self.file_gc_add(gc_list)?;
        self.manifest.attribute_mut().sub_file_count(r.file_count);
        self.manifest.attribute_mut().sub_data_len(r.length);
        self.save_root_pack_struct()?;
        self.throttled_save()?;

        Ok(())
    }

    /// 在嵌套路径中擦除目录 / Erase a directory at a nested path
    fn erase_dir_all_nested(
        &mut self,
        path_list: &[String],
        strategy: OverwriteStrategy,
    ) -> Result<DirFileAddReturn> {
        let first_name = &path_list[0];
        let s_path = PathBuf::from(first_name);
        let mut first_item = self
            .manifest
            .root_struct_mut()
            .remove_item(first_name)
            .ok_or(PackFileError::NotFound(format!(
                r#"虚拟路径"{first_name}"的结构项不存在"#
            )))?;

        if !matches!(first_item.item_type(), PackStructItemType::Dir { .. }) {
            self.manifest
                .root_struct_mut()
                .add_item(first_name.clone(), first_item);
            return Err(PackFileError::NotADirectory(format!(
                r#"虚拟路径"{first_name}"是文件不是目录"#
            )));
        }

        let (r, gc_list) = {
            let (struct_file_pos, pack_struct) = if let PackStructItemType::Dir {
                struct_file_pos,
                pack_struct,
            } = first_item.item_type_mut()
            {
                (struct_file_pos, pack_struct)
            } else {
                unreachable!()
            };

            if pack_struct.is_none() {
                *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
            }
            let sub_ps = pack_struct
                .as_mut()
                .ok_or(PackFileError::State("子目录结构未加载".into()))?;

            let result = self.erase_dir_inner(sub_ps, &path_list[1..], &s_path, strategy);
            let (r, gc_list) = match result {
                Ok(v) => v,
                Err(e) => {
                    self.manifest
                        .root_struct_mut()
                        .add_item(first_name.clone(), first_item);
                    return Err(e);
                }
            };

            let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
            if new_block {
                *struct_file_pos = pos;
            }

            self.load_metadata_to_item(&s_path, &mut first_item)?;
            if let PackFileMetadataRun::Loaded(metadata) = first_item.metadata_mut() {
                metadata.sub_file_count(r.file_count);
                metadata.sub_dir_count(r.dir_count);
                metadata.sub_len(r.length);
                let (new_block, pos) = self.save_metadata_write(metadata)?;
                if new_block {
                    first_item.set_metadata_file_pos(pos);
                }
            }

            (r, gc_list)
        };

        self.manifest
            .root_struct_mut()
            .add_item(first_name.clone(), first_item);
        self.file_gc_add(gc_list)?;
        self.manifest.attribute_mut().sub_file_count(r.file_count);
        self.manifest.attribute_mut().sub_dir_count(r.dir_count);
        self.manifest.attribute_mut().sub_data_len(r.length);
        self.save_root_pack_struct()?;
        self.throttled_save()?;

        Ok(r)
    }

    // ==================== 递归内部方法 / Recursive Inner Methods ====================

    /// 递归导航到目标文件并返回删除结果（含 GC 列表）
    /// Navigate recursively to the target file and return deletion result (with GC list)
    fn delete_file_inner(
        &mut self,
        parent_ps: &mut PackStruct,
        path_list: &[String],
        s_path: &Path,
    ) -> Result<(DirFileAddReturn, Vec<(u64, u64)>)> {
        let name = &path_list[0];
        let this_path = s_path.join(name);

        if path_list.len() == 1 {
            // 目标文件 / Target file
            let mut item = parent_ps
                .remove_item(name)
                .ok_or(PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    this_path.display()
                )))?;

            // 验证是文件 / Verify it is a file
            if !matches!(item.item_type(), PackStructItemType::File { .. }) {
                parent_ps.add_item(name.clone(), item);
                return Err(PackFileError::Format(format!(
                    r#"虚拟路径"{}"不是文件"#,
                    this_path.display()
                )));
            }

            self.load_metadata_to_item(&this_path, &mut item)?;
            let metadata = item.metadata_mut().try_lock().map_err(|_| {
                parent_ps.add_item(name.clone(), item);
                PackFileError::Lock("文件正在被使用 / File is in use".into())
            })?;

            let data_pos_list = match metadata.file_type() {
                PackFileMetadataType::File { data_pos_list, .. } => data_pos_list.list().clone(),
                _ => unreachable!(),
            };

            let file_len = metadata.len();

            Ok((
                DirFileAddReturn {
                    length: file_len,
                    file_count: 1,
                    dir_count: 0,
                    unlocked_occurred: false,
                },
                data_pos_list,
            ))
        } else {
            // 导航更深 / Navigate deeper
            let mut item = parent_ps
                .remove_item(name)
                .ok_or(PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    this_path.display()
                )))?;

            if !matches!(item.item_type(), PackStructItemType::Dir { .. }) {
                parent_ps.add_item(name.clone(), item);
                return Err(PackFileError::NotADirectory(format!(
                    r#"虚拟路径"{}"是文件不是目录"#,
                    this_path.display()
                )));
            }

            let (struct_file_pos, pack_struct) = if let PackStructItemType::Dir {
                struct_file_pos,
                pack_struct,
            } = item.item_type_mut()
            {
                (struct_file_pos, pack_struct)
            } else {
                unreachable!()
            };

            if pack_struct.is_none() {
                *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
            }
            let sub_ps = pack_struct
                .as_mut()
                .ok_or(PackFileError::State("子目录结构未加载".into()))?;

            let result = self.delete_file_inner(sub_ps, &path_list[1..], &this_path);
            let (r, gc_list) = match result {
                Ok(v) => v,
                Err(e) => {
                    parent_ps.add_item(name.clone(), item);
                    return Err(e);
                }
            };

            let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
            if new_block {
                *struct_file_pos = pos;
            }

            self.load_metadata_to_item(&this_path, &mut item)?;
            if let PackFileMetadataRun::Loaded(metadata) = item.metadata_mut() {
                metadata.sub_file_count(r.file_count);
                metadata.sub_dir_count(r.dir_count);
                metadata.sub_len(r.length);
                let (new_block, pos) = self.save_metadata_write(metadata)?;
                if new_block {
                    item.set_metadata_file_pos(pos);
                    parent_ps.mark_dirty();
                }
            }

            parent_ps.add_item(name.clone(), item);

            Ok((r, gc_list))
        }
    }

    /// 递归导航到目标目录并返回删除结果（含 GC 列表）
    /// Navigate recursively to the target directory and return deletion result (with GC list)
    fn delete_dir_inner(
        &mut self,
        parent_ps: &mut PackStruct,
        path_list: &[String],
        s_path: &Path,
    ) -> Result<(DirFileAddReturn, Vec<(u64, u64)>)> {
        let name = &path_list[0];
        let this_path = s_path.join(name);

        if path_list.len() == 1 {
            // 目标目录 / Target directory
            let mut item = parent_ps
                .remove_item(name)
                .ok_or(PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    this_path.display()
                )))?;

            if !matches!(item.item_type(), PackStructItemType::Dir { .. }) {
                parent_ps.add_item(name.clone(), item);
                return Err(PackFileError::NotADirectory(format!(
                    r#"虚拟路径"{}"是文件不是目录"#,
                    this_path.display()
                )));
            }

            let (struct_file_pos, pack_struct) = if let PackStructItemType::Dir {
                struct_file_pos,
                pack_struct,
            } = item.item_type_mut()
            {
                (struct_file_pos, pack_struct)
            } else {
                unreachable!()
            };

            if pack_struct.is_none() {
                *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
            }
            let sub_ps = pack_struct
                .as_mut()
                .ok_or(PackFileError::State("子目录结构未加载".into()))?;

            let (mut r, gc_list) = self.delete_dir_contents(sub_ps, &this_path)?;
            r.dir_count += 1;

            Ok((r, gc_list))
        } else {
            // 导航更深 / Navigate deeper
            let mut item = parent_ps
                .remove_item(name)
                .ok_or(PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    this_path.display()
                )))?;

            if !matches!(item.item_type(), PackStructItemType::Dir { .. }) {
                parent_ps.add_item(name.clone(), item);
                return Err(PackFileError::NotADirectory(format!(
                    r#"虚拟路径"{}"是文件不是目录"#,
                    this_path.display()
                )));
            }

            let (struct_file_pos, pack_struct) = if let PackStructItemType::Dir {
                struct_file_pos,
                pack_struct,
            } = item.item_type_mut()
            {
                (struct_file_pos, pack_struct)
            } else {
                unreachable!()
            };

            if pack_struct.is_none() {
                *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
            }
            let sub_ps = pack_struct
                .as_mut()
                .ok_or(PackFileError::State("子目录结构未加载".into()))?;

            let result = self.delete_dir_inner(sub_ps, &path_list[1..], &this_path);
            let (r, gc_list) = match result {
                Ok(v) => v,
                Err(e) => {
                    parent_ps.add_item(name.clone(), item);
                    return Err(e);
                }
            };

            let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
            if new_block {
                *struct_file_pos = pos;
            }

            self.load_metadata_to_item(&this_path, &mut item)?;
            if let PackFileMetadataRun::Loaded(metadata) = item.metadata_mut() {
                metadata.sub_file_count(r.file_count);
                metadata.sub_dir_count(r.dir_count);
                metadata.sub_len(r.length);
                let (new_block, pos) = self.save_metadata_write(metadata)?;
                if new_block {
                    item.set_metadata_file_pos(pos);
                    parent_ps.mark_dirty();
                }
            }

            parent_ps.add_item(name.clone(), item);

            Ok((r, gc_list))
        }
    }

    /// 递归删除目录的所有内容 / Recursively delete all contents of a directory
    fn delete_dir_contents(
        &mut self,
        dir_ps: &mut PackStruct,
        s_path: &Path,
    ) -> Result<(DirFileAddReturn, Vec<(u64, u64)>)> {
        let mut total_r = DirFileAddReturn {
            length: 0,
            file_count: 0,
            dir_count: 0,
            unlocked_occurred: false,
        };
        let mut total_gc: Vec<(u64, u64)> = Vec::new();

        let names: Vec<String> = dir_ps.items().keys().cloned().collect();
        for name in names {
            let this_path = s_path.join(&name);
            let mut item = dir_ps
                .remove_item(&name)
                .ok_or(PackFileError::NotFound(format!("结构项\"{name}\"不存在")))?;

            match item.item_type() {
                PackStructItemType::File { .. } => {
                    self.load_metadata_to_item(&this_path, &mut item)?;
                    let metadata = item.metadata_mut().try_lock().map_err(|_| {
                        PackFileError::Lock("文件正在被使用 / File is in use".into())
                    })?;

                    let data_pos_list = match metadata.file_type() {
                        PackFileMetadataType::File { data_pos_list, .. } => {
                            data_pos_list.list().clone()
                        }
                        _ => unreachable!(),
                    };

                    total_gc.extend(data_pos_list);
                    total_r.file_count += 1;
                    total_r.length += metadata.len();
                }
                PackStructItemType::Dir {
                    struct_file_pos,
                    pack_struct,
                } => {
                    let mut sub_ps = match pack_struct {
                        Some(ps) => ps.clone(),
                        None => self.load_pack_struct(*struct_file_pos)?,
                    };

                    let (sub_r, sub_gc) = self.delete_dir_contents(&mut sub_ps, &this_path)?;

                    total_gc.extend(sub_gc);
                    total_r.file_count += sub_r.file_count;
                    total_r.dir_count += sub_r.dir_count + 1;
                    total_r.length += sub_r.length;
                }
            }
        }

        Ok((total_r, total_gc))
    }

    /// 递归导航到目标文件并执行擦除（含 GC 列表）
    /// Navigate recursively to the target file and perform erase (with GC list)
    fn erase_file_inner(
        &mut self,
        parent_ps: &mut PackStruct,
        path_list: &[String],
        s_path: &Path,
        strategy: OverwriteStrategy,
    ) -> Result<(DirFileAddReturn, Vec<(u64, u64)>)> {
        let name = &path_list[0];
        let this_path = s_path.join(name);

        if path_list.len() == 1 {
            // 目标文件 / Target file
            let mut item = parent_ps
                .remove_item(name)
                .ok_or(PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    this_path.display()
                )))?;

            if !matches!(item.item_type(), PackStructItemType::File { .. }) {
                parent_ps.add_item(name.clone(), item);
                return Err(PackFileError::Format(format!(
                    r#"虚拟路径"{}"不是文件"#,
                    this_path.display()
                )));
            }

            self.load_metadata_to_item(&this_path, &mut item)?;
            let mut metadata = item.metadata_mut().try_lock().map_err(|_| {
                parent_ps.add_item(name.clone(), item);
                PackFileError::Lock("文件正在被使用 / File is in use".into())
            })?;

            let data_pos_list = match metadata.file_type() {
                PackFileMetadataType::File { data_pos_list, .. } => data_pos_list.list().clone(),
                _ => unreachable!(),
            };

            let file_len = metadata.len();
            let metadata_file_pos = metadata.data_block_mut().file_pos();

            // 覆写每个数据块 / Overwrite each data block
            for &(pos, len) in &data_pos_list {
                self.overwrite_data_block(pos, len, strategy)?;
            }
            // 覆写元数据块 / Overwrite metadata block
            self.overwrite_manifest_block(metadata_file_pos, strategy)?;

            Ok((
                DirFileAddReturn {
                    length: file_len,
                    file_count: 1,
                    dir_count: 0,
                    unlocked_occurred: false,
                },
                data_pos_list,
            ))
        } else {
            // 导航更深 / Navigate deeper
            let mut item = parent_ps
                .remove_item(name)
                .ok_or(PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    this_path.display()
                )))?;

            if !matches!(item.item_type(), PackStructItemType::Dir { .. }) {
                parent_ps.add_item(name.clone(), item);
                return Err(PackFileError::NotADirectory(format!(
                    r#"虚拟路径"{}"是文件不是目录"#,
                    this_path.display()
                )));
            }

            let (struct_file_pos, pack_struct) = if let PackStructItemType::Dir {
                struct_file_pos,
                pack_struct,
            } = item.item_type_mut()
            {
                (struct_file_pos, pack_struct)
            } else {
                unreachable!()
            };

            if pack_struct.is_none() {
                *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
            }
            let sub_ps = pack_struct
                .as_mut()
                .ok_or(PackFileError::State("子目录结构未加载".into()))?;

            let result = self.erase_file_inner(sub_ps, &path_list[1..], &this_path, strategy);
            let (r, gc_list) = match result {
                Ok(v) => v,
                Err(e) => {
                    parent_ps.add_item(name.clone(), item);
                    return Err(e);
                }
            };

            let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
            if new_block {
                *struct_file_pos = pos;
            }

            self.load_metadata_to_item(&this_path, &mut item)?;
            if let PackFileMetadataRun::Loaded(metadata) = item.metadata_mut() {
                metadata.sub_file_count(r.file_count);
                metadata.sub_dir_count(r.dir_count);
                metadata.sub_len(r.length);
                let (new_block, pos) = self.save_metadata_write(metadata)?;
                if new_block {
                    item.set_metadata_file_pos(pos);
                    parent_ps.mark_dirty();
                }
            }

            parent_ps.add_item(name.clone(), item);

            Ok((r, gc_list))
        }
    }

    /// 递归导航到目标目录并执行擦除（含 GC 列表）
    /// Navigate recursively to the target directory and perform erase (with GC list)
    fn erase_dir_inner(
        &mut self,
        parent_ps: &mut PackStruct,
        path_list: &[String],
        s_path: &Path,
        strategy: OverwriteStrategy,
    ) -> Result<(DirFileAddReturn, Vec<(u64, u64)>)> {
        let name = &path_list[0];
        let this_path = s_path.join(name);

        if path_list.len() == 1 {
            // 目标目录 / Target directory
            let mut item = parent_ps
                .remove_item(name)
                .ok_or(PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    this_path.display()
                )))?;

            if !matches!(item.item_type(), PackStructItemType::Dir { .. }) {
                parent_ps.add_item(name.clone(), item);
                return Err(PackFileError::NotADirectory(format!(
                    r#"虚拟路径"{}"是文件不是目录"#,
                    this_path.display()
                )));
            }

            let (struct_file_pos, pack_struct) = if let PackStructItemType::Dir {
                struct_file_pos,
                pack_struct,
            } = item.item_type_mut()
            {
                (struct_file_pos, pack_struct)
            } else {
                unreachable!()
            };

            if pack_struct.is_none() {
                *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
            }
            let sub_ps = pack_struct
                .as_mut()
                .ok_or(PackFileError::State("子目录结构未加载".into()))?;

            let (mut r, gc_list) = self.erase_dir_contents(sub_ps, &this_path, strategy)?;

            // 覆写此目录的 PackStruct 数据块 / Overwrite this dir's PackStruct block
            self.overwrite_manifest_block(*struct_file_pos, strategy)?;

            r.dir_count += 1;

            Ok((r, gc_list))
        } else {
            // 导航更深 / Navigate deeper
            let mut item = parent_ps
                .remove_item(name)
                .ok_or(PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    this_path.display()
                )))?;

            if !matches!(item.item_type(), PackStructItemType::Dir { .. }) {
                parent_ps.add_item(name.clone(), item);
                return Err(PackFileError::NotADirectory(format!(
                    r#"虚拟路径"{}"是文件不是目录"#,
                    this_path.display()
                )));
            }

            let (struct_file_pos, pack_struct) = if let PackStructItemType::Dir {
                struct_file_pos,
                pack_struct,
            } = item.item_type_mut()
            {
                (struct_file_pos, pack_struct)
            } else {
                unreachable!()
            };

            if pack_struct.is_none() {
                *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
            }
            let sub_ps = pack_struct
                .as_mut()
                .ok_or(PackFileError::State("子目录结构未加载".into()))?;

            let result = self.erase_dir_inner(sub_ps, &path_list[1..], &this_path, strategy);
            let (r, gc_list) = match result {
                Ok(v) => v,
                Err(e) => {
                    parent_ps.add_item(name.clone(), item);
                    return Err(e);
                }
            };

            let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
            if new_block {
                *struct_file_pos = pos;
            }

            self.load_metadata_to_item(&this_path, &mut item)?;
            if let PackFileMetadataRun::Loaded(metadata) = item.metadata_mut() {
                metadata.sub_file_count(r.file_count);
                metadata.sub_dir_count(r.dir_count);
                metadata.sub_len(r.length);
                let (new_block, pos) = self.save_metadata_write(metadata)?;
                if new_block {
                    item.set_metadata_file_pos(pos);
                    parent_ps.mark_dirty();
                }
            }

            parent_ps.add_item(name.clone(), item);

            Ok((r, gc_list))
        }
    }

    /// 递归擦除目录的所有内容（覆写后删除，含 GC 列表）
    /// Recursively erase all contents of a directory (overwrite then delete, with GC list)
    fn erase_dir_contents(
        &mut self,
        dir_ps: &mut PackStruct,
        s_path: &Path,
        strategy: OverwriteStrategy,
    ) -> Result<(DirFileAddReturn, Vec<(u64, u64)>)> {
        let mut total_r = DirFileAddReturn {
            length: 0,
            file_count: 0,
            dir_count: 0,
            unlocked_occurred: false,
        };
        let mut total_gc: Vec<(u64, u64)> = Vec::new();

        let names: Vec<String> = dir_ps.items().keys().cloned().collect();
        for name in names {
            let this_path = s_path.join(&name);
            let mut item = dir_ps
                .remove_item(&name)
                .ok_or(PackFileError::NotFound(format!("结构项\"{name}\"不存在")))?;

            match item.item_type() {
                PackStructItemType::File { .. } => {
                    self.load_metadata_to_item(&this_path, &mut item)?;
                    let mut metadata = item.metadata_mut().try_lock().map_err(|_| {
                        PackFileError::Lock("文件正在被使用 / File is in use".into())
                    })?;

                    let data_pos_list = match metadata.file_type() {
                        PackFileMetadataType::File { data_pos_list, .. } => {
                            data_pos_list.list().clone()
                        }
                        _ => unreachable!(),
                    };

                    let metadata_file_pos = metadata.data_block_mut().file_pos();

                    // 覆写每个数据块 / Overwrite each data block
                    for &(pos, len) in &data_pos_list {
                        self.overwrite_data_block(pos, len, strategy)?;
                    }
                    // 覆写元数据块 / Overwrite metadata block
                    self.overwrite_manifest_block(metadata_file_pos, strategy)?;

                    total_gc.extend(data_pos_list);
                    total_r.file_count += 1;
                    total_r.length += metadata.len();
                }
                PackStructItemType::Dir {
                    struct_file_pos,
                    pack_struct,
                } => {
                    let mut sub_ps = match pack_struct {
                        Some(ps) => ps.clone(),
                        None => self.load_pack_struct(*struct_file_pos)?,
                    };

                    let (sub_r, sub_gc) =
                        self.erase_dir_contents(&mut sub_ps, &this_path, strategy)?;

                    // 覆写子目录的 PackStruct 数据块 / Overwrite sub-dir's PackStruct block
                    self.overwrite_manifest_block(*struct_file_pos, strategy)?;

                    total_gc.extend(sub_gc);
                    total_r.file_count += sub_r.file_count;
                    total_r.dir_count += sub_r.dir_count + 1;
                    total_r.length += sub_r.length;
                }
            }
        }

        Ok((total_r, total_gc))
    }

    // ==================== 底层覆写操作 / Low-Level Overwrite Operations ====================

    /// 使用指定策略覆写文件中的数据块 / Overwrite a data block in the pack file
    ///
    /// 不读取原始数据——直接生成覆写模式并写入整块。
    /// 因为数据块分配大小按 DATA_DATA_BLOCK_LEN 对齐，写入量可能小于分配量，
    /// 读取整块会导致 UnexpectedEof。
    ///
    /// Does NOT read original data — generates the overwrite pattern directly
    /// and writes the entire block. Since data block allocation is aligned to
    /// DATA_DATA_BLOCK_LEN, the written amount may be smaller than the allocation,
    /// and reading the full block would cause UnexpectedEof.
    fn overwrite_data_block(
        &mut self,
        pos: u64,
        len: u64,
        strategy: OverwriteStrategy,
    ) -> Result<()> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let len_us = usize::try_from(len).map_err(|e| {
            PackFileError::Other(format!("无法将len的u64的数字转为usize, err: {e}"))
        })?;
        let mut buf = vec![0u8; len_us];

        match strategy {
            OverwriteStrategy::Zero => {
                Self::fill_zero(&mut buf);
                pack_file.set_pos_write(pos)?;
                pack_file.write_all(&buf)?;
            }
            OverwriteStrategy::Random => {
                Self::fill_random(&mut buf);
                pack_file.set_pos_write(pos)?;
                pack_file.write_all(&buf)?;
            }
            OverwriteStrategy::Dod5220 => {
                // DoD 5220.22-M: 三次独立磁盘覆写 / Three independent disk overwrites
                // Pass 1: 全 0x00 → 磁盘 / All zeros → disk
                Self::fill_zero(&mut buf);
                pack_file.set_pos_write(pos)?;
                pack_file.write_all(&buf)?;
                // Pass 2: 全 0xFF → 磁盘 / All ones → disk
                Self::fill_ones(&mut buf);
                pack_file.set_pos_write(pos)?;
                pack_file.write_all(&buf)?;
                // Pass 3: 随机字节 → 磁盘 / Random bytes → disk
                Self::fill_random(&mut buf);
                pack_file.set_pos_write(pos)?;
                pack_file.write_all(&buf)?;
            }
        }

        Ok(())
    }

    fn overwrite_manifest_block(
        &mut self,
        block_pos: u64,
        strategy: OverwriteStrategy,
    ) -> Result<()> {
        let block = self.manifest_data_block_read(block_pos)?;
        let block_len = block.get_this_block_len_u64();
        let block_len_us = usize::try_from(block_len).map_err(|e| {
            PackFileError::Other(format!("无法将block_len的u64的数字转为usize, err: {e}"))
        })?;
        let mut buf = vec![0u8; block_len_us];

        match strategy {
            OverwriteStrategy::Zero => {
                Self::fill_zero(&mut buf);
                self.manifest_data_block_write(&buf, false, block_pos, block_len)?;
            }
            OverwriteStrategy::Random => {
                Self::fill_random(&mut buf);
                self.manifest_data_block_write(&buf, false, block_pos, block_len)?;
            }
            OverwriteStrategy::Dod5220 => {
                // DoD 5220.22-M: 三次独立磁盘覆写 / Three independent disk overwrites
                // Pass 1: 全 0x00 → 磁盘 / All zeros → disk
                Self::fill_zero(&mut buf);
                self.manifest_data_block_write(&buf, false, block_pos, block_len)?;
                // Pass 2: 全 0xFF → 磁盘 / All ones → disk
                Self::fill_ones(&mut buf);
                self.manifest_data_block_write(&buf, false, block_pos, block_len)?;
                // Pass 3: 随机字节 → 磁盘 / Random bytes → disk
                Self::fill_random(&mut buf);
                self.manifest_data_block_write(&buf, false, block_pos, block_len)?;
            }
        }

        Ok(())
    }

    fn fill_zero(buf: &mut [u8]) {
        buf.fill(0x00);
    }

    fn fill_ones(buf: &mut [u8]) {
        buf.fill(0xFF);
    }

    fn fill_random(buf: &mut [u8]) {
        for chunk in buf.chunks_mut(4096) {
            let random_bytes: Vec<u8> = (0..chunk.len()).map(|_| rand::random::<u8>()).collect();
            chunk.copy_from_slice(&random_bytes);
        }
    }
}
