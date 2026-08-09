use crate::tools::PathTool;
use crate::wb_files_pack::error::{PackFileError, Result};
use crate::wb_files_pack::manager::DEFAULT_HASH_TYPE;
use crate::wb_files_pack::{
    DATA_DATA_BLOCK_LEN, DataPosList, MAX_DATA_SEGMENTS, PackFileMetadata, PackFileMetadataRun,
    PackFileMetadataType, PackStruct, PackStructItem, PackStructItemType,
};
use std::path::Path;
use std::time::SystemTime;

use super::{DirFileAddReturn, WBFPManager};

impl WBFPManager {
    /// 创建虚拟文件（未知大小用 4MiB 分配，已知大小按 128B 精确分配）。
    ///
    /// `alloc_size`：`Some(len)` 已知大小 → 128B 精确对齐分配，不浪费空间；
    /// `None` 未知大小 → 按 4MiB 整块分配，多余空间由 GC 回收。
    ///
    /// Create a virtual file (`Some(len)` known size → exact 128B-aligned allocation;
    /// `None` unknown size → whole 4MiB blocks, excess reclaimed by GC).
    pub(crate) fn create_file_auto_sized<P: AsRef<Path>>(
        &mut self,
        path: P,
        alloc_size: Option<u64>,
    ) -> Result<(Vec<String>, PackFileMetadata)> {
        let alloc_len = alloc_size.unwrap_or(DATA_DATA_BLOCK_LEN);
        self.create_file_raw(
            path,
            if let Ok(d) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                d.as_millis()
            } else {
                0
            },
            alloc_len,
            self.cow,
            DEFAULT_HASH_TYPE,
            alloc_size.is_none(),
        )
    }

    /// 创建虚拟文件（已知大小，按 128B 精确分配）。
    /// Create a virtual file with a known size (exact 128B-aligned allocation).
    pub(crate) fn create_file<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
    ) -> Result<(Vec<String>, PackFileMetadata)> {
        self.create_file_raw(path, modified, len, self.cow, DEFAULT_HASH_TYPE, false)
    }

    pub(crate) fn create_file_raw<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
        cow: bool,
        hash_type: u8,
        data_alloc_4mib: bool,
    ) -> Result<(Vec<String>, PackFileMetadata)> {
        let path_list = PathTool::path_to_string_vec(&path);
        if self.path_exists(&path) {
            Err(PackFileError::Other(format!(
                r#"虚拟路径"{}"文件或目录已存在"#,
                path.as_ref().display()
            )))?;
        }
        if path_list.len() > 1 {
            self.create_dir_all2(&path_list[..path_list.len() - 1])?;
        }
        //已知大小：128B 精确分配；未知大小：4MiB 碎片拼接分配
        //Known size: exact 128B allocation; unknown size: 4MiB fragment stitching
        let data_pos = if data_alloc_4mib {
            self.get_data_file_pos(len, 0, MAX_DATA_SEGMENTS)?
        } else {
            vec![self.get_file_pos(len)?]
        };
        let metadata = PackFileMetadata::new(
            cow,
            len,
            modified,
            PackFileMetadataType::File {
                hash_type,
                hash_value: Vec::new(),
                data_pos_list: DataPosList::new(data_pos),
            },
        );
        Ok((path_list, metadata))
    }

    pub(crate) fn create_dir_all<P: AsRef<Path>>(&mut self, path: &P) -> Result<()> {
        self.create_dir_all2(&PathTool::path_to_string_vec(path))
    }

    pub(crate) fn create_dir_all2(&mut self, path_list: &[String]) -> Result<()> {
        let mut path_list = path_list.iter();
        let root_struct = self.manifest.root_struct_mut();
        let two_name = path_list
            .next()
            .ok_or(PackFileError::Format("路径为空".into()))?;
        let (mut two_item, mut two_r) = if let Some(mut item) = root_struct.remove_item(two_name) {
            match item.item_type_mut() {
                PackStructItemType::Dir {
                    struct_file_pos,
                    pack_struct,
                } => {
                    if pack_struct.is_none() {
                        *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                    }
                }
                PackStructItemType::File { .. } => {
                    Err(PackFileError::Format(format!(
                        r#"虚拟路径"{two_name}"是文件不是目录"#
                    )))?;
                }
            }
            (
                item,
                DirFileAddReturn {
                    length: 0,
                    file_count: 0,
                    dir_count: 0,
                    unlocked_occurred: false,
                },
            )
        } else {
            (
                PackStructItem::new_empty_dir(two_name, PackFileMetadata::new_empty_dir(self.cow)),
                DirFileAddReturn {
                    length: 0,
                    file_count: 0,
                    dir_count: 1,
                    unlocked_occurred: false,
                },
            )
        };
        //重开（从 .wbm 加载）后已存在目录的元数据处于 NoLoad 状态，
        //必须先加载再更新统计，否则下面的 Loaded 分支会 panic
        //After reopening, metadata of an existing dir is NoLoad; load it first
        //before updating stats, otherwise the Loaded branch below panics.
        self.load_metadata_to_item(Path::new(two_name.as_str()), &mut two_item)?;
        match two_item.item_type_mut() {
            PackStructItemType::Dir {
                struct_file_pos,
                pack_struct,
            } => {
                if let Some(two_ps) = pack_struct {
                    let r = self.create_dir_all_inner(
                        two_ps,
                        &mut path_list,
                        self.cow,
                        two_name.as_ref(),
                    )?;
                    two_r.dir_count += r.dir_count;
                    two_r.file_count += r.file_count;
                    two_r.length += r.length;
                    let (new_block, pos) = self.save_pack_struct_write(two_ps)?;
                    if new_block {
                        *struct_file_pos = pos;
                    }
                    if let PackFileMetadataRun::Loaded(metadata) = two_item.metadata_mut() {
                        metadata.add_len(r.length);
                        metadata.add_file_count(r.file_count);
                        metadata.add_dir_count(r.dir_count);
                        let (new_block, pos) = self.save_metadata_write(metadata)?;
                        if new_block {
                            two_item.set_metadata_file_pos(pos);
                        }
                    } else {
                        panic!("逻辑错误， 元数据运行应为已加载");
                    }
                    self.manifest
                        .root_struct_mut()
                        .add_item(two_name.clone(), two_item);
                } else {
                    self.manifest
                        .root_struct_mut()
                        .add_item(two_name.clone(), two_item);
                    Err(PackFileError::Other(format!(
                        r#"虚拟路径"{two_name}"的结构不存在"#
                    )))?;
                }
            }
            PackStructItemType::File { .. } => {
                self.manifest
                    .root_struct_mut()
                    .add_item(two_name.clone(), two_item);
                Err(PackFileError::Format(format!(
                    r#"虚拟路径"{two_name}"是文件不是目录"#
                )))?;
            }
        }
        self.manifest.attribute_mut().add_dir_count(two_r.dir_count);
        self.manifest
            .attribute_mut()
            .add_file_count(two_r.file_count);
        self.manifest.attribute_mut().add_data_len(two_r.length);
        {
            let pack_file = self.pack_file.clone();
            let mut pack_file = pack_file
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pack_file.run_data.all_cr_file_count += two_r.file_count + two_r.dir_count;
        }
        self.save_root_pack_struct()?;
        self.throttled_save()?;
        Ok(())
    }

    fn create_dir_all_inner(
        &mut self,
        s_pack_struct: &mut PackStruct,
        path_list: &mut core::slice::Iter<String>,
        cow: bool,
        s_path: &Path,
    ) -> Result<DirFileAddReturn> {
        if let Some(name) = path_list.next() {
            let this_path = s_path.join(name);
            if let Some(item) = s_pack_struct.get_item_mut(name) {
                self.load_metadata_to_item(&this_path, item)?;
                let r = (match item.item_type_mut() {
                    PackStructItemType::Dir {
                        struct_file_pos,
                        pack_struct,
                    } => {
                        if pack_struct.is_none() {
                            *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                        }
                        if let Some(pack_struct) = pack_struct {
                            let r =
                                self.create_dir_all_inner(pack_struct, path_list, cow, &this_path)?;
                            let (new_block, pos) = self.save_pack_struct_write(pack_struct)?;
                            if new_block {
                                *struct_file_pos = pos;
                            }
                            Ok(r)
                        } else {
                            Err(PackFileError::State(format!(
                                r#"虚拟路径"{}"的结构没有被加载"#,
                                this_path.display()
                            )))
                        }
                    }
                    PackStructItemType::File { .. } => Err(PackFileError::NotADirectory(format!(
                        "虚拟路径{}存在同名文件",
                        this_path.display()
                    ))),
                })?;
                if r.dir_count != 0 || r.file_count != 0 || r.length != 0 {
                    if let PackFileMetadataRun::Loaded(metadata) = item.metadata_mut() {
                        metadata.add_dir_count(r.dir_count);
                        metadata.add_file_count(r.file_count);
                        metadata.add_len(r.length);
                        let (new_block, pos) = self.save_metadata_write(metadata)?;
                        if new_block {
                            item.set_metadata_file_pos(pos);
                            s_pack_struct.mark_dirty();
                        }
                    }
                } else {
                    match item.metadata_mut() {
                        PackFileMetadataRun::Loaded(_) => (),
                        PackFileMetadataRun::Locked => {
                            Err(PackFileError::State("元数据被锁定".into()))?;
                        }
                        PackFileMetadataRun::NoLoad => panic!("元数据没有被加载"),
                        PackFileMetadataRun::None => panic!("逻辑错误：目录的元数据为空"),
                    }
                }
                Ok(DirFileAddReturn {
                    dir_count: r.dir_count,
                    file_count: r.file_count,
                    length: r.length,
                    unlocked_occurred: false,
                })
            } else {
                let mut item =
                    PackStructItem::new_empty_dir(name, PackFileMetadata::new_empty_dir(cow));
                let r = if let PackStructItemType::Dir {
                    struct_file_pos,
                    pack_struct,
                } = item.item_type_mut()
                    && let Some(pack_struct) = pack_struct
                {
                    let r = self.create_dir_all_inner(pack_struct, path_list, cow, &this_path)?;
                    let (_, pos) = self.save_pack_struct_write(pack_struct)?;
                    *struct_file_pos = pos;
                    if let PackFileMetadataRun::Loaded(metadata) = item.metadata_mut() {
                        metadata.add_dir_count(r.dir_count);
                        metadata.add_file_count(r.file_count);
                        metadata.add_len(r.length);
                        let (_, pos) = self.save_metadata_write(metadata)?;
                        item.set_metadata_file_pos(pos);
                    } else {
                        panic!("逻辑错误");
                    }
                    r
                } else {
                    panic!("逻辑错误")
                };
                s_pack_struct.add_item(name.clone(), item);
                Ok(DirFileAddReturn {
                    length: r.length,
                    file_count: r.file_count,
                    dir_count: r.dir_count + 1,
                    unlocked_occurred: false,
                })
            }
        } else {
            Ok(DirFileAddReturn {
                length: 0,
                file_count: 0,
                dir_count: 0,
                unlocked_occurred: false,
            })
        }
    }
}
