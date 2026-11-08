//! 包内条目重命名 / In-pack entry rename
//!
//! 只允许同一目录内修改叶子名称，不移动条目；跨目录移动必须使用独立 API。
//! Only the leaf name may change in the same directory; entries are never
//! relocated here. Cross-directory moves must use a separate API.

//AI=== 包内同目录重命名 / in-pack same-directory rename
use crate::wb_files_pack::error::{PackFileError, Result};
use crate::wb_files_pack::{PackFileMetadataRun, PackStruct, PackStructItem, PackStructItemType};
use std::path::{Path, PathBuf};

use super::WBFPManager;

impl WBFPManager {
    /// 包内同目录重命名。
    ///
    /// `old_path` 与 `new_path` 必须等长且父路径完全一致；本方法只修改叶子名称，
    /// 不移动文件或目录。跨目录移动请使用 `move_entry`。
    ///
    /// 文件或目录子树存在打开的虚拟文件句柄时返回 [`PackFileError::Lock`]：
    /// 文件句柄在打开时记录了路径，重命名后句柄关闭会按旧路径回写元数据。
    ///
    /// Rename an entry inside the pack without moving it. `old_path` and
    /// `new_path` must have the same parent; only the leaf name changes.
    /// Use `move_entry` for a cross-directory move.
    ///
    /// Returns [`PackFileError::Lock`] when the file or any file in the
    /// directory subtree has an open virtual-file handle: a handle captures its
    /// path on open and writes metadata back through that path on close.
    pub(crate) fn rename(&mut self, old_path: &[String], new_path: &[String]) -> Result<()> {
        if old_path.is_empty() || new_path.is_empty() {
            return Err(PackFileError::Format(
                "重命名路径不能为空 / rename paths must not be empty".into(),
            ));
        }
        if old_path.len() != new_path.len()
            || old_path[..old_path.len() - 1] != new_path[..new_path.len() - 1]
        {
            return Err(PackFileError::Other(
                "rename 只允许同目录改名，跨目录移动请使用 move_entry / rename may only change the name in the same directory; use move_entry for a cross-directory move"
                    .into(),
            ));
        }

        let old_name = &old_path[old_path.len() - 1];
        let new_name = &new_path[new_path.len() - 1];
        validate_new_name(new_name)?;
        if old_name == new_name {
            return Ok(());
        }

        if old_path.len() == 1 {
            self.rename_root(old_name, new_name)
        } else {
            self.rename_nested(old_path, new_name)
        }
    }

    /// 根目录下的条目重命名 / Rename an entry directly under the root.
    fn rename_root(&mut self, old_name: &str, new_name: &str) -> Result<()> {
        let root = self.manifest.root_struct_mut();
        if root.get_item(old_name).is_none() {
            return Err(PackFileError::NotFound(format!(
                r#"虚拟路径"{old_name}"的结构项不存在"#
            )));
        }
        if root.get_item(new_name).is_some() {
            return Err(PackFileError::Other(format!(
                r#"目标名称"{new_name}"已存在 / target name already exists"#
            )));
        }

        let mut item = root.remove_item(old_name).ok_or_else(|| {
            PackFileError::NotFound(format!(r#"虚拟路径"{old_name}"的结构项不存在"#))
        })?;
        if let Err(err) = Self::check_item_not_locked(&item, Path::new(old_name)) {
            root.add_item(old_name.to_string(), item);
            return Err(err);
        }
        item.set_name(new_name.to_string());
        root.add_item(new_name.to_string(), item);
        self.save_root_pack_struct()?;
        Ok(())
    }

    /// 嵌套目录中的条目重命名 / Rename an entry below the root.
    fn rename_nested(&mut self, old_path: &[String], new_name: &str) -> Result<()> {
        let first_name = &old_path[0];
        let this_path = PathBuf::from(first_name);
        let mut first_item = self
            .manifest
            .root_struct_mut()
            .remove_item(first_name)
            .ok_or_else(|| {
                PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    this_path.display()
                ))
            })?;

        let result = self.rename_nested_inner(&mut first_item, old_path, new_name, &this_path);
        self.manifest
            .root_struct_mut()
            .add_item(first_name.clone(), first_item);
        result?;
        self.save_root_pack_struct()?;
        Ok(())
    }

    fn rename_nested_inner(
        &mut self,
        first_item: &mut PackStructItem,
        old_path: &[String],
        new_name: &str,
        this_path: &Path,
    ) -> Result<()> {
        let PackStructItemType::Dir {
            struct_file_pos,
            pack_struct,
        } = first_item.item_type_mut()
        else {
            return Err(PackFileError::NotADirectory(format!(
                r#"虚拟路径"{}"是文件不是目录"#,
                this_path.display()
            )));
        };
        if pack_struct.is_none() {
            *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
        }
        let sub_ps = pack_struct.as_mut().ok_or_else(|| {
            PackFileError::State(format!(
                r#"虚拟路径"{}"的结构实例没有被加载"#,
                this_path.display()
            ))
        })?;

        self.rename_inner(sub_ps, &old_path[1..], new_name, this_path)?;
        let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
        if new_block {
            *struct_file_pos = pos;
        }
        Ok(())
    }

    /// 递归定位父目录并修改叶子名称 / Walk to the parent directory and rename the leaf.
    fn rename_inner(
        &mut self,
        parent_ps: &mut PackStruct,
        path_list: &[String],
        new_name: &str,
        s_path: &Path,
    ) -> Result<()> {
        let name = &path_list[0];
        let this_path = s_path.join(name);

        if path_list.len() == 1 {
            if parent_ps.get_item(name).is_none() {
                return Err(PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    this_path.display()
                )));
            }
            if parent_ps.get_item(new_name).is_some() {
                return Err(PackFileError::Other(format!(
                    r#"目标名称"{new_name}"已存在 / target name already exists"#
                )));
            }

            let mut item = parent_ps.remove_item(name).ok_or_else(|| {
                PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    this_path.display()
                ))
            })?;
            if let Err(err) = Self::check_item_not_locked(&item, &this_path) {
                parent_ps.add_item(name.clone(), item);
                return Err(err);
            }
            item.set_name(new_name.to_string());
            parent_ps.add_item(new_name.to_string(), item);
            return Ok(());
        }

        let mut item = parent_ps.remove_item(name).ok_or_else(|| {
            PackFileError::NotFound(format!(
                r#"虚拟路径"{}"的结构项不存在"#,
                this_path.display()
            ))
        })?;
        let PackStructItemType::Dir {
            struct_file_pos,
            pack_struct,
        } = item.item_type_mut()
        else {
            parent_ps.add_item(name.clone(), item);
            return Err(PackFileError::NotADirectory(format!(
                r#"虚拟路径"{}"是文件不是目录"#,
                this_path.display()
            )));
        };
        if pack_struct.is_none() {
            match self.load_pack_struct(*struct_file_pos) {
                Ok(loaded) => *pack_struct = Some(loaded),
                Err(err) => {
                    parent_ps.add_item(name.clone(), item);
                    return Err(err);
                }
            }
        }
        let sub_ps = match pack_struct.as_mut() {
            Some(pack_struct) => pack_struct,
            None => {
                parent_ps.add_item(name.clone(), item);
                return Err(PackFileError::State(format!(
                    r#"虚拟路径"{}"的结构实例没有被加载"#,
                    this_path.display()
                )));
            }
        };

        let result = self.rename_inner(sub_ps, &path_list[1..], new_name, &this_path);
        let result = match result {
            Ok(()) => match self.save_pack_struct_write(sub_ps) {
                Ok((new_block, pos)) => {
                    if new_block {
                        *struct_file_pos = pos;
                    }
                    Ok(())
                }
                Err(err) => Err(err),
            },
            Err(err) => Err(err),
        };
        parent_ps.add_item(name.clone(), item);
        result
    }

    /// 检查条目及其子树没有打开的虚拟文件句柄。
    ///
    /// Check that the entry and its subtree have no open virtual-file handles.
    pub(super) fn check_item_not_locked(item: &PackStructItem, path: &Path) -> Result<()> {
        if matches!(item.metadata(), PackFileMetadataRun::Locked) {
            return Err(PackFileError::Lock(format!(
                "条目正在被使用，禁止重命名 / entry is in use: {}",
                path.display()
            )));
        }
        match item.item_type() {
            PackStructItemType::File { handle } => {
                if handle
                    .as_ref()
                    .is_some_and(|handle| handle.strong_count() != 0)
                {
                    Err(PackFileError::Lock(format!(
                        "文件正在被使用，禁止重命名 / file is in use: {}",
                        path.display()
                    )))
                } else {
                    Ok(())
                }
            }
            PackStructItemType::Dir { pack_struct, .. } => {
                if pack_struct
                    .as_ref()
                    .is_some_and(|pack_struct| pack_struct.child_locked_count() != 0)
                {
                    Err(PackFileError::Lock(format!(
                        "目录内有文件正在被使用，禁止重命名 / directory has open file handles: {}",
                        path.display()
                    )))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// 校验新的叶子名称 / Validate the new leaf name.
pub(super) fn validate_new_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(PackFileError::Format(
            "重命名后的名称无效 / invalid rename name".into(),
        ));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(PackFileError::Format(
            "重命名后的名称不能包含路径分隔符 / rename name must not contain a path separator"
                .into(),
        ));
    }
    if name.len() > usize::from(u16::MAX) {
        return Err(PackFileError::Format(
            "重命名后的名称过长 / rename name is too long".into(),
        ));
    }
    Ok(())
}
//AI_END===
