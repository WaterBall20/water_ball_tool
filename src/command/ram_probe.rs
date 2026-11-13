//内存测试工具

use crate::command::{PACK_PROGRESS_STYLE_TEMPLATE, create_pb};
use blake3::{Hash, Hasher};
use clap::Args;
use indicatif::{MultiProgress, ProgressBar};
use rand::Rng;
use rand::rngs::SmallRng;
use std::alloc::{Layout, alloc, dealloc};
use std::error::Error;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, mpsc};
use std::thread;
use std::thread::JoinHandle;
use sysinfo::System;
use tracing::{error, warn};
use water_ball_tool_lib::tools::MemTool;

const BLOCK_LEN: usize = 64 * 1024;
struct PageBlock {
    ptr: *mut u8,
    size: usize,
}

impl PageBlock {
    fn new(size: usize) -> Self {
        debug_assert_ne!(size, 0, "Size cannot be zero");
        Self {
            ptr: std::ptr::null_mut(),
            size,
        }
    }

    fn set_ptr(&mut self, ptr: *mut u8) {
        debug_assert!(!ptr.is_null(), "Pointer cannot be null");
        self.ptr = ptr;
    }

    fn set_size(&mut self, size: usize) {
        debug_assert_ne!(size, 0, "Size cannot be zero");
        self.size = size;
    }

    fn get(&self) -> &[u8] {
        debug_assert!(!self.ptr.is_null(), "Pointer cannot be null");
        debug_assert_ne!(self.size, 0, "Size cannot be zero");
        unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
    }

    fn get_mut(&mut self) -> &mut [u8] {
        debug_assert!(!self.ptr.is_null(), "Pointer cannot be null");
        debug_assert_ne!(self.size, 0, "Size cannot be zero");
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.size) }
    }

    fn get_addr(&self) -> usize {
        debug_assert!(!self.ptr.is_null(), "Pointer cannot be null");
        self.ptr as usize
    }
}

struct Ptr(*mut u8);
unsafe impl Send for Ptr {}

impl Ptr {
    fn new(ptr: *mut u8) -> Self {
        debug_assert!(!ptr.is_null(), "Pointer cannot be null");
        Self(ptr)
    }
    fn get(self) -> *mut u8 {
        debug_assert!(!self.0.is_null(), "Pointer cannot be null");
        self.0
    }
}

struct MemManager {
    ptr_list: Vec<*mut u8>,
    layout_list: Vec<Layout>,
    hash_list: Vec<blake3::Hash>,
    pos: usize,
    block_size: usize,
}
impl MemManager {
    fn new(block_size: usize) -> Self {
        assert!(block_size > 0, "Size cannot be zero");
        Self {
            ptr_list: Vec::new(),
            layout_list: Vec::new(),
            hash_list: Vec::new(),
            pos: 0,
            block_size,
        }
    }

    fn list_len(&self) -> usize {
        self.ptr_list.len()
    }

    fn mem_len(&self) -> usize {
        self.ptr_list.len() * self.block_size
    }

    fn capacity(&self) -> usize {
        self.ptr_list.capacity()
    }

    fn reserve_exact(&mut self, additional: usize) {
        self.ptr_list.reserve_exact(additional);
        self.layout_list.reserve_exact(additional);
        self.hash_list.reserve_exact(additional);
    }

    fn push(&mut self, value: (Ptr, Layout, Hash)) {
        let ptr = value.0.get();
        self.ptr_list.push(ptr);
        self.layout_list.push(value.1);
        self.hash_list.push(value.2);
    }

    fn remove_count(&mut self, count: usize) {
        let strat = (self.list_len() - count).min(self.list_len());
        if self.pos >= strat {
            self.pos = 0;
        }
        let prt_s = self.ptr_list.drain(strat..);
        let layout_s = self.layout_list.drain(strat..);
        let hash_s = self.hash_list.drain(strat..);
        assert_eq!(prt_s.len(), layout_s.len(), "length mismatch");
        assert_eq!(prt_s.len(), hash_s.len(), "length mismatch");
        for (ptr, layout) in prt_s.zip(layout_s) {
            // 释放内存
            unsafe { dealloc(ptr, layout) };
        }
    }

    fn clear(&mut self) {
        self.pos = 0;
        self.remove_count(self.list_len());
    }

    fn get(&mut self) -> Option<(usize, &blake3::Hash, *mut u8)> {
        let list_len = self.ptr_list.len();
        if list_len == 0 {
            return None;
        }
        let pos = self.pos;
        let Some(ptr) = self.ptr_list.get(pos) else {
            unreachable!()
        };
        let Some(hash) = self.hash_list.get(pos) else {
            unreachable!()
        };
        self.pos = if self.pos + 1 == list_len {
            0
        } else {
            self.pos + 1
        };
        Some((pos, hash, *ptr))
    }
}
fn this_create_user_pb(mp: &MultiProgress, prefix: &'static str) -> ProgressBar {
    let pb = create_pb(mp);
    pb.set_style(crate::command::progress_style(
        "{prefix:<25.bold.green} [{wide_bar:.cyan/blue}] \
{percent_precise:>7.magenta}% {bytes:>11} / {total_bytes:>11} \n{msg:.yellow}",
    ));
    pb.set_prefix(prefix);
    pb
}

pub fn alloc_block(size: usize, page_size: usize) -> Result<(*mut u8, Layout), Box<dyn Error>> {
    let layout = Layout::from_size_align(size, page_size)?;
    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() {
        return Err("failed to allocate memory".into());
    }
    Ok((ptr, layout))
}

#[derive(Debug, Args)]
pub struct RamProbeArgs {
    /// 更新大小循环周期次数
    #[arg(default_value_t = 100)]
    reset_count: usize,
    /// 目标占用百分比
    #[arg(default_value_t = 0.80)]
    ratio: f64,
}

pub(super) fn args(args: RamProbeArgs) -> Result<(), Box<dyn Error>> {
    let Some(page_size) = MemTool::page_size() else {
        return Err("无法获取系统内存页大小".into());
    };
    let test_page_block_len = BLOCK_LEN;
    assert!(
        test_page_block_len.is_multiple_of(page_size),
        "测试块大小未对齐"
    );
    // 初始化日志和进度条
    let mp = crate::multi_progress_init("rem_probe");

    // 添加线程
    let thread_stop = Arc::new(AtomicBool::new(false));
    let add_thread_count = thread::available_parallelism().map_or(
        match NonZero::new(8) {
            Some(v) => v.get(),
            None => 8,
        },
        NonZero::get,
    );
    let add_count = Arc::new(AtomicUsize::new(0));
    let this_add_count = Arc::new(AtomicUsize::new(0));
    let add_thread_list = {
        let mut list = Vec::with_capacity(add_thread_count);
        for i in 0..add_thread_count {
            let (tx, rx) = mpsc::channel::<Sender<_>>();
            let add_thread_stop = thread_stop.clone();
            let this_add_count = this_add_count.clone();
            let add_count = add_count.clone();
            let t = thread::Builder::new()
                .name(format!("add-{i}"))
                .spawn(move || {
                    let mut page_block = PageBlock::new(test_page_block_len);
                    let mut hasher = blake3::Hasher::new();
                    let mut ran = rand::make_rng::<SmallRng>();
                    for tx in rx {
                        if add_thread_stop.load(Ordering::Relaxed) {
                            return;
                        }
                        while this_add_count.load(Ordering::Acquire)
                            < add_count.load(Ordering::Acquire)
                        {
                            hasher.reset();
                            let (ptr, layout) = match alloc_block(test_page_block_len, page_size) {
                                Ok(p) => p,
                                Err(e) => {
                                    error!("分配页块失败, err: {e}");
                                    continue;
                                }
                            };
                            page_block.set_ptr(ptr);
                            let block = page_block.get_mut();
                            ran.fill_bytes(block);
                            hasher.update(block);
                            let hash = hasher.finalize();
                            if tx.send((Ptr::new(ptr), layout, hash)).is_err() {
                                unreachable!("接收方已释放");
                            }
                            this_add_count.fetch_add(1, Ordering::Release);
                        }
                    }
                })
                .expect("failed to spawn thread");
            list.push((tx, t));
        }
        list
    };

    let test_loop_thread = thread::Builder::new()
        .name("test_loop_thread".into())
        .spawn(move || {
            test_loop(
                test_page_block_len,
                args,
                &mp,
                add_count,
                thread_stop.clone(),
                this_add_count,
                add_thread_list,
            );
        })
        .expect("failed to spawn test loop thread");
    test_loop_thread
        .join()
        .map_err(|e| format!("fail to join test loop thread, err: {e:?}"))?;
    Ok(())
}

fn test_loop(
    test_page_block_len: usize,
    args: RamProbeArgs,
    mp: &MultiProgress,
    add_count: Arc<AtomicUsize>,
    thread_stop: Arc<AtomicBool>,
    this_add_count: Arc<AtomicUsize>,
    mut add_thread_list: Vec<(Sender<Sender<(Ptr, Layout, Hash)>>, JoinHandle<()>)>,
) {
    let mut system_info = System::new();
    let system_user_pb = this_create_user_pb(mp, "系统所有程序利用率");
    let this_user_system_pb = this_create_user_pb(mp, "测试内存占系统利用率");
    let this_user_other_pb = this_create_user_pb(mp, "测试内存占所有程序利用率");
    let test_pb = create_pb(mp);
    let reset_count = args.reset_count;
    let ratio = args.ratio;
    let mut mem_manager = MemManager::new(test_page_block_len);
    let mut page_block = PageBlock::new(test_page_block_len);
    let mut hasher = Hasher::new();
    let mut last_mem_len = 0;
    let mut loop_count = 0usize;
    let mut test_loop_count = 0;
    system_info.refresh_memory();
    let total_len = system_info.total_memory();
    let total_radio_len = (total_len as f64 * ratio) as u64;
    let total_radio_len_add = (total_len as f64 * (ratio - 0.01)) as u64;
    system_user_pb.set_length(total_len);
    this_user_system_pb.set_length(total_len);
    test_pb.set_style(crate::command::progress_style(PACK_PROGRESS_STYLE_TEMPLATE));
    test_pb.set_prefix("测试进度");
    loop {
        if thread_stop.load(Ordering::Relaxed) {
            return;
        }
        let refresh = loop_count.is_multiple_of(10);
        if refresh {
            system_info.refresh_memory();
            let used_len = system_info.used_memory();
            system_user_pb.set_position(used_len);
            let mem_lem = mem_manager.mem_len() as u64;
            this_user_system_pb.set_position(mem_lem);
            this_user_other_pb.set_length(used_len);
            this_user_other_pb.set_position(mem_lem);
        }
        if loop_count.is_multiple_of(reset_count) {
            if !refresh {
                system_info.refresh_memory();
            }
            let used_len = system_info.used_memory();
            if total_radio_len_add > used_len {
                //添加
                let block_all_len = total_radio_len - (used_len - mem_manager.mem_len() as u64);
                let block_all_len = usize::try_from(block_all_len).unwrap_or(usize::MAX);
                let block_all_count = block_all_len / test_page_block_len;
                if block_all_count > mem_manager.capacity() {
                    let add = block_all_count - mem_manager.list_len();
                    mem_manager.reserve_exact(add);
                }
                if mem_manager.list_len() < block_all_count {
                    let add_count_t = block_all_count - mem_manager.list_len();
                    let pb = create_pb(mp);
                    pb.set_style(crate::command::progress_style(PACK_PROGRESS_STYLE_TEMPLATE));
                    pb.set_prefix("正在增加测试内存");
                    pb.set_length((add_count_t * test_page_block_len) as u64);
                    add_count.store(add_count_t, Ordering::Release);
                    this_add_count.store(0, Ordering::Release);
                    let (tx, rx) = mpsc::channel();
                    for (t_tx, _) in &mut add_thread_list {
                        let tx = tx.clone();
                        if t_tx.send(tx).is_err() {
                            unreachable!("接收方已关闭")
                        }
                    }
                    drop(tx);
                    let mut this_add_mem_len = 0;
                    for item in rx {
                        mem_manager.push(item);
                        this_add_mem_len += test_page_block_len;
                        pb.set_position(this_add_mem_len as u64);
                    }
                }
                last_mem_len = mem_manager.mem_len();
            } else if total_radio_len < used_len {
                //移除
                let remove_len = usize::try_from(used_len - total_radio_len).unwrap_or(0);
                let remove_count = remove_len / test_page_block_len;
                //移除
                if mem_manager.list_len() > remove_count {
                    mem_manager.remove_count(remove_count);
                } else if last_mem_len > 0 {
                    warn!("系统内存超出设置比例({ratio})，已释放所有测试内存");
                    mem_manager.clear();
                    last_mem_len = 0;
                }
            }
        }
        test_pb.set_length(mem_manager.mem_len() as u64);
        let list_len = mem_manager.list_len();
        if let Some((pos, hash, ptr)) = mem_manager.get() {
            if pos + 1 == list_len {
                test_loop_count += 1;
            }
            page_block.set_ptr(ptr);
            let addr = page_block.get_addr();
            let addr_end = addr + test_page_block_len;
            test_pb.set_position((pos * test_page_block_len) as u64);
            test_pb.set_message(format!(
                "测试循环次数：{test_loop_count}，虚拟地址：0x{addr:x} - 0x{addr_end:x} ({test_page_block_len} bytes)",
            ));
            hasher.reset();
            hasher.update(page_block.get());
            if *hash != hasher.finalize() {
                error!(
                    "测试失败：虚拟内存地址：0x{addr:x} - 0x{addr_end:x} ({test_page_block_len} bytes)"
                );
            }
        }
        loop_count = loop_count.wrapping_add(1);
    }
}
