use crate::command::music_play::effect::{AudioFormat, PreEffect};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use std::cmp::PartialEq;
use std::error::Error;

const RESAMPLER_CHUNK_LEN: usize = 1024;

pub struct ResamplerEffect {
    enabled: bool,
    error: Option<String>,
    resampler: Option<Fft<f32>>,
    in_buf: Vec<f32>,
    out_temp_buf: Vec<f32>,
    input_format: AudioFormat,
    output_format: AudioFormat,
    max_out_frames: usize,
    chunk_len: usize,
}

impl PreEffect for ResamplerEffect {
    fn init(&mut self, input_format: AudioFormat) -> Result<(), Box<dyn Error + 'static>> {
        self.input_format = input_format;
        self.in_buf.clear();
        if self.output_format == input_format {
            self.resampler.take();
            return Ok(());
        }
        let channels = self.input_format.channels as usize;
        self.chunk_len = ResamplePlan::compute_plan(
            input_format.sample_rate,
            self.output_format.sample_rate,
            32768,
            0.10,
            2048,
            32768,
        )
        .out_frames_max as usize;
        let resampler = Fft::<f32>::new(
            self.input_format.sample_rate as usize,
            self.output_format.sample_rate as usize,
            self.chunk_len, // 如 1024
            channels,
            FixedSync::Input,
        )?;
        self.max_out_frames = resampler.output_frames_max();
        self.in_buf.reserve_exact(self.max_out_frames * 2);
        self.out_temp_buf.resize(self.max_out_frames * 2, 0.0);
        self.resampler = Some(resampler);
        self.error.take();
        Ok(())
    }

    fn output_format(&self) -> AudioFormat {
        self.output_format
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        let Some(resampler) = &mut self.resampler else {
            return if self.error.is_some() {
                0
            } else {
                out.clear();
                out.reserve_exact(input.len());
                out.extend_from_slice(input);
                out.len()
            };
        };
        let chunk_len = self.chunk_len;
        let channels = self.input_format.channels as usize;
        let channels_chunk_len = self.chunk_len * channels;
        let max_out_frames = self.max_out_frames;
        let in_buf = &mut self.in_buf;
        let out_temp_buf = &mut self.out_temp_buf;
        in_buf.extend_from_slice(input);
        if in_buf.len() < channels_chunk_len {
            return 0;
        }
        let drain_len = (in_buf.len() / channels_chunk_len) * channels_chunk_len;
        out.clear();
        out.reserve_exact(drain_len);
        if let Err(e) = Self::process_inner(
            in_buf,
            out_temp_buf,
            out,
            chunk_len,
            channels,
            max_out_frames,
            resampler,
        ) {
            self.set_error(e);
        }
        self.in_buf.drain(..drain_len);
        out.len()
    }

    fn reset(&mut self, input_format: AudioFormat) {
        if let Err(e) = self.init(input_format) {
            self.set_error(e);
        }
    }

    fn input_eof(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        let Some(resampler) = &mut self.resampler else {
            out.clear();
            out.reserve_exact(input.len());
            out.extend_from_slice(input);
            return out.len();
        };
        let channels = self.input_format.channels as usize;
        //基本大小
        let chunk_len = self.chunk_len;
        let channels_chunk_len = self.chunk_len * channels;
        let max_out_frames = self.max_out_frames;
        let in_buf = &mut self.in_buf;
        let out_temp_buf = &mut self.out_temp_buf;
        let data_len = in_buf.len() + input.len();
        //这次重采样所需大小
        let this_input_len = (data_len / channels_chunk_len + 1) * channels_chunk_len;

        in_buf.extend_from_slice(input);
        in_buf.resize(this_input_len, 0.0);
        if let Err(e) = Self::process_inner(
            in_buf,
            out_temp_buf,
            out,
            chunk_len,
            channels,
            max_out_frames,
            resampler,
        ) {
            self.set_error(e);
            return 0;
        }
        self.in_buf.drain(..this_input_len);
        out.len()
    }

    fn try_reset(&mut self, input_format: AudioFormat) -> Result<(), Box<dyn Error + 'static>> {
        self.init(input_format)
    }

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
impl ResamplerEffect {
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
    ) -> Result<(), Box<dyn Error>> {
        for block in in_buf.chunks_exact(chunk_len * channels) {
            let input_adapter = InterleavedSlice::new(block, channels, chunk_len)?;
            let mut output_adapter =
                InterleavedSlice::new_mut(out_temp_buf, channels, max_out_frames)?;
            let (_, frames_out) =
                resampler.process_into_buffer(&input_adapter, &mut output_adapter, None)?;
            out.extend_from_slice(&out_temp_buf[..frames_out * channels]);
        }
        Ok(())
    }
}
impl ResamplerEffect {
    pub fn new(output_format: AudioFormat) -> Self {
        Self {
            enabled: true,
            error: None,
            resampler: None,
            in_buf: Vec::new(),
            out_temp_buf: Vec::new(),
            input_format: AudioFormat::default(),
            output_format,
            max_out_frames: 0,
            chunk_len: RESAMPLER_CHUNK_LEN,
        }
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
