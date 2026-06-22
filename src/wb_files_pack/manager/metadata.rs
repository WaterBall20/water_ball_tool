use crate::wb_files_pack::{
    PackFileMetadata, PackFileMetadataRun, PackStruct, PackStructItem, PackStructItemType,
};
use std::io;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::vec::IntoIter;

use super::{DirFileAddReturn, WBFPManager};

impl WBFPManager {
    pub(crate) fn file_metadata_update(
        &mut self,
        path_list: Vec<String>,
        mut metadata: PackFileMetadata,
    ) -> io::Result<()> {
        let mut path_list = path_list.into_iter();
        let two_name = path_list.next().ok_or(Error::other("路径为空"))?;
        if let Some(mut struct_item) = self.manifest.root_struct_mut().remove_item(&two_name) {
            match struct_item.item_type_mut() {
                PackStructItemType::Dir {
                    struct_file_pos,
                    pack_struct,
                } => {
                    if pack_struct.is_none() {
                        *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                    }
                    if let Some(pack_struct) = pack_struct {
                        let r = self.file_metadata_update_inner(
                            path_list,
                            &PathBuf::from(&two_name),
                            pack_struct,
                            metadata,
                        )?;
                        let (new_pos, pos) = self.save_pack_struct_write(pack_struct)?;
                        if new_pos {
                            *struct_file_pos = pos;
                        }
                        self.save_metadata(&mut struct_item, &r)?;
                        self.manifest
                            .root_struct_mut()
                            .add_item(two_name, struct_item);
                        self.manifest.attribute_mut().add_file_count(r.file_count);
                        self.manifest.attribute_mut().add_data_len(r.length);
                    } else {
                        panic!("逻辑错误")
                    }
                }
                PackStructItemType::File => {
                    if path_list.next().is_none() {
                        let (new_pos, pos) = self.save_metadata_write(&mut metadata)?;
                        if new_pos {
                            struct_item.set_metadata_file_pos(pos);
                        }
                        struct_item.metadata_mut().unlock(metadata);
                        self.manifest
                            .root_struct_mut()
                            .add_item(two_name, struct_item);
                    } else {
                        Err(Error::new(
                            ErrorKind::NotADirectory,
                            format!(r#"虚拟路径"{two_name}"是文件不是目录"#),
                        ))?;
                    }
                }
            }
        } else if path_list.next().is_none() {
            let (_, metadata_file_pos) =
                self.save_metadata_write(&mut metadata)
                    .or(Err(Error::other(format!(
                        r#"无法保存文件"{}"的元数据"#,
                        two_name
                    ))))?;
            let len = metadata.len();
            let pack_struct_item = PackStructItem::new(
                two_name.clone(),
                PackStructItemType::File,
                metadata_file_pos,
                PackFileMetadataRun::Loaded(Box::from(metadata)),
            );
            self.manifest
                .root_struct_mut()
                .add_item(two_name, pack_struct_item);
            self.manifest.attribute_mut().add_file_count(1);
            self.manifest.attribute_mut().add_data_len(len);
        } else {
            Err(Error::new(
                ErrorKind::NotFound,
                format!(r#"虚拟路径"{two_name}"不存在"#),
            ))?;
        }
        self.save_root_pack_struct()
    }

    fn save_metadata(
        &mut self,
        struct_item: &mut PackStructItem,
        r: &DirFileAddReturn,
    ) -> Result<(), Error> {
        if let PackFileMetadataRun::Loaded(metadata) = struct_item.metadata_mut() {
            metadata.add_len(r.length);
            metadata.add_file_count(r.file_count);
            let (new_pos, pos) = self.save_metadata_write(metadata)?;
            if new_pos {
                struct_item.set_metadata_file_pos(pos);
            }
        }
        Ok(())
    }

    fn file_metadata_update_inner(
        &mut self,
        mut path_list: IntoIter<String>,
        s_path: &Path,
        pack_struct: &mut PackStruct,
        mut metadata: PackFileMetadata,
    ) -> io::Result<DirFileAddReturn> {
        if let Some(name) = path_list.next() {
            let this_path = s_path.join(&name);
            if let Some(item) = pack_struct.get_item_mut(&name) {
                let r = {
                    let r = match item.item_type_mut() {
                        PackStructItemType::Dir {
                            struct_file_pos,
                            pack_struct: sub_ps,
                        } => {
                            if sub_ps.is_none() {
                                *sub_ps = Some(self.load_pack_struct(*struct_file_pos)?);
                            }
                            if let Some(pack_struct) = sub_ps {
                                let r = self.file_metadata_update_inner(
                                    path_list,
                                    &PathBuf::from(name.clone()),
                                    pack_struct,
                                    metadata,
                                )?;
                                let (new_pos, pos) =
                                    self.save_pack_struct_write(pack_struct)?;
                                if new_pos {
                                    *struct_file_pos = pos;
                                }
                                Ok(r)
                            } else {
                                Err(Error::other("逻辑错误"))
                            }
                        }
                        PackStructItemType::File => {
                            if path_list.next().is_none() {
                                let (new_pos, pos) =
                                    self.save_metadata_write(&mut metadata)?;
                                if new_pos {
                                    item.set_metadata_file_pos(pos);
                                }
                                item.metadata_mut().unlock(metadata);
                                Ok(DirFileAddReturn {
                                    length: 0,
                                    file_count: 0,
                                    dir_count: 0,
                                })
                            } else {
                                Err(Error::new(
                                    ErrorKind::NotADirectory,
                                    format!(
                                        r#"虚拟路径"{}"是文件不是目录"#,
                                        this_path.display()
                                    ),
                                ))
                            }
                        }
                    }?;
                    self.save_metadata(item, &r)?;
                    Ok::<_, Error>(r)
                }?;
                pack_struct.mark_dirty();
                Ok(r)
            } else if path_list.next().is_none() {
                let (_, metadata_file_pos) =
                    self.save_metadata_write(&mut metadata)
                        .or(Err(Error::other(format!(
                            r#"无法保存文件"{}"的元数据"#,
                            this_path.display()
                        ))))?;
                let len = metadata.len();
                let psi = PackStructItem::new(
                    name.clone(),
                    PackStructItemType::File,
                    metadata_file_pos,
                    PackFileMetadataRun::Loaded(Box::from(metadata)),
                );
                pack_struct.add_item(name, psi);
                Ok(DirFileAddReturn {
                    length: len,
                    file_count: 1,
                    dir_count: 0,
                })
            } else {
                Err(Error::new(
                    ErrorKind::NotFound,
                    format!(r#"虚拟路径"{}"不存在"#, this_path.display()),
                ))
            }
        } else {
            panic!("逻辑错误")
        }
    }

    pub(crate) fn file_metadata_lock(
        &mut self,
        path_list: &[String],
    ) -> io::Result<PackFileMetadata> {
        let mut path_list = path_list.iter();
        let two_name = path_list.next().ok_or(Error::other("路径为空"))?;
        let file_metadata = if let Some(mut pack_struct_item) =
            self.manifest.root_struct_mut().remove_item(two_name)
        {
            match pack_struct_item.item_type_mut() {
                PackStructItemType::Dir {
                    struct_file_pos,
                    pack_struct,
                } => {
                    if pack_struct.is_none() {
                        *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                    }
                    if let Some(pack_struct) = pack_struct {
                        let r = self.file_metadata_lock_inner(
                            &mut path_list,
                            &PathBuf::from(two_name),
                            pack_struct,
                        )?;
                        self.manifest
                            .root_struct_mut()
                            .add_item(two_name.clone(), pack_struct_item);
                        r
                    } else {
                        panic!("逻辑错误");
                    }
                }
                PackStructItemType::File => {
                    if path_list.next().is_none() {
                        let r = pack_struct_item.metadata_mut().try_lock()?;
                        self.manifest
                            .root_struct_mut()
                            .add_item(two_name.clone(), pack_struct_item);
                        r
                    } else {
                        self.manifest
                            .root_struct_mut()
                            .add_item(two_name.clone(), pack_struct_item);
                        Err(Error::new(
                            ErrorKind::NotADirectory,
                            format!(r#"虚拟路径"{two_name}是文件不是目录""#),
                        ))?
                    }
                }
            }
        } else {
            Err(Error::new(ErrorKind::NotFound, "文件或目录不存在"))?
        };
        Ok(file_metadata)
    }

    fn file_metadata_lock_inner(
        &mut self,
        path_list: &mut core::slice::Iter<String>,
        s_path: &Path,
        pack_struct: &mut PackStruct,
    ) -> io::Result<PackFileMetadata> {
        if let Some(name) = path_list.next() {
            let this_path = s_path.join(name);
            if let Some(item) = pack_struct.get_item_mut(name) {
                match item.item_type_mut() {
                    PackStructItemType::Dir {
                        struct_file_pos,
                        pack_struct: sub_ps,
                    } => {
                        if sub_ps.is_none() {
                            *sub_ps = Some(self.load_pack_struct(*struct_file_pos)?);
                        }
                        if let Some(pack_struct) = sub_ps {
                            self.file_metadata_lock_inner(path_list, &this_path, pack_struct)
                        } else {
                            panic!("逻辑错误")
                        }
                    }
                    PackStructItemType::File => {
                        if path_list.next().is_none() {
                            self.load_metadata_to_item(&this_path, item)?;
                            item.metadata_mut().try_lock()
                        } else {
                            Err(Error::new(
                                ErrorKind::NotADirectory,
                                format!(r#"虚拟路径"{}"是文件不是目录"#, this_path.display()),
                            ))?
                        }
                    }
                }
            } else {
                Err(Error::new(
                    ErrorKind::NotFound,
                    format!(r#"虚拟路径"{}"不存在"#, this_path.display()),
                ))?
            }
        } else {
            panic!("逻辑错误")
        }
    }
}
