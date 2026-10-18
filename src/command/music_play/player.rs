use crate::command::music_play::decoder::{
    Decoder, DecoderEventDirectSink, DecoderEventSink, DecoderEventType, DecoderResult,
};
use crate::command::music_play::effect::{
    AudioFormat, Decision, PlayCmd, PostEffect, PostEffectChain,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use tracing::error;
use water_ball_tool_lib::ring_buffer_wait::RingBufferWait;

#[derive(Clone)]
pub(super) struct Player {
    played_samples: Arc<AtomicU64>,
    sound_engine: SoundEngine,
    content: Content,
    event_content: EventContent,
    rx_cmd_thread: Arc<JoinHandle<()>>,
}

#[derive(Clone)]
struct SoundEngine {
    ring_buffer: RingBufferWait<f32>,
    decoder: Arc<Mutex<Option<Decoder>>>,
    stream_config: Arc<StreamConfig>,
    stream: Arc<Stream>,
    post_effect_chain: Arc<Mutex<PostEffectChain>>,
}

#[derive(Clone)]
struct Content {
    is_playing: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    tx_cmd: Sender<PlayCmd>,
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
        content: Content,
        played_samples: Arc<AtomicU64>,
        event_content: EventContent,
        rx_cmd: Receiver<PlayCmd>,
        tx_done: Sender<PlayCmd>,
        effect_chain: Arc<Mutex<PostEffectChain>>,
    ) -> Result<Stream, cpal::Error> {
        let mut scratch = Vec::new();
        device.build_output_stream(
            config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                //暂停处理
                let volume = f32::from_bits(content.volume.load(Ordering::Relaxed));
                if !content.is_playing.load(Ordering::Acquire) || volume == 0.0 {
                    data.fill(0.0);
                    return;
                }
                let len = ring_buf.read(data);

                //效果处理
                if let Ok(mut effect_chain) = effect_chain.try_lock() {
                    // 1. 收指令，做决策（绝不阻塞、不分配）
                    while let Ok(cmd) = rx_cmd.try_recv() {
                        if matches!(effect_chain.dispatch_transport(cmd), Decision::Pass) {
                            let _ = tx_done.try_send(cmd);
                        }
                    }
                    // 2. 处理音频
                    effect_chain.process(data[len..].as_mut());
                    // 3. 上报已完成的延迟指令（Vec 预分配，复用）
                    scratch.clear();
                    effect_chain.poll_transport(&mut scratch);
                    for cmd in scratch.drain(..) {
                        let _ = tx_done.try_send(cmd);
                    }
                }

                //音量处理
                if (volume - 1.0).abs() > 0.0 {
                    for datum in &mut data[..len] {
                        *datum *= volume;
                    }
                }

                //进度计数
                played_samples.fetch_add(len as u64, Ordering::Relaxed);
                data[len..].fill(0.0);
                //检查解码是否EOF
                let decoder_eof = &event_content.decoder_eof;
                let decoder_eof_pos = &event_content.decoder_eof_pos;

                if decoder_eof.load(Ordering::Relaxed) {
                    //安全减法
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
}
impl Player {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            panic!("No output device available");
        };
        let config = device.default_output_config()?;
        let channels = config.channels();
        let sample_rate = config.sample_rate();

        let stream_config = Arc::new(config.config());
        // 在你的播放器初始化阶段
        let ring_buffer =
            RingBufferWait::new_max_len(sample_rate as usize * channels as usize * 5)?;
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

        let post_effect_chain = Arc::new(Mutex::new(PostEffectChain::new(AudioFormat::new(
            channels,
            sample_rate,
        ))));

        // 主线程 → 音频线程：transport 指令
        let (tx_cmd, rx_cmd) = crossbeam_channel::bounded::<PlayCmd>(1);
        // 音频线程 → 主线程：已放行的指令
        let (tx_done, rx_done) = crossbeam_channel::bounded::<PlayCmd>(8);

        let content = Content {
            is_playing: is_playing.clone(),
            volume,
            tx_cmd,
        };

        let stream = Arc::new(Self::create_stream(
            &device,
            *stream_config,
            ring_buffer.clone(),
            content.clone(),
            played_samples.clone(),
            event_content.clone(),
            rx_cmd,
            tx_done,
            post_effect_chain.clone(),
        )?);

        let rx_cmd_thread = {
            let stream = stream.clone();
            Arc::new(
                thread::Builder::new()
                    .name("rx_cmd".to_string())
                    .spawn(move || {
                        for cmd in rx_done {
                            match cmd {
                                PlayCmd::Play => {
                                    is_playing.store(true, Ordering::Relaxed);
                                    if let Err(e) = stream.play() {
                                        error!("播放发生错误： {e}");
                                    }
                                }
                                PlayCmd::Pause => {
                                    is_playing.store(false, Ordering::Relaxed);
                                    if let Err(e) = stream.pause() {
                                        error!("暂停发生错误{e}");
                                    }
                                }
                                PlayCmd::Seek(_) => {
                                    todo!();
                                }
                            }
                        }
                    })
                    .expect(r#"无法创建"rx_cmd"线程"#),
            )
        };

        Ok(Self {
            sound_engine: SoundEngine {
                ring_buffer,
                decoder,
                stream_config,
                stream,
                post_effect_chain,
            },
            content,
            played_samples,
            event_content,
            rx_cmd_thread,
        })
    }

    pub fn is_playing(&self) -> bool {
        self.content.is_playing.load(Ordering::Acquire)
    }

    pub fn is_paused(&self) -> bool {
        !self.is_playing()
    }

    pub fn get_pos(&self) -> Duration {
        Duration::from_millis(
            self.played_samples.load(Ordering::Acquire) * 1000
                / u64::from(self.sound_engine.stream_config.channels)
                / u64::from(self.sound_engine.stream_config.sample_rate),
        )
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.content.volume.load(Ordering::Acquire))
    }

    pub fn set_volume(&self, volume: f32) {
        //安全范围处理
        let volume = if volume > 1.0 {
            (volume / 100.0).min(1.0)
        } else if volume < 0.0 {
            0.0
        } else {
            volume
        };
        self.content
            .volume
            .store(volume.to_bits(), Ordering::Relaxed);
    }

    pub fn play(&self) -> Result<(), Box<dyn std::error::Error>> {
        _ = self.content.tx_cmd.try_send(PlayCmd::Play);
        self.content.is_playing.store(true, Ordering::Relaxed);
        self.sound_engine.stream.play()?;
        Ok(())
    }

    pub fn pause(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.content.tx_cmd.send(PlayCmd::Pause)?;
        Ok(())
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

    pub fn try_seek(&mut self, pos: Duration) -> Result<(), Box<dyn std::error::Error>> {
        todo!();
        let mut decoder = self
            .sound_engine
            .decoder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(decoder) = &mut *decoder {
            let pos = decoder.try_seek(pos)?.actual_ts;
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

    pub fn add_audio_post_effect<E>(&mut self, effect: E)
    where
        E: PostEffect + Send + 'static,
    {
        self.sound_engine
            .post_effect_chain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(effect);
    }
}

pub trait PlayerEventSink: Send + 'static {
    fn emit(&mut self, event: Arc<PlayerEvent>);
    fn new<F>(f: F) -> Self
    where
        F: Fn(&PlayerEvent) + Send + 'static,
        Self: Sized;
}

pub struct PlayerEventChannelSink(mpsc::Sender<Arc<PlayerEvent>>);

impl PlayerEventSink for PlayerEventChannelSink {
    fn emit(&mut self, event: Arc<PlayerEvent>) {
        let _ = self.0.send(event);
    }

    fn new<F>(f: F) -> Self
    where
        F: Fn(&PlayerEvent) + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel::<Arc<PlayerEvent>>();
        thread::Builder::new()
            .name("player_event_channel_sink".to_string())
            .spawn(move || {
                for event in receiver {
                    f(&event);
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
