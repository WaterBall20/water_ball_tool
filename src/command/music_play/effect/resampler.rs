use crate::command::music_play::effect::{AudioFormat, PreEffect};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
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
    fn init(&mut self, input_format: AudioFormat) -> Result<(), Box<(dyn Error + 'static)>> {
        self.input_format = input_format;
        let channels = self.input_format.channels as usize;
        let resampler = Fft::<f32>::new(
            self.input_format.sample_rate as usize,
            self.output_format.sample_rate as usize,
            self.chunk_len, // 如 1024
            channels,
            FixedSync::Input,
        )?;
        self.max_out_frames = resampler.output_frames_max();
        self.in_buf.clear();
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
            return 0;
        };
        let channels = self.input_format.channels as usize;
        let chunk_len = self.chunk_len * channels;
        self.in_buf.extend_from_slice(input);
        if self.in_buf.len() < chunk_len {
            return 0;
        }
        let drain_len = (input.len() / chunk_len) * chunk_len;
        out.clear();
        out.reserve_exact(drain_len);
        for block in self.in_buf.chunks_exact(chunk_len) {
            let input_adapter = match InterleavedSlice::new(block, channels, self.chunk_len) {
                Ok(v) => v,
                Err(e) => {
                    self.set_error(Box::new(e));
                    return 0;
                }
            };
            let mut output_adapter = match InterleavedSlice::new_mut(
                &mut self.out_temp_buf,
                channels,
                self.max_out_frames,
            ) {
                Ok(v) => v,
                Err(e) => {
                    self.set_error(Box::new(e));
                    return 0;
                }
            };
            let (_, frames_out) =
                match resampler.process_into_buffer(&input_adapter, &mut output_adapter, None) {
                    Ok(v) => v,
                    Err(e) => {
                        self.set_error(Box::new(e));
                        return 0;
                    }
                };
            out.extend_from_slice(&self.out_temp_buf[..frames_out * channels]);
        }
        self.in_buf.drain(..drain_len);
        out.len()
    }

    fn reset(&mut self, input_format: AudioFormat) {
        if let Err(e) = self.init(input_format) {
            self.set_error(e);
        }
    }

    fn try_reset(&mut self, input_format: AudioFormat) -> Result<(), Box<(dyn Error + 'static)>> {
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
}
impl ResamplerEffect {
    fn set_error(&mut self, error: Box<dyn Error + 'static>) {
        self.enabled = false;
        self.error = Some(error.to_string());
    }
}
impl ResamplerEffect {
    pub fn new(output_format: AudioFormat) -> Self {
        Self {
            enabled: false,
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
