use crate::wb_files_pack::pack_io::PackIO;
use crate::wb_files_pack::WBFilesPackManifest;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[cfg(test)]
mod test;

/// 默认写时复制开关 / Default copy-on-write flag
pub const DEFAULT_COW: bool = false;
/// 默认分离清单文件开关 / Default separate manifest file flag
pub const DEFAULT_SEPARATE_MANIFEST: bool = true;
/// 默认哈希类型（1 = Blake3）/ Default hash type (1 = Blake3)
pub const DEFAULT_HASH_TYPE: u8 = 1;

pub(crate) struct WBFPManager {
    pub(crate) manifest: WBFilesPackManifest,
    pub(crate) pack_file: Arc<Mutex<PackIO>>,
    pub(crate) cow: bool,
    pub(crate) separate_manifest: bool,
    pub(crate) run_data: WBFPManagerRun,
}

pub(crate) struct WBFPManagerRun {
    pub(crate) write_lock: bool,
    pub(crate) write_lock_path: PathBuf,
    pub(crate) write_lock_file: Option<File>,
}

impl WBFPManagerRun {
    fn new(write_lock_path: PathBuf, write_lock_file: Option<File>) -> Self {
        Self {
            write_lock: false,
            write_lock_path,
            write_lock_file,
        }
    }
}

impl WBFPManager {
    fn new<P: AsRef<Path>>(
        pack_path: P,
        manifest: WBFilesPackManifest,
        pack_file: Arc<Mutex<PackIO>>,
        separate_manifest: bool,
        write_lock_file: Option<File>,
    ) -> Self {
        let cow = manifest.attribute().cow();
        let mut write_lock_path =
            String::from(pack_path.as_ref().to_str().expect("无法将转换路径成文本"));
        write_lock_path.push_str(".lock");
        let write_lock_path = Path::new(&write_lock_path).to_path_buf();
        Self {
            manifest,
            pack_file,
            cow,
            separate_manifest,
            run_data: WBFPManagerRun::new(write_lock_path, write_lock_file),
        }
    }
}

impl Drop for WBFPManager {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.save_all().expect("保存数据错误");
            self.write_unlock().expect("无法解除写入锁");
        }
    }
}

pub(crate) struct DirFileAddReturn {
    pub(crate) length: u64,
    pub(crate) file_count: u64,
    pub(crate) dir_count: u64,
}

pub(crate) enum PackLockType {
    File,
    Dir,
    Symlink,
    _None,
}

pub(crate) struct PackLockInfo {
    pub(crate) run_lock: bool,
    pub(crate) file_lock_type: PackLockType,
    pub(crate) file_lock_pid: Option<u32>,
    pub(crate) file_lock_pid_run: Option<bool>,
}

mod file_ops;
mod gc;
mod lock;
mod metadata;
mod open;
mod save;
mod tree;
