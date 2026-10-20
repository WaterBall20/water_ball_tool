use crate::command::music_play::effect::{AudioFormat, PreEffect};
use std::collections::VecDeque;
use std::error::Error;
use std::f32::consts::FRAC_PI_2;
use std::time::Duration;

pub struct CrossBridge {
    enabled: bool,
    buf: VecDeque<f32>,
    b_buf: Vec<f32>,
    buf_max_len: usize,
    output_format: AudioFormat,
    duration: Duration,
    is_effect: bool,
}

impl PreEffect for CrossBridge {
    fn init(&mut self, _: AudioFormat) -> Result<(), Box<dyn Error>> {
        self.set_duration(self.duration);
        Ok(())
    }

    fn output_format(&self) -> AudioFormat {
        self.output_format
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        if self.is_effect {
            self.process_crossfade(input, out)
        } else {
            self.process_normal(input, out)
        }
    }

    fn input_eof(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
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

    fn input_none(&mut self) {
        self.is_effect = false;
        self.b_buf.clear();
        self.buf.clear();
    }

    fn reset(&mut self, _input_format: AudioFormat) {
        // reset 在每首歌开始输入数据前调用。
        // 若 is_effect = true，说明上一首的 input_eof 已触发，
        // buf 里是 A 的尾部，正在等本首数据做交叉。
        // 此时必须保留 buf、b_buf 和 is_effect，否则交叉失效。
        // 非交叉状态才清理残留。
        if !self.is_effect {
            self.b_buf.clear();
            self.buf.clear();
        }
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn name(&self) -> &'static str {
        "CrossBridge"
    }
}

impl CrossBridge {
    pub fn new(output_format: AudioFormat) -> Self {
        Self {
            enabled: true,
            buf: VecDeque::new(),
            b_buf: Vec::new(),
            buf_max_len: 0,
            output_format,
            duration: Duration::from_secs(1),
            is_effect: false,
        }
    }

    /// duration 钳到 [1 帧, 60s]，四舍五入到最近样本并整帧对齐
    pub fn set_duration(&mut self, duration: Duration) {
        if duration.is_zero() {
            return;
        }

        let channels = self.output_format.channels as usize;
        if channels == 0 {
            self.buf_max_len = 0;
            return;
        }

        let sample_rate = self.output_format.sample_rate as f64;
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
    }

    /// 延迟线核心：把 input 追加到 buf，超出 buf_max_len 的部分按时间顺序写入 out。
    /// 不清空 out，返回 out.len()；缓冲不满时直接入队，返回 out.len()（即 0）。
    fn process_normal_append(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        let this_len = self.buf.len();

        if this_len + input.len() <= self.buf_max_len {
            for v in input {
                self.buf.push_back(*v);
            }
            return out.len();
        }

        let out_len = this_len + input.len() - self.buf_max_len;
        let pop_len = out_len.min(this_len);

        for _ in 0..pop_len {
            out.push(self.buf.pop_front().expect("buf is empty"));
        }

        let in_to_out_len = out_len.saturating_sub(pop_len);
        if in_to_out_len > 0 {
            out.extend_from_slice(&input[..in_to_out_len]);
        }

        for v in &input[in_to_out_len..] {
            self.buf.push_back(*v);
        }

        out.len()
    }

    fn process_normal(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        out.clear();
        self.process_normal_append(input, out)
    }

    /// 交叉：
    /// 1) 等待期间 buf 不可变，target = buf.len() 稳定；
    /// 2) 收满后原地把 A*cos + B*sin 写回 buf（长度不变）；
    /// 3) input_rest 走与非交叉完全一致的延迟线逻辑。
    fn process_crossfade(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        let channels = self.output_format.channels as usize;
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

        // 交叉计算：原地写回 buf
        let cross_frames = target / channels;
        if cross_frames > 0 {
            let denom = (cross_frames - 1).max(1) as f32;
            let (buf, b_buf) = (&mut self.buf, &self.b_buf);
            let buf_slice = buf.make_contiguous();

            for i in 0..cross_frames {
                let t = if cross_frames <= 1 {
                    1.0
                } else {
                    i as f32 / denom
                };
                let theta = t * FRAC_PI_2;
                let g_out = theta.cos();
                let g_in = theta.sin();
                let base = i * channels;
                for ch in 0..channels {
                    let idx = base + ch;
                    buf_slice[idx] = buf_slice[idx] * g_out + b_buf[idx] * g_in;
                }
            }
        }

        self.b_buf.clear();
        self.is_effect = false;

        // input_rest 与非交叉计算完全一致
        out.clear();
        self.process_normal_append(input_rest, out)
    }
}
