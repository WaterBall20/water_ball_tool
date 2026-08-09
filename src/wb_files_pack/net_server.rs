use crate::wb_files_pack::manager_sync::ManagerSync;
use crate::wb_files_pack::error::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

struct WBFPServer {
    run: Arc<Mutex<WBFPServerRun>>,
}

struct WBFPServerRun {
    pack_list: Arc<Mutex<HashMap<String, ManagerSync>>>,
}
impl WBFPServerRun {
    fn add_pack(&mut self, path: &Path, sync: ManagerSync) -> Result<()> {
        let pack_list = self.pack_list.clone();
        let mut pack_list = pack_list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pack_list.insert(path.display().to_string(), sync);
        Ok(())
    }
}

struct WBFPServerClient {
    pack: ManagerSync,
}
