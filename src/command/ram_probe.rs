//! 内存测试器（`rp`）：按目标比例分配页对齐内存块，并持续重算块内容哈希以检测内存错误。
//! 增长是一次会话：常驻线程池持续分配，期间每 256 MiB 复查系统内存，保留量取
//! max(512 MiB, 总内存 1%)，因此其他程序同时增长时不会把机器推到 OOM。
//! 块索引由分区式固定容量存储维护：分区按需分配、清空即释放，表头/分区头存三份并多数表决。
//! 每个块（`PageBlock`）独享自己的分配，所有权经生产者通道转移，`Drop` 时释放。分配描述符（地址/大小/对齐）
//! 存三份，单份损坏由多数票恢复；无法恢复时拒绝访问并宁可泄漏也不按错误布局释放。
//! 每个块把哈希存成两份副本加一份取反副本，单份元数据损坏可被纠正；校验失败时能区分哈希元数据内存与数据块内存。
//!
//! RAM probe (`rp`): allocates page-aligned blocks up to a target ratio and keeps
//! re-hashing each block to detect memory corruption. Growth runs as one session over a
//! persistent worker pool that re-checks system memory every 256 MiB; the reserve is
//! max(512 MiB, 1% of total), so other processes growing at the same time cannot push the
//! machine into OOM. Each block owns its allocation (`PageBlock`); ownership moves through
//! the producer channel and `Drop` frees it. Blocks are indexed by a partitioned fixed-capacity
//! store: partitions are allocated on demand and freed when empty, and the table/partition
//! headers have three copies recovered by majority. Allocation descriptors (address/size/
//! alignment) are stored three times; one corrupted copy is outvoted, and an undecidable
//! descriptor is never dereferenced and is leaked rather than freed with a bad layout. Every
//! block stores two hash copies plus a byte-wise complement, so one corrupted copy is corrected
//! and a mismatch is attributed to the hash metadata or to the data block.

use crate::command::{PACK_PROGRESS_STYLE_TEMPLATE, create_pb};
use blake3::{Hash, Hasher};
use clap::Args;
use indicatif::{MultiProgress, ProgressBar};
use rand::Rng;
use rand::rngs::SmallRng;
use std::alloc::{Layout, alloc, dealloc};
//AI=== 失败块只报告一次 / a failed block is reported once
use std::cell::Cell;
//AI_END===
use std::error::Error;
use std::num::NonZero;
//AI=== 数据块自持内存需要 NonNull / owning blocks need NonNull
use std::ptr::NonNull;
//AI_END===
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
//AI=== 交接队列有界，限制分配超前量 / bounded hand-off queue caps the producer lead
use std::sync::mpsc::{Sender, SyncSender};
//AI_END===
use std::sync::{Arc, mpsc};
use std::thread;
use std::thread::JoinHandle;
//AI=== 分配失败后的重试冷却 / retry cooldown after an allocation failure
use std::time::{Duration, Instant};
//AI_END===
use sysinfo::System;
use tracing::{error, warn};
use water_ball_tool_lib::tools::MemTool;

//AI=== 分区式自纠块存储 / partitioned self-correcting block store
mod store;
use store::{BlockStore, StoreError, majority};
//AI_END===

const BLOCK_LEN: usize = 64 * 1024;

//AI=== 增长策略：保留 max(512 MiB, 总量 1%)，目标 = min(total*ratio, total-reserve)
// Growth policy: reserve max(512 MiB, 1% of total); target = min(total*ratio, total-reserve).
/// 分配期间检查内存的间隔：64 KiB × 4096 = 256 MiB。
///
/// Memory check interval during allocation: 64 KiB × 4096 = 256 MiB.
const CHECK_BLOCKS: usize = 4096;
/// 生产者最多领先消费者 256 块（16 MiB），限制触顶瞬间的过冲。
///
/// Producers lead the consumer by at most 256 blocks (16 MiB), bounding the overshoot at the limit.
const QUEUE_CAP: usize = 256;
/// 最小保留内存 / minimum reserved memory.
const MIN_RESERVE: u64 = 512 * 1024 * 1024;

fn reserve_bytes(total: u64) -> u64 {
    (total / 100).max(MIN_RESERVE)
}

fn target_bytes(total: u64, ratio: f64) -> u64 {
    let wanted = (total as f64 * ratio) as u64;
    wanted.min(total.saturating_sub(reserve_bytes(total)))
}

fn add_threshold_bytes(total: u64, ratio: f64) -> u64 {
    let lower = (ratio - 0.01).max(ratio * 0.5);
    (total as f64 * lower) as u64
}

/// 本次增长会话的块数；`0` 表示不增长。
///
/// Blocks for the next growth session; `0` means no growth.
fn grow_block_count(total: u64, ratio: f64, used: u64, available: u64) -> usize {
    if used >= add_threshold_bytes(total, ratio) {
        return 0;
    }
    let budget = available.saturating_sub(reserve_bytes(total));
    let bytes = target_bytes(total, ratio).saturating_sub(used).min(budget);
    usize::try_from(bytes / BLOCK_LEN as u64).unwrap_or(usize::MAX)
}
//AI_END===

//AI=== 数据块改为自持内存的所有者：构造即分配、Drop 即释放，裸指针不出现在函数签名里
// Page blocks become owners: allocate on construction, free on Drop, no raw pointers in signatures.
/// 页对齐内存块（唯一所有者）：对外只暴露切片与地址，生命周期由 `&self` / `&mut self` 约束。
///
/// Page-aligned memory block (sole owner): only slices and the address are exposed;
/// access is tied to `&self` / `&mut self`.
//AI=== 分配描述符三副本：地址/大小/对齐被单份损坏时由多数票恢复，损坏到无法表决则不访问、不释放
// Three descriptor copies: a single corrupted copy is outvoted; an undecidable one is never dereferenced or freed.
/// 分配描述符 / allocation descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Descriptor {
    addr: usize,
    size: usize,
    align: usize,
}

struct PageBlock {
    /// 三份冗余描述符：地址、大小、对齐。
    ///
    /// Three redundant descriptors: address, size and alignment.
    copies: [Descriptor; 3],
}

impl PageBlock {
    /// 分配 `size` 字节、按 `align` 对齐的内存块；分配失败返回 `None`。
    ///
    /// Allocate `size` bytes aligned to `align`; `None` when allocation fails.
    fn alloc(size: usize, align: usize) -> Option<Self> {
        let layout = Layout::from_size_align(size, align).ok()?;
        // 零尺寸分配是未定义行为，直接拒绝 / a zero-sized allocation is UB, reject it
        if layout.size() == 0 {
            return None;
        }
        // # SAFETY: layout 由 `Layout::from_size_align` 校验且尺寸非零
        let ptr = NonNull::new(unsafe { alloc(layout) })?;
        let descriptor = Descriptor {
            addr: ptr.as_ptr() as usize,
            size: layout.size(),
            align: layout.align(),
        };
        Some(Self {
            copies: [descriptor; 3],
        })
    }

    /// 未使用槽位的哨兵：三份描述符互不相同，任何访问与释放都会被拒绝。
    ///
    /// Sentinel for an unused slot: the three copies disagree, so access and drop are refused.
    fn poison() -> Self {
        Self {
            copies: [
                Descriptor {
                    addr: 0,
                    size: 0,
                    align: 0,
                },
                Descriptor {
                    addr: 1,
                    size: 0,
                    align: 0,
                },
                Descriptor {
                    addr: 2,
                    size: 0,
                    align: 0,
                },
            ],
        }
    }

    /// 多数表决恢复描述符；任一字段的单份损坏都会被纠正。
    ///
    /// Recover the descriptor by majority vote; a single corrupted copy is outvoted.
    fn verified(&self) -> Option<Descriptor> {
        let [a, b, c] = self.copies;
        if a == b || a == c {
            Some(a)
        } else if b == c {
            Some(b)
        } else {
            None
        }
    }

    fn as_mut_slice(&mut self) -> Option<&mut [u8]> {
        let descriptor = self.verified()?;
        // # SAFETY: 同上；&mut self 保证独占访问。
        Some(unsafe { std::slice::from_raw_parts_mut(descriptor.addr as *mut u8, descriptor.size) })
    }

    /// 校验描述符后同时返回切片与起始地址。
    ///
    /// Verify the descriptor once and return the slice together with the start address.
    fn checked_slice(&self) -> Option<(&[u8], usize)> {
        let descriptor = self.verified()?;
        // # SAFETY: 描述符由三份副本多数票恢复；单份损坏已纠正，地址/大小与分配时一致。
        // 本工具只承诺单故障容错；两份同时损坏属于不可纠错场景。
        let data =
            unsafe { std::slice::from_raw_parts(descriptor.addr as *const u8, descriptor.size) };
        Some((data, descriptor.addr))
    }
}

impl Drop for PageBlock {
    fn drop(&mut self) {
        let Some(descriptor) = self.verified() else {
            // 描述符损坏到无法表决：泄漏优于按错误布局释放 / leak rather than free with a bad layout.
            return;
        };
        let Ok(layout) = Layout::from_size_align(descriptor.size, descriptor.align) else {
            return;
        };
        // # SAFETY: 地址/布局由分配时记录并由多数票恢复。
        unsafe { dealloc(descriptor.addr as *mut u8, layout) };
    }
}

//AI=== 自校验哈希戳：哈希存三份（两份副本 + 一份取反），单份元数据损坏可纠正
// Self-checking hash stamp: hash, mirror and complement; one corrupted copy is corrected.
/// 哈希的三份冗余：`hash`、`mirror` 与逐字节取反的 `inverted`。
///
/// Three redundant copies of the hash: `hash`, `mirror`, and the byte-wise complement
/// `inverted`. A single corrupted copy is outvoted by the other two.
#[derive(Clone, Copy)]
struct HashStamp {
    hash: Hash,
    mirror: Hash,
    inverted: Hash,
}

/// 哈希戳的恢复结果 / recovery result of a hash stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StampState {
    /// 三份一致 / all copies agree.
    Intact(Hash),
    /// 单份损坏，已由另外两份恢复 / one copy was corrupted and recovered from the other two.
    Corrected(Hash),
    /// 多份不一致，无法恢复 / copies disagree, no recovery.
    Unrecoverable,
}

fn complement(hash: Hash) -> Hash {
    let mut bytes = *hash.as_bytes();
    for byte in &mut bytes {
        *byte = !*byte;
    }
    Hash::from_bytes(bytes)
}

impl HashStamp {
    fn new(hash: Hash) -> Self {
        Self {
            hash,
            mirror: hash,
            inverted: complement(hash),
        }
    }

    /// 未使用槽位的哨兵：三份互不一致，无法恢复。
    ///
    /// Sentinel for an unused slot: all three copies disagree and cannot be recovered.
    fn poison() -> Self {
        Self {
            hash: Hash::from_bytes([0; 32]),
            mirror: Hash::from_bytes([1; 32]),
            inverted: Hash::from_bytes([2; 32]),
        }
    }

    /// 恢复哈希：两份独立副本一致时取该值，否则由取反副本裁定单份损坏。
    ///
    /// Recover the hash: matching independent copies win; otherwise the complement decides
    /// which single copy is corrupted.
    fn recover(&self) -> StampState {
        if self.hash == self.mirror {
            return if self.inverted == complement(self.hash) {
                StampState::Intact(self.hash)
            } else {
                StampState::Corrected(self.hash)
            };
        }
        if self.inverted == complement(self.hash) {
            StampState::Corrected(self.hash)
        } else if self.inverted == complement(self.mirror) {
            StampState::Corrected(self.mirror)
        } else {
            StampState::Unrecoverable
        }
    }
}

/// 数据校验失败的归因 / attribution of a data mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// 重读与期望哈希一致：第一次读取瞬时出错 / the re-read matches: transient read error.
    TransientData,
    /// 重读仍不匹配：数据块内容损坏 / the re-read still mismatches: stable corruption.
    Data,
}

fn classify(reread_matches: bool) -> Verdict {
    if reread_matches {
        Verdict::TransientData
    } else {
        Verdict::Data
    }
}
//AI_END===

/// 已填充并算好哈希的数据块。
///
/// A filled data block together with its self-checking hash stamp.
struct Block {
    mem: PageBlock,
    //AI=== 元数据自校验 / self-checking metadata
    stamp: HashStamp,
    //AI_END===
    //AI=== 数据/元数据错误各自只报告一次；用 u8 避免非法 bool 位模式
    // Data and metadata errors are each reported once; u8 avoids invalid bool bit patterns.
    failed: Cell<u8>,
    meta_reported: Cell<u8>,
    //AI_END===
}

impl Block {
    /// 未使用槽位的哨兵块 / sentinel block for an unused slot.
    fn poison() -> Self {
        Self {
            mem: PageBlock::poison(),
            stamp: HashStamp::poison(),
            failed: Cell::new(0),
            meta_reported: Cell::new(0),
        }
    }
}
//AI_END===

//AI=== 分区式自纠块存储替换 Vec；游标也三副本表决 / partitioned self-correcting store; TMR cursor
struct MemManager {
    store: BlockStore,
    pos: [usize; 3],
}

impl MemManager {
    fn new(max_blocks: usize) -> Result<Self, StoreError> {
        Ok(Self {
            store: BlockStore::new(max_blocks, store::PARTITION_BLOCKS)?,
            pos: [0; 3],
        })
    }

    #[cfg(test)]
    fn with_partition_blocks(
        max_blocks: usize,
        partition_blocks: usize,
    ) -> Result<Self, StoreError> {
        Ok(Self {
            store: BlockStore::new(max_blocks, partition_blocks)?,
            pos: [0; 3],
        })
    }

    fn list_len(&self) -> usize {
        self.store.len()
    }

    fn mem_len(&self) -> usize {
        self.store.len() * BLOCK_LEN
    }

    fn is_broken(&self) -> bool {
        self.store.is_broken()
    }

    fn push(&mut self, block: Block) -> Result<(), StoreError> {
        self.store.push(block)
    }

    fn remove_count(&mut self, count: usize) {
        self.store.remove_count(count);
    }

    fn clear(&mut self) {
        self.store.remove_count(usize::MAX);
    }

    /// 轮询取出下一块；游标损坏时归零，越界时钳制。
    ///
    /// Round-robin access; a corrupted cursor resets to 0 and is clamped to the length.
    fn get(&mut self) -> Option<(usize, &Block)> {
        let len = self.store.len();
        if len == 0 {
            return None;
        }
        let mut pos = majority(&self.pos).unwrap_or(0);
        if pos >= len {
            pos = 0;
        }
        let next = if pos + 1 == len { 0 } else { pos + 1 };
        self.pos = [next; 3];
        Some((pos, self.store.get(pos)?))
    }
}
//AI_END===
fn this_create_user_pb(mp: &MultiProgress, prefix: &'static str) -> ProgressBar {
    let pb = create_pb(mp);
    pb.set_style(crate::command::progress_style(
        "{prefix:<25.bold.green} [{wide_bar:.cyan/blue}] \
{percent_precise:>7.magenta}% {bytes:>11} / {total_bytes:>11} \n{msg:.yellow}",
    ));
    pb.set_prefix(prefix);
    pb
}

//AI=== 三块占用进度条共用一次刷新：读内存与更新进度条绑定
// The three usage bars share one refresh: reading memory also updates the bars.
struct UsageBars {
    system: ProgressBar,
    ours_of_system: ProgressBar,
    ours_of_used: ProgressBar,
}

impl UsageBars {
    fn new(mp: &MultiProgress) -> Self {
        Self {
            system: this_create_user_pb(mp, "系统所有程序利用率"),
            ours_of_system: this_create_user_pb(mp, "测试内存占系统利用率"),
            ours_of_used: this_create_user_pb(mp, "测试内存占所有程序利用率"),
        }
    }

    /// 刷新系统内存并同步三块占用进度条，返回 `(已用, 可用)`。
    ///
    /// Refresh system memory and sync the three bars; returns `(used, available)`.
    fn refresh(&self, system_info: &mut System, ours: u64) -> (u64, u64) {
        system_info.refresh_memory();
        let used = system_info.used_memory();
        let available = system_info.available_memory();
        self.system.set_position(used);
        self.ours_of_system.set_position(ours);
        self.ours_of_used.set_length(used);
        self.ours_of_used.set_position(ours);
        (used, available)
    }
}
//AI_END===

//AI=== 单次增长会话：常驻线程池只接收一次 burst 信号，一个进度条，期间定期复查内存
// One growth session over the persistent worker pool: a single bar, re-checking memory periodically.
struct Growth<'a> {
    workers: &'a [(Sender<SyncSender<Block>>, JoinHandle<()>)],
    add_count: &'a AtomicUsize,
    this_add_count: &'a AtomicUsize,
    grow_stop: &'a AtomicBool,
    manager: &'a mut MemManager,
    system_info: &'a mut System,
    bars: &'a UsageBars,
    mp: &'a MultiProgress,
    total: u64,
    ratio: f64,
}

impl Growth<'_> {
    /// 执行一次增长会话，返回 `(实际收到块数, 是否因触及上限而停止, 存储错误)`。
    ///
    /// Run one session; returns `(received blocks, stopped at a limit, store error)`.
    fn run(&mut self, planned: usize) -> (usize, bool, Option<StoreError>) {
        self.add_count.store(planned, Ordering::Release);
        self.this_add_count.store(0, Ordering::Release);
        self.grow_stop.store(false, Ordering::Release);

        let reserve = reserve_bytes(self.total);
        let target = target_bytes(self.total, self.ratio);
        let pb = create_pb(self.mp);
        pb.set_style(crate::command::progress_style(PACK_PROGRESS_STYLE_TEMPLATE));
        pb.set_prefix("正在增加测试内存");
        pb.set_length((planned.saturating_mul(BLOCK_LEN)) as u64);
        pb.set_message(format!("目标 {target}，保留 {reserve}"));

        let (tx, rx) = mpsc::sync_channel(QUEUE_CAP);
        for (worker, _) in self.workers {
            if worker.send(tx.clone()).is_err() {
                unreachable!("增长线程已退出 / growth worker exited");
            }
        }
        drop(tx);

        let mut received = 0usize;
        let mut stopped_by_limit = false;
        let mut store_error = None;
        for block in rx {
            match self.manager.push(block) {
                Ok(()) => {}
                Err(StoreError::Full) => {
                    self.grow_stop.store(true, Ordering::Release);
                    stopped_by_limit = true;
                    break;
                }
                Err(error) => {
                    self.grow_stop.store(true, Ordering::Release);
                    store_error = Some(error);
                    break;
                }
            }
            received += 1;
            pb.set_position((received.saturating_mul(BLOCK_LEN)) as u64);
            if received.is_multiple_of(CHECK_BLOCKS) {
                let (used, available) = self
                    .bars
                    .refresh(self.system_info, self.manager.mem_len() as u64);
                pb.set_message(format!(
                    "系统可用 {available}，保留 {reserve}，已用 {used} / 目标 {target}"
                ));
                if used >= target || available <= reserve {
                    self.grow_stop.store(true, Ordering::Release);
                    stopped_by_limit = true;
                }
            }
            // 计划已满：给被损坏的原子计数器兜底 / the plan is complete: guard against corrupt counters
            if received >= planned {
                self.grow_stop.store(true, Ordering::Release);
                stopped_by_limit = true;
                break;
            }
        }
        pb.finish_and_clear();
        (received, stopped_by_limit, store_error)
    }
}
//AI_END===

//AI=== 删除返回裸指针的分配函数：分配与释放都收敛到 PageBlock
// Removed the raw-pointer allocator: allocation and deallocation now live in PageBlock.
//AI_END===

#[derive(Debug, Args)]
pub struct RamProbeArgs {
    /// 更新大小循环周期次数
    ///
    /// Verification loops between memory checks; must be at least 1.
    #[arg(default_value_t = 100)]
    reset_count: usize,
    /// 目标占用百分比
    ///
    /// Target share of total memory, in `(0, 1]`.
    #[arg(default_value_t = 0.80)]
    ratio: f64,
}

pub(super) fn args(args: RamProbeArgs) -> Result<(), Box<dyn Error>> {
    //AI=== 运行期输入校验：0 会触发除零，非法比例会空转或过量分配
    // Runtime input validation: zero panics on `is_multiple_of`; bad ratios idle or over-allocate.
    if args.reset_count == 0 {
        return Err("reset_count 必须大于 0".into());
    }
    if !args.ratio.is_finite() || args.ratio <= 0.0 || args.ratio > 1.0 {
        return Err("ratio 必须在 (0, 1] 区间内".into());
    }
    //AI_END===
    let Some(page_size) = MemTool::page_size() else {
        return Err("无法获取系统内存页大小".into());
    };
    if !BLOCK_LEN.is_multiple_of(page_size) {
        return Err("测试块大小未对齐".into());
    }
    // 初始化日志和进度条
    let mp = crate::multi_progress_init("rem_probe");

    //AI=== 常驻分配线程池：启动时创建一次，所有增长会话复用 / workers are spawned once and reused by every session
    let grow_stop = Arc::new(AtomicBool::new(false));
    let add_thread_count = thread::available_parallelism().map_or(8, NonZero::get);
    let add_count = Arc::new(AtomicUsize::new(0));
    let this_add_count = Arc::new(AtomicUsize::new(0));
    let add_thread_list = {
        let mut list = Vec::with_capacity(add_thread_count);
        for i in 0..add_thread_count {
            let (tx, rx) = mpsc::channel::<SyncSender<Block>>();
            let this_add_count = this_add_count.clone();
            let add_count = add_count.clone();
            let grow_stop = grow_stop.clone();
            let t = thread::Builder::new()
                .name(format!("add-{i}"))
                .spawn(move || {
                    let mut hasher = Hasher::new();
                    let mut ran = rand::make_rng::<SmallRng>();
                    for tx in rx {
                        while this_add_count.load(Ordering::Acquire)
                            < add_count.load(Ordering::Acquire)
                            && !grow_stop.load(Ordering::Acquire)
                        {
                            hasher.reset();
                            let Some(mut block) = PageBlock::alloc(BLOCK_LEN, page_size) else {
                                // 分配失败即结束本会话，由主循环冷却后重试
                                // Allocation failure ends this session; the controller cools down and retries.
                                grow_stop.store(true, Ordering::Release);
                                break;
                            };
                            // 刚分配的描述符三份一致；这里失败说明内存在实时损坏
                            // A fresh descriptor is consistent; failure here means live corruption.
                            let Some(data) = block.as_mut_slice() else {
                                grow_stop.store(true, Ordering::Release);
                                break;
                            };
                            ran.fill_bytes(data);
                            hasher.update(data);
                            let hash = hasher.finalize();
                            if tx
                                .send(Block {
                                    mem: block,
                                    stamp: HashStamp::new(hash),
                                    failed: Cell::new(0),
                                    meta_reported: Cell::new(0),
                                })
                                .is_err()
                            {
                                break;
                            }
                            this_add_count.fetch_add(1, Ordering::Release);
                        }
                    }
                })
                .map_err(|e| format!("创建 add-{i} 线程失败: {e}"))?;
            list.push((tx, t));
        }
        list
    };

    let test_loop_thread = thread::Builder::new()
        .name("test_loop_thread".into())
        .spawn(move || {
            test_loop(
                args,
                &mp,
                add_count,
                grow_stop,
                this_add_count,
                add_thread_list,
            )
        })
        .map_err(|e| format!("创建 test loop 线程失败: {e}"))?;
    //AI_END===
    let result = test_loop_thread
        .join()
        .map_err(|e| format!("fail to join test loop thread, err: {e:?}"))?;
    result?;
    Ok(())
}

fn test_loop(
    args: RamProbeArgs,
    mp: &MultiProgress,
    add_count: Arc<AtomicUsize>,
    grow_stop: Arc<AtomicBool>,
    this_add_count: Arc<AtomicUsize>,
    //AI=== 通道传递块的所有权而不是地址 / the channel carries block ownership, not an address
    add_thread_list: Vec<(Sender<SyncSender<Block>>, JoinHandle<()>)>,
    //AI_END===
) -> Result<(), String> {
    let mut system_info = System::new();
    let bars = UsageBars::new(mp);
    //AI=== 启动即取总内存与第一份占用快照 / read total memory and the first usage snapshot at startup
    bars.refresh(&mut system_info, 0);
    let total_len = system_info.total_memory();
    bars.system.set_length(total_len);
    bars.ours_of_system.set_length(total_len);
    //AI_END===
    let test_pb = create_pb(mp);
    let reset_count = args.reset_count;
    let ratio = args.ratio;
    //AI=== 先算目标块数，再创建分区式块存储 / compute the target blocks before creating the store
    let target = target_bytes(total_len, ratio);
    let reserve = reserve_bytes(total_len);
    let max_blocks = usize::try_from(target / BLOCK_LEN as u64).unwrap_or(usize::MAX);
    let mut mem_manager = match MemManager::new(max_blocks) {
        Ok(manager) => manager,
        Err(e) => {
            return Err(format!(
                "创建块存储失败 / failed to create block store: {e:?}"
            ));
        }
    };
    //AI_END===
    let mut hasher = Hasher::new();
    let mut loop_count = 0usize;
    let mut test_loop_count = 0u64;
    //AI=== 分配失败后的重试冷却 / retry cooldown after a failed growth session
    let mut retry_after: Option<Instant> = None;
    //AI_END===
    if target == 0 {
        warn!(
            "目标占用量为 0（保留 {reserve} 过大），不会分配内存 / target is zero, no memory will be allocated"
        );
    }
    test_pb.set_style(crate::command::progress_style(PACK_PROGRESS_STYLE_TEMPLATE));
    test_pb.set_prefix("测试进度");
    let mut used = 0u64;
    let mut available = 0u64;
    loop {
        // AI=== 任一自管元数据无法表决就停止，不再继续读 / stop as soon as owned metadata cannot be recovered
        if mem_manager.is_broken() {
            return Err("块存储元数据损坏，停止测试 / block store metadata corrupted".into());
        }
        // AI_END===
        let refresh = loop_count.is_multiple_of(10);
        if refresh {
            (used, available) = bars.refresh(&mut system_info, mem_manager.mem_len() as u64);
        }
        if loop_count.is_multiple_of(reset_count) {
            if !refresh {
                (used, available) = bars.refresh(&mut system_info, mem_manager.mem_len() as u64);
            }
            //AI=== 增长：一次会话分配完或触及上限；收缩：按超出目标的量截断
            // Growth: one session runs to completion or the limit; shrink: truncate the overage.
            if used < add_threshold_bytes(total_len, ratio) {
                let retry_ready = retry_after.is_none_or(|at| at <= Instant::now());
                if retry_ready {
                    let planned = grow_block_count(total_len, ratio, used, available);
                    if planned > 0 {
                        let (received, stopped_by_limit, store_error) = Growth {
                            workers: &add_thread_list,
                            add_count: &add_count,
                            this_add_count: &this_add_count,
                            grow_stop: &grow_stop,
                            manager: &mut mem_manager,
                            system_info: &mut system_info,
                            bars: &bars,
                            mp,
                            total: total_len,
                            ratio,
                        }
                        .run(planned);
                        match store_error {
                            Some(StoreError::Corrupt) => {
                                return Err(
                                    "块存储元数据损坏，停止测试 / block store metadata corrupted"
                                        .into(),
                                );
                            }
                            Some(StoreError::Alloc) => {
                                retry_after = Some(Instant::now() + Duration::from_secs(2));
                                warn!(
                                    "块存储分配失败，暂停增长 / block store allocation failed, pausing growth"
                                );
                            }
                            Some(StoreError::Full) | None => {
                                if received < planned && !stopped_by_limit {
                                    retry_after = Some(Instant::now() + Duration::from_secs(2));
                                    warn!(
                                        "页块分配失败，暂停增长 / block allocation failed, pausing growth"
                                    );
                                }
                            }
                        }
                        _ = bars.refresh(&mut system_info, mem_manager.mem_len() as u64);
                    }
                }
            } else if used > target {
                //移除
                let remove_count =
                    usize::try_from((used - target) / BLOCK_LEN as u64).unwrap_or(usize::MAX);
                if mem_manager.list_len() > remove_count {
                    mem_manager.remove_count(remove_count);
                } else if mem_manager.list_len() > 0 {
                    warn!("系统内存超出设置比例({ratio})，已释放所有测试内存");
                    mem_manager.clear();
                }
                //释放后立即回写占用进度条 / update the usage bars right after releasing
                _ = bars.refresh(&mut system_info, mem_manager.mem_len() as u64);
            }
            //AI_END===
        }
        test_pb.set_length(mem_manager.mem_len() as u64);
        let list_len = mem_manager.list_len();
        //AI=== 直接读取管理器借出的块，切片生命周期与块所有权绑定 / read the borrowed block; slice lifetime follows ownership
        if let Some((pos, block)) = mem_manager.get() {
            //AI=== 先校验分配描述符；描述符损坏时不访问内存 / verify the descriptor before any access
            if let Some((data, addr)) = block.mem.checked_slice() {
                if pos + 1 == list_len {
                    test_loop_count = test_loop_count.wrapping_add(1);
                }
                let addr_end = addr + BLOCK_LEN;
                test_pb.set_position((pos * BLOCK_LEN) as u64);
                test_pb.set_message(format!(
                    "测试循环次数：{test_loop_count}，虚拟地址：0x{addr:x} - 0x{addr_end:x} ({BLOCK_LEN} bytes)",
                ));
                hasher.reset();
                hasher.update(data);
                //AI=== 哈希戳可自纠一份副本；数据错误区分瞬时与稳定，每块各报一次
                // The stamp corrects one corrupted copy; data errors distinguish transient and stable reads.
                let actual = hasher.finalize();
                let stamp_addr = std::ptr::from_ref(&block.stamp) as usize;
                let expected = match block.stamp.recover() {
                    StampState::Intact(hash) => Some(hash),
                    StampState::Corrected(hash) => {
                        if block.meta_reported.replace(1) == 0 {
                            error!(
                                "测试失败：哈希元数据单份副本损坏，已纠正 / hash metadata copy corrupted, corrected，哈希地址 0x{stamp_addr:x}，数据块 0x{addr:x} - 0x{addr_end:x}，位置 {pos}"
                            );
                        }
                        Some(hash)
                    }
                    StampState::Unrecoverable => {
                        if block.meta_reported.replace(1) == 0 {
                            error!(
                                "测试失败：哈希元数据损坏且无法纠正 / hash metadata corrupted and unrecoverable，哈希地址 0x{stamp_addr:x}，数据块 0x{addr:x} - 0x{addr_end:x}，位置 {pos}"
                            );
                        }
                        None
                    }
                };
                if let Some(expected) = expected
                    && actual != expected
                    && block.failed.replace(1) == 0
                {
                    hasher.reset();
                    hasher.update(data);
                    let reread = hasher.finalize();
                    match classify(reread == expected) {
                        Verdict::TransientData => error!(
                            "测试失败：数据块瞬时读取错误 / transient data read error，数据块 0x{addr:x} - 0x{addr_end:x}，位置 {pos}"
                        ),
                        Verdict::Data => error!(
                            "测试失败：数据块内容损坏 / data block corrupted，数据块 0x{addr:x} - 0x{addr_end:x}，位置 {pos}"
                        ),
                    }
                }
                //AI_END===
            } else if block.meta_reported.replace(1) == 0 {
                error!(
                    "测试失败：数据块分配描述符损坏，已跳过访问 / block descriptor corrupted, access skipped，位置 {pos}"
                );
            }
            //AI_END===
        }
        //AI_END===
        loop_count = loop_count.wrapping_add(1);
    }
}

//AI=== 内存管理单元测试：轮询顺序、裁剪后的游标、页对齐
// Unit tests for the memory manager: round-robin order, cursor after truncation, page alignment.
#[cfg(test)]
mod test {
    use super::*;

    fn test_block(marker: u8) -> Block {
        let Some(mut mem) = PageBlock::alloc(BLOCK_LEN, 4096) else {
            panic!("测试块分配失败 / test block allocation failed");
        };
        let Some(data) = mem.as_mut_slice() else {
            panic!("测试块描述符损坏 / test block descriptor corrupted");
        };
        data.fill(marker);
        Block {
            mem,
            stamp: HashStamp::new(Hasher::new().finalize()),
            failed: Cell::new(0),
            meta_reported: Cell::new(0),
        }
    }

    fn block_data(block: &Block) -> &[u8] {
        let Some((data, _)) = block.mem.checked_slice() else {
            panic!("测试块描述符损坏 / test block descriptor corrupted");
        };
        data
    }

    fn manager(max_blocks: usize) -> MemManager {
        let Ok(manager) = MemManager::with_partition_blocks(max_blocks, 2) else {
            panic!("创建测试管理器失败 / failed to create test manager");
        };
        manager
    }

    #[test]
    fn mem_manager_round_robin() {
        let mut manager = manager(8);
        for i in 0..3 {
            assert!(manager.push(test_block(i)).is_ok());
        }
        let mut order = Vec::new();
        for _ in 0..4 {
            let Some((pos, block)) = manager.get() else {
                panic!("管理器为空 / manager returned no block");
            };
            assert_eq!(block_data(block)[0], pos as u8);
            order.push(pos);
        }
        assert_eq!(order, vec![0, 1, 2, 0]);
    }

    #[test]
    fn mem_manager_truncate_clamps_pos() {
        let mut manager = manager(8);
        for i in 0..3 {
            assert!(manager.push(test_block(i)).is_ok());
        }
        for _ in 0..2 {
            assert!(manager.get().is_some());
        }
        manager.remove_count(2);
        assert_eq!(manager.list_len(), 1);
        let Some((pos, block)) = manager.get() else {
            panic!("管理器为空 / manager returned no block");
        };
        assert_eq!(pos, 0);
        assert_eq!(block_data(block)[0], 0);
    }

    #[test]
    fn mem_manager_remove_more_than_len_clears() {
        let mut manager = manager(8);
        assert!(manager.push(test_block(0)).is_ok());
        assert!(manager.push(test_block(1)).is_ok());
        manager.remove_count(10);
        assert_eq!(manager.list_len(), 0);
        assert!(manager.get().is_none());
    }

    #[test]
    fn page_block_is_page_aligned() {
        let page_size = MemTool::page_size().unwrap_or(4096);
        let Some(mem) = PageBlock::alloc(BLOCK_LEN, page_size) else {
            panic!("测试块分配失败 / test block allocation failed");
        };
        let Some((data, addr)) = mem.checked_slice() else {
            panic!("测试块描述符损坏 / test block descriptor corrupted");
        };
        assert_eq!(addr % page_size, 0);
        assert_eq!(data.len(), BLOCK_LEN);
        assert_eq!(data[0], 0);
    }

    //AI=== 描述符三副本测试 / descriptor triple-copy tests
    #[test]
    fn page_block_corrects_one_corrupted_descriptor_copy() {
        let Some(mut mem) = PageBlock::alloc(BLOCK_LEN, 4096) else {
            panic!("测试块分配失败 / test block allocation failed");
        };
        let Some(original) = mem.verified() else {
            panic!("测试块描述符损坏 / test block descriptor corrupted");
        };
        let mut copies = mem.copies;
        copies[0].addr ^= 0x1000;
        mem.copies = copies;
        assert_eq!(mem.verified(), Some(original));
        assert!(mem.checked_slice().is_some());
    }

    #[test]
    fn page_block_skips_access_when_descriptors_disagree() {
        let Some(mut mem) = PageBlock::alloc(BLOCK_LEN, 4096) else {
            panic!("测试块分配失败 / test block allocation failed");
        };
        let Some(original) = mem.verified() else {
            panic!("测试块描述符损坏 / test block descriptor corrupted");
        };
        let mut copies = mem.copies;
        copies[0].addr ^= 0x1000;
        copies[1].size ^= 0x1;
        mem.copies = copies;
        assert!(mem.verified().is_none());
        assert!(mem.checked_slice().is_none());
        // 无法表决时 Drop 选择泄漏；测试里先恢复描述符以免遗留内存。
        // Drop leaks when undecidable; restore the descriptor here so the test frees its block.
        mem.copies = [original; 3];
    }
    //AI_END===

    //AI=== 增长策略测试：保留量、目标上限、可用预算与滞回
    // Growth policy tests: reserve floor, target cap, available budget and hysteresis.
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;

    #[test]
    fn reserve_floor_is_512_mib() {
        assert_eq!(reserve_bytes(4 * GIB), 512 * MIB);
    }

    #[test]
    fn reserve_is_one_percent_on_large_machines() {
        assert_eq!(reserve_bytes(128 * GIB), 128 * GIB / 100);
    }

    #[test]
    fn target_is_capped_by_reserve() {
        assert_eq!(target_bytes(4 * GIB, 0.5), 2 * GIB);
        assert_eq!(target_bytes(4 * GIB, 1.0), 4 * GIB - 512 * MIB);
    }

    #[test]
    fn growth_is_limited_by_available_budget() {
        // 4 GiB 总量、1 GiB 可用、保留 512 MiB → 最多再分配 512 MiB。
        // 4 GiB total, 1 GiB available, 512 MiB reserve -> at most 512 MiB of new blocks.
        assert_eq!(
            grow_block_count(4 * GIB, 0.8, 0, GIB),
            (512 * MIB / BLOCK_LEN as u64) as usize
        );
        assert_eq!(grow_block_count(4 * GIB, 0.8, 0, 512 * MIB), 0);
    }

    #[test]
    fn growth_stops_at_add_threshold() {
        let used = add_threshold_bytes(4 * GIB, 0.8);
        assert_eq!(grow_block_count(4 * GIB, 0.8, used, 4 * GIB), 0);
    }

    #[test]
    fn tiny_ratio_keeps_a_positive_hysteresis_band() {
        assert!(add_threshold_bytes(4 * GIB, 0.005) > 0);
    }
    //AI_END===

    //AI=== 哈希戳与归因测试 / hash stamp and attribution tests
    #[test]
    fn hash_stamp_is_intact_after_construction() {
        let hash = Hasher::new().finalize();
        assert_eq!(HashStamp::new(hash).recover(), StampState::Intact(hash));
    }

    #[test]
    fn hash_stamp_corrects_one_corrupted_copy() {
        let hash = Hasher::new().finalize();
        let stamp = HashStamp::new(hash);

        let mut hash_bytes = *stamp.hash.as_bytes();
        hash_bytes[0] ^= 0x01;
        let corrupted_hash = HashStamp {
            hash: Hash::from_bytes(hash_bytes),
            ..stamp
        };
        assert_eq!(corrupted_hash.recover(), StampState::Corrected(hash));

        let mut mirror_bytes = *stamp.mirror.as_bytes();
        mirror_bytes[0] ^= 0x01;
        let corrupted_mirror = HashStamp {
            mirror: Hash::from_bytes(mirror_bytes),
            ..stamp
        };
        assert_eq!(corrupted_mirror.recover(), StampState::Corrected(hash));

        let mut inverted_bytes = *stamp.inverted.as_bytes();
        inverted_bytes[0] ^= 0x01;
        let corrupted_inverted = HashStamp {
            inverted: Hash::from_bytes(inverted_bytes),
            ..stamp
        };
        assert_eq!(corrupted_inverted.recover(), StampState::Corrected(hash));
    }

    #[test]
    fn hash_stamp_reports_unrecoverable_when_copies_disagree() {
        let stamp = HashStamp::new(Hasher::new().finalize());
        let mut hash_bytes = *stamp.hash.as_bytes();
        hash_bytes[0] ^= 0x01;
        let mut mirror_bytes = *stamp.mirror.as_bytes();
        mirror_bytes[0] ^= 0x02;
        let corrupted = HashStamp {
            hash: Hash::from_bytes(hash_bytes),
            mirror: Hash::from_bytes(mirror_bytes),
            inverted: stamp.inverted,
        };
        assert_eq!(corrupted.recover(), StampState::Unrecoverable);
    }

    #[test]
    fn data_check_distinguishes_transient_and_stable_errors() {
        assert_eq!(classify(true), Verdict::TransientData);
        assert_eq!(classify(false), Verdict::Data);
    }
    //AI_END===
}
//AI_END===
