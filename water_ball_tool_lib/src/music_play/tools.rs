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
    #[must_use]
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

#[derive(Debug, Default, Clone)]
pub struct DurationStatsWindow {
    pub total_ns: u64,
    pub count: u64,
    pub max_ns: u64,
}

impl DurationStatsWindow {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    #[must_use]
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
    //AI=== 链路延迟统计：只做低频原子读写，不进入实时路径的锁 / end-to-end latency gauges: low-frequency atomics only
    /// 解码器内部待写样本数（解码格式，含声道）/ decoder-side samples not yet written (decoder format, channels included)
    pub decoder_buffered_samples: AtomicU64,
    /// 前效果器输入环待消费样本数（解码格式，含声道）/ samples queued for the pre-effect thread (decoder format, channels included)
    pub pre_effect_queue_samples: AtomicU64,
    /// 前效果器内部延迟线时长（纳秒）/ pre-effect internal delay-line duration in nanoseconds
    pub effect_delay_ns: AtomicU64,
    /// 最近一次音频回调缓冲区样本数（设备格式，含声道）/ samples in the last audio callback buffer (device format, channels included)
    pub callback_samples: AtomicU64,
    //AI_END===
}
impl TimeMetrics {
    #[must_use]
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            // 每个字段零初始化
            ..Default::default() // 或手写各字段 Default::default()
        }
    }

    //AI=== 前效果器排队计数：写入由解码线程加，消费由效果线程减 / pre-effect queue accounting: decoder adds, effect thread subtracts
    /// 解码线程在写入前效果环前登记，写入被拒时用 `take_pre_effect_queue_samples` 撤回
    /// Called by the decoder before the pre-effect ring push; take the samples back with
    /// `take_pre_effect_queue_samples` if the push is rejected.
    pub fn add_pre_effect_queue_samples(&self, samples: usize) {
        self.pre_effect_queue_samples
            .fetch_add(samples as u64, Ordering::Relaxed);
    }

    /// 效果线程取走一块后调用；饱和减法避免切歌竞态下下溢
    /// Called by the effect thread after taking a block; saturating subtraction avoids underflow during track changes.
    pub fn take_pre_effect_queue_samples(&self, samples: usize) {
        let samples = samples as u64;
        let _ = self.pre_effect_queue_samples.try_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |current| Some(current.saturating_sub(samples)),
        );
    }
    //AI_END===
}
pub const DURATION_ACCUMULATOR_FLUSH_INTERVAL: u32 = 10; // 约 0.3–1.2s 一次，视 block 大小

//AI=== 排队计数饱和减法测试 / pre-effect queue saturating subtraction test
#[cfg(test)]
mod test {
    use super::{Ordering, TimeMetrics};

    #[test]
    fn pre_effect_queue_samples_saturate_at_zero() {
        let metrics = TimeMetrics::new();
        metrics.add_pre_effect_queue_samples(100);
        metrics.take_pre_effect_queue_samples(30);
        assert_eq!(70, metrics.pre_effect_queue_samples.load(Ordering::Relaxed));

        // 切歌竞态下可能多减，饱和到 0 而不是下溢 / a track-change race may over-take; saturate at 0 instead of underflowing
        metrics.take_pre_effect_queue_samples(200);
        assert_eq!(0, metrics.pre_effect_queue_samples.load(Ordering::Relaxed));
    }
}
//AI_END===
