use super::*;
//AI=== seek 效果器测试的导入 / imports for the seek effect tests
use crate::music_play::effect::cross_bridge::CrossBridgeEffect;
use crate::music_play::effect::resampler::ResamplePlan;
use crate::music_play::effect::resampler::ResamplerEffect;
use pretty_assertions::assert_eq;
//AI_END===

//AI=== 清空输入环必须同时清零排队延迟，否则换曲后累计 / clearing the input ring must reset the queue gauge or track changes accumulate
#[test]
fn clearing_input_ring_resets_pre_effect_queue_gauge() {
    use crate::concurrent_ring::ConcurrentRing;
    use crate::music_play::effect::runtime::PreEffectRuntime;
    use crate::music_play::tools::TimeMetrics;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let input = match ConcurrentRing::new_max_capacity(4) {
        Ok(ring) => ring,
        Err(_) => panic!("ring creation must succeed"),
    };
    let (_reader, writer) = crate::split_ring_buffer::unlocked_read_locked_write_ring_buffer(64);
    let metrics = Arc::new(TimeMetrics::new());
    let runtime = PreEffectRuntime::create(input, writer, metrics.clone());

    if runtime
        .pre_effect_ring_buffer()
        .push(AudioData::Block(vec![0.0; 64]))
        .is_err()
    {
        panic!("block push must succeed");
    }
    metrics.add_pre_effect_queue_samples(64);

    if runtime.send_skip().is_err() {
        panic!("send_skip must succeed");
    }
    assert_eq!(0, metrics.pre_effect_queue_samples.load(Ordering::Relaxed));
}
//AI_END===

//AI=== 命令通道：无音频数据时也能唤醒效果器线程并应用开关 / the command channel wakes the effect thread and applies a toggle even without audio data
#[test]
fn runtime_applies_queued_toggle_without_audio_data() {
    use crate::concurrent_ring::ConcurrentRing;
    use crate::music_play::effect::runtime::PreEffectRuntime;
    use crate::music_play::tools::TimeMetrics;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let input = match ConcurrentRing::new_max_capacity(4) {
        Ok(ring) => ring,
        Err(_) => panic!("ring creation must succeed"),
    };
    let (_reader, writer) = crate::split_ring_buffer::unlocked_read_locked_write_ring_buffer(64);
    let metrics = Arc::new(TimeMetrics::new());
    let mut runtime = PreEffectRuntime::create(input, writer, metrics);

    let format = AudioFormat::new(2, 1_000);
    let mut chain = PreEffectChain::new();
    let Ok(()) = chain.push(CrossBridgeEffect::new(format)) else {
        panic!("cross bridge must be pushed");
    };
    let Ok(()) = chain.reset(format, ResetReason::TrackStart) else {
        panic!("chain reset must succeed");
    };
    runtime.set_effects(chain);
    if runtime.run().is_err() {
        panic!("effect thread must start");
    }

    assert!(runtime.request_effect_enabled(0, false));

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(summaries) = runtime.effect_summaries()
            && !summaries[0].enabled
        {
            break;
        }
        if Instant::now() > deadline {
            panic!("queued toggle was not applied in time");
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    if runtime.stop().is_err() {
        panic!("effect thread must stop cleanly");
    }
}
//AI_END===

//AI=== Drop 必须唤醒阻塞的效果器线程，否则 join 会挂死 / Drop must wake the blocked effect thread or join hangs
#[test]
fn dropping_running_runtime_wakes_blocked_effect_thread() {
    use crate::concurrent_ring::ConcurrentRing;
    use crate::music_play::effect::runtime::PreEffectRuntime;
    use crate::music_play::tools::TimeMetrics;
    use std::sync::Arc;

    let input = match ConcurrentRing::new_max_capacity(2) {
        Ok(ring) => ring,
        Err(_) => panic!("ring creation must succeed"),
    };
    let (_reader, writer) = crate::split_ring_buffer::unlocked_read_locked_write_ring_buffer(64);
    let metrics = Arc::new(TimeMetrics::new());
    let mut runtime = PreEffectRuntime::create(input, writer, metrics);
    if runtime.run().is_err() {
        panic!("effect thread must start");
    }

    // 没有 stop：Drop 自己置位停止标志并主动唤醒读者 / no stop call: Drop sets the flag and wakes the reader itself
    drop(runtime);
}
//AI_END===

//AI=== 写端被唤醒后缓存剩余数据：开关命令不会丢掉当前块的后半段 / the runtime caches the remaining half of a block after a non-destructive wake
#[test]
fn runtime_keeps_unwritten_output_after_wake() {
    use crate::music_play::effect::runtime::PreEffectRuntime;
    use crate::music_play::tools::TimeMetrics;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let (reader, writer) = crate::split_ring_buffer::locked_ring_buffer::<f32>(4);
    let input = match crate::concurrent_ring::ConcurrentRing::new_max_capacity(4) {
        Ok(ring) => ring,
        Err(_) => panic!("ring creation must succeed"),
    };
    let metrics = Arc::new(TimeMetrics::new());
    let mut runtime = PreEffectRuntime::create(input, writer, metrics);
    // 空链：输出等于输入，便于精确断言 / empty chain: output equals input for exact assertions
    runtime.set_effects(PreEffectChain::new());
    if runtime.run().is_err() {
        panic!("effect thread must start");
    }

    let block: Vec<f32> = (1..=8).map(|v| v as f32).collect();
    if runtime
        .pre_effect_ring_buffer()
        .push(AudioData::Block(block.clone()))
        .is_err()
    {
        panic!("input block push must succeed");
    }

    // 输出环容量 4：前 4 个样本写入后写端阻塞 / output capacity 4: the writer fills it and blocks
    let deadline = Instant::now() + Duration::from_secs(2);
    while reader.len() != 4 {
        if Instant::now() > deadline {
            panic!("writer did not fill the output ring in time");
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // 非破坏性唤醒：未写完的 4 个样本必须进入运行时缓存 / non-destructive wake must retain the remaining 4 samples
    assert!(runtime.request_effect_enabled(0, false));

    let mut first = [0.0f32; 4];
    assert_eq!(4, reader.read(&mut first));
    assert_eq!([1.0, 2.0, 3.0, 4.0], first);

    let deadline = Instant::now() + Duration::from_secs(2);
    while reader.len() != 4 {
        if Instant::now() > deadline {
            panic!("cached remainder was not written in time");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut second = [0.0f32; 4];
    assert_eq!(4, reader.read(&mut second));
    assert_eq!([5.0, 6.0, 7.0, 8.0], second);

    if runtime.stop().is_err() {
        panic!("effect thread must stop cleanly");
    }
}
//AI_END===

//AI=== 效果器列表只通过 trait 方法生成 / the effect list is built from trait methods only
#[test]
fn chain_effect_summaries_use_trait_metadata() {
    let format = AudioFormat::new(2, 1_000);
    let mut chain = PreEffectChain::new();
    let Ok(()) = chain.push(CrossBridgeEffect::new(format)) else {
        panic!("cross bridge must be pushed");
    };
    let Ok(()) = chain.reset(format, ResetReason::TrackStart) else {
        panic!("chain reset must succeed");
    };
    let mut out = Vec::new();
    chain.process(&[0.1; 2_000], &mut out);

    let summaries = chain.effect_summaries();
    let Some(summary) = summaries.first() else {
        panic!("the chain must report its effect");
    };
    assert_eq!(EffectChainKind::Pre, summary.chain);
    assert_eq!("交叉桥接", summary.name);
    assert!(summary.enabled);
    assert!(!summary.description.is_empty());
    assert_eq!(std::time::Duration::from_millis(500), summary.latency);
}
//AI_END===

//AI=== 效果器启用/禁用只走 trait 方法 / enabling/disabling goes through trait methods only
#[test]
fn chain_toggle_updates_effect_summary() {
    let format = AudioFormat::new(2, 1_000);
    let mut chain = PreEffectChain::new();
    let Ok(()) = chain.push(CrossBridgeEffect::new(format)) else {
        panic!("cross bridge must be pushed");
    };
    let Ok(()) = chain.reset(format, ResetReason::TrackStart) else {
        panic!("chain reset must succeed");
    };

    let mut drained = Vec::new();
    assert!(chain.effect_summaries()[0].enabled);
    assert!(chain.set_effect_enabled(0, false, &mut drained).is_ok());
    assert!(!chain.effect_summaries()[0].enabled);
    assert!(chain.set_effect_enabled(0, true, &mut drained).is_ok());
    assert!(chain.effect_summaries()[0].enabled);
    assert!(matches!(
        chain.set_effect_enabled(99, true, &mut drained),
        Err(EffectToggleError::InvalidIndex)
    ));
}
//AI_END===

//AI=== 禁用交叉桥接：排空延迟线后旁路，重新启用重新填充 / disabling cross-bridge drains the line, bypasses, and refills on enable
#[test]
fn disabling_cross_bridge_drains_and_bypasses() {
    let format = AudioFormat::new(2, 1_000);
    let mut chain = PreEffectChain::new();
    let Ok(()) = chain.push(CrossBridgeEffect::new(format)) else {
        panic!("cross bridge must be pushed");
    };
    let Ok(()) = chain.reset(format, ResetReason::TrackStart) else {
        panic!("chain reset must succeed");
    };

    // 填满延迟线：输入 2000、输出 1000 / fill the line: 2000 in, 1000 out
    let mut out = Vec::new();
    chain.process(&[0.5; 2_000], &mut out);
    assert_eq!(1_000, out.len());

    let mut drained = Vec::new();
    assert!(
        chain.set_effect_enabled(0, false, &mut drained).is_ok(),
        "disabling an allowed effect must succeed"
    );
    assert_eq!(1_000, drained.len());
    assert!(!chain.effect_summaries()[0].enabled);
    assert_eq!(
        std::time::Duration::ZERO,
        chain.effect_summaries()[0].latency
    );

    // 旁路后输入原样通过 / once bypassed, input passes through unchanged
    let mut bypassed = Vec::new();
    chain.process(&[0.1; 100], &mut bypassed);
    assert_eq!(100, bypassed.len());

    // 重新启用后从空延迟线开始重新填充 / enabling starts from an empty line and refills
    let mut drained = Vec::new();
    assert!(chain.set_effect_enabled(0, true, &mut drained).is_ok());
    let mut refill = Vec::new();
    chain.process(&[0.1; 100], &mut refill);
    assert_eq!(50, refill.len());
}
//AI_END===

//AI=== 重采样器静态禁止禁用：转换中旁路会让输出格式与设备不一致 / the resampler is statically non-disableable
#[test]
fn resampler_is_statically_not_disableable() {
    let mut chain = PreEffectChain::new();
    let Ok(()) = chain.push(ResamplerEffect::new(AudioFormat::new(2, 48_000))) else {
        panic!("resampler must be pushed");
    };
    let Ok(()) = chain.reset(AudioFormat::new(2, 44_100), ResetReason::TrackStart) else {
        panic!("chain reset must succeed");
    };

    assert!(!chain.effect_summaries()[0].can_disable);
    let mut drained = Vec::new();
    assert!(matches!(
        chain.set_effect_enabled(0, false, &mut drained),
        Err(EffectToggleError::Forbidden)
    ));
    assert!(chain.effect_summaries()[0].enabled);
}
//AI_END===

//AI=== 链延迟使用管理器登记的输入格式 / chain latency uses the manager-registered input format
#[test]
fn chain_buffered_duration_uses_registered_input_format() {
    let format = AudioFormat::new(2, 1_000);
    let mut chain = PreEffectChain::new();
    let Ok(()) = chain.push(CrossBridgeEffect::new(format)) else {
        panic!("cross bridge must be pushed");
    };
    let Ok(()) = chain.reset(format, ResetReason::TrackStart) else {
        panic!("chain reset must succeed");
    };

    // 1 秒延迟线填到一半：保留 1000 个交错样本 = 0.5 秒
    // Half of a 1 s line: 1000 interleaved samples = 0.5 s.
    let mut out = Vec::new();
    chain.process(&[0.1; 2_000], &mut out);
    assert_eq!(
        std::time::Duration::from_millis(500),
        chain.buffered_duration()
    );
}
//AI_END===

//AI=== seek 必须丢弃旧位置的效果器状态 / seek must drop old-position effect state
#[test]
fn resampler_seek_discards_buffered_input() {
    let mut effect = ResamplerEffect::new(AudioFormat::new(2, 48_000));
    let Ok(_) = effect.init(AudioFormat::new(2, 44_100)) else {
        panic!("resampler init must succeed");
    };

    // 奇数长度保证至少留一个样本在内部缓冲 / an odd length guarantees a buffered remainder
    let mut out = Vec::new();
    effect.process(&vec![0.5; 44_101], &mut out);
    assert!(
        effect.buffered_len() > 0,
        "test setup must leave buffered input"
    );

    let mut discarded = Vec::new();
    effect.on_discontinuity(&[], &mut discarded, PreEffectDiscontinuity::Seek);
    assert_eq!(0, effect.buffered_len());
}

#[test]
fn cross_bridge_seek_drops_delay_line() {
    let format = AudioFormat::new(2, 1_000);
    let mut effect = CrossBridgeEffect::new(format);
    let Ok(_) = effect.init(format) else {
        panic!("cross bridge init must succeed");
    };

    // 旧音频进入延迟线，未满时仍会输出一半 / old audio enters the line and half is still emitted while filling
    let mut out = Vec::new();
    effect.process(&[0.5; 2_000], &mut out);
    assert_eq!(1_000, out.len());

    let mut discarded = Vec::new();
    effect.on_discontinuity(&[], &mut discarded, PreEffectDiscontinuity::Seek);

    // seek 后仅新样本可以出现 / only new samples may appear after the seek
    let mut out = Vec::new();
    effect.process(&[0.1; 2_000], &mut out);
    assert_eq!(1_000, out.len());
    assert!(
        (out[0] - 0.1).abs() < 1e-6,
        "old delay-line samples leaked after seek"
    );
}

#[test]
fn cross_bridge_fills_the_delay_line_asymmetrically() {
    let format = AudioFormat::new(2, 1_000); // 1 s line = 2000 samples
    let mut effect = CrossBridgeEffect::new(format);
    let Ok(_) = effect.init(format) else {
        panic!("cross bridge init must succeed");
    };

    let block = [0.1f32; 100];
    let mut sizes = Vec::new();
    for _ in 0..41 {
        let mut out = Vec::new();
        effect.process(&block, &mut out);
        sizes.push(out.len());
    }

    // 未满的 40 个块：输入 100、输出 50；补满后回到 1:1
    // While filling (40 blocks): 100 in, 50 out; once full it returns to 1:1.
    assert!(sizes[..40].iter().all(|len| *len == 50));
    assert_eq!(100, sizes[40]);
}

#[test]
fn short_tail_crossfade_uses_the_full_curve() {
    let format = AudioFormat::new(2, 1_000);
    let mut effect = CrossBridgeEffect::new(format);
    let Ok(_) = effect.init(format) else {
        panic!("cross bridge init must succeed");
    };

    // 留出约 250ms 的尾长（缓冲 500 样本）/ leave a ~250 ms tail (500 buffered samples)
    let mut out = Vec::new();
    effect.process(&[0.5; 1_000], &mut out);
    let mut discarded = Vec::new();
    effect.on_discontinuity(&[], &mut discarded, PreEffectDiscontinuity::Eof);

    // B 填满交叉区后，剩余输出应平滑接上，不能出现阶跃
    // Once B fills the crossfade region the remaining output must join smoothly, without a step.
    let mut blend = Vec::new();
    effect.process(&[0.1; 500], &mut blend);
    for _ in 0..20 {
        effect.process(&[0.1; 100], &mut blend);
    }

    assert!(blend.len() >= 500);
    let max_step = blend
        .windows(2)
        .map(|window| (window[1] - window[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(max_step < 0.05, "crossfade step too large: {max_step}");
}

//AI_END===

#[test]
fn to_384k_quality_dominant() {
    let p = ResamplePlan::compute_plan(44100, 384_000, 32768, 0.2, 1024, 32768);
    assert_eq!(p.block_frames, 3822); // 147 × 26
    assert_eq!(p.period, 147);
    assert!((p.t_play_sec - 0.0867).abs() < 1e-3);
    assert!(p.out_fft_equiv >= 32768 - 100);

    let p = ResamplePlan::compute_plan(48000, 384_000, 32768, 0.2, 1024, 32768);
    assert_eq!(p.block_frames, 4096);
    let p = ResamplePlan::compute_plan(96000, 384_000, 32768, 0.2, 1024, 32768);
    assert_eq!(p.block_frames, 8192);
}

#[test]
fn time_bound_dominant() {
    // 44.1k → 48k，target 太大，被 t_play_max 钳住
    let p = ResamplePlan::compute_plan(44100, 48000, 32768, 0.2, 1024, 32768);
    assert!(p.t_play_sec <= 0.2 + 1e-6);
    assert_eq!(p.period, 147);
}

#[test]
fn large_period_falls_back_to_pow2() {
    // period=44101 > MAX_ALIGN，退化为 2 的幂
    let p = ResamplePlan::compute_plan(44101, 48000, 32768, 0.2, 1024, 32768);
    assert!(p.block_frames.is_power_of_two());
}

#[test]
fn clamp_bounds() {
    let p = ResamplePlan::compute_plan(8000, 384_000, 32768, 0.5, 4096, 16384);
    assert!(p.block_frames >= 4096 && p.block_frames <= 16384);
}
