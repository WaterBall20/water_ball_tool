use crate::command::music_play::effect::{
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
    // 预计算增益表，按 sample 索引，长度 = buf_max_len
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
        out: &mut Vec<f32>,
        kind: PreEffectDiscontinuity,
    ) -> usize {
        if PreEffectDiscontinuity::Skip == kind {
            self.is_effect = false;
            self.b_buf.clear();
            self.buf.clear();
            0
        } else if PreEffectDiscontinuity::NoNext == kind {
            let out_len = self.buf.len() + input.len();
            out.reserve_exact(out_len);
            for v in self.buf.drain(..) {
                out.push(v);
            }
            out.extend_from_slice(input);
            out.len()
        } else {
            if self.is_effect {
                self.is_effect = false;
                self.b_buf.clear();
            }

            let n = self.process_normal(input, out);

            if self.buf.is_empty() {
                return n;
            }

            self.is_effect = true;
            self.b_buf.clear();
            n
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
                input_format: Default::default(),
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

        // 预计算等功率增益表，按 sample 展开
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

    /// 延迟线核心：把 input 追加到 buf，超出 buf_max_len 的部分按时间顺序写入 out。
    /// 不清空 out，返回 out.len()。
    fn process_normal_append(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        let this_len = self.buf.len();

        if this_len + input.len() <= self.buf_max_len {
            self.buf.extend(input.iter().copied());
            return out.len();
        }

        let out_len = this_len + input.len() - self.buf_max_len;
        let pop_len = out_len.min(this_len);
        let in_to_out_len = out_len - pop_len;

        out.reserve(out_len);

        // 批量从 buf 头部取出 pop_len 个，走 memcpy
        if pop_len > 0 {
            {
                let (front, back) = self.buf.as_slices();
                let n1 = front.len().min(pop_len);
                let n2 = (pop_len - n1).min(back.len());
                out.extend_from_slice(&front[..n1]);
                if n2 > 0 {
                    out.extend_from_slice(&back[..n2]);
                }
            }
            self.buf.drain(..pop_len);
        }

        if in_to_out_len > 0 {
            out.extend_from_slice(&input[..in_to_out_len]);
        }

        if in_to_out_len < input.len() {
            self.buf.extend(input[in_to_out_len..].iter().copied());
        }

        out.len()
    }

    fn process_normal(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        out.clear();
        self.process_normal_append(input, out)
    }

    /// 交叉：
    /// 1) 等待期间 buf 不可变，target = buf.len() 稳定；
    /// 2) 收满后原地把 A*cos + B*sin 写回 buf（长度不变），查预计算增益表；
    /// 3) input_rest 走与非交叉完全一致的延迟线逻辑。
    fn process_crossfade(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        let channels = self.info.output_format.channels as usize;
        let target = self.buf.len();

        if channels == 0 || target == 0 {
            self.is_effect = false;
            self.b_buf.clear();
            out.clear();
            out.extend_from_slice(input);
            return out.len();
        }

        let needed = target.saturating_sub(self.b_buf.len());

        // 等待期间：只累积 B，不触碰 buf，不触碰 out
        if input.len() < needed {
            self.b_buf.extend_from_slice(input);
            return 0;
        }

        self.b_buf.extend_from_slice(&input[..needed]);
        let input_rest = &input[needed..];

        // 交叉计算：拉平到 sample 索引，查表 + FMA
        {
            let (buf, b_buf, gain_out, gain_in) =
                (&mut self.buf, &self.b_buf, &self.gain_out, &self.gain_in);
            let buf_slice = buf.make_contiguous();
            debug_assert!(target <= gain_out.len());

            for idx in 0..target {
                buf_slice[idx] = buf_slice[idx] * gain_out[idx] + b_buf[idx] * gain_in[idx];
            }
        }

        self.b_buf.clear();
        self.is_effect = false;

        out.clear();
        self.process_normal_append(input_rest, out)
    }
}
