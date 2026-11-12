//内存测试工具

use crate::command::{PACK_PROGRESS_STYLE_TEMPLATE, create_pb};
use clap::Args;
use indicatif::{MultiProgress, ProgressBar};
use std::alloc::{Layout, alloc, dealloc};
use std::error::Error;
use std::num::NonZero;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::vec::Drain;
use tracing::{error, warn};
use water_ball_tool_lib::tools::MemTool;

#[derive(Debug, Args)]
pub struct RamProbeArgs {
    /// 更新大小循环周期次数
    #[arg(default_value_t = 100)]
    reset_count: usize,
    /// 目标占用百分比
    #[arg(default_value_t = 0.57)]
    ratio: f64,
}
const BLOCK_LEN: usize = 4096;
const MEM_BLOCK_DATA_LEN: usize = BLOCK_LEN - blake3::OUT_LEN;
struct PageBlock {
    base_addr: usize, // 虚拟地址
    size: usize,
    ptr: *mut u8,
    layout: Layout,
}

unsafe impl Send for PageBlock {}
impl PageBlock {
    pub fn alloc_block(size: usize) -> Result<Self, Box<dyn Error>> {
        let layout = Layout::from_size_align(size, size)?;
        let ptr = unsafe { alloc(layout) };
        if ptr.is_null() {
            return Err("failed to allocate memory".into());
        }
        Ok(PageBlock {
            base_addr: ptr as usize,
            size,
            ptr,
            layout,
        })
    }

    pub fn get(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
    }

    pub fn get_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.size) }
    }
}
impl Drop for PageBlock {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.ptr, self.layout);
        }
    }
}

struct MemManager {
    men_block_list: Vec<(blake3::Hash, PageBlock)>,
    pos: usize,
    len: usize,
}
impl MemManager {
    fn new() -> Self {
        Self {
            men_block_list: Vec::new(),
            pos: 0,
            len: 0,
        }
    }

    fn list_len(&self) -> usize {
        self.men_block_list.len()
    }

    fn mem_len(&self) -> usize {
        self.len
    }

    fn capacity(&self) -> usize {
        self.men_block_list.capacity()
    }

    fn reserve_exact(&mut self, additional: usize) {
        self.men_block_list.reserve_exact(additional);
    }

    fn push(&mut self, mem_block: (blake3::Hash, PageBlock)) {
        self.len += mem_block.1.size;
        self.men_block_list.push(mem_block);
    }

    fn remove_count(&mut self, count: usize) -> Drain<'_, (blake3::Hash, PageBlock)> {
        let strat = self.list_len() - count;
        if self.pos >= (self.list_len() - count) {
            self.pos = 0;
        }
        let r = self.men_block_list.drain(strat..);
        for v in r.as_slice() {
            self.len -= v.1.size;
        }
        r
    }

    fn clear(&mut self) {
        self.pos = 0;
        self.men_block_list.clear();
    }

    fn get(&mut self) -> Option<&(blake3::Hash, PageBlock)> {
        match self.get_mut() {
            Some(mem_block) => Some(&*mem_block),
            None => None,
        }
    }

    fn get_mut(&mut self) -> Option<&mut (blake3::Hash, PageBlock)> {
        let list_len = self.men_block_list.len();
        if list_len == 0 {
            return None;
        }
        let Some(r) = self.men_block_list.get_mut(self.pos) else {
            unreachable!()
        };
        self.pos = if self.pos + 1 == list_len {
            0
        } else {
            self.pos + 1
        };
        Some(r)
    }
}
fn this_create_user_pb(mp: &MultiProgress, prefix: &'static str) -> ProgressBar {
    let pb = create_pb(&mp);
    pb.set_style(crate::command::progress_style(
        "{prefix:<15.bold.green} [{wide_bar:.cyan/blue}] \
{percent_precise:>7.magenta}% {bytes:>11} / {total_bytes:>11} \n{msg:.yellow}",
    ));
    pb.set_prefix(prefix);
    pb
}
pub(super) fn args(args: RamProbeArgs) -> Result<(), Box<dyn Error>> {
    let Some(page_size) = MemTool::page_size() else {
        return Err("无法获取系统内存页大小".into());
    };
    let test_page_block_len = page_size * 1024;
    let reset_count = args.reset_count;
    let ratio = args.ratio;
    let mp = crate::multi_progress_init("rem_probe");
    let system_user_pb = this_create_user_pb(&mp, "系统所有程序利用率");
    let this_user_system_pb = this_create_user_pb(&mp, "测试内存占系统利用率");
    let this_user_other_pb = this_create_user_pb(&mp, "测试内存占所有程序利用率");
    let mut system_info = sysinfo::System::new();
    let mut mem_manager = MemManager::new();
    let mut loop_count = 0;
    let mut last_mem_len = 0;
    let mut hasher = blake3::Hasher::new();
    system_info.refresh_memory();
    let total_len = system_info.total_memory();
    let total_radio_len_add = (total_len as f64 * (ratio - 0.05)) as u64;
    let total_radio_len = (total_len as f64 * ratio) as u64;
    let total_radio_len_remove = (total_len as f64 * (ratio + 0.05)) as u64;
    system_user_pb.set_length(total_len);
    this_user_system_pb.set_length(total_len);
    loop {
        let refresh = loop_count % 10 == 0;
        if refresh {
            system_info.refresh_memory();
            let used_len = system_info.used_memory();
            system_user_pb.set_position(used_len);
            let mem_lem = mem_manager.mem_len() as u64;
            this_user_system_pb.set_position(mem_lem);
            this_user_other_pb.set_length(used_len);
            this_user_other_pb.set_position(mem_lem);
        };
        if loop_count % reset_count == 0 {
            if !refresh {
                system_info.refresh_memory();
            }
            let used_len = system_info.used_memory();
            if total_radio_len_add > used_len {
                //添加
                let block_all_len = total_radio_len - used_len;
                let block_all_len = usize::try_from(block_all_len).unwrap_or(usize::MAX);
                let block_all_count = block_all_len / test_page_block_len;
                if block_all_count > mem_manager.capacity() {
                    let add = block_all_count - mem_manager.list_len();
                    mem_manager.reserve_exact(add);
                }
                if mem_manager.list_len() < block_all_count {
                    let add_len = block_all_count - mem_manager.list_len();
                    let pb = create_pb(&mp);
                    pb.set_style(crate::command::progress_style(PACK_PROGRESS_STYLE_TEMPLATE));
                    pb.set_prefix("正在增加测试内存");
                    pb.set_length((add_len * test_page_block_len) as u64);
                    let this_add_len = Arc::new(AtomicUsize::new(0));
                    let thread_count = thread::available_parallelism().map_or(
                        match NonZero::new(8) {
                            Some(v) => v.get(),
                            None => 8,
                        },
                        NonZero::get,
                    );
                    let (tx, rx) = mpsc::channel();
                    {
                        let tx = tx;
                        for _ in 0..thread_count {
                            let add_len = add_len;
                            let this_add_len = this_add_len.clone();
                            let tx = tx.clone();
                            thread::spawn(move || {
                                let mut hasher = blake3::Hasher::new();
                                while this_add_len.load(Ordering::Acquire) < add_len {
                                    hasher.reset();
                                    let mut page_block =
                                        match PageBlock::alloc_block(test_page_block_len) {
                                            Ok(p) => p,
                                            Err(e) => {
                                                error!("分配页块失败, err: {e}");
                                                continue;
                                            }
                                        };
                                    let block = page_block.get_mut();
                                    for v in &mut *block {
                                        *v = rand::random();
                                    }
                                    hasher.update(&block);
                                    let hash = hasher.finalize();
                                    if let Err(e) = tx.send((hash, page_block)) {
                                        unreachable!("对象转移线程失败, err: {}", e);
                                    };
                                    this_add_len.fetch_add(1, Ordering::Release);
                                }
                            });
                        }
                    }

                    let mut this_add_mem_len = 0;
                    for item in rx {
                        mem_manager.push(item);
                        this_add_mem_len += test_page_block_len;
                        pb.set_position(this_add_mem_len as u64);
                    }
                }
                last_mem_len = mem_manager.mem_len();
            } else if total_radio_len_remove < used_len {
                //移除
                let remove_len = used_len - total_radio_len;
                let remove_len = usize::try_from(remove_len).unwrap_or(0);
                let remove_count = remove_len / test_page_block_len;
                //移除
                if mem_manager.list_len() > remove_count {
                    let sub_len = mem_manager.list_len() - remove_count;
                    mem_manager.remove_count(sub_len);
                } else {
                    if last_mem_len > 0 {
                        warn!("系统内存超出设置比例({ratio})，已释放所有测试内存");
                        mem_manager.clear();
                        last_mem_len = 0;
                    }
                }
            }
        }
        if let Some((hash, block)) = mem_manager.get() {
            hasher.reset();
            hasher.update(&block.get());
            if *hash != hasher.finalize() {
                error!(
                    "测试失败：虚拟内存地址：0x{:x} - 0x{:x} ({} bytes)",
                    block.base_addr,
                    block.base_addr + block.size,
                    block.size
                )
            }
        }
        loop_count = loop_count.wrapping_add(1);
    }
}
