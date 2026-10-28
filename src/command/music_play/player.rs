use crate::command::music_play::decoder::{Decoder, DecoderResult};
use crate::command::music_play::effect::cross_bridge::CrossBridgeEffect;
use crate::command::music_play::effect::resampler::ResamplerEffect;
use crate::command::music_play::effect::runtime::{
    PreEffectRuntime, PreEffectRuntimeEventDirectSink, PreEffectRuntimeEventSink,
    PreEffectRuntimeEventType,
};
use crate::command::music_play::effect::{
    AudioData, AudioFormat, Decision, PlayCmd, PostEffect, PostEffectChain,
};
use crate::command::music_play::tools;
use crate::command::music_play::tools::{DurationAccumulator, TimeMetrics};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tracing::error;
use water_ball_tool_lib::concurrent_ring::ConcurrentRing;
use water_ball_tool_lib::split_ring_buffer::{LockedWriter, UnlockedReader};

#[derive(Clone)]
pub(super) struct Player {
    played_samples: Arc<AtomicU64>,
    sound_engine: SoundEngine,
    control: Control,
    event_content: EventContent,
    rx_cmd_thread: Arc<JoinHandle<()>>,
}

#[derive(Clone)]
struct SoundEngine {
    pre_effect_ring_buf: Arc<ConcurrentRing<AudioData>>,
    audio_ring_buffer: LockedWriter<f32>,
    decoder: Arc<Mutex<Option<Decoder>>>,
    stream_config: Arc<StreamConfig>,
    stream: Arc<Stream>,
    effect_chain: Arc<Mutex<PostEffectChain>>,
    pre_effect_runtime: PreEffectRuntime,
    time_metrics: Arc<TimeMetrics>,
}

#[derive(Clone)]
struct Control {
    is_playing: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    tx_cmd: Sender<PlayCmd>,
}

/// 事件控制
#[derive(Clone)]
struct EventContent {
    pre_effect_item_eof: Arc<AtomicBool>,
    pre_effect_item_eof_pos: Arc<AtomicUsize>,
    list: Arc<Mutex<Vec<Box<dyn PlayerEventSink>>>>,
}

impl Player {
    fn create_stream(
        device: &Device,
        config: StreamConfig,
        ring_buf: UnlockedReader<f32>,
        control: Control,
        played_samples: Arc<AtomicU64>,
        event_content: EventContent,
        rx_cmd: Receiver<PlayCmd>,
        tx_done: Sender<PlayCmd>,
        effect_chain: Arc<Mutex<PostEffectChain>>,
        time_metrics: Arc<TimeMetrics>,
    ) -> Result<Stream, cpal::Error> {
        let mut ring_buf = ring_buf;
        let mut scratch = Vec::new();
        let mut total_acc = DurationAccumulator::new();
        let mut copy_acc = DurationAccumulator::new();
        let mut post_effect_acc = DurationAccumulator::new();
        let mut blocks = 0;

        device.build_output_stream(
            config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                let pre_effect_item_eof = &event_content.pre_effect_item_eof;
                let pre_effect_item_eof_pos = &event_content.pre_effect_item_eof_pos;
                let t0 = Instant::now();
                // 1. 先消费 transport 命令，不受播放状态/音量/数据/clear 锁门控
                if let Ok(mut effect_chain) = effect_chain.try_lock() {
                    while let Ok(cmd) = rx_cmd.try_recv() {
                        if cmd == PlayCmd::ClearEofBuf
                            && pre_effect_item_eof.load(Ordering::Acquire)
                        {
                            let re_len = pre_effect_item_eof_pos.load(Ordering::Acquire);
                            let _ = ring_buf.release(0..re_len);
                            pre_effect_item_eof_pos.store(0, Ordering::Release);
                            pre_effect_item_eof.store(false, Ordering::Release);
                            played_samples.store(0, Ordering::Release);
                        }
                        if matches!(effect_chain.dispatch_transport(cmd), Decision::Pass) {
                            let _ = tx_done.try_send(cmd);
                        }
                    }
                }

                // 2. 暂停/静音/读数据
                let volume = f32::from_bits(control.volume.load(Ordering::Relaxed));
                let is_playing = control.is_playing.load(Ordering::Acquire);
                let t1 = Instant::now();
                let len = if is_playing && volume != 0.0 {
                    let len = ring_buf.read(data);
                    copy_acc.record(t1.elapsed());
                    len
                } else {
                    data.fill(0.0);
                    0
                };

                // 3. 效果处理（仅当有数据）
                if len > 0 {
                    let t2 = Instant::now();
                    if let Ok(mut effect_chain) = effect_chain.try_lock() {
                        effect_chain.process(data[..len].as_mut());
                        scratch.clear();
                        effect_chain.poll_transport(&mut scratch);
                        for cmd in scratch.drain(..) {
                            let _ = tx_done.try_send(cmd);
                        }
                        post_effect_acc.record(t2.elapsed());
                    }

                    if (volume - 1.0).abs() > 0.0 {
                        for datum in &mut data[..len] {
                            *datum *= volume;
                        }
                    }

                    played_samples.fetch_add(len as u64, Ordering::Relaxed);
                    data[len..].fill(0.0);
                }

                // 4. EOF 检查（无论 try_read 成功/失败都走）
                if pre_effect_item_eof.load(Ordering::Acquire) {
                    let pos = pre_effect_item_eof_pos.load(Ordering::Acquire);
                    if len >= pos {
                        pre_effect_item_eof_pos.store(0, Ordering::Release);
                    } else {
                        pre_effect_item_eof_pos.fetch_sub(len, Ordering::Release);
                    }
                    if pre_effect_item_eof_pos.load(Ordering::Acquire) == 0 {
                        pre_effect_item_eof.store(false, Ordering::Release);
                        played_samples.store(0, Ordering::Relaxed);
                        Self::emit(
                            PlayerEvent {
                                result: Ok(PlayerEventType::TrackEnded),
                            },
                            &event_content.list,
                        );
                    }
                }

                total_acc.record(t0.elapsed()); // 整个回调总耗时

                blocks += 1;
                if blocks % tools::DURATION_ACCUMULATOR_FLUSH_INTERVAL == 0 {
                    total_acc.flush_into(&time_metrics.callback_total);
                    copy_acc.flush_into(&time_metrics.callback_copy);
                    post_effect_acc.flush_into(&time_metrics.callback_post_effect);
                }
            },
            |err| error!("流错误: {err}"),
            None,
        )
    }

    fn emit(event: PlayerEvent, list: &Arc<Mutex<Vec<Box<dyn PlayerEventSink>>>>) {
        let event = Arc::new(event);
        let mut list = list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for sink in list.iter_mut() {
            sink.emit(event.clone());
        }
    }
}
impl Player {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            return Err("No output device available".into());
        };
        let config = device.default_output_config()?;
        if config.sample_format() != SampleFormat::F32 {
            return Err(format!(
                "Unsupported sample format {:?}, only F32 is supported",
                config.sample_format()
            )
            .into());
        }
        let channels = config.channels();
        let sample_rate = config.sample_rate();
        let stream_config = Arc::new(config.config());

        // 在你的播放器初始化阶段
        //效果缓冲区
        let pre_effect_ring_buf = Arc::new(ConcurrentRing::new_max_capacity(10)?);
        //声音缓冲区
        let (audio_ring_buffer_reader, audio_ring_buffer) =
            water_ball_tool_lib::split_ring_buffer::unlocked_read_locked_write_ring_buffer(
                sample_rate as usize * channels as usize / 4,
            );
        //时间统计
        let time_metrics = Arc::new(TimeMetrics::new());
        //效果运行时
        let mut pre_effect_runtime = PreEffectRuntime::create(
            pre_effect_ring_buf.clone(),
            audio_ring_buffer.clone(),
            time_metrics.clone(),
        );
        let output_format = AudioFormat::new(channels, sample_rate);
        pre_effect_runtime.add_effect(ResamplerEffect::new(output_format))?;
        pre_effect_runtime.add_effect(CrossBridgeEffect::new(output_format))?;

        let decoder = Arc::new(Mutex::new(None));
        let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let played_samples = Arc::new(AtomicU64::new(0));
        let is_playing = Arc::new(AtomicBool::new(false));

        let pre_effect_item_eof = Arc::new(AtomicBool::new(false));
        let pre_effect_item_eof_pos = Arc::new(AtomicUsize::new(0));
        let list = Arc::new(Mutex::new(Vec::new()));

        {
            let pre_effect_item_eof = pre_effect_item_eof.clone();
            let pre_effect_item_eof_pos = pre_effect_item_eof_pos.clone();
            let audio_ring_buffer_writer = audio_ring_buffer.clone();
            pre_effect_runtime.add_event(Box::new(PreEffectRuntimeEventDirectSink::new(
                move |e| {
                    if let Ok(e) = e.result()
                        && let PreEffectRuntimeEventType::Eof = e
                    {
                        pre_effect_item_eof_pos
                            .store(audio_ring_buffer_writer.len(), Ordering::Release);
                        pre_effect_item_eof.store(true, Ordering::Release);
                    }
                },
            )));
            pre_effect_runtime.run();
        }

        let event_content = EventContent {
            pre_effect_item_eof,
            pre_effect_item_eof_pos,
            list,
        };

        let effect_chain = Arc::new(Mutex::new(PostEffectChain::new(AudioFormat::new(
            channels,
            sample_rate,
        ))));

        // 主线程 → 音频线程：transport 指令
        let (tx_cmd, rx_cmd) = crossbeam_channel::bounded::<PlayCmd>(1);
        // 音频线程 → 主线程：已放行的指令
        let (tx_done, rx_done) = crossbeam_channel::unbounded::<PlayCmd>();

        let control = Control {
            is_playing: is_playing.clone(),
            volume,
            tx_cmd,
        };

        let stream = Arc::new(Self::create_stream(
            &device,
            *stream_config,
            audio_ring_buffer_reader,
            control.clone(),
            played_samples.clone(),
            event_content.clone(),
            rx_cmd,
            tx_done,
            effect_chain.clone(),
            time_metrics.clone(),
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
                                    // C3: 不 panic
                                    error!("Seek is not implemented");
                                }
                                PlayCmd::ClearEofBuf => {}
                            }
                        }
                    })
                    .map_err(|e| format!("无法创建 rx_cmd 线程: {e}"))?,
            )
        };

        Ok(Self {
            sound_engine: SoundEngine {
                pre_effect_ring_buf,
                audio_ring_buffer,
                decoder,
                stream_config,
                stream,
                effect_chain,
                pre_effect_runtime,
                time_metrics,
            },
            control,
            played_samples,
            event_content,
            rx_cmd_thread,
        })
    }

    pub fn is_playing(&self) -> bool {
        self.control.is_playing.load(Ordering::Acquire)
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
        f32::from_bits(self.control.volume.load(Ordering::Acquire))
    }

    pub fn volume_percent(&self) -> f32 {
        self.volume() * 100.0
    }

    /// 归一化音量，范围 [0.0, 1.0]，越界 clamp，NaN 视为 1.0
    pub fn set_volume(&self, volume: f32) {
        let volume = if volume.is_nan() {
            1.0
        } else {
            volume.clamp(0.0, 1.0)
        };
        self.control
            .volume
            .store(volume.to_bits(), Ordering::Relaxed);
    }

    /// 百分比音量，范围 [0.0, 100.0]，越界 clamp，NaN 视为 100.0
    pub fn set_volume_percent(&self, percent: f32) {
        let percent = if percent.is_nan() {
            100.0
        } else {
            percent.clamp(0.0, 100.0)
        };
        self.set_volume(percent / 100.0);
    }

    pub fn play(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.control
            .tx_cmd
            .try_send(PlayCmd::Play)
            .map_err(|e| format!("send Play command failed: {e:?}"))?;
        self.sound_engine.stream.play()?;
        self.control.is_playing.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn pause(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.control.is_playing.load(Ordering::Acquire) {
            self.control
                .tx_cmd
                .try_send(PlayCmd::Pause)
                .map_err(|e| format!("send Pause command failed: {e:?}"))?;
        }
        Ok(())
    }

    pub fn audio_ring_buffer(&self) -> &LockedWriter<f32> {
        &self.sound_engine.audio_ring_buffer
    }

    pub fn pre_effect_ring_buffer(&self) -> &Arc<ConcurrentRing<AudioData>> {
        &self.sound_engine.pre_effect_ring_buf
    }

    pub fn pre_effect_runtime(&self) -> &PreEffectRuntime {
        &self.sound_engine.pre_effect_runtime
    }

    pub fn time_metrics(&self) -> &Arc<TimeMetrics> {
        &self.sound_engine.time_metrics
    }

    pub fn stop(self) -> DecoderResult<()> {
        self.try_stop()
    }

    pub fn try_stop(&self) -> DecoderResult<()> {
        if let Some(decoder) = self
            .sound_engine
            .decoder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            decoder.stop()?;
        }
        if let Err(e) = self.sound_engine.pre_effect_runtime.stop() {
            error!("stop pre_effect runtime error: {e:?}");
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
        {
            let per = &mut self.sound_engine.pre_effect_runtime;
            per.send_skip();
            per.reset(*decoder.decoder_format());
        }
        self.sound_engine.audio_ring_buffer.clear();
        // A5: 作废旧自动 Eof 状态
        self.event_content
            .pre_effect_item_eof
            .store(false, Ordering::Release);
        self.event_content
            .pre_effect_item_eof_pos
            .store(0, Ordering::Release);
        // B3: 在启动新 decoder 之前清零进度
        self.played_samples.store(0, Ordering::Relaxed);
        decoder.run()?;
        decoder_lock.replace(decoder);
        Ok(())
    }

    /// 无缝切换 decoder：保留输入/输出缓冲区，让旧曲尾音播完后接新曲。
    ///
    /// # 前置条件（必须由调用方保证）
    ///
    /// 必须在旧 decoder **已自然 EOF**（已 push 完所有数据并退出解码线程）后调用。
    /// 本方法不调用 `Decoder::stop()`、不清空缓冲区，因此无法唤醒阻塞在
    /// `push_wait` 上的旧解码线程。在旧 decoder 尚未 EOF 时调用会导致旧解码线程
    /// **永久阻塞（线程泄漏）**，且 `reset` 会与旧线程 push 竞争。
    ///
    /// 手动切换 / 需要丢弃旧曲尾音 / 无法保证已 EOF 时，请使用 `set_decoder`。
    pub fn set_decoder_no_clear(&mut self, decoder: Decoder) -> DecoderResult<()> {
        let mut decoder = decoder;
        let mut decoder_lock = self
            .sound_engine
            .decoder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        decoder_lock.take();
        self.sound_engine
            .pre_effect_runtime
            .reset(*decoder.decoder_format());
        decoder.run()?;
        decoder_lock.replace(decoder);
        Ok(())
    }

    pub fn try_seek(&mut self, pos: Duration) -> Result<(), Box<dyn std::error::Error>> {
        Err("Seek is not implemented".into())
    }

    pub fn get_sample_rate(&self) -> u32 {
        self.sound_engine.stream_config.sample_rate
    }

    pub fn get_pre_effect_item_eof(&self) -> bool {
        self.event_content
            .pre_effect_item_eof
            .load(Ordering::Acquire)
    }

    /// 清理解码结束剩余在缓冲区的数据并重置
    pub fn clear_pre_effect_item_eof_buf(&self) {
        if self.get_pre_effect_item_eof()
            && self
                .event_content
                .pre_effect_item_eof_pos
                .load(Ordering::Acquire)
                != 0
        {
            self.pre_effect_ring_buffer().clear();
            self.sound_engine.pre_effect_runtime.send_skip();
            if let Err(e) = self.control.tx_cmd.send(PlayCmd::ClearEofBuf) {
                error!("clear_pre_effect_item_eof send PlayCmd::ClearEofBuf failed: {e:?}");
            }
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
            .effect_chain
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
    fn type_of(&self) -> &'static str;
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
    fn type_of(&self) -> &'static str {
        "PlayerEventChannelSink"
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
