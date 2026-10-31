use crate::command::music_play::effect::{
    AudioData, AudioFormat, PreEffect, PreEffectChain, PreEffectDiscontinuity, PreEffectError,
    PreEffectInfo,
};
use crate::command::music_play::tools::{DurationAccumulator, TimeMetrics};
use std::any::Any;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;
use tracing::{debug, error};
use water_ball_tool_lib::concurrent_ring::ConcurrentRing;
use water_ball_tool_lib::split_ring_buffer::LockedWriter;

#[derive(Clone)]
pub struct PreEffectRuntime {
    effects: Arc<Mutex<PreEffectChain>>,
    input_buf: ConcurrentRing<AudioData>,
    audio_buf: LockedWriter<f32>,
    stop: Arc<AtomicBool>,
    effect_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    event_list: Arc<Mutex<Vec<Box<dyn PreEffectRuntimeEventSink>>>>,
    time_metrics: Arc<TimeMetrics>,
}

impl PreEffectRuntime {
    fn emit(
        event: PreEffectRuntimeEvent,
        list: &Arc<Mutex<Vec<Box<dyn PreEffectRuntimeEventSink>>>>,
    ) {
        let event = Arc::new(event);
        //AI=== 锁中毒时恢复内部数据而非 panic / Recover from mutex poisoning instead of panicking
        let mut list = list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        //AI_END===
        for sink in list.iter_mut() {
            sink.emit(event.clone());
        }
    }
}

impl PreEffectRuntime {
    fn send_input_and_clear_buf_inner(&self, audio_data: AudioData) -> Result<(), PreEffectError> {
        self.input_buf.reject_waiting_writers();
        self.input_buf.push_wait(audio_data)?;
        Ok(())
    }

    fn audio_buf_write_all(
        buf: &LockedWriter<f32>,
        data: &[f32],
    ) -> Result<bool, water_ball_tool_lib::split_ring_buffer::error::Error<f32>> {
        if let Err(e) = buf.write_all(data) {
            if let water_ball_tool_lib::split_ring_buffer::error::Kind::WriteRejected = e.kind {
                return Ok(true);
            }
            error!("Failed to write all audio data: {}", e);
            return Err(e);
        }
        Ok(false)
    }
}

impl PreEffectRuntime {
    pub fn create(
        input_buf: ConcurrentRing<AudioData>,
        audio_buf: LockedWriter<f32>,
        time_metrics: Arc<TimeMetrics>,
    ) -> Self {
        Self {
            effects: Arc::new(Mutex::new(PreEffectChain::new())),
            input_buf,
            audio_buf,
            stop: Arc::new(AtomicBool::new(false)),
            effect_thread: Arc::new(Mutex::new(None)),
            event_list: Arc::new(Mutex::new(Vec::new())),
            time_metrics,
        }
    }

    //AI=== run 返回 Result：effect 线程创建失败必须回传调用方，不能只记录日志后返回 / `run` returns a Result: a failed effect-thread spawn must propagate to the caller instead of being logged and swallowed
    pub fn run(&mut self) -> Result<(), PreEffectError> {
        //AI_END===
        let mut effect_thread = self.effect_thread.lock().unwrap_or_else(|e| {
            self.effect_thread.clear_poison();
            e.into_inner()
        });
        if effect_thread.is_none() {
            let effects = self.effects.clone();
            let input_buf = self.input_buf.clone();
            let audio_buf = self.audio_buf.clone();
            let stop = self.stop.clone();
            let event_list = self.event_list.clone();
            let time_metrics = self.time_metrics.clone();
            //AI=== 保留 spawn 结果，失败时通过 `?` 返回错误而非 panic / Keep the spawn result and propagate a failure through `?` instead of panicking
            let spawn_result = {
                //AI_END===
                std::thread::Builder::new()
                    .name("pre-effect-running".to_string())
                    .spawn(move || {
                        //统计
                        let mut out_temp_buf = Vec::new();
                        let mut read_wait_acc = DurationAccumulator::new();
                        let mut process_acc = DurationAccumulator::new();
                        let mut write_wait_acc = DurationAccumulator::new();
                        let mut blocks = 0;
                        debug!("Pre effect thread started");
                        loop {
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            let mut effects = effects.lock().unwrap_or_else(|e| {
                                effects.clear_poison();
                                e.into_inner()
                            });
                            //数据处理
                            let t0 = Instant::now();
                            let data = match input_buf.pop_wait() {
                                Ok(v) => v,
                                //AI=== pop_wait 失败是真实运行期错误（环已拒绝/关闭），上报后结束线程；
                                // 继续循环只会立刻再次失败或空转。
                                // A pop_wait failure is a real runtime error (ring rejected/closed): report it and end the
                                // thread; continuing would only fail again immediately or spin.
                                Err(e) => {
                                    error!("Pre effect thread popped an error: {e}");
                                    break;
                                }
                                //AI_END===
                            };
                            read_wait_acc.record(t0.elapsed());
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            match data {
                                AudioData::Block(data) => {
                                    let t1 = Instant::now();
                                    let len = effects.process(&data, &mut out_temp_buf);
                                    process_acc.record(t1.elapsed());
                                    if len == 0 {
                                        continue;
                                    }
                                    let t2 = Instant::now();
                                    //AI=== 写失败已在 audio_buf_write_all 内 error! 上报：硬错误结束线程；
                                    // WriteRejected 表示环已拒绝写入（停止/切曲中），保留 continue 以便继续消费 Eof/NoNext 等控制数据。
                                    // Write failures are already reported through error! inside audio_buf_write_all: hard errors end
                                    // the thread; WriteRejected means the ring rejects writes (shutdown/track change), so keep
                                    // continuing to consume control data such as Eof/NoNext.
                                    match Self::audio_buf_write_all(&audio_buf, &out_temp_buf[..len]) {
                                        Ok(true) => continue,
                                        Ok(false) => {}
                                        Err(_) => break,
                                    }
                                    //AI_END===
                                    write_wait_acc.record(t2.elapsed());
                                    blocks += 1;
                                    if blocks % crate::command::music_play::tools::DURATION_ACCUMULATOR_FLUSH_INTERVAL == 0 {
                                        read_wait_acc.flush_into(&time_metrics.effect_read_wait);
                                        process_acc.flush_into(&time_metrics.effect_process);
                                        write_wait_acc.flush_into(&time_metrics.effect_write_wait);
                                    }
                                }
                                AudioData::Eof
                                | AudioData::NoNext
                                | AudioData::Skip
                                | AudioData::Seek => {
                                    //AI=== 该转换失败只可能在逻辑不变量被破坏时发生（外层 match 已排除 Block/Reset），
                                    // 属于逻辑错误，按规范用 unreachable! 终止，而不是静默降级为 Skip 后继续。
                                    // This conversion can fail only when a logic invariant is broken (the outer match already
                                    // excludes Block/Reset), so it is a logic error: abort with unreachable! instead of
                                    // silently degrading to Skip and continuing.
                                    let discontinuity =
                                        match PreEffectDiscontinuity::try_from(&data) {
                                            Ok(discontinuity) => discontinuity,
                                            Err(e) => {
                                                unreachable!(
                                                    "AudioData 不连续变体转换失败（逻辑不变量被破坏）: {e} / \
                                                     discontinuity AudioData variant conversion failed (logic invariant broken): {e}"
                                                )
                                            }
                                        };
                                    let len = effects.on_discontinuity(
                                        &mut out_temp_buf,
                                        discontinuity,
                                    );
                                    //AI_END===
                                    match Self::audio_buf_write_all(&audio_buf, &out_temp_buf[..len]) {
                                        Ok(true) => continue,
                                        Ok(false) => {}
                                        Err(_) => break,
                                    }
                                    if let AudioData::Eof = data {
                                        Self::emit(
                                            PreEffectRuntimeEvent {
                                                result: Ok(PreEffectRuntimeEventType::Eof),
                                            },
                                            &event_list,
                                        );
                                    }
                                }
                                AudioData::Reset(input_format) => {
                                    if let Err(e) = effects.reset(input_format) {
                                        //AI=== reset 失败属于可恢复的运行期错误：PreEffectChain::reset 已把失败的效果器降级
                                        // 为 disabled 透传，本曲仍可继续播放，因此保留 continue（不结束线程），但必须上报。
                                        // A reset failure is a recoverable runtime error: PreEffectChain::reset has already degraded
                                        // the failing effect to disabled passthrough, so this track can keep playing. Keep looping
                                        // (do not end the thread) but report through error! + emit.
                                        error!(
                                            "Pre effect reset failed, failing effects degraded: {e:?} / 前效果器 reset 失败，失败效果器已降级: {e:?}"
                                        );
                                        Self::emit(
                                            PreEffectRuntimeEvent {
                                                result: Err(PreEffectRuntimeEventError::new(
                                                    "Reset pre effect err",
                                                    PreEffectRuntimeEventErrorKind::from(e),
                                                )),
                                            },
                                            &event_list,
                                        );
                                        //AI_END===
                                    }
                                }
                            }
                        }
                        debug!("Pre effect thread end");
                    })
            };
            //AI=== 线程创建失败时返回 Err（io::Error 装箱为 PreEffectError）/ Return Err when the thread cannot be created (box the io::Error as PreEffectError)
            let thread_handle = spawn_result.map_err(|e| -> PreEffectError { Box::new(e) })?;
            *effect_thread = Some(thread_handle);
            //AI_END===
        }
        //AI=== run 成功启动后返回 Ok / Return Ok once the thread has started successfully
        Ok(())
        //AI_END===
    }

    pub fn stop(&self) -> Result<(), Box<dyn Any + Send + 'static>> {
        self.stop.store(true, Ordering::Release);
        self.input_buf.reject_waiting_writers();
        self.audio_buf.reject_pending_writers();
        _ = self.input_buf.push(AudioData::NoNext);
        if let Some(join_handle) = self
            .effect_thread
            .lock()
            .unwrap_or_else(|e| {
                self.effect_thread.clear_poison();
                e.into_inner()
            })
            .take()
        {
            join_handle.join()?;
        }
        Ok(())
    }

    pub fn send_skip(&self) -> Result<(), PreEffectError> {
        self.send_input_and_clear_buf_inner(AudioData::Skip)
    }

    pub fn send_no_next(&self) -> Result<(), PreEffectError> {
        self.send_input_and_clear_buf_inner(AudioData::NoNext)
    }

    pub fn reset(&self, format: AudioFormat) -> Result<(), PreEffectError> {
        self.input_buf.push_wait(AudioData::Reset(format))?;
        Ok(())
    }

    pub fn add_effect<E>(&mut self, effect: E) -> Result<(), Box<dyn Error + Send + Sync>>
    where
        E: PreEffect + Send + 'static,
    {
        self.effects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(effect)
    }

    pub fn add_event(&mut self, event: Box<dyn PreEffectRuntimeEventSink>) {
        self.event_list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

#[derive(Debug)]
pub struct PreEffectRuntimeEvent {
    result: Result<PreEffectRuntimeEventType, PreEffectRuntimeEventError>,
}

impl PreEffectRuntimeEvent {
    pub fn result(&self) -> &Result<PreEffectRuntimeEventType, PreEffectRuntimeEventError> {
        &self.result
    }
}

#[derive(Debug)]
pub enum PreEffectRuntimeEventType {
    Eof,
}

pub struct PreEffectRuntimeEventError {
    message: String,
    kind: PreEffectRuntimeEventErrorKind,
}

impl PreEffectRuntimeEventError {
    pub fn new(message: &str, kind: PreEffectRuntimeEventErrorKind) -> Self {
        Self {
            message: message.to_string(),
            kind,
        }
    }

    pub fn kind(&self) -> &PreEffectRuntimeEventErrorKind {
        &self.kind
    }

    pub fn message(&self) -> &String {
        &self.message
    }
}

impl Debug for PreEffectRuntimeEventError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind() {
            PreEffectRuntimeEventErrorKind::ResetErrors(e) => &format!("ResetError({e:?})"),
        };
        write!(
            f,
            "PreEffectRuntimeEventError, kind: {kind}, message: {}",
            self.message
        )
    }
}

impl Display for PreEffectRuntimeEventError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "PreEffectRuntimeEventError : {}", self.message)
    }
}

impl Error for PreEffectRuntimeEventError {}

pub enum PreEffectRuntimeEventErrorKind {
    ResetErrors(Vec<(PreEffectInfo, PreEffectError)>),
}

impl From<Vec<(PreEffectInfo, PreEffectError)>> for PreEffectRuntimeEventErrorKind {
    fn from(value: Vec<(PreEffectInfo, PreEffectError)>) -> Self {
        Self::ResetErrors(value)
    }
}

pub trait PreEffectRuntimeEventSink: Send + 'static {
    fn emit(&mut self, event: Arc<PreEffectRuntimeEvent>);
    fn type_of(&self) -> &'static str;
}

pub struct PreEffectRuntimeEventChannelSink(Sender<Arc<PreEffectRuntimeEvent>>);

//AI=== 构造器改为 inherent 方法：事件线程创建失败必须返回 io::Result 而非仅记录日志 / Constructor moved to an inherent method: a failed event-thread spawn must be returned as io::Result instead of merely logged
impl PreEffectRuntimeEventChannelSink {
    pub fn new<F>(f: F) -> std::io::Result<Self>
    where
        F: Fn(&PreEffectRuntimeEvent) + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel::<Arc<PreEffectRuntimeEvent>>();
        std::thread::Builder::new()
            .name("pre_effect_running_event_channel_sink".to_string())
            .spawn(move || {
                for event in receiver {
                    f(&event);
                }
            })?;
        Ok(Self(sender))
    }
}
//AI_END===

impl PreEffectRuntimeEventSink for PreEffectRuntimeEventChannelSink {
    fn emit(&mut self, event: Arc<PreEffectRuntimeEvent>) {
        // send 非阻塞；满了就丢弃（或 try_send + 计数）
        let _ = self.0.send(event);
    }

    fn type_of(&self) -> &'static str {
        "PreEffectRuntimeEventChannelSink"
    }
}

pub struct PreEffectRuntimeEventDirectSink(Box<dyn Fn(&PreEffectRuntimeEvent) + Send + 'static>);

//AI=== 构造器改为 inherent 方法（无失败路径，不需要 Result）/ Constructor moved to an inherent method (infallible, so no Result is needed)
impl PreEffectRuntimeEventDirectSink {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&PreEffectRuntimeEvent) + Send + 'static,
    {
        Self(Box::new(f))
    }
}
//AI_END===

impl PreEffectRuntimeEventSink for PreEffectRuntimeEventDirectSink {
    fn emit(&mut self, event: Arc<PreEffectRuntimeEvent>) {
        self.0(&event); // 就在解码线程里跑
    }

    fn type_of(&self) -> &'static str {
        "PreEffectRuntimeEventDirectSink"
    }
}
