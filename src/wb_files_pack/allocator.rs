use crate::tools::PathTool;
use crate::wb_files_pack::manager::{
    WBFPManager, DEFAULT_COW, DEFAULT_HASH_TYPE, DEFAULT_S_MANIFEST_FILE,
};
use crate::wb_files_pack::pack_io::file::PackFileWR;
use crate::wb_files_pack::pack_io::PackIO;
use crate::wb_files_pack::{Attribute, PackStruct, PackStructItem, PackStructItemType};
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::Error;
use std::path::Path;
use std::sync::{Arc, Mutex};
#[cfg(test)]
mod test;

pub struct Allocator {
    manager: Arc<Mutex<WBFPManager>>,
    pack_io: Arc<Mutex<PackIO>>,
}
impl Allocator {
    pub fn open_pack_file<P: AsRef<Path>>(path: &P) -> io::Result<Allocator> {
        //打开水球包文件
        let pack_file = File::options().read(true).write(true).open(path)?;
        let pack_io = PackIO::new(pack_file);
        let pack_io = Arc::new(Mutex::new(pack_io));
        let manager = WBFPManager::open_pack_file(path, pack_io.clone())?;
        let manager = Arc::new(Mutex::new(manager));
        Ok(Self { manager, pack_io })
    }

    pub fn create_new_pack_file2<P: AsRef<Path>>(path: &P) -> io::Result<Allocator> {
        Self::create_new_pack_file(path, DEFAULT_COW, DEFAULT_S_MANIFEST_FILE)
    }
    pub fn create_new_pack_file<P: AsRef<Path>>(
        path: &P,
        cow: bool,
        s_manifest_file: bool,
    ) -> io::Result<Allocator> {
        //判断文件是否存在
        match path.as_ref().try_exists() {
            Ok(true) => Err(Error::other("文件可能已存在，无法创建！")),
            Ok(false) | Err(_) => Self::create_pack_file(path, cow, s_manifest_file, true),
        }
    }

    pub fn create_pack_file<P: AsRef<Path>>(
        path: &P,
        cow: bool,
        s_manifest_file: bool,
        create_new: bool,
    ) -> io::Result<Allocator> {
        //创建包文件文件
        let pack_file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .create_new(create_new)
            .open(path)?;
        //创建包文件数据文件
        let pack_io = PackIO::new(pack_file);
        let pack_io = Arc::new(Mutex::new(pack_io));
        let mut manager =
            WBFPManager::create_pack_file(path, pack_io.clone(), cow, s_manifest_file, create_new)?;
        manager.init_new_pack()?;
        let manager = Arc::new(Mutex::new(manager));
        Ok(Self { manager, pack_io })
    }
}

impl Allocator /*读*/ {
    pub fn file_is_some<P: AsRef<Path>>(&mut self, path: P) -> io::Result<bool> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.file_is_some(path))
    }
    pub fn get_manifest_attribute(&self) -> io::Result<Attribute> {
        let manager = self.manager.clone();
        let manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_manifest_attribute().clone())
    }
    pub fn get_root_struct_items(&self) -> io::Result<HashMap<String, PackStructItem>> {
        let manager = self.manager.clone();
        let manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_root_struct_items().clone())
    }
    pub fn get_root_struct_item_name_list(&mut self) -> io::Result<Vec<String>> {
        let manager = self.manager.clone();
        let manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_root_struct_item_name_list())
    }
    pub fn get_struct_item_name_list<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> io::Result<Vec<String>> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        manager.get_struct_item_name_list(path)
    }

    pub fn get_dir_pack_struct_items<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> io::Result<HashMap<String, PackStructItem>> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_dir_pack_struct_items(path)?.clone())
    }

    pub fn get_pack_struct_item_dir<P: AsRef<Path>>(
        &mut self,
        path: P,
    ) -> io::Result<PackStructItem> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_pack_struct_item_dir(path)?.clone())
    }

    pub fn get_pack_struct_item<P: AsRef<Path>>(&mut self, path: P) -> io::Result<PackStructItem> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_pack_struct_item(path)?.clone())
    }

    pub fn load_all_data(&mut self, no_err: bool) -> io::Result<()> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        manager.load_all_data(no_err)
    }

    pub fn load_pack_struct_metadata_path<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        manager.load_pack_struct_metadata_path(path)
    }

    pub fn get_dir<P: AsRef<Path>>(&mut self, path: P) -> io::Result<PackStruct> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        Ok(manager.get_dir(path)?.clone())
    }
}

impl Allocator /*写*/ {
    pub fn create_dir_all<P: AsRef<Path>>(&mut self, path: &P) -> io::Result<()> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        manager.create_dir_all(path)
    }

    pub fn create_file2<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
    ) -> io::Result<PackFileWR> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        let (path_list, metadata) = manager.create_file2(path, modified, len)?;
        PackFileWR::create(
            self.manager.clone(),
            self.pack_io.clone(),
            path_list,
            metadata,
        )
    }

    pub fn get_file_wr<P: AsRef<Path>>(&mut self, path: P) -> io::Result<PackFileWR> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        let path_list = PathTool::path_to_string_vec(path);
        let metadata = manager.file_metadata_lock(&path_list)?;
        PackFileWR::create(
            self.manager.clone(),
            self.pack_io.clone(),
            path_list,
            metadata,
        )
    }

    pub fn _create_file_no_len<P: AsRef<Path>>(&mut self, path: P) -> io::Result<PackFileWR> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        let (path_list, metadata) = manager.create_file_no_len(path)?;
        PackFileWR::create(
            self.manager.clone(),
            self.pack_io.clone(),
            path_list,
            metadata,
        )
    }
    pub fn create_file3<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
        cow: bool,
    ) -> io::Result<PackFileWR> {
        self.create_file(path, modified, len, cow, DEFAULT_HASH_TYPE)
    }

    pub fn create_file<P: AsRef<Path>>(
        &mut self,
        path: P,
        modified: u128,
        len: u64,
        cow: bool,
        hash_type: u8,
    ) -> io::Result<PackFileWR> {
        let manager = self.manager.clone();
        let mut manager = manager
            .lock()
            .map_err(|e| Error::other(format!("无法获得管理器锁, err:{e}")))?;
        let (path_list, metadata) = manager.create_file(path, modified, len, cow, hash_type)?;
        PackFileWR::create(
            self.manager.clone(),
            self.pack_io.clone(),
            path_list,
            metadata,
        )
    }
}

impl Allocator /*工具方法*/ {
    pub fn verify_all_file_hash(&mut self) -> io::Result<VerifyHashR> {
        let mut vhr = VerifyHashR::new();
        let root_struct = self.get_root_struct_item_name_list()?;
        for name in root_struct {
            self.verify_all_file_hash_inner(name.as_ref(), &mut vhr)?;
        }
        Ok(vhr)
    }
    fn verify_all_file_hash_inner(
        &mut self,
        path: &Path,
        verify_hash_r: &mut VerifyHashR,
    ) -> io::Result<()> {
        let item = self.get_pack_struct_item(path)?;
        match &item.item_type {
            PackStructItemType::Dir { .. } => {
                let s_file_name = self.get_struct_item_name_list(path)?;
                for name in s_file_name {
                    self.verify_all_file_hash_inner(&path.join(name), verify_hash_r)?;
                }
                Ok(())
            }
            PackStructItemType::File => {
                let mut file = self.get_file_wr(path)?;
                match file.verify_hash() {
                    Ok(true) => verify_hash_r
                        .ok_path
                        .push(path.to_str().unwrap().to_string()),
                    Ok(false) => verify_hash_r
                        .err_path
                        .push((path.to_str().unwrap().to_string(), None)),
                    Err(err) => verify_hash_r
                        .err_path
                        .push((path.to_str().unwrap().to_string(), Some(err))),
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug)]
pub struct VerifyHashR {
    ok_path: Vec<String>,
    err_path: Vec<(String, Option<Error>)>,
}
impl VerifyHashR {
    fn new() -> Self {
        Self {
            ok_path: Vec::new(),
            err_path: Vec::new(),
        }
    }
}
