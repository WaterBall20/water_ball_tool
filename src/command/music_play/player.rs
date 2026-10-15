use crate::command::music_play::decoder::{
    Decoder, DecoderEventDirectSink, DecoderEventSink, DecoderEventType, DecoderResult,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Error, SAMPLE_RATE_48K, Stream, StreamConfig};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use tracing::{error, event};
use water_ball_tool_lib::ring_buffer_wait::RingBufferWait;

const BUF_LEN_U32: u32 = 1024;
const BUF_LEN: usize = BUF_LEN_U32 as usize;
const SAMPLE_RATE: u32 = SAMPLE_RATE_48K;
const SAMPLE_RATE_F64: f64 = SAMPLE_RATE as f64;

const RING_BUF_LEN: usize = SAMPLE_RATE as usize;

#[derive(Clone)]
pub(super) struct Player {
    played_samples: Arc<AtomicU64>,
    sound_engine: SoundEngine,
    content: Content,
    event_content: EventContent,
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

/// 事件控制
#[derive(Clone)]
struct EventContent {
    decoder_eof: Arc<AtomicBool>,
    decoder_eof_pos: Arc<AtomicUsize>,
    list: Arc<Mutex<Vec<Box<dyn PlayerEventSink>>>>,
}

impl Player {
    fn create_stream(
        device: &Device,
        config: StreamConfig,
        mut ring_buf: RingBufferWait<f32>,
        volume: Arc<AtomicU32>,
        played_samples: Arc<AtomicU64>,
        event_content: EventContent,
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
                //检查解码是否EOF
                let decoder_eof = &event_content.decoder_eof;
                let decoder_eof_pos = &event_content.decoder_eof_pos;

                if decoder_eof.load(Ordering::Relaxed) {
                    if len > decoder_eof_pos.load(Ordering::Acquire) {
                        decoder_eof_pos.store(0, Ordering::Relaxed);
                    } else {
                        decoder_eof_pos.fetch_sub(len, Ordering::Relaxed);
                    }
                    if decoder_eof_pos.load(Ordering::Acquire) == 0 {
                        //触发事件和重置
                        decoder_eof.store(false, Ordering::Relaxed);
                        played_samples.store(0, Ordering::Relaxed);
                        Self::emit(
                            PlayerEvent {
                                result: Ok(PlayerEventType::TrackEnded),
                            },
                            &event_content.list,
                        );
                    }
                }
            },
            |err| error!("流错误: {err}"),
            None,
        )
    }

    fn emit(event: PlayerEvent, list: &Arc<Mutex<Vec<Box<dyn PlayerEventSink>>>>) {
        let event = Arc::new(event);
        let mut list = list.lock().expect("list is poisoned");
        for sink in list.iter_mut() {
            sink.emit(event.clone());
        }
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
        let ring_buffer = RingBufferWait::new_max_len(RING_BUF_LEN)?;
        let decoder = Arc::new(Mutex::new(None));
        let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let played_samples = Arc::new(AtomicU64::new(0));
        let is_playing = Arc::new(AtomicBool::new(false));

        let decoder_eof = Arc::new(AtomicBool::new(false));
        let decoder_eof_pos = Arc::new(AtomicUsize::new(0));
        let list = Arc::new(Mutex::new(Vec::new()));

        let event_content = EventContent {
            decoder_eof,
            decoder_eof_pos,
            list,
        };

        let stream = Arc::new(Self::create_stream(
            &device,
            *stream_config,
            ring_buffer.clone(),
            volume.clone(),
            played_samples.clone(),
            event_content.clone(),
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
            event_content,
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
            self.played_samples.load(Ordering::Relaxed) * 1000
                / u64::from(self.sound_engine.stream_config.channels)
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

    pub fn stop(mut self) -> DecoderResult<()> {
        self.try_stop()
    }

    pub fn try_stop(&mut self) -> DecoderResult<()> {
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
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(decoder) = decoder_lock.take() {
            decoder.stop()?;
        }
        self.add_decoder_event_inner(&mut decoder);
        decoder.run()?;
        decoder_lock.replace(decoder);
        self.played_samples.store(0, Ordering::Relaxed);
        Ok(())
    }

    pub fn set_decoder_no_clear(&mut self, decoder: Decoder) -> DecoderResult<()> {
        let mut decoder = decoder;
        let mut decoder_lock = self
            .sound_engine
            .decoder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        decoder_lock.take();
        self.add_decoder_event_inner(&mut decoder);
        decoder.run()?;
        decoder_lock.replace(decoder);
        Ok(())
    }

    fn add_decoder_event_inner(&self, decoder: &mut Decoder) {
        let decoder_eof_pos = self.event_content.decoder_eof_pos.clone();
        let decoder_eof = self.event_content.decoder_eof.clone();
        let ring_buf = self.sound_engine.ring_buffer.clone();
        decoder.add_event(Box::new(DecoderEventDirectSink::new(move |e| {
            if let Ok(e) = e.result()
                && let DecoderEventType::Eof = e
            {
                decoder_eof_pos.store(ring_buf.len(), Ordering::Relaxed);
                decoder_eof.store(true, Ordering::Relaxed);
            }
        })));
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

    pub fn get_sample_rate(&self) -> u32 {
        self.sound_engine.stream_config.sample_rate
    }

    pub fn get_decoder_eof(&self) -> bool {
        self.event_content.decoder_eof.load(Ordering::Acquire)
    }

    /// 清理解码结束剩余在缓冲区的数据并重置
    pub fn clear_decoder_eof_buf(&self) {
        if self.get_decoder_eof() && self.event_content.decoder_eof_pos.load(Ordering::Acquire) != 0
        {
            let mut ring_buf = self
                .sound_engine
                .ring_buffer
                .lock()
                .expect("ring buffer is poisoned");
            let remove_len = ring_buf.len();
            for _ in 0..remove_len {
                _ = ring_buf.pop_front();
            }
            self.event_content
                .decoder_eof
                .store(false, Ordering::Release);
            self.event_content
                .decoder_eof_pos
                .store(0, Ordering::Release);
        }
    }

    pub fn add_event(&mut self, event: Box<dyn PlayerEventSink>) {
        self.event_content
            .list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

pub trait PlayerEventSink: Send + 'static {
    fn emit(&mut self, event: Arc<PlayerEvent>);
    fn new<F>(f: F) -> Self
    where
        F: Fn(&PlayerEvent) + Send + 'static,
        Self: Sized;
}

pub struct PlayerEventChannelSink(Sender<Arc<PlayerEvent>>);

impl PlayerEventSink for PlayerEventChannelSink {
    fn emit(&mut self, event: Arc<PlayerEvent>) {
        let _ = self.0.send(event);
    }

    fn new<F>(f: F) -> Self
    where
        F: Fn(&PlayerEvent) + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel::<Arc<PlayerEvent>>();
        std::thread::Builder::new()
            .name("player_event_channel_sink".to_string())
            .spawn(move || {
                for event in receiver {
                    f(&*event);
                }
            })
            .expect("创建线程失败");
        Self(sender)
    }
}

#[derive(Debug, Clone)]
pub(super) struct PlayerEvent {
    result: Result<PlayerEventType, PlayerEventError>,
}

impl PlayerEvent {
    pub fn result(&self) -> &Result<PlayerEventType, PlayerEventError> {
        &self.result
    }
    pub fn get_result(self) -> Result<PlayerEventType, PlayerEventError> {
        self.result
    }
}

#[derive(Debug, Clone)]
pub(super) enum PlayerEventType {
    TrackEnded,
}

#[derive(Debug, Clone)]
pub(super) struct PlayerEventError {
    message: String,
    kind: PlayerEventErrorKind,
}

#[derive(Debug, Clone)]
pub(super) enum PlayerEventErrorKind {}
