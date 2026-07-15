use crate::tools::PathTool;
use crate::wb_files_pack::{
    PackFileMetadata,
    PackFileMetadataRun,
    PackStruct,
    PackStructItem,
    PackStructItemType,
};
use core::slice::Iter;
use std::collections::HashMap;
use crate::wb_files_pack::error::{Result, PackFileError};
use std::path::{Path, PathBuf};

use super::WBFPManager;

impl WBFPManager {
    pub(crate) fn path_exists<P: AsRef<Path>>(&mut self, path: P) -> bool {
        self.get_pack_struct_item(path).is_ok()
    }
    pub(crate) fn get_manifest_attribute(&self) -> &crate::wb_files_pack::Attribute {
        self.manifest.attribute()
    }
    pub(crate) fn get_root_struct_items(&self) -> &HashMap<String, PackStructItem> {
        self.manifest.root_struct().items()
    }

    pub(crate) fn get_root_struct_item_name_list(&self) -> Vec<String> {
        let mut name_list = Vec::with_capacity(self.manifest.root_struct().items().len());
        for name in self.manifest.root_struct().items().keys() {
            name_list.push(name.clone());
        }
        name_list
    }

    pub(crate) fn get_struct_item_name_list<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> Result<Vec<String>> {
        let item = self.get_pack_struct_item(&path)?;
        match &item.item_type() {
            PackStructItemType::Dir { pack_struct, .. } => {
                if let Some(pack_struct) = pack_struct {
                    let mut name_list = Vec::with_capacity(pack_struct.items().len());
                    for name in pack_struct.items().keys() {
                        name_list.push(name.clone());
                    }
                    Ok(name_list)
                } else {
                    Err(
                        PackFileError::State(
                            format!(r#"虚拟路径"{}"的结构实例没有被加载"#, path.as_ref().display())
                        )
                    )
                }
            }
            PackStructItemType::File { .. } =>
                Err(
                    PackFileError::NotADirectory(
                        format!(r#"虚拟路径"{}"是文件不是目录"#, path.as_ref().display())
                    )
                ),
        }
    }

    pub(crate) fn get_dir_pack_struct_items<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> Result<&HashMap<String, PackStructItem>> {
        self.load_pack_struct_metadata_path(&path)?;
        let item = self.get_pack_struct_item_dir(&path)?;
        match &item.item_type() {
            PackStructItemType::Dir { pack_struct, .. } => {
                if let Some(pack_struct) = pack_struct {
                    Ok(pack_struct.items())
                } else {
                    Err(
                        PackFileError::State(
                            format!(r#"虚拟路径"{}"的结构实例没有被加载"#, path.as_ref().display())
                        )
                    )
                }
            }
            PackStructItemType::File { .. } =>
                Err(
                    PackFileError::NotADirectory(
                        format!(r#"提供的路径"{}"是文件不是目录"#, path.as_ref().display())
                    )
                ),
        }
    }

    pub(crate) fn get_pack_struct_item_dir<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> Result<&PackStructItem> {
        let path_list = PathTool::path_to_string_vec(path);
        self.load_pack_struct_metadata_path2(&path_list, true)?;
        self.get_pack_struct_item2(&path_list)
    }

    pub(crate) fn get_pack_struct_item<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> Result<&PackStructItem> {
        let path_list = PathTool::path_to_string_vec(path);
        self.load_pack_struct_metadata_path2(&path_list, false)?;
        self.get_pack_struct_item2(&path_list)
    }

    fn get_pack_struct_item2(&self, path_list: &[String]) -> Result<&PackStructItem> {
        if path_list.len() > 1 {
            let mut name_list = path_list.iter();
            let mut s_name = name_list.next();
            let mut s_pack_struct = self.manifest.root_struct();
            let mut s_path = PathBuf::new();
            while let Some(name) = s_name {
                let this_path = s_path.join(name);
                if let Some(item) = s_pack_struct.items().get(name) {
                    if let Some(next_name) = name_list.next() {
                        match &item.item_type() {
                            PackStructItemType::Dir { pack_struct, .. } => {
                                if let Some(pack_struct) = pack_struct {
                                    s_pack_struct = pack_struct;
                                    s_path = this_path;
                                    s_name = Some(next_name);
                                } else {
                                    Err(
                                        PackFileError::State(
                                            format!(
                                                r#"虚拟路径"{}"实例没有加载"#,
                                                this_path.display()
                                            )
                                        )
                                    )?;
                                }
                            }
                            PackStructItemType::File { .. } => {
                                Err(
                                    PackFileError::NotADirectory(
                                        format!(
                                            r#"虚拟路径"{}"是文件不是目录"#,
                                            this_path.display()
                                        )
                                    )
                                )?;
                            }
                        }
                    } else {
                        return Ok(item);
                    }
                } else {
                    Err(
                        PackFileError::NotFound(
                            format!(r#"虚拟路径目录"{}"的结构项不存在"#, this_path.display())
                        )
                    )?;
                }
            }
            Err(PackFileError::NotFound(format!("未找到路径{path_list:?}")))
        } else if let Some(v) = self.manifest.root_struct().items().get(&path_list[0]) {
            Ok(v)
        } else if !path_list.is_empty() {
            Err(
                PackFileError::NotFound(
                    format!(r#"虚拟路径"{}"的结构项不存在"#, path_list[0])
                )
            )
        } else {
            Err(PackFileError::Format("提供了无效或空的路径".into()))
        }
    }

    pub(crate) fn load_all_data(&mut self, no_err: bool) -> Result<()> {
        fn m_load_all_data(
            wbfp_manager: &mut WBFPManager,
            pack_struct: &mut PackStruct,
            no_err: bool,
            s_path: &Path,
        ) -> Result<()> {
            let keys: Vec<String> = pack_struct.items().keys().cloned().collect();
            for name in keys {
                let item = pack_struct.get_item_mut(&name).unwrap();
                let item_name = item.name().clone();
                if
                let PackStructItemType::Dir { struct_file_pos, pack_struct: sub_ps } =
                    item.item_type_mut()
                {
                    let struct_file_pos = *struct_file_pos;
                    if sub_ps.is_none() {
                        let mut this_pack_struct = wbfp_manager.load_pack_struct(struct_file_pos)?;
                        if
                        let Err(err) = m_load_all_data(
                            wbfp_manager,
                            &mut this_pack_struct,
                            no_err,
                            &s_path.join(&item_name),
                        ) &&
                            !no_err
                        {
                            Err(err)?;
                        }
                        *sub_ps = Some(this_pack_struct);
                    }
                }
                if let PackFileMetadataRun::NoLoad = item.metadata() {
                    let this_path = s_path.join(&item_name);
                    let metadata_file_pos = item.metadata_file_pos();
                    let metadata = match wbfp_manager.load_pack_file_metadata(metadata_file_pos) {
                        Ok(v) => PackFileMetadataRun::Loaded(Box::new(v)),
                        Err(err) =>
                            Err(
                                PackFileError::State(
                                    format!(
                                        r#"虚拟路径"{}"的元数据无法加载, err:{err}"#,
                                        this_path.display()
                                    )
                                )
                            )?,
                    };
                    *item.metadata_mut() = metadata;
                }
            }
            Ok(())
        }
        let root_keys: Vec<String> = self.manifest.root_struct().items().keys().cloned().collect();
        let mut temp_ps = PackStruct::default();
        for name in &root_keys {
            let item = self.manifest.root_struct_mut().remove_item(name).unwrap();
            temp_ps.add_item(name.clone(), item);
        }
        if let Err(err) = m_load_all_data(self, &mut temp_ps, no_err, &PathBuf::new()) && !no_err {
            for name in &root_keys {
                let item = temp_ps.remove_item(name).unwrap();
                self.manifest.root_struct_mut().add_item(name.clone(), item);
            }
            Err(err)?;
        }
        for name in &root_keys {
            let item = temp_ps.remove_item(name).unwrap();
            self.manifest.root_struct_mut().add_item(name.clone(), item);
        }
        Ok(())
    }

    pub(crate) fn load_pack_struct_metadata_path<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> Result<()> {
        self.load_pack_struct_metadata_path2(&PathTool::path_to_string_vec(path), false)
    }

    fn load_pack_struct_metadata_path2(
        &mut self,
        path_list: &[String],
        is_dir: bool,
    ) -> Result<()> {
        let mut path_list = path_list.iter();
        let two_name = path_list.next().unwrap();
        let two_pack_struct = if
        let Some(mut struct_item) = self.manifest.root_struct_mut().remove_item(two_name)
        {
            match struct_item.item_type_mut() {
                PackStructItemType::Dir { struct_file_pos, pack_struct } => {
                    if pack_struct.is_none() {
                        *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                    }
                    if let Some(pack_struct) = pack_struct {
                        if
                        let Err(err) = self.s_load_pack_struct_metadata_path(
                            pack_struct,
                            &mut path_list,
                            is_dir,
                            &PathBuf::from(two_name),
                        )
                        {
                            self.manifest.root_struct_mut().add_item(two_name.clone(), struct_item);
                            return Err(err);
                        }
                        if let PackFileMetadataRun::NoLoad = struct_item.metadata() {
                            let metadata_file_pos = struct_item.metadata_file_pos();
                            *struct_item.metadata_mut() = PackFileMetadataRun::Loaded(match
                            self.load_pack_file_metadata(metadata_file_pos)
                            {
                                Ok(v) => Box::from(v),
                                Err(err) =>
                                    Err(
                                        PackFileError::State(
                                            format!(
                                                r#"虚拟路径"{two_name}"的元数据加载失败, err:{err}"#
                                            )
                                        )
                                    )?,
                            });
                        }
                        struct_item
                    } else {
                        Err(PackFileError::State(format!(r#"虚拟路径"{two_name}"的结构实例没有被加载"#)))?
                    }
                }
                PackStructItemType::File { .. } => {
                    if is_dir || path_list.next().is_some() {
                        Err(
                            PackFileError::NotADirectory(
                                format!(r#"虚拟路径"{two_name}"是文件不是目录"#)
                            )
                        )?
                    } else {
                        if let PackFileMetadataRun::NoLoad = struct_item.metadata() {
                            let metadata_file_pos = struct_item.metadata_file_pos();
                            *struct_item.metadata_mut() = PackFileMetadataRun::Loaded(match
                            self.load_pack_file_metadata(metadata_file_pos)
                            {
                                Ok(v) => Box::from(v),
                                Err(err) =>
                                    Err(
                                        PackFileError::State(
                                            format!(
                                                r#"虚拟路径"{two_name}"的元数据无法加载, err:{err}"#
                                            )
                                        )
                                    )?,
                            });
                        }
                        struct_item
                    }
                }
            }
        } else {
            Err(PackFileError::NotFound(format!(r#"虚拟路径"{two_name}"的结构项不存在"#)))?
        };
        self.manifest.root_struct_mut().add_item(two_pack_struct.name().clone(), two_pack_struct);
        Ok(())
    }

    fn s_load_pack_struct_metadata_path(
        &mut self,
        s_pack_struct: &mut PackStruct,
        path_list: &mut Iter<String>,
        is_dir: bool,
        s_path: &Path,
    ) -> Result<()> {
        if let Some(this_name) = path_list.next() {
            let this_path = s_path.join(this_name);
            if let Some(item) = s_pack_struct.get_item_mut(this_name) {
                match item.item_type_mut() {
                    PackStructItemType::Dir { struct_file_pos, pack_struct } => {
                        if pack_struct.is_none() {
                            *pack_struct = Some(self.load_pack_struct(*struct_file_pos)?);
                        }
                        if let Some(pack_struct) = pack_struct {
                            self.s_load_pack_struct_metadata_path(
                                pack_struct,
                                path_list,
                                is_dir,
                                &this_path,
                            )?;
                            self.load_metadata_to_item(&this_path, item)?;
                            Ok(())
                        } else {
                            Err(
                                PackFileError::State(
                                    format!(
                                        r#"虚拟路径"{}"的结构实例没有被加载"#,
                                        this_path.display()
                                    )
                                )
                            )
                        }
                    }
                    PackStructItemType::File { .. } => {
                        if is_dir || path_list.next().is_some() {
                            Err(
                                PackFileError::NotADirectory(
                                    format!(r#"虚拟路径"{}"是文件不是目录"#, this_path.display())
                                )
                            )
                        } else {
                            self.load_metadata_to_item(&this_path, item)?;
                            Ok(())
                        }
                    }
                }
            } else {
                Err(
                    PackFileError::NotFound(
                        format!(r#"虚拟路径"{}"的结构项不存在"#, this_path.display())
                    )
                )?
            }
        } else {
            Ok(())
        }
    }

    pub(super) fn load_metadata_to_item(
        &mut self,
        this_path: &Path,
        item: &mut PackStructItem,
    ) -> Result<()> {
        if let PackFileMetadataRun::NoLoad = item.metadata() {
            let metadata_file_pos = item.metadata_file_pos();
            *item.metadata_mut() = PackFileMetadataRun::Loaded(match
            self.load_pack_file_metadata(metadata_file_pos)
            {
                Ok(v) => Box::from(v),
                Err(err) =>
                    Err(
                        PackFileError::State(
                            format!(
                                r#"虚拟路径"{}"的元数据无法加载, err: {err}"#,
                                this_path.display()
                            )
                        )
                    )?,
            });
        }
        Ok(())
    }

    pub(crate) fn get_dir<P: AsRef<Path>>(&mut self, path: P) -> Result<&PackStruct> {
        let path_list = PathTool::path_to_string_vec(path);
        if path_list.is_empty() {
            Err(PackFileError::Format("提供了无效或空的路径".into()))
        } else {
            self.load_pack_struct_metadata_path2(&path_list, true)?;
            let mut pack_struct = self.manifest.root_struct();
            let mut path_list_iter = path_list.iter();
            let mut name = path_list_iter.next();
            let mut path = PathBuf::new();
            while let Some(this_name) = name {
                let this_path = path.join(this_name);
                if let Some(item) = pack_struct.items().get(this_name) {
                    let this_pack_struct = match &item.item_type() {
                        PackStructItemType::Dir { pack_struct, .. } => {
                            if let Some(pack_struct) = pack_struct {
                                pack_struct
                            } else {
                                Err(
                                    PackFileError::State(
                                        format!(
                                            r#"虚拟路径"{}"的结构没有被加载"#,
                                            this_path.display()
                                        )
                                    )
                                )?
                            }
                        }
                        PackStructItemType::File { .. } =>
                            Err(
                                PackFileError::NotADirectory(
                                    format!(r#"虚拟路径"{}"是文件不是目录"#, this_path.display())
                                )
                            )?,
                    };
                    let next_name = path_list_iter.next();
                    if next_name.is_some() {
                        name = next_name;
                        pack_struct = this_pack_struct;
                        path = this_path;
                    } else {
                        return Ok(this_pack_struct);
                    }
                } else {
                    Err(
                        PackFileError::NotFound(
                            format!(r#"虚拟路径"{}不存在""#, this_path.display())
                        )
                    )?;
                }
            }
            Err(PackFileError::NotFound(format!(r#"未找到路径"{}""#, path.display())))
        }
    }

    pub(super) fn load_pack_struct(&self, file_pos: u64) -> Result<PackStruct> {
        let data_block = self.manifest_data_block_read(file_pos)?;
        PackStruct::load(data_block)
    }

    pub(super) fn load_pack_file_metadata(&self, file_pos: u64) -> Result<PackFileMetadata> {
        let data_block = self.manifest_data_block_read(file_pos)?;
        PackFileMetadata::load(data_block)
    }
}
