use crate::command::music_play_test::decoder::{Decoder, DecoderResult};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Error, SAMPLE_RATE_48K, Stream, StreamConfig};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tracing::error;
use water_ball_tool_lib::ring_buffer_wait::RingBufferWait;

const BUF_LEN_U32: u32 = 1024;
const BUF_LEN: usize = BUF_LEN_U32 as usize;
const SAMPLE_RATE: u32 = SAMPLE_RATE_48K;
const SAMPLE_RATE_F64: f64 = SAMPLE_RATE as f64;

#[derive(Clone)]
pub(super) struct Audio {
    ring_buf: RingBufferWait<f32>,
    decoder: Arc<Mutex<Option<Decoder>>>,
    volume: Arc<AtomicU32>,
    stream_config: Arc<StreamConfig>,
    stream: Arc<Stream>,
    played_samples: Arc<AtomicU64>,
}
impl Audio {
    fn create_stream(
        device: Device,
        config: StreamConfig,
        mut ring_buf: RingBufferWait<f32>,
        volume: Arc<AtomicU32>,
        played_samples: Arc<AtomicU64>,
    ) -> Result<Stream, Error> {
        device.build_output_stream(
            config,
            move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                let len = ring_buf.try_read(data).unwrap_or(0);
                for datum in &mut data[..len] {
                    *datum *= f32::from_bits(volume.load(Ordering::Relaxed));
                }
                played_samples.fetch_add(len as u64, Ordering::Relaxed);
                data[len..].fill(0.0);
            },
            |err| error!("流错误: {err}"),
            None,
        )
    }
}
impl Audio {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            panic!("No output device available");
        };
        let config = device.default_output_config()?;
        let channels = config.channels();

        let stream_config = Arc::new(StreamConfig {
            channels,
            sample_rate: SAMPLE_RATE,
            buffer_size: cpal::BufferSize::Fixed(BUF_LEN_U32),
        });
        // 在你的播放器初始化阶段
        let ring_capacity = SAMPLE_RATE as usize * channels as usize;
        let ring_buf = RingBufferWait::new_max_len(ring_capacity)?;

        let decoder = Arc::new(Mutex::new(None));
        let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let played_samples = Arc::new(AtomicU64::new(0));

        let stream = Arc::new(Self::create_stream(
            device,
            *stream_config,
            ring_buf.clone(),
            volume.clone(),
            played_samples.clone(),
        )?);
        Ok(Self {
            ring_buf,
            decoder,
            volume,
            stream_config,
            stream,
            played_samples,
        })
    }

    pub fn play(&mut self) -> Result<(), Error> {
        self.stream.play()
    }

    pub fn pause(&mut self) -> Result<(), Error> {
        self.stream.pause()
    }

    pub fn ring_buffer(&self) -> &RingBufferWait<f32> {
        &self.ring_buf
    }

    pub fn stop(self) -> DecoderResult<()> {
        if let Some(decoder) = self
            .decoder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            decoder.stop()?;
        }
        Ok(())
    }

    pub fn set_decoder(&mut self, decoder: Decoder) -> DecoderResult<()> {
        let mut decoder = decoder;
        let mut decoder_lock = self.decoder.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(decoder) = decoder_lock.take() {
            decoder.stop()?;
        }
        decoder.run()?;
        decoder_lock.replace(decoder);
        self.played_samples.store(0, Ordering::Relaxed);
        Ok(())
    }
}
