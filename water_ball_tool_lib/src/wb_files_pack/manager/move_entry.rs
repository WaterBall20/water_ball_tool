//! 包内条目移动 / In-pack entry relocation
//!
//! 与 [`super::rename`] 分开：`move_entry` 必须换父目录，可同时修改叶子名称。
//! Same-directory name changes stay in `rename`; `move_entry` requires a
//! different parent and may also change the leaf name.

//AI=== 包内跨目录移动 + 改名 / in-pack multi-directory move with optional rename
use crate::wb_files_pack::error::{PackFileError, Result};
use crate::wb_files_pack::{
    PackFileMetadataRun, PackFileMetadataType, PackStruct, PackStructItem, PackStructItemType,
};
use std::path::{Path, PathBuf};

use super::WBFPManager;
use super::rename::validate_new_name;

#[derive(Clone, Copy)]
struct MoveStats {
    file_count: u64,
    dir_count: u64,
    len: u64,
}

impl WBFPManager {
    /// 移动包内条目到另一个目录，可同时修改叶子名称。
    ///
    /// 源与目标父目录必须不同；同目录改名请使用 `rename`。目标父目录必须已存在，
    /// 不会自动创建；目标名称必须不存在。目录不能移动到自己的子树内。
    /// 被移动文件或目录子树存在打开的虚拟文件句柄时返回 [`PackFileError::Lock`]。
    ///
    /// Move an entry to another directory, optionally changing its leaf name.
    /// The source and destination parents must differ; use `rename` for a
    /// same-directory name change. The destination parent must already exist and
    /// the target name must be free. A directory cannot move into its own
    /// subtree. Returns [`PackFileError::Lock`] while the file or any descendant
    /// of a moved directory has an open virtual-file handle.
    pub(crate) fn move_entry(&mut self, old_path: &[String], new_path: &[String]) -> Result<()> {
        if old_path.is_empty() || new_path.is_empty() {
            return Err(PackFileError::Format(
                "移动路径不能为空 / move paths must not be empty".into(),
            ));
        }

        let old_name = &old_path[old_path.len() - 1];
        let new_name = &new_path[new_path.len() - 1];
        let old_parent = &old_path[..old_path.len() - 1];
        let new_parent = &new_path[..new_path.len() - 1];
        if old_parent == new_parent {
            return Err(PackFileError::Other(
                "move_entry 只用于更换父目录；同目录改名请使用 rename / move_entry requires a different parent; use rename for a same-directory name change"
                    .into(),
            ));
        }
        validate_new_name(new_name)?;

        let old_path_text = path_text(old_path);
        self.load_pack_struct_metadata_path(path_buf(old_path))?;
        let (is_dir, stats) = {
            let item = self.get_pack_struct_item(path_buf(old_path))?;
            Self::check_item_not_locked(item, Path::new(&old_path_text))?;
            (
                matches!(item.item_type(), PackStructItemType::Dir { .. }),
                entry_stats(item)?,
            )
        };
        if is_dir && path_has_prefix(new_parent, old_path) {
            return Err(PackFileError::Other(
                "不能把目录移动到自己的子树 / a directory cannot move into its own subtree".into(),
            ));
        }

        if !new_parent.is_empty() {
            let _ = self.get_pack_struct_item_dir(path_buf(new_parent))?;
        }
        let new_path_text = path_text(new_path);
        match self.get_pack_struct_item(path_buf(new_path)) {
            Ok(_) => {
                return Err(PackFileError::Other(format!(
                    r#"目标名称"{new_path_text}"已存在 / target name already exists"#
                )));
            }
            Err(PackFileError::NotFound(_)) => {}
            Err(err) => return Err(err),
        }

        let item = if old_path.len() == 1 {
            self.manifest
                .root_struct_mut()
                .remove_item(old_name)
                .ok_or_else(|| {
                    PackFileError::NotFound(format!(r#"虚拟路径{old_path_text:?}"的结构项不存在"#))
                })?
        } else {
            self.detach_nested(old_path, &stats)?
        };

        // shortcut: 目标块先写、源块后写，两次写入不是事务；中途崩溃可能留下重复引用，
        // 但不会丢失条目。需要严格原子移动时再引入清单事务。
        // shortcut: destination is saved before source; the two writes are not a
        // transaction. A crash can leave a duplicate reference but never loses the
        // entry. Add a manifest transaction if strict atomicity is required.
        self.attach_nested(new_parent, item, new_name, &stats)?;
        self.save_source_chain(old_path)
    }

    /// 从嵌套目录中取出条目并更新源祖先的累计值。
    fn detach_nested(&mut self, path: &[String], stats: &MoveStats) -> Result<PackStructItem> {
        let first_name = &path[0];
        let first_path = PathBuf::from(first_name);
        let mut first_item = self
            .manifest
            .root_struct_mut()
            .remove_item(first_name)
            .ok_or_else(|| {
                PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    first_path.display()
                ))
            })?;

        let result = (|| -> Result<PackStructItem> {
            let sub_ps = self.dir_struct_mut(&mut first_item, &first_path)?;
            self.detach_under(sub_ps, &path[1..], &first_path, stats)
        })();
        let result = match result {
            Ok(item) => match self.adjust_stats(&mut first_item, stats, true, &first_path) {
                Ok(()) => Ok(item),
                Err(err) => Err(err),
            },
            Err(err) => Err(err),
        };
        self.manifest
            .root_struct_mut()
            .add_item(first_name.clone(), first_item);
        result
    }

    fn detach_under(
        &mut self,
        parent_ps: &mut PackStruct,
        path: &[String],
        s_path: &Path,
        stats: &MoveStats,
    ) -> Result<PackStructItem> {
        let name = &path[0];
        let this_path = s_path.join(name);
        if path.len() == 1 {
            return parent_ps.remove_item(name).ok_or_else(|| {
                PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    this_path.display()
                ))
            });
        }

        let mut child = parent_ps.remove_item(name).ok_or_else(|| {
            PackFileError::NotFound(format!(
                r#"虚拟路径"{}"的结构项不存在"#,
                this_path.display()
            ))
        })?;
        let result = (|| -> Result<PackStructItem> {
            let sub_ps = self.dir_struct_mut(&mut child, &this_path)?;
            self.detach_under(sub_ps, &path[1..], &this_path, stats)
        })();
        let result = match result {
            Ok(item) => match self.adjust_stats(&mut child, stats, true, &this_path) {
                Ok(()) => Ok(item),
                Err(err) => Err(err),
            },
            Err(err) => Err(err),
        };
        parent_ps.add_item(name.clone(), child);
        result
    }

    /// 把条目挂到目标目录并更新目标祖先的累计值。
    fn attach_nested(
        &mut self,
        parent_path: &[String],
        mut item: PackStructItem,
        new_name: &str,
        stats: &MoveStats,
    ) -> Result<()> {
        if parent_path.is_empty() {
            let root = self.manifest.root_struct_mut();
            if root.get_item(new_name).is_some() {
                return Err(PackFileError::Other(format!(
                    r#"目标名称"{new_name}"已存在 / target name already exists"#
                )));
            }
            item.set_name(new_name.to_string());
            root.add_item(new_name.to_string(), item);
            self.save_root_pack_struct()?;
            return Ok(());
        }

        let first_name = &parent_path[0];
        let first_path = PathBuf::from(first_name);
        let mut first_item = self
            .manifest
            .root_struct_mut()
            .remove_item(first_name)
            .ok_or_else(|| {
                PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    first_path.display()
                ))
            })?;
        let mut item = Some(item);
        let result = (|| -> Result<()> {
            let sub_ps = self.dir_struct_mut(&mut first_item, &first_path)?;
            self.attach_under(
                sub_ps,
                &parent_path[1..],
                &mut item,
                new_name,
                stats,
                &first_path,
            )?;
            let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
            if new_block {
                set_dir_struct_pos(&mut first_item, pos);
            }
            Ok(())
        })();
        let result = match result {
            Ok(()) => self.apply_stats(&mut first_item, stats, false, &first_path),
            Err(err) => Err(err),
        };
        self.manifest
            .root_struct_mut()
            .add_item(first_name.clone(), first_item);
        result?;
        self.save_root_pack_struct()?;
        Ok(())
    }

    fn attach_under(
        &mut self,
        parent_ps: &mut PackStruct,
        dir_path: &[String],
        item: &mut Option<PackStructItem>,
        new_name: &str,
        stats: &MoveStats,
        s_path: &Path,
    ) -> Result<()> {
        if dir_path.is_empty() {
            if parent_ps.get_item(new_name).is_some() {
                return Err(PackFileError::Other(format!(
                    r#"目标名称"{new_name}"已存在 / target name already exists"#
                )));
            }
            let mut item = item.take().ok_or_else(|| {
                PackFileError::State("移动条目已丢失 / moved item is missing".into())
            })?;
            item.set_name(new_name.to_string());
            parent_ps.add_item(new_name.to_string(), item);
            return Ok(());
        }

        let name = &dir_path[0];
        let this_path = s_path.join(name);
        let mut child = parent_ps.remove_item(name).ok_or_else(|| {
            PackFileError::NotFound(format!(
                r#"虚拟路径"{}"的结构项不存在"#,
                this_path.display()
            ))
        })?;
        let result = (|| -> Result<()> {
            let sub_ps = self.dir_struct_mut(&mut child, &this_path)?;
            self.attach_under(sub_ps, &dir_path[1..], item, new_name, stats, &this_path)?;
            let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
            if new_block {
                set_dir_struct_pos(&mut child, pos);
            }
            Ok(())
        })();
        let result = match result {
            Ok(()) => self.apply_stats(&mut child, stats, false, &this_path),
            Err(err) => Err(err),
        };
        parent_ps.add_item(name.clone(), child);
        result
    }

    /// 读取或加载目录项的子结构 / Borrow or load a directory item's child struct.
    fn dir_struct_mut<'a>(
        &mut self,
        item: &'a mut PackStructItem,
        path: &Path,
    ) -> Result<&'a mut PackStruct> {
        let PackStructItemType::Dir {
            struct_file_pos,
            pack_struct,
        } = item.item_type_mut()
        else {
            return Err(PackFileError::NotADirectory(format!(
                r#"虚拟路径"{}"是文件不是目录"#,
                path.display()
            )));
        };
        if pack_struct.is_none() {
            *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
        }
        pack_struct.as_mut().ok_or_else(|| {
            PackFileError::State(format!(
                r#"虚拟路径"{}"的结构实例没有被加载"#,
                path.display()
            ))
        })
    }

    /// 增加或减少目录元数据中的累计值 / Add or subtract a directory's cumulative stats.
    fn apply_stats(
        &mut self,
        item: &mut PackStructItem,
        stats: &MoveStats,
        subtract: bool,
        path: &Path,
    ) -> Result<()> {
        self.adjust_stats(item, stats, subtract, path)?;
        self.save_dir_metadata(item, path)
    }

    /// 只修改内存中的目录累计值，不写盘 / Adjust cumulative stats in memory without writing.
    fn adjust_stats(
        &mut self,
        item: &mut PackStructItem,
        stats: &MoveStats,
        subtract: bool,
        path: &Path,
    ) -> Result<()> {
        self.load_metadata_to_item(path, item)?;
        match item.metadata_mut() {
            PackFileMetadataRun::Loaded(metadata) => {
                if subtract {
                    metadata.sub_file_count(stats.file_count);
                    metadata.sub_dir_count(stats.dir_count);
                    metadata.sub_len(stats.len);
                } else {
                    metadata.add_file_count(stats.file_count);
                    metadata.add_dir_count(stats.dir_count);
                    metadata.add_len(stats.len);
                }
                Ok(())
            }
            PackFileMetadataRun::Locked => Err(PackFileError::Lock(format!(
                "目录正在被使用，禁止移动 / directory is in use: {}",
                path.display()
            ))),
            PackFileMetadataRun::NoLoad | PackFileMetadataRun::None => Err(PackFileError::State(
                format!(r#"虚拟路径"{}"的元数据不可用"#, path.display()),
            )),
        }
    }

    /// 将目录项的元数据写回并更新位置 / Persist a directory item's metadata and position.
    fn save_dir_metadata(&mut self, item: &mut PackStructItem, path: &Path) -> Result<()> {
        self.load_metadata_to_item(path, item)?;
        let (new_block, pos) = match item.metadata_mut() {
            PackFileMetadataRun::Loaded(metadata) => self.save_metadata_write(metadata.as_mut())?,
            PackFileMetadataRun::Locked => {
                return Err(PackFileError::Lock(format!(
                    "目录正在被使用，禁止移动 / directory is in use: {}",
                    path.display()
                )));
            }
            PackFileMetadataRun::NoLoad | PackFileMetadataRun::None => {
                return Err(PackFileError::State(format!(
                    r#"虚拟路径"{}"的元数据不可用"#,
                    path.display()
                )));
            }
        };
        if new_block {
            item.set_metadata_file_pos(pos);
        }
        Ok(())
    }

    /// 目标写完后保存源祖先链 / Persist the source ancestor chain after the destination is saved.
    fn save_source_chain(&mut self, old_path: &[String]) -> Result<()> {
        let parent = &old_path[..old_path.len() - 1];
        if parent.is_empty() {
            return self.save_root_pack_struct();
        }

        let first_name = &parent[0];
        let first_path = PathBuf::from(first_name);
        let mut first_item = self
            .manifest
            .root_struct_mut()
            .remove_item(first_name)
            .ok_or_else(|| {
                PackFileError::NotFound(format!(
                    r#"虚拟路径"{}"的结构项不存在"#,
                    first_path.display()
                ))
            })?;
        let result = (|| -> Result<()> {
            let sub_ps = self.dir_struct_mut(&mut first_item, &first_path)?;
            self.save_chain_under(sub_ps, &parent[1..], &first_path)?;
            let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
            if new_block {
                set_dir_struct_pos(&mut first_item, pos);
            }
            self.save_dir_metadata(&mut first_item, &first_path)?;
            Ok(())
        })();
        self.manifest
            .root_struct_mut()
            .add_item(first_name.clone(), first_item);
        result?;
        self.save_root_pack_struct()
    }

    fn save_chain_under(
        &mut self,
        parent_ps: &mut PackStruct,
        names: &[String],
        s_path: &Path,
    ) -> Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        let name = &names[0];
        let this_path = s_path.join(name);
        let mut child = parent_ps.remove_item(name).ok_or_else(|| {
            PackFileError::NotFound(format!(
                r#"虚拟路径"{}"的结构项不存在"#,
                this_path.display()
            ))
        })?;
        let result = (|| -> Result<()> {
            let sub_ps = self.dir_struct_mut(&mut child, &this_path)?;
            self.save_chain_under(sub_ps, &names[1..], &this_path)?;
            let (new_block, pos) = self.save_pack_struct_write(sub_ps)?;
            if new_block {
                set_dir_struct_pos(&mut child, pos);
            }
            self.save_dir_metadata(&mut child, &this_path)?;
            Ok(())
        })();
        parent_ps.add_item(name.clone(), child);
        result
    }
}

fn entry_stats(item: &PackStructItem) -> Result<MoveStats> {
    let PackFileMetadataRun::Loaded(metadata) = item.metadata() else {
        return Err(PackFileError::State(
            "移动条目的元数据不可用 / moved entry metadata is unavailable".into(),
        ));
    };
    match metadata.file_type() {
        PackFileMetadataType::File { .. } => Ok(MoveStats {
            file_count: 1,
            dir_count: 0,
            len: metadata.len(),
        }),
        PackFileMetadataType::Dir {
            file_count,
            dir_count,
        } => Ok(MoveStats {
            file_count: *file_count,
            dir_count: *dir_count + 1,
            len: metadata.len(),
        }),
    }
}

fn path_text(path: &[String]) -> String {
    path.join("/")
}

fn path_buf(path: &[String]) -> PathBuf {
    let mut result = PathBuf::new();
    for name in path {
        result.push(name);
    }
    result
}

fn path_has_prefix(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && path[..prefix.len()] == *prefix
}

fn set_dir_struct_pos(item: &mut PackStructItem, pos: u64) {
    match item.item_type_mut() {
        PackStructItemType::Dir {
            struct_file_pos, ..
        } => *struct_file_pos = pos,
        PackStructItemType::File { .. } => {
            unreachable!("逻辑错误，目录项在移动后应保持目录类型 / directory item changed type")
        }
    }
}
//AI_END===
