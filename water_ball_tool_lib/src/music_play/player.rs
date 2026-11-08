//AI_END===
use crate::concurrent_ring::ConcurrentRing;
use crate::music_play::decoder_runtime::{DecoderError, DecoderResult, DecoderRuntime};
use crate::music_play::effect::cross_bridge::CrossBridgeEffect;
use crate::music_play::effect::resampler::ResamplerEffect;
use crate::music_play::effect::runtime::{
    PreEffectRuntime, PreEffectRuntimeEventDirectSink, PreEffectRuntimeEventType,
};
use crate::music_play::effect::{
    AudioFormat, Decision, EffectChainKind, EffectSummary, PlayCmd, PostEffect, PostEffectChain,
    PreEffectChain, PreEffectError,
};
use crate::music_play::event::{EventChannelSink, EventSink};
//AI_END===
//AI=== 引入事件分发线程：实时回调只投递事件/日志，加锁与 sink 调用移到分发线程
use crate::music_play::event_dispatch::{EventDispatcher, LogLevel};
use crate::music_play::tools;
use crate::music_play::tools::{DurationAccumulator, TimeMetrics};
//AI=== 看门狗：线程崩溃后暂停并重启被监视线程 / watchdog: pause and restart watched threads after a crash
use crate::music_play::watchdog::{SLOT_DECODER, SLOT_PRE_EFFECT, WatchNotify};
use crate::split_ring_buffer::{LockedWriter, UnlockedReader};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{Receiver, Sender};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tracing::{debug, error};

pub struct Player {
    pos: Arc<AtomicU64>,
    sound_engine: SoundEngine,
    control: Control,
    event_content: EventContent,
    //AI=== rx_cmd 线程句柄改为 Option，停止时可取出并 join（不再依赖进程退出回收线程）
    /// rx_cmd 线程句柄：`try_stop` 通过取出并 join 来回收该线程。
    ///
    /// Handle of the `rx_cmd` thread: `try_stop` takes and joins it so the thread is
    /// reclaimed instead of leaking until process exit.
    rx_cmd_thread: Arc<Mutex<Option<JoinHandle<()>>>>,
    //AI=== 关闭标志：保证 try_stop 只执行一次完整回收 / ensures the shutdown sequence runs once
    /// 关闭标志：`try_stop` 首次调用置位，重复调用直接返回（幂等）。
    ///
    /// Shutdown flag: set by the first `try_stop` call; later calls return immediately (idempotent).
    shutdown: Arc<AtomicBool>,
    //AI_END===
    //AI_END===
    //AI=== 看门狗：被监视线程退出通知 + 看门狗线程句柄 / watchdog notification and thread handle
    /// 看门狗共享状态：被监视线程退出时唤醒看门狗。
    watchdog_notify: Arc<WatchNotify>,
    /// 看门狗线程句柄：`try_stop` 取出并 join。
    watchdog_thread: Arc<Mutex<Option<JoinHandle<()>>>>,
    //AI_END===
    strong: Arc<AtomicUsize>,
}
impl Clone for Player {
    fn clone(&self) -> Self {
        self.strong.fetch_add(1, Ordering::Release);
        Self {
            pos: self.pos.clone(),
            sound_engine: self.sound_engine.clone(),
            control: self.control.clone(),
            event_content: self.event_content.clone(),
            rx_cmd_thread: self.rx_cmd_thread.clone(),
            shutdown: self.shutdown.clone(),
            //AI=== 看门狗句柄随句柄共享 / the watchdog handle is shared with clones
            watchdog_notify: self.watchdog_notify.clone(),
            watchdog_thread: self.watchdog_thread.clone(),
            //AI_END===
            strong: self.strong.clone(),
        }
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        if self.strong.fetch_sub(1, Ordering::Release) == 1 {
            _ = self.try_stop();
        }
    }
}

#[derive(Clone)]
struct SoundEngine {
    audio_ring_buffer: LockedWriter<f32>,
    decoder_runtime: DecoderRuntime,
    stream_config: Arc<StreamConfig>,
    stream: Arc<Stream>,
    post_effect_chain: Arc<Mutex<PostEffectChain>>,
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
    list: Arc<Mutex<Vec<Box<dyn EventSink<PlayerEvent>>>>>,
    //AI=== 音频回调事件走独立分发线程，回调内不再加锁 / callback events go through the dispatch thread
    /// 事件分发器：音频回调只投递，sink 调用在分发线程执行。
    ///
    /// Event dispatcher: the audio callback only posts, sink invocation runs on the dispatch thread.
    dispatcher: EventDispatcher,
    //AI_END===
}

impl Player {
    fn create_stream(
        device: &Device,
        config: StreamConfig,
        ring_buf: UnlockedReader<f32>,
        control: Control,
        pos: Arc<AtomicU64>,
        event_content: EventContent,
        rx_cmd: Receiver<PlayCmd>,
        tx_done: Sender<PlayCmd>,
        effect_chain: Arc<Mutex<PostEffectChain>>,
        time_metrics: Arc<TimeMetrics>,
    ) -> Result<Stream, cpal::Error> {
        let mut ring_buf = ring_buf;
        let mut scratch = Vec::<PlayCmd>::new();
        let mut total_acc = DurationAccumulator::new();
        let mut copy_acc = DurationAccumulator::new();
        let mut post_effect_acc = DurationAccumulator::new();
        let mut blocks = 0;
        //AI=== 错误回调的日志也移到分发线程，避免在音频线程里走日志 writer 的锁与分配
        // 错误回调可能由音频线程调用，日志统一交给分发线程输出。
        // The error callback may run on the audio thread; logs are routed to the dispatch thread.
        let error_dispatcher = event_content.dispatcher.clone();
        //AI_END===

        device.build_output_stream(
            config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                //AI=== 回调缓冲区长度用于播放延迟估算 / callback buffer length feeds the playback-latency estimate
                time_metrics
                    .callback_samples
                    .store(data.len() as u64, Ordering::Relaxed);
                //AI_END===
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
                            pos.store(0, Ordering::Release);
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
                    //测试用，直接清空缓冲区
                    #[cfg(feature = "debug-no-audio")]
                    {
                        let len = ring_buf.len();
                        let _ = ring_buf.release(0..len);
                        copy_acc.record(t1.elapsed());
                        len
                    }
                    #[cfg(not(feature = "debug-no-audio"))]
                    {
                        let len = ring_buf.read(data);
                        copy_acc.record(t1.elapsed());
                        len
                    }
                } else {
                    data.fill(0.0);
                    0
                };

                // 3. 效果处理（仅当有数据）
                #[cfg(not(feature = "debug-no-audio"))]
                {
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
                    }
                    data[len..].fill(0.0);
                }
                pos.fetch_add(len as u64, Ordering::Relaxed);
                // 4. EOF 检查（无论 try_read 成功/失败都走）
                if pre_effect_item_eof.load(Ordering::Acquire) {
                    let mut this_pos = pre_effect_item_eof_pos.load(Ordering::Acquire);
                    this_pos = this_pos.saturating_sub(len);
                    pre_effect_item_eof_pos.store(this_pos, Ordering::Release);
                    if this_pos == 0 {
                        pre_effect_item_eof.store(false, Ordering::Release);
                        pos.store(0, Ordering::Release);
                        //AI=== 实时回调不再直接调用 sink：只把事件投递给分发线程
                        // 音频回调里不做加锁/回调调用，事件由分发线程处理。
                        // The realtime callback no longer invokes sinks directly; it only posts
                        // the event, and the dispatch thread performs the locking and callbacks.
                        event_content
                            .dispatcher
                            .try_emit_event(PlayerEvent::track_ended());
                        //AI_END===
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
            //AI=== 流错误日志改由分发线程输出 / stream errors are logged on the dispatch thread
            move |err| {
                error_dispatcher.try_log(LogLevel::Error, format!("流错误 / stream error: {err}"));
            },
            //AI_END===
            None,
        )
    }

    //AI=== 旧的直接 emit 已移除：sink 调用统一由 EventDispatcher 线程执行 / removed in favor of EventDispatcher
    //AI_END===

    fn add_pre_effect_event_inner(
        pre_effect_runtime: &mut PreEffectRuntime,
        pre_effect_item_eof_pos: Arc<AtomicUsize>,
        audio_ring_buffer: LockedWriter<f32>,
        pre_effect_item_eof: Arc<AtomicBool>,
    ) {
        pre_effect_runtime.add_event(Box::new(PreEffectRuntimeEventDirectSink::create(
            move |e| {
                debug!("前效果器事件触发：{e:?}");
                if let Ok(e) = e.result()
                    && let PreEffectRuntimeEventType::Eof = e
                {
                    pre_effect_item_eof_pos.store(audio_ring_buffer.len(), Ordering::Release);
                    pre_effect_item_eof.store(true, Ordering::Release);
                }
            },
        )));
    }

    fn create_rx_cmd_thread(
        stream: Arc<Stream>,
        shutdown: Arc<AtomicBool>,
        rx_done: Receiver<PlayCmd>,
        is_playing: Arc<AtomicBool>,
    ) -> Result<Arc<Mutex<Option<JoinHandle<()>>>>, Box<dyn Error + Send + Sync>> {
        let stream = stream.clone();
        let shutdown = shutdown.clone();
        let join_handle = thread::Builder::new()
            .name("rx_cmd".to_string())
            .spawn(move || {
                while !shutdown.load(Ordering::Acquire) {
                    let cmd = match rx_done.recv_timeout(Duration::from_millis(50)) {
                        Ok(cmd) => cmd,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    };
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
                            //AI=== 当前没有发送方：seek 在命令线程完成，保留分支兼容 transport API
                            // No sender today: seek runs on the command thread; keep the arm for the transport API.
                            debug!("Seek transport ignored / 忽略 Seek transport");
                            //AI_END===
                        }
                        PlayCmd::ClearEofBuf => {}
                    }
                }
                debug!("rx_cmd 线程退出 / rx_cmd thread exited");
            })
            .map_err(|e| format!("无法创建 rx_cmd 线程: {e}"))?;
        Ok(Arc::new(Mutex::new(Some(join_handle))))
    }

    fn get_audio_config()
    -> Result<(Device, AudioFormat, Arc<StreamConfig>), Box<dyn Error + Send + Sync>> {
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
        Ok((
            device,
            AudioFormat::new(channels, sample_rate),
            stream_config,
        ))
    }
}
impl Player {
    pub fn new() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let (device, audio_format, stream_config) = Self::get_audio_config()?;
        // 在你的播放器初始化阶段
        //创建缓冲区
        //效果缓冲区
        let pre_effect_ring_buf = ConcurrentRing::new_max_capacity(5)?;
        //声音缓冲区
        let (audio_ring_buffer_reader, audio_ring_buffer) =
            crate::split_ring_buffer::unlocked_read_locked_write_ring_buffer(
                audio_format.sample_rate() as usize * audio_format.channels() as usize / 4,
            );
        //时间统计
        let time_metrics = Arc::new(TimeMetrics::new());
        //AI=== 看门狗共享状态：线程退出时唤醒 / watchdog shared state, woken when a watched thread exits
        let watch_notify = WatchNotify::new();
        //AI_END===
        //效果运行时
        let mut pre_effect_runtime = PreEffectRuntime::create(
            pre_effect_ring_buf,
            audio_ring_buffer.clone(),
            time_metrics.clone(),
        );
        //AI=== 注册重建工厂并构建初始链：崩溃恢复时整条链重建（旧链可能已毒化）/ register the rebuild factory and build the initial chain
        let make_effects = move || -> Result<PreEffectChain, PreEffectError> {
            let mut chain = PreEffectChain::new();
            chain.push(ResamplerEffect::new(audio_format))?;
            chain.push(CrossBridgeEffect::new(audio_format))?;
            Ok(chain)
        };
        // 初始链同样由工厂构建，保证首次启动与重建一致 / the initial chain comes from the same factory
        pre_effect_runtime.set_effects(make_effects()?);
        pre_effect_runtime.set_chain_factory(make_effects);
        //AI_END===
        //AI=== 注入看门狗退出通知（必须在 `run` 之前）/ inject the watchdog notification before `run`
        pre_effect_runtime.set_exit_notify(watch_notify.clone());
        //AI_END===

        let mut decoder_runtime =
            DecoderRuntime::create(pre_effect_runtime.clone(), time_metrics.clone());
        //AI=== 解码线程创建失败必须中止播放器初始化 / abort player init when the decoder thread cannot start
        // `DecoderError` 不满足 `Sync`，因此把启动失败转成可读错误信息传播。
        // `DecoderError` is not `Sync`, so the startup failure is propagated as a readable message.
        decoder_runtime.set_exit_notify(watch_notify.clone());
        decoder_runtime
            .run()
            .map_err(|e| format!("启动解码器线程失败 / failed to start decoder runtime: {e:?}"))?;
        //AI_END===
        let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let pos = Arc::new(AtomicU64::new(0));
        let is_playing = Arc::new(AtomicBool::new(false));

        let pre_effect_item_eof = Arc::new(AtomicBool::new(false));
        let pre_effect_item_eof_pos = Arc::new(AtomicUsize::new(0));
        let list = Arc::new(Mutex::new(Vec::new()));
        //AI=== 事件分发线程创建失败必须中止播放器初始化 / abort player init when the dispatch thread cannot start
        let dispatcher = EventDispatcher::new(list.clone())?;
        //AI_END===
        //注册事件
        Self::add_pre_effect_event_inner(
            &mut pre_effect_runtime,
            pre_effect_item_eof_pos.clone(),
            audio_ring_buffer.clone(),
            pre_effect_item_eof.clone(),
        );
        //AI=== 前效果器线程创建失败必须中止播放器初始化 / abort player init when the pre-effect thread cannot start
        pre_effect_runtime.run()?;
        //AI_END===

        let event_content = EventContent {
            pre_effect_item_eof,
            pre_effect_item_eof_pos,
            list,
            dispatcher,
        };
        //创建后效果器控制器
        let post_effect_chain = Arc::new(Mutex::new(PostEffectChain::new(audio_format)));

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
            pos.clone(),
            event_content.clone(),
            rx_cmd,
            tx_done,
            post_effect_chain.clone(),
            time_metrics.clone(),
        )?);

        //AI=== rx_cmd 线程改为可停止：带退出标志与可取出 join 的句柄
        // rx_cmd 线程通过退出标志结束阻塞等待；句柄存入 Mutex，`try_stop` 可取出并 join。
        // The rx_cmd thread exits through a stop flag; its handle is kept in a Mutex so that
        // `try_stop` can take and join it.
        let shutdown = Arc::new(AtomicBool::new(false));
        let rx_cmd_thread =
            Self::create_rx_cmd_thread(stream.clone(), shutdown.clone(), rx_done, is_playing)?;
        //AI_END===

        let player = Self {
            sound_engine: SoundEngine {
                audio_ring_buffer,
                decoder_runtime,
                stream_config,
                stream,
                post_effect_chain,
                pre_effect_runtime,
                time_metrics,
            },
            control,
            pos,
            event_content,
            rx_cmd_thread,
            //AI=== 关闭标志初始为 false，首次 try_stop 置位 / shutdown starts false, set by the first try_stop
            shutdown,
            //AI_END===
            //AI=== 看门狗（线程崩溃 → 暂停 + 重启被监视线程）/ watchdog (crash → pause + restart the watched thread)
            watchdog_notify: watch_notify,
            watchdog_thread: Arc::new(Mutex::new(None)),
            //AI_END===
            strong: Arc::new(AtomicUsize::new(1)),
            //AI=== 构造完成后再启动看门狗：它需要 player 的运行时句柄 / start the watchdog once the player exists; it needs the runtime handles
        };
        player.start_watchdog()
        //AI_END===
    }

    pub fn is_playing(&self) -> bool {
        self.control.is_playing.load(Ordering::Acquire)
    }

    pub fn is_paused(&self) -> bool {
        !self.is_playing()
    }

    pub fn get_pos(&self) -> Duration {
        Duration::from_millis(
            self.pos.load(Ordering::Acquire) * 1000
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

    pub fn play(&self) -> Result<(), Box<dyn Error>> {
        self.control
            .tx_cmd
            .try_send(PlayCmd::Play)
            .map_err(|e| format!("send Play command failed: {e:?}"))?;
        self.sound_engine.stream.play()?;
        self.control.is_playing.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn pause(&self) -> Result<(), Box<dyn Error>> {
        if self.control.is_playing.load(Ordering::Acquire) {
            self.control
                .tx_cmd
                .try_send(PlayCmd::Pause)
                .map_err(|e| format!("send Pause command failed: {e:?}"))?;
        }
        Ok(())
    }

    pub fn decoder_runtime(&self) -> &DecoderRuntime {
        &self.sound_engine.decoder_runtime
    }

    pub fn decoder_runtime_mut(&mut self) -> &mut DecoderRuntime {
        &mut self.sound_engine.decoder_runtime
    }
    pub fn audio_ring_buffer(&self) -> &LockedWriter<f32> {
        &self.sound_engine.audio_ring_buffer
    }
    pub fn audio_ring_buffer_mut(&mut self) -> &mut LockedWriter<f32> {
        &mut self.sound_engine.audio_ring_buffer
    }
    pub fn pre_effect_runtime(&self) -> &PreEffectRuntime {
        &self.sound_engine.pre_effect_runtime
    }
    pub fn pre_effect_runtime_mut(&mut self) -> &mut PreEffectRuntime {
        &mut self.sound_engine.pre_effect_runtime
    }
    pub fn time_metrics(&self) -> &Arc<TimeMetrics> {
        &self.sound_engine.time_metrics
    }

    //AI=== 效果器窗口数据：前效果在前、后效果在后；任一链正忙时返回 None 由 UI 保留上次结果
    // Effects-window data: pre effects first, post effects after; None when either chain is busy, so the UI keeps its last snapshot.
    /// 效果器链展示快照 / effect-chain display snapshot
    #[must_use]
    pub fn effect_summaries(&self) -> Option<Vec<EffectSummary>> {
        let mut summaries = self.sound_engine.pre_effect_runtime.effect_summaries()?;
        let post = self.sound_engine.post_effect_chain.try_lock().ok()?;
        summaries.extend(post.effect_summaries());
        Some(summaries)
    }
    //AI_END===

    //AI=== 启用/禁用效果器：按链与索引分派，只调用 trait 方法 / enable/disable an effect by chain and index via trait methods
    /// 设置指定效果器的启用状态；索引或链无效时返回 false
    /// Set the enabled state of an effect; returns false when the chain or index is invalid.
    pub fn set_effect_enabled(&self, chain: EffectChainKind, index: usize, enabled: bool) -> bool {
        match chain {
            //AI=== 前效果器在效果器线程应用；后效果器严格同步，直接切换 / pre effects apply on their thread; post effects toggle directly
            EffectChainKind::Pre => self
                .sound_engine
                .pre_effect_runtime
                .request_effect_enabled(index, enabled),
            EffectChainKind::Post => self
                .sound_engine
                .post_effect_chain
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_effect_enabled(index, enabled),
            //AI_END===
        }
    }
    //AI_END===

    pub fn stop(self) -> DecoderResult<()> {
        let mut s = self;
        s.try_stop()
    }

    //AI=== 退出路径：按「暂停回调 → 停前效果器 → 停解码器 → 停事件线程 → join rx_cmd」顺序回收资源
    /// 完整停止播放器：暂停音频回调，依次停止前效果器线程、解码线程、事件分发线程，
    /// 最后 join rx_cmd 线程。可从任意克隆重复调用（幂等）。
    ///
    /// 顺序说明：先停前效果器，其 `reject_waiting_writers` 会唤醒阻塞在 `push_wait` 上的
    /// 解码线程，避免 join 时互相等待。
    ///
    /// Fully stop the player: pause the audio callbacks, then stop the pre-effect thread, the
    /// decoder thread and the event dispatch thread in order, and finally join the rx_cmd
    /// thread. Safe to call repeatedly from any clone (idempotent).
    ///
    /// Ordering: the pre-effect runtime is stopped first because its
    /// `reject_waiting_writers` wakes producers blocked in `push_wait`, preventing a join
    /// deadlock.
    pub fn try_stop(&mut self) -> DecoderResult<()> {
        //AI=== 幂等：只有第一次调用执行完整回收 / idempotent: only the first call runs the shutdown
        if self.shutdown.swap(true, Ordering::AcqRel) {
            // 已停止：不重复暂停、停线程或记录错误日志 / already stopped: avoid duplicate work and logs
            return Ok(());
        }
        //AI_END===
        //AI=== 先关停看门狗，避免停止过程中被它重启线程 / stop the watchdog first so it cannot restart threads during shutdown
        self.watchdog_notify.shutdown();
        //AI_END===
        // 1. 暂停音频回调，避免回调继续产生事件 / pause callbacks so no new events appear
        self.control.is_playing.store(false, Ordering::Release);
        if let Err(e) = self.sound_engine.stream.pause() {
            error!("停止播放时暂停音频流失败 / failed to pause the stream: {e}");
        }
        // 2. 先停前效果器：拒绝写入会唤醒阻塞的生产者 / stop the pre-effect runtime first
        if let Err(e) = self.sound_engine.pre_effect_runtime.stop() {
            error!("停止前效果器运行时失败 / failed to stop the pre-effect runtime: {e:?}");
        }
        // 3. 停解码线程 / stop the decoder thread
        let decoder_runtime = &mut self.sound_engine.decoder_runtime;
        if let Err(e) = decoder_runtime.stop() {
            error!("停止解码器运行时失败 / failed to stop the decoder runtime: {e:?}");
        }
        // 4. 停事件分发线程（回调已暂停，不会再有新事件）/ stop the dispatch thread
        self.event_content.dispatcher.stop();
        //AI=== 停止后报告分发通道的丢包数量 / report dropped dispatch messages after stopping
        let dropped = self.event_content.dispatcher.dropped();
        if dropped > 0 {
            debug!(
                "事件分发线程丢弃了 {dropped} 条消息 / dispatch thread dropped {dropped} messages"
            );
        }
        //AI_END===
        // 5. 等待 rx_cmd 线程退出 / join the rx_cmd thread
        Self::join_rx_cmd_thread(&self.rx_cmd_thread);
        //AI=== 最后 join 看门狗线程 / join the watchdog thread last
        Self::join_watchdog(&self.watchdog_thread);
        //AI_END===
        self.event_content
            .list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        Ok(())
    }

    /// 取出并 join rx_cmd 线程；线程 panic 只记录错误，不向上传播。
    ///
    /// Take and join the rx_cmd thread; a thread panic is logged instead of propagated.
    fn join_rx_cmd_thread(handle: &Arc<Mutex<Option<JoinHandle<()>>>>) {
        let join_handle = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(join_handle) = join_handle
            && let Err(e) = join_handle.join()
        {
            error!("rx_cmd 线程异常退出 / rx_cmd thread panicked: {e:?}");
        }
    }

    //AI=== 看门狗：线程崩溃 → 暂停（不恢复播放）+ 重启被监视线程 / watchdog: crash → pause (no resume) + restart the watched thread
    /// 启动看门狗线程：被监视线程退出（含 panic）时经 `ThreadExitGuard` 的 `Drop` 唤醒。
    /// Spawn the watchdog; a watched thread's exit (panic included) wakes it through `ThreadExitGuard`.
    fn start_watchdog(self) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let notify = self.watchdog_notify.clone();
        let shutdown = self.shutdown.clone();
        let control = self.control.clone();
        let mut decoder_runtime = self.sound_engine.decoder_runtime.clone();
        let mut pre_effect_runtime = self.sound_engine.pre_effect_runtime.clone();
        let handle = thread::Builder::new()
            .name("player-watchdog".to_string())
            .spawn(move || {
                while let Some(exited) = notify.wait_exit() {
                    if shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    // 崩溃只暂停，不恢复播放 / a crash only pauses; playback is not resumed automatically
                    if control.is_playing.load(Ordering::Acquire) {
                        let _ = control.tx_cmd.try_send(PlayCmd::Pause);
                    }
                    if exited & SLOT_DECODER != 0 && !decoder_runtime.is_stopped() {
                        error!("解码线程已退出，暂停并重启 / decoder thread exited; pausing and restarting");
                        // 置毒化：`Play` 会先换曲再播放 / poisoned so `Play` changes track first
                        decoder_runtime.set_poisoned();
                        if let Err(e) = decoder_runtime.run() {
                            error!("重启解码线程失败 / failed to restart the decoder thread: {e:?}");
                        }
                    }
                    if exited & SLOT_PRE_EFFECT != 0 && !pre_effect_runtime.is_stopped() {
                        error!(
                            "前效果器线程已退出，重建效果器并重启 / pre-effect thread exited; rebuilding effects and restarting"
                        );
                        // 旧效果器可能已毒化：整条链重建 / the old effects may be poisoned: rebuild the whole chain
                        if let Err(e) = pre_effect_runtime.rebuild_effects() {
                            error!("重建效果链失败 / failed to rebuild the effect chain: {e:?}");
                        }
                        if let Err(e) = pre_effect_runtime.run() {
                            error!("重启前效果器线程失败 / failed to restart the pre-effect thread: {e:?}");
                        }
                    }
                }
            })?;
        *self
            .watchdog_thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle);
        Ok(self)
    }

    /// 取出并 join 看门狗线程；panic 只记录，不向上传播。
    fn join_watchdog(handle: &Arc<Mutex<Option<JoinHandle<()>>>>) {
        let join_handle = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(join_handle) = join_handle
            && let Err(e) = join_handle.join()
        {
            error!("看门狗线程异常退出 / watchdog thread panicked: {e:?}");
        }
    }
    //AI_END===
    //AI_END===

    pub fn set_decoder_path(&self, path: &Path) -> DecoderResult<()> {
        self.sound_engine.decoder_runtime.set_resetting();
        //AI=== 用 `?` 传播 skip 失败（`From<PreEffectError> for DecoderError` 已实现）/ propagate via `?`
        self.sound_engine
            .pre_effect_runtime
            .send_skip()
            .map_err(DecoderError::from)?;
        //AI_END===
        self.sound_engine.audio_ring_buffer.reject_pending_writers();
        self.sound_engine.decoder_runtime.set_decoder_path(path)?;
        // A5: 作废旧自动 Eof 状态
        self.event_content
            .pre_effect_item_eof
            .store(false, Ordering::Release);
        self.event_content
            .pre_effect_item_eof_pos
            .store(0, Ordering::Release);
        // B3: 在启动新 decoder 之前清零进度
        self.pos.store(0, Ordering::Relaxed);
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
    pub fn set_decoder_path_no_clear(&self, path: &Path) -> DecoderResult<()> {
        self.sound_engine.decoder_runtime.set_decoder_path(path)
    }

    //AI=== 同曲跳转：清已生成的旧音频 → 解码器 seek（内部清输入环并基本复位效果链）→ 作废 EOF 并归位进度
    // Seek within the track: drop produced audio, seek in the decoder (it clears the input ring and basic-resets the
    // effect chain), then invalidate EOF state and rebase the progress counter.
    /// 跳到同曲指定位置。进度为近似值：回调增量与归位之间可能相差一个音频块。
    ///
    /// Seek within the current track. The reported position is approximate: the callback may add one audio block
    /// between the reset and the rebase.
    pub fn try_seek(&self, pos: Duration) -> Result<(), Box<dyn Error>> {
        if self.sound_engine.decoder_runtime.decoder_format().is_none() {
            return Err("no decoder / 尚未加载曲目".into());
        }
        if self.sound_engine.decoder_runtime.is_poisoned() {
            return Err("decoder poisoned / 解码器异常，无法跳转".into());
        }

        // 先清输出环，效果线程可能卡在写满的环上，清空后它才能处理 Seek 控制块
        // Clear the output ring first: the effect thread may be blocked on a full ring and must drain to handle Seek.
        self.sound_engine.audio_ring_buffer.reject_pending_writers();
        // 先作废一次 EOF 状态，缩小回调在 seek 过程中误发 TrackEnded 的窗口
        // Invalidate EOF once up front to narrow the window where the callback may emit TrackEnded mid-seek.
        self.event_content
            .pre_effect_item_eof
            .store(false, Ordering::Release);
        self.event_content
            .pre_effect_item_eof_pos
            .store(0, Ordering::Release);
        self.sound_engine.decoder_runtime.try_seek(pos)?;

        // seek 期间效果线程可能又处理了队列里的旧 Eof，返回后再清一次
        // The effect thread may still process a queued old Eof during the seek, so clear again after it returns.
        self.event_content
            .pre_effect_item_eof
            .store(false, Ordering::Release);
        self.event_content
            .pre_effect_item_eof_pos
            .store(0, Ordering::Release);

        // pos 按输出（重采样后）的交错样本数计数，必须用设备格式而不是解码格式
        // pos counts interleaved output (post-resampler) samples, so use the device format, not the decode format.
        let channels = f64::from(self.sound_engine.stream_config.channels);
        let rate = f64::from(self.sound_engine.stream_config.sample_rate);
        let samples = (pos.as_secs_f64() * rate * channels).round() as u64;
        self.pos.store(samples, Ordering::Release);
        Ok(())
    }
    //AI_END===

    pub fn get_sample_rate(&self) -> u32 {
        self.sound_engine.stream_config.sample_rate
    }

    //AI=== 输出设备格式：播放延迟按设备采样率与声道换算 / output device format for playback-latency conversion
    /// 输出设备格式 / output device format
    #[must_use]
    pub fn output_format(&self) -> AudioFormat {
        AudioFormat::new(
            self.sound_engine.stream_config.channels,
            self.sound_engine.stream_config.sample_rate,
        )
    }
    //AI_END===

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
            self.pre_effect_runtime()
                .pre_effect_ring_buffer()
                .reject_waiting_writers();
            //AI=== skip 发送失败只记录日志并继续清理 / log a failed skip and keep cleaning up
            if let Err(e) = self.sound_engine.pre_effect_runtime.send_skip() {
                error!("清理 Eof 缓冲区时发送 skip 失败 / failed to send skip: {e}");
            }
            //AI_END===
            if let Err(e) = self.control.tx_cmd.send(PlayCmd::ClearEofBuf) {
                error!("clear_pre_effect_item_eof send PlayCmd::ClearEofBuf failed: {e:?}");
            }
        }
    }

    pub fn add_event(&mut self, event: Box<dyn EventSink<PlayerEvent>>) {
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

pub struct PlayerEventChannelSink;

impl PlayerEventChannelSink {
    //AI=== 线程创建失败返回错误 / return an error when the thread cannot be created
    /// 创建基于通道的 sink：内部事件转发线程创建失败时返回错误。
    ///
    /// Create a channel-backed sink; returns an error if the forwarding thread cannot be spawned.
    pub fn create<F>(f: F) -> Result<EventChannelSink<PlayerEvent>, PlayerEventError>
    where
        F: Fn(&PlayerEvent) + Send + 'static,
    {
        EventChannelSink::new("PlayerEventChannelSink", f).map_err(|e| {
            PlayerEventError::new(
                "Create PlayerEventChannelSink failed",
                PlayerEventErrorKind::Create(e),
            )
        })
    }
    //AI_END===
}

#[derive(Debug)]
pub struct PlayerEvent {
    result: Result<PlayerEventType, PlayerEventError>,
}

impl PlayerEvent {
    //AI=== 新增 TrackEnded 构造器：供音频回调与测试使用 / constructor shared by the callback and tests
    /// 构造「一首曲目播放结束」事件 / Build a "track ended" event.
    #[must_use]
    pub fn track_ended() -> Self {
        Self {
            result: Ok(PlayerEventType::TrackEnded),
        }
    }
    //AI_END===

    #[must_use]
    pub fn result(&self) -> &Result<PlayerEventType, PlayerEventError> {
        &self.result
    }

    #[must_use]
    pub fn get_result(self) -> Result<PlayerEventType, PlayerEventError> {
        self.result
    }
}

#[derive(Debug, Clone)]
pub enum PlayerEventType {
    TrackEnded,
}

#[derive(Debug)]
pub struct PlayerEventError {
    message: String,
    kind: PlayerEventErrorKind,
}
impl PlayerEventError {
    fn new(message: &str, kind: PlayerEventErrorKind) -> Self {
        Self {
            message: message.to_string(),
            kind,
        }
    }
}

impl PlayerEventError {
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn kind(&self) -> &PlayerEventErrorKind {
        &self.kind
    }
}

impl Display for PlayerEventError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for PlayerEventError {}

#[derive(Debug)]
pub enum PlayerEventErrorKind {
    Create(Box<dyn Error + Send + Sync + 'static>),
}
