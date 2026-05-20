use crate::wb_files_pack::allocator::Allocator;
use std::collections::HashMap;
use std::io;
use std::io::Error;
use std::path::Path;
use std::sync::{Arc, Mutex};

struct WBFPServer {
    run: Arc<Mutex<WBFPServerRun>>,
}

struct WBFPServerRun {
    pack_list: Arc<Mutex<HashMap<String, Allocator>>>,
}
impl WBFPServerRun {
    fn add_pack(&mut self, path: &Path, allocator: Allocator) -> io::Result<()> {
        let pack_list = self.pack_list.clone();
        let mut pack_list = pack_list
            .lock()
            .map_err(|e| Error::other(format!("无法获得包文件实例列表对象锁, err:{e}")))?;
        pack_list.insert(path.display().to_string(), allocator);
        Ok(())
    }
}

struct WBFPServerClient {
    pack: Allocator,
}
