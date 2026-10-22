use crate::command::music_play::effect::{AudioFormat, PreEffect};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use std::cmp::PartialEq;
use std::collections::VecDeque;
use std::error::Error;

const RESAMPLER_CHUNK_LEN: usize = 1024;

pub struct ResamplerEffect {
    enabled: bool,
    error: Option<String>,
    resampler: Option<Fft<f32>>,
    /// 当前 resampler 对应的输入格式；output_format 构造后固定，无需单列
    resampler_input_format: Option<AudioFormat>,
    in_buf: VecDeque<f32>,
    out_temp_buf: Vec<f32>,
    input_format: AudioFormat,
    output_format: AudioFormat,
    max_out_frames: usize,
    chunk_len: usize,
}

impl ResamplerEffect {
    pub fn new(output_format: AudioFormat) -> Self {
        Self {
            enabled: true,
            error: None,
            resampler: None,
            resampler_input_format: None,
            in_buf: VecDeque::new(),
            out_temp_buf: Vec::new(),
            input_format: AudioFormat::default(),
            output_format,
            max_out_frames: 0,
            chunk_len: RESAMPLER_CHUNK_LEN,
        }
    }

    fn set_error(&mut self, error: Box<dyn Error + 'static>) {
        self.enabled = false;
        self.error = Some(error.to_string());
    }

    fn process_inner(
        in_buf: &[f32],
        out_temp_buf: &mut Vec<f32>,
        out: &mut Vec<f32>,
        chunk_len: usize,
        channels: usize,
        max_out_frames: usize,
        resampler: &mut Fft<f32>,
    ) -> (usize, Option<Box<dyn Error>>) {
        let mut consumed = 0usize;
        for block in in_buf.chunks_exact(chunk_len * channels) {
            let input_adapter = match InterleavedSlice::new(block, channels, chunk_len) {
                Ok(a) => a,
                Err(e) => return (consumed, Some(Box::from(e))),
            };
            let mut output_adapter =
                match InterleavedSlice::new_mut(out_temp_buf, channels, max_out_frames) {
                    Ok(a) => a,
                    Err(e) => return (consumed, Some(Box::from(e))),
                };
            match resampler.process_into_buffer(&input_adapter, &mut output_adapter, None) {
                Ok((_, frames_out)) => {
                    out.extend_from_slice(&out_temp_buf[..frames_out * channels]);
                    consumed += chunk_len * channels;
                }
                Err(e) => return (consumed, Some(Box::from(e))),
            }
        }
        (consumed, None)
    }

    /// 直通 = 出错 / 格式相同 / 无重采样器
    fn is_passthrough(&self) -> bool {
        self.error.is_some() || self.output_format == self.input_format || self.resampler.is_none()
    }
}

impl PreEffect for ResamplerEffect {
    fn init(
        &mut self,
        input_format: AudioFormat,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        if input_format.channels == 0 {
            return Err("channels must be > 0".into());
        }

        self.in_buf.clear();

        // 直通：保留 resampler 供后续切回复用，不 take()
        if self.output_format == input_format {
            self.input_format = input_format;
            return Ok(());
        }

        let channels = input_format.channels as usize;
        let plan = ResamplePlan::compute_plan(
            input_format.sample_rate,
            self.output_format.sample_rate,
            32768,
            0.10,
            2048,
            32768,
        );
        let chunk_len = plan.block_frames as usize;

        // 命中：已有 resampler 且输入格式相同（output_format 固定），复用 + reset
        if self.resampler.is_some() && self.resampler_input_format == Some(input_format) {
            self.input_format = input_format;
            self.in_buf.reserve(chunk_len * channels * 2);
            if let Some(r) = &mut self.resampler {
                r.reset();
            }
            self.error.take();
            return Ok(());
        }

        // 未命中：先构造到局部，成功后再一次性提交，保证 init 原子性
        let resampler = Fft::<f32>::new(
            input_format.sample_rate as usize,
            self.output_format.sample_rate as usize,
            chunk_len,
            channels,
            FixedSync::Input,
        )?;
        let max_out_frames = resampler.output_frames_max();
        let mut out_temp_buf = Vec::new();
        out_temp_buf.resize(max_out_frames * channels, 0.0);
        let mut in_buf = VecDeque::new();
        in_buf.reserve(chunk_len * channels * 2);

        // 提交
        self.input_format = input_format;
        self.chunk_len = chunk_len;
        self.max_out_frames = max_out_frames;
        self.out_temp_buf = out_temp_buf;
        self.in_buf = in_buf;
        self.resampler = Some(resampler);
        self.resampler_input_format = Some(input_format);
        self.error.take();
        Ok(())
    }

    fn output_format(&self) -> AudioFormat {
        self.output_format
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        if self.error.is_some() {
            out.clear();
            return 0;
        }

        if self.output_format == self.input_format || self.resampler.is_none() {
            out.clear();
            out.reserve_exact(input.len());
            out.extend_from_slice(input);
            return out.len();
        }

        let channels = self.input_format.channels as usize;
        if channels == 0 {
            out.clear();
            return 0;
        }
        let chunk_len = self.chunk_len;
        let channels_chunk_len = chunk_len * channels;
        let max_out_frames = self.max_out_frames;

        self.in_buf.reserve(input.len());
        self.in_buf.extend(input.iter().copied());
        if self.in_buf.len() < channels_chunk_len {
            out.clear();
            return 0;
        }

        let drain_len = (self.in_buf.len() / channels_chunk_len) * channels_chunk_len;
        out.clear();
        out.reserve_exact((drain_len / channels_chunk_len) * max_out_frames * channels);

        let Some(resampler) = self.resampler.as_mut() else {
            // 防御：resampler 不可用时兜底透传，不 panic
            out.clear();
            out.reserve_exact(input.len());
            out.extend_from_slice(input);
            return out.len();
        };
        let (consumed, err) = {
            let contiguous = self.in_buf.make_contiguous();
            Self::process_inner(
                &contiguous[..drain_len],
                &mut self.out_temp_buf,
                out,
                chunk_len,
                channels,
                max_out_frames,
                resampler,
            )
        };

        self.in_buf.drain(..consumed);

        if let Some(e) = err {
            self.set_error(e);
            return out.len();
        }
        out.len()
    }

    fn input_eof(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        if self.is_passthrough() {
            out.clear();
            out.reserve_exact(self.in_buf.len() + input.len());
            out.extend(self.in_buf.drain(..));
            out.extend_from_slice(input);
            return out.len();
        }

        let channels = self.input_format.channels as usize;
        if channels == 0 {
            out.clear();
            return 0;
        }
        let chunk_len = self.chunk_len;
        let channels_chunk_len = chunk_len * channels;
        let max_out_frames = self.max_out_frames;

        self.in_buf.reserve(input.len());
        self.in_buf.extend(input.iter().copied());
        let data_len = self.in_buf.len();
        let this_input_len = (data_len / channels_chunk_len + 1) * channels_chunk_len;
        self.in_buf.resize(this_input_len, 0.0);

        out.clear();
        out.reserve_exact((this_input_len / channels_chunk_len) * max_out_frames * channels);

        let Some(resampler) = self.resampler.as_mut() else {
            // 防御：不可达，但用透传兜底，绝不 panic、绝不吞数据
            out.clear();
            out.reserve_exact(self.in_buf.len() + input.len());
            out.extend(self.in_buf.drain(..));
            out.extend_from_slice(input);
            return out.len();
        };
        let (consumed, err) = {
            let contiguous = self.in_buf.make_contiguous();
            Self::process_inner(
                &contiguous[..this_input_len],
                &mut self.out_temp_buf,
                out,
                chunk_len,
                channels,
                max_out_frames,
                resampler,
            )
        };

        if let Some(e) = err {
            self.set_error(e);
            out.extend(self.in_buf.drain(consumed..));
            return out.len();
        }
        self.in_buf.clear();
        out.len()
    }

    fn reset(&mut self, input_format: AudioFormat) {
        if let Err(e) = self.init(input_format) {
            self.set_error(e);
        }
    }

    /*    fn try_reset(&mut self, input_format: AudioFormat) -> Result<(), Box<dyn Error + 'static>> {
        self.init(input_format)
    }*/

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        if enabled && self.error.is_some() {
            return;
        }
        self.enabled = enabled;
    }

    fn name(&self) -> &'static str {
        "ResamplerEffect"
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResamplePlan {
    pub block_frames: u32,   // 每声道输入帧数（喂给 process）
    pub out_frames_max: u32, // 每声道输出上限（分配输出缓冲用，含余量）
    pub out_fft_equiv: u32,  // 实际输出侧等效窗口（诊断）
    pub t_play_sec: f64,     // 单次处理对应的播放时长
    pub period: u32,         // 输入侧最小周期（诊断）
}
impl ResamplePlan {
    fn next_pow2(x: u32) -> u32 {
        if x <= 1 {
            return 1;
        }
        x.checked_next_power_of_two().unwrap_or(u32::MAX)
    }

    /// 完全动态计算重采样块大小。
    ///
    /// - `fin` / `fout`：任意输入/输出采样率（Hz），运行时决定
    /// - `target_out_fft`：输出侧期望等效窗口，解码线程内用 32768，独立线程用 65536
    /// - `t_play_max_sec`：单次处理允许的最长播放时长，解码线程内 0.08~0.15
    /// - `n_min` / `n_max`：输入块帧数硬上下限，如 1024 / 32768
    pub fn compute_plan(
        fin: u32,
        fout: u32,
        target_out_fft: u32,
        t_play_max_sec: f64,
        n_min: u32,
        n_max: u32,
    ) -> ResamplePlan {
        const MAX_ALIGN: u32 = 1024;
        assert!(
            fin > 0 && fout > 0 && target_out_fft > 0,
            "rates must be > 0"
        );
        assert!(n_min > 0 && n_min <= n_max, "invalid bounds");
        assert!(t_play_max_sec > 0.0, "t_play_max_sec must be > 0");

        // 1) 两约束取小：质量下界 vs 播放时长上界
        let n_quality = f64::from(target_out_fft) * f64::from(fin) / f64::from(fout);
        let n_time = t_play_max_sec * f64::from(fin);
        let n_raw = n_quality.min(n_time).ceil().max(1.0) as u32;

        // 2) 自适应对齐
        let g = Self::gcd_u32(fin, fout); // 需要 Rust 1.67+，或手写 gcd
        let period = fin / g; // 44.1k→384k: 147；48k→384k: 1
        let mut n = if period <= MAX_ALIGN {
            n_raw.div_ceil(period).saturating_mul(period) // div_ceil 需要 1.73+
        } else {
            Self::next_pow2(n_raw)
        };

        // 3) 硬上下限
        n = n.clamp(n_min, n_max);

        // 4) 诊断与输出缓冲上限
        let ratio = f64::from(fout) / f64::from(fin);
        let out_fft_equiv = (f64::from(n) * ratio).round() as u32;
        let t_play_sec = f64::from(n) / f64::from(fin);
        let out_frames_max = ((f64::from(n) * ratio).ceil() as u32).saturating_add(64); // 滤波器群延迟余量

        ResamplePlan {
            block_frames: n,
            out_frames_max,
            out_fft_equiv,
            t_play_sec,
            period,
        }
    }

    fn gcd_u32(mut a: u32, mut b: u32) -> u32 {
        while b != 0 {
            let t = a % b;
            a = b;
            b = t;
        }
        a
    }
}
