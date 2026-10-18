use super::*;
use crate::command::music_play::effect::resampler::ResamplePlan;

#[test]
fn to_384k_quality_dominant() {
    let p = ResamplePlan::compute_plan(44100, 384000, 32768, 0.2, 1024, 32768);
    assert_eq!(p.block_frames, 3822); // 147 × 26
    assert_eq!(p.period, 147);
    assert!((p.t_play_sec - 0.0867).abs() < 1e-3);
    assert!(p.out_fft_equiv >= 32768 - 100);

    let p = ResamplePlan::compute_plan(48000, 384000, 32768, 0.2, 1024, 32768);
    assert_eq!(p.block_frames, 4096);
    let p = ResamplePlan::compute_plan(96000, 384000, 32768, 0.2, 1024, 32768);
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
    let p = ResamplePlan::compute_plan(8000, 384000, 32768, 0.5, 4096, 16384);
    assert!(p.block_frames >= 4096 && p.block_frames <= 16384);
}
