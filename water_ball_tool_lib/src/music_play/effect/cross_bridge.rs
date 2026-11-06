use crate::music_play::effect::{
    AudioFormat, PreEffect, PreEffectDiscontinuity, PreEffectInfo, PreEffectOutputFormat,
};
use std::collections::VecDeque;
use std::error::Error;
use std::f32::consts::FRAC_PI_2;
use std::time::Duration;

pub struct CrossBridgeEffect {
    info: PreEffectInfo,
    buf: VecDeque<f32>,
    b_buf: Vec<f32>,
    buf_max_len: usize,
    duration: Duration,
    is_effect: bool,
    // 预计算增益表，按 sample 展开；短尾交叉按实际尾长缩放索引
    // Precomputed gain table expanded per sample; a short-tail crossfade scales the index to the actual tail.
    gain_out: Vec<f32>,
    gain_in: Vec<f32>,
}

impl PreEffect for CrossBridgeEffect {
    fn init(
        &mut self,
        _: AudioFormat,
    ) -> Result<PreEffectOutputFormat, Box<dyn Error + Send + Sync>> {
        self.set_duration(self.duration);
        Ok(PreEffectOutputFormat {
            output_format: self.info.output_format,
        })
    }

    fn reset(
        &mut self,
        _input_format: AudioFormat,
    ) -> Result<PreEffectOutputFormat, Box<dyn Error + Send + Sync + 'static>> {
        // reset 在每首歌开始输入数据前调用。
        // 若 is_effect = true，说明上一首 input_eof 已触发，
        // buf 里是 A 尾部，正在等本首数据做交叉，必须保留。
        if !self.is_effect {
            self.b_buf.clear();
            self.buf.clear();
        }
        Ok(PreEffectOutputFormat {
            output_format: self.info.output_format,
        })
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        if self.is_effect {
            self.process_crossfade(input, out)
        } else {
            self.process_normal(input, out)
        }
    }

    fn on_discontinuity(
        &mut self,
        input: &[f32],
        output: &mut Vec<f32>,
        kind: PreEffectDiscontinuity,
    ) -> usize {
        //AI=== Seek 丢弃交叉淡入的旧尾巴与延迟线 / Seek drops the old crossfade tail and delay line
        if PreEffectDiscontinuity::Seek == kind {
            self.is_effect = false;
            self.b_buf.clear();
            self.buf.clear();
            return 0;
        }
        //AI_END===
        if PreEffectDiscontinuity::Skip == kind {
            self.is_effect = false;
            self.b_buf.clear();
            self.buf.clear();
            0
        } else if PreEffectDiscontinuity::NoNext == kind {
            let out_len = self.buf.len() + input.len();
            output.reserve_exact(out_len);
            for v in self.buf.drain(..) {
                output.push(v);
            }
            output.extend_from_slice(input);
            debug_assert!(
                !output.is_empty(),
                "cross_bridge on_discontinuity no skip, output is \
            empty"
            );
            output.len()
        } else {
            if self.is_effect {
                self.is_effect = false;
                self.b_buf.clear();
            }

            let len = self.process_normal(input, output);

            if self.buf.is_empty() {
                return len;
            }

            self.is_effect = true;
            self.b_buf.clear();
            len
        }
    }

    fn info(&self) -> &PreEffectInfo {
        &self.info
    }

    fn enabled(&self) -> bool {
        self.info.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.info.enabled = enabled;
    }
}

impl CrossBridgeEffect {
    pub fn new(output_format: AudioFormat) -> Self {
        Self {
            info: PreEffectInfo {
                name: "CrossBridge".to_string(),
                enabled: true,
                input_format: AudioFormat::default(),
                output_format,
                latency_samples: 0,
                tail_samples: 0,
            },
            buf: VecDeque::new(),
            b_buf: Vec::new(),
            buf_max_len: 0,
            duration: Duration::from_secs(1),
            is_effect: false,
            gain_out: Vec::new(),
            gain_in: Vec::new(),
        }
    }

    /// duration 钳到 [1 帧, 60s]，四舍五入到最近样本并整帧对齐；
    /// 同时预计算等功率增益表。
    pub fn set_duration(&mut self, duration: Duration) {
        if duration.is_zero() {
            return;
        }

        let channels = self.info.output_format.channels as usize;
        if channels == 0 {
            self.buf_max_len = 0;
            return;
        }

        let sample_rate = self.info.output_format.sample_rate as f64;
        if sample_rate <= 0.0 {
            self.buf_max_len = 0;
            return;
        }

        let frame_rate = sample_rate * channels as f64;
        let min_secs = 1.0 / sample_rate;
        let max_secs = 60.0;
        let secs = duration.as_secs_f64().clamp(min_secs, max_secs);
        self.duration = Duration::from_secs_f64(secs);

        let samples = (frame_rate * secs).round() as usize;
        let frames = (samples / channels).max(1);
        self.buf_max_len = frames * channels;

        self.buf.reserve_exact(self.buf_max_len);
        self.b_buf.clear();
        self.b_buf.reserve_exact(self.buf_max_len);

        // 预计算等功率增益表，按 sample 展开 / precompute the equal-power gain table, one entry per sample
        self.gain_out.clear();
        self.gain_in.clear();
        self.gain_out.reserve_exact(self.buf_max_len);
        self.gain_in.reserve_exact(self.buf_max_len);

        let denom = (frames - 1).max(1) as f32;
        for i in 0..frames {
            let t = if frames <= 1 { 1.0 } else { i as f32 / denom };
            let theta = t * FRAC_PI_2;
            let g_out = theta.cos();
            let g_in = theta.sin();
            for _ in 0..channels {
                self.gain_out.push(g_out);
                self.gain_in.push(g_in);
            }
        }
    }

    //AI=== 非对称填充：未满时输出小于输入，差值逐步补满延迟线；补满后回到 1:1
    // Asymmetric fill: while the line is not full, output less than the input so the difference fills it; once
    // full, fall back to 1:1. The retained amount is at most half a block and at least one sample, so the line
    // always makes progress and input never equals output while filling.
    fn process_normal(&mut self, input: &[f32], output: &mut Vec<f32>) -> usize {
        if input.is_empty() {
            return output.len();
        }

        let deficit = self.buf_max_len.saturating_sub(self.buf.len());
        let retain = if deficit == 0 {
            0
        } else {
            (input.len() / 2).clamp(1, deficit).min(input.len())
        };
        let out_len = input.len() - retain;

        self.buf.extend(input.iter().copied());
        if out_len > 0 {
            output.reserve(out_len);
            let (front, back) = self.buf.as_slices();
            let n1 = front.len().min(out_len);
            let n2 = (out_len - n1).min(back.len());
            output.extend_from_slice(&front[..n1]);
            if n2 > 0 {
                output.extend_from_slice(&back[..n2]);
            }
            self.buf.drain(..out_len);
        }
        output.len()
    }
    //AI_END===

    /// 交叉：
    /// 1) 等待期间 buf 不可变，target = buf.len() 稳定；
    /// 2) 收满后原地把 A*cos + B*sin 写回 buf（长度不变）；实际尾长短于配置时长时缩放增益表索引；
    /// 3) input_rest 走与非交叉完全一致的延迟线逻辑。
    fn process_crossfade(&mut self, input: &[f32], output: &mut Vec<f32>) -> usize {
        let channels = self.info.output_format.channels as usize;
        let target = self.buf.len();

        if channels == 0 || target == 0 {
            self.is_effect = false;
            self.b_buf.clear();
            output.clear();
            output.extend_from_slice(input);
            return output.len();
        }

        let needed = target.saturating_sub(self.b_buf.len());

        // 等待期间：只累积 B，不触碰 buf，不触碰 out
        if input.len() < needed {
            self.b_buf.extend_from_slice(input);
            return 0;
        }

        self.b_buf.extend_from_slice(&input[..needed]);
        let input_rest = &input[needed..];

        //AI=== 短尾交叉：表按完整交叉时长预计算，实际尾长短于它时缩放索引，让 A 仍衰减到 0、B 升到 1
        // Short-tail crossfade: the table covers the full duration, so scale the index to the actual tail; A still
        // fades to 0 and B rises to 1 instead of the curve being truncated.
        let target_frames = target / channels;
        let table_frames = self.gain_out.len() / channels;
        let last_table_frame = table_frames.saturating_sub(1);
        {
            let (buf, b_buf, gain_out, gain_in) =
                (&mut self.buf, &self.b_buf, &self.gain_out, &self.gain_in);
            let buf_slice = buf.make_contiguous();
            for frame in 0..target_frames {
                let table_frame = if target_frames > 1 && table_frames > 1 {
                    frame * last_table_frame / (target_frames - 1)
                } else {
                    0
                };
                let table_idx = table_frame * channels;
                let base = frame * channels;
                for channel in 0..channels {
                    let idx = base + channel;
                    let gain_idx = table_idx + channel;
                    buf_slice[idx] =
                        buf_slice[idx] * gain_out[gain_idx] + b_buf[idx] * gain_in[gain_idx];
                }
            }
        }
        //AI_END===

        self.b_buf.clear();
        self.is_effect = false;

        output.clear();
        self.process_normal(input_rest, output)
    }
}
