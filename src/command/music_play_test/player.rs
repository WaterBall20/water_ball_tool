use crate::command::music_play_test::decoder::{Decoder, DecoderResult};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Error, SAMPLE_RATE_48K, Stream, StreamConfig};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::error;
use water_ball_tool_lib::ring_buffer_wait::RingBufferWait;

const BUF_LEN_U32: u32 = 1024;
const BUF_LEN: usize = BUF_LEN_U32 as usize;
const SAMPLE_RATE: u32 = SAMPLE_RATE_48K;
const SAMPLE_RATE_F64: f64 = SAMPLE_RATE as f64;

#[derive(Clone)]
pub(super) struct Player {
    played_samples: Arc<AtomicU64>,
    sound_engine: SoundEngine,
    content: Content,
}

#[derive(Clone)]
struct SoundEngine {
    ring_buffer: RingBufferWait<f32>,
    decoder: Arc<Mutex<Option<Decoder>>>,
    stream_config: Arc<StreamConfig>,
    stream: Arc<Stream>,
}

#[derive(Clone)]
struct Content {
    is_playing: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
}

impl Player {
    fn create_stream(
        device: &Device,
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
impl Player {
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
        let ring_buffer = RingBufferWait::new_max_len(ring_capacity)?;

        let decoder = Arc::new(Mutex::new(None));
        let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let played_samples = Arc::new(AtomicU64::new(0));
        let is_playing = Arc::new(AtomicBool::new(false));

        let stream = Arc::new(Self::create_stream(
            &device,
            *stream_config,
            ring_buffer.clone(),
            volume.clone(),
            played_samples.clone(),
        )?);
        Ok(Self {
            sound_engine: SoundEngine {
                ring_buffer,
                decoder,
                stream_config,
                stream,
            },
            content: Content { is_playing, volume },
            played_samples,
        })
    }

    pub fn is_playing(&self) -> bool {
        self.content.is_playing.load(Ordering::Relaxed)
    }

    pub fn is_paused(&self) -> bool {
        !self.is_playing()
    }

    pub fn get_pos(&self) -> Duration {
        Duration::from_millis(
            self.played_samples.load(Ordering::Relaxed)
                / self.sound_engine.stream_config.channels as u64
                / u64::from(self.sound_engine.stream_config.sample_rate),
        )
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.content.volume.load(Ordering::Relaxed))
    }

    pub fn set_volume(&self, volume: f32) {
        self.content
            .volume
            .store(volume.to_bits(), Ordering::Relaxed);
    }

    pub fn play(&self) -> Result<(), Error> {
        self.content.is_playing.store(true, Ordering::Relaxed);
        self.sound_engine.stream.play()
    }

    pub fn pause(&self) -> Result<(), Error> {
        self.content.is_playing.store(false, Ordering::Relaxed);
        self.sound_engine.stream.pause()
    }

    pub fn ring_buffer(&self) -> &RingBufferWait<f32> {
        &self.sound_engine.ring_buffer
    }

    pub fn stop(self) -> DecoderResult<()> {
        if let Some(decoder) = self
            .sound_engine
            .decoder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            decoder.stop()?;
        }
        Ok(())
    }

    pub fn set_decoder(&mut self, decoder: Decoder) -> DecoderResult<()> {
        let mut decoder = decoder;
        let mut decoder_lock = self
            .sound_engine
            .decoder
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(decoder) = decoder_lock.take() {
            decoder.stop()?;
        }
        decoder.run()?;
        decoder_lock.replace(decoder);
        self.played_samples.store(0, Ordering::Relaxed);
        Ok(())
    }

    pub fn try_seek(&mut self, pos: Duration) -> Result<(), Box<dyn std::error::Error>> {
        let mut decoder = self
            .sound_engine
            .decoder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(decoder) = &mut *decoder {
            let pos = decoder.try_seek(pos)?.actual_ts;
            todo!()
        }
        Ok(())
    }
}
