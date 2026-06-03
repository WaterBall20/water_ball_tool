use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::wb_files_pack::allocator::Allocator;

struct WBFPServer {
    pack_list: Arc<Mutex<HashMap<String, Allocator>>>
}