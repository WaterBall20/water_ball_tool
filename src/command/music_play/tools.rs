use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// 单线程热路径累积，无锁、无原子。
#[derive(Default)]
pub struct DurationAccumulator {
    total: Duration,
    count: u64,
    max: Duration,
}

impl DurationAccumulator {
    pub fn new() -> Self {
        Self::default()
    }
    #[inline]
    pub fn record(&mut self, d: Duration) {
        self.total += d;
        self.max = self.max.max(d);
        self.count += 1;
    }

    /// 本地 → 共享，冲完清零。
    pub fn flush_into(&mut self, stats: &DurationStats) {
        stats
            .total_ns
            .fetch_add(self.total.as_nanos() as u64, Ordering::Relaxed);
        stats.count.fetch_add(self.count, Ordering::Relaxed);
        stats
            .max_ns
            .fetch_max(self.max.as_nanos() as u64, Ordering::Relaxed);
        self.total = Duration::ZERO;
        self.count = 0;
        self.max = Duration::ZERO;
    }
}

/// 跨线程可读的原子镜像，只由 flush_into 写。
#[derive(Default, Debug)]
pub struct DurationStats {
    total_ns: AtomicU64,
    count: AtomicU64,
    max_ns: AtomicU64,
}

impl DurationStats {
    /// 外部统计线程用 swap 取走一个窗口。
    pub fn take(&self) -> Option<DurationStatsWindow> {
        if self.count.load(Ordering::Acquire) == 0 {
            return None;
        }
        Some(DurationStatsWindow {
            total_ns: self.total_ns.swap(0, Ordering::Relaxed),
            count: self.count.swap(0, Ordering::Relaxed),
            max_ns: self.max_ns.swap(0, Ordering::Relaxed),
        })
    }
}

#[derive(Debug, Default)]
pub struct DurationStatsWindow {
    pub total_ns: u64,
    pub count: u64,
    pub max_ns: u64,
}

impl DurationStatsWindow {
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn mean(&self) -> Duration {
        Duration::from_nanos(self.total_ns / self.count.max(1))
    }
}

#[derive(Default, Debug)]
pub struct TimeMetrics {
    pub enabled: AtomicBool,
    pub decode: DurationStats,
    pub decode_write_wait: DurationStats,
    pub effect_read_wait: DurationStats,
    pub effect_process: DurationStats,
    pub effect_write_wait: DurationStats,
    pub callback_total: DurationStats,
    pub callback_copy: DurationStats,
    pub callback_post_effect: DurationStats,
    pub lock_fail: AtomicU64, // 低频事件直接原子加，不必本地累积
    pub underrun: AtomicU64,
}
impl TimeMetrics {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            // 每个字段零初始化
            ..Default::default() // 或手写各字段 Default::default()
        }
    }
}
pub const DURATION_ACCUMULATOR_FLUSH_INTERVAL: u32 = 10; // 约 0.3–1.2s 一次，视 block 大小
