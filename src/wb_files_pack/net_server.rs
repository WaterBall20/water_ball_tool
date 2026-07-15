use crate::wb_files_pack::allocator::Allocator;
use std::collections::HashMap;
use crate::wb_files_pack::error::{Result, PackFileError};
use std::path::Path;
use std::sync::{Arc, Mutex};

struct WBFPServer {
    run: Arc<Mutex<WBFPServerRun>>,
}

struct WBFPServerRun {
    pack_list: Arc<Mutex<HashMap<String, Allocator>>>,
}
impl WBFPServerRun {
    fn add_pack(&mut self, path: &Path, allocator: Allocator) -> Result<()> {
        let pack_list = self.pack_list.clone();
        let mut pack_list = pack_list
            .lock()
            .map_err(|e| PackFileError::Lock(format!("无法获得包文件实例列表对象锁, err:{e}")))?;
        pack_list.insert(path.display().to_string(), allocator);
        Ok(())
    }
}

struct WBFPServerClient {
    pack: Allocator,
}
