use super::*;
use crate::music_play::effect::resampler::ResamplePlan;
//AI=== seek 效果器测试的导入 / imports for the seek effect tests
use crate::music_play::effect::cross_bridge::CrossBridgeEffect;
use crate::music_play::effect::resampler::ResamplerEffect;
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
