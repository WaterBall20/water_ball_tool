use crate::concurrent_ring::ConcurrentRing;
use crate::music_play::effect::{
    AudioData, AudioFormat, PreEffect, PreEffectChain, PreEffectDiscontinuity, PreEffectError,
    PreEffectInfo,
};
use crate::music_play::event::{EventChannelSink, EventDirectSink, EventSink};
use crate::music_play::tools::{DurationAccumulator, TimeMetrics};
//AI=== 看门狗线程退出通知 / watchdog thread-exit notification
use crate::music_play::watchdog::{SLOT_PRE_EFFECT, ThreadExitGuard, WatchNotify};
use crate::split_ring_buffer::LockedWriter;
use std::any::Any;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, error};
//AI_END===

pub struct PreEffectRuntime {
    effects: Arc<Mutex<PreEffectChain>>,
    input_buf: ConcurrentRing<AudioData>,
    audio_buf: LockedWriter<f32>,
    stop: Arc<AtomicBool>,
    effect_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    event_list: Arc<Mutex<Vec<Box<dyn EventSink<PreEffectRuntimeEvent>>>>>,
    time_metrics: Arc<TimeMetrics>,
    //AI=== 效果链重建工厂：线程崩溃后需要整条重建（旧链可能已毒化）/ effect-chain factory: a crash requires a full rebuild (the old chain may be poisoned)
    chain_factory: Arc<Mutex<Option<ChainFactory>>>,
    //AI=== 线程退出通知：线程结束时唤醒看门狗 / thread exit notification for the watchdog
    exit_notify: Option<Arc<WatchNotify>>,
    //AI_END===
    strong: Arc<AtomicUsize>,
}
impl Clone for PreEffectRuntime {
    fn clone(&self) -> Self {
        self.strong.fetch_add(1, Ordering::Release);
        Self {
            effects: self.effects.clone(),
            input_buf: self.input_buf.clone(),
            audio_buf: self.audio_buf.clone(),
            stop: self.stop.clone(),
            effect_thread: self.effect_thread.clone(),
            event_list: self.event_list.clone(),
            time_metrics: self.time_metrics.clone(),
            chain_factory: self.chain_factory.clone(),
            exit_notify: self.exit_notify.clone(),
            strong: self.strong.clone(),
        }
    }
}
impl Drop for PreEffectRuntime {
    fn drop(&mut self) {
        if self.strong.fetch_sub(1, Ordering::Release) == 1 {
            self.stop.store(true, Ordering::Release);
            if let Some(thread) = self
                .effect_thread
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                _ = thread.join();
            }
            self.event_list
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }
    }
}

//AI=== 效果链重建工厂类型 / effect-chain factory type
type ChainFactory = Box<dyn Fn() -> Result<PreEffectChain, PreEffectError> + Send + Sync + 'static>;
//AI_END===

impl PreEffectRuntime {
    fn emit(
        event: PreEffectRuntimeEvent,
        list: &Arc<Mutex<Vec<Box<dyn EventSink<PreEffectRuntimeEvent>>>>>,
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

    fn send_input_and_clear_buf_inner(&self, audio_data: AudioData) -> Result<(), PreEffectError> {
        self.input_buf.reject_waiting_writers();
        self.input_buf.push_wait(audio_data)?;
        Ok(())
    }

    fn audio_buf_write_all(
        buf: &LockedWriter<f32>,
        data: &[f32],
        event_list: &Arc<Mutex<Vec<Box<dyn EventSink<PreEffectRuntimeEvent>>>>>,
    ) -> Result<bool, crate::split_ring_buffer::error::Error<f32>> {
        if let Err(e) = buf.write_all(data) {
            if let crate::split_ring_buffer::error::Kind::WriteRejected = e.kind {
                return Ok(true);
            }
            Self::emit(
                PreEffectRuntimeEvent {
                    result: Err(PreEffectRuntimeEventError::new(
                        "Pre effect runtime write error",
                        PreEffectRuntimeEventErrorKind::Runtime(e.to_string()),
                    )),
                },
                &event_list,
            );
            //AI=== 硬错误只上报，是否退出线程由调用方决定（现在不退出）/ a hard error is only reported; the caller decides (it no longer exits)
            error!("Failed to write all audio data: {}", e);
            //AI_END===
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
            //AI=== 默认无重建工厂、不通知看门狗，由 `Player` 注入 / no factory and no watchdog by default, `Player` injects them
            chain_factory: Arc::new(Mutex::new(None)),
            exit_notify: None,
            //AI_END===
            strong: Arc::new(AtomicUsize::new(1)),
        }
    }

    //AI=== 注入看门狗退出通知（必须在 `run` 之前调用）/ inject the watchdog exit notification (call before `run`)
    pub fn set_exit_notify(&mut self, notify: Arc<WatchNotify>) {
        self.exit_notify = Some(notify);
    }

    /// 注册效果链重建工厂：崩溃恢复时整条链重建（旧效果器可能已被毒化）。
    /// Register the chain factory used by crash recovery; the whole chain is rebuilt because the
    /// old effects may already be poisoned.
    pub fn set_chain_factory<F>(&mut self, factory: F)
    where
        F: Fn() -> Result<PreEffectChain, PreEffectError> + Send + Sync + 'static,
    {
        *self
            .chain_factory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(factory));
    }

    /// 用工厂重建整条效果链（线程崩溃后调用，恢复时不再复用旧链）。
    /// Rebuild the whole effect chain through the factory (used after a crash; the old chain is not reused).
    pub fn rebuild_effects(&self) -> Result<(), PreEffectError> {
        let new_chain = {
            let factory = self
                .chain_factory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match factory.as_ref() {
                Some(factory) => factory()?,
                None => {
                    return Err(Box::new(std::io::Error::other(
                        "effect chain factory is not set",
                    )));
                }
            }
        };
        self.set_effects(new_chain);
        Ok(())
    }

    /// 整体替换效果链（首次构建与崩溃重建共用）。
    /// Replace the whole effect chain (shared by the initial build and crash recovery).
    pub fn set_effects(&self, chain: PreEffectChain) {
        *self
            .effects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = chain;
    }

    /// 线程是否已因 stop 停止（看门狗用它区分正常关停与崩溃）。
    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
    //AI_END===
    //AI=== 环错误后的退避：避免写端缺失时空转 / back off after a ring error so a missing writer cannot spin the loop
    const ERROR_BACKOFF: Duration = Duration::from_millis(10);
    //AI_END===
    //AI=== run 返回 Result：effect 线程创建失败必须回传调用方；已退出的线程先回收再重建（看门狗重启）
    /// `run` returns a Result: a failed effect-thread spawn must propagate to the caller; a finished
    /// thread is reclaimed and respawned (watchdog restart).
    pub fn run(&mut self) -> Result<(), PreEffectError> {
        //AI_END===
        //AI=== 已退出的线程先 join 回收，并清空输入环残留 / reclaim a finished thread and drop stale input
        {
            let mut effect_thread_lock = self
                .effect_thread
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let finished = effect_thread_lock
                .as_ref()
                .is_some_and(std::thread::JoinHandle::is_finished);
            if finished {
                let handle = effect_thread_lock.take();
                // 旧线程已死：清掉环里崩溃前未消费的块，避免重启后先播残留音频
                // The old thread is gone: drop blocks it never consumed so a restart does not replay them.
                self.input_buf.reject_waiting_writers();
                if let Some(handle) = handle
                    && let Err(e) = handle.join()
                {
                    error!("前效果器线程异常退出（panic）/ pre-effect thread crashed: {e:?}");
                }
            }
        }
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
            //AI=== 线程退出的最后一步（含 panic）唤醒看门狗 / the guard's Drop (panic included) wakes the watchdog
            let exit_guard = self
                .exit_notify
                .as_ref()
                .map(|notify| notify.exit_guard(SLOT_PRE_EFFECT));
            //AI_END===
            //AI=== 保留 spawn 结果，失败时通过 `?` 返回错误而非 panic / Keep the spawn result and propagate a failure through `?` instead of panicking
            let spawn_result = {
                //AI_END===
                std::thread::Builder::new()
                    .name("pre-effect-running".to_string())
                    .spawn(move || {
                        //AI=== 守卫必须活到线程结束 / the guard must live until the thread ends
                        let _exit_guard: Option<ThreadExitGuard> = exit_guard;
                        //AI_END===
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
                                //AI=== 环错误不退出线程：复位效果器（skip 信号）、上报事件，退避后继续；
                                // 写端全部消失时 `pop_wait` 会持续返回 WritersClosed，靠退避等待其重新登记。
                                // A ring error no longer ends the thread: reset the effects (skip signal), report the
                                // error, back off and continue. While all writers are gone `pop_wait` keeps returning
                                // WritersClosed; the backoff waits for a writer to register again.
                                Err(e) => {
                                    error!("Pre effect thread popped an error: {e} / 前效果器线程读取环失败");
                                    let _ = effects.on_discontinuity(
                                        &mut out_temp_buf,
                                        PreEffectDiscontinuity::Skip,
                                    );
                                    Self::emit(
                                        PreEffectRuntimeEvent {
                                            result: Err(PreEffectRuntimeEventError::new(
                                                "Pre effect runtime ring error",
                                                PreEffectRuntimeEventErrorKind::Runtime(e.to_string()),
                                            )),
                                        },
                                        &event_list,
                                    );
                                    drop(effects);
                                    std::thread::sleep(Self::ERROR_BACKOFF);
                                    continue;
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
                                    //AI=== 写失败已在 audio_buf_write_all 内 error! 上报：现在不退出线程——
                                    // 硬错误上报事件后丢弃本块继续；WriteRejected 表示环拒绝写入（停止/切曲中），直接跳过本块继续消费控制数据。
                                    // Write failures are already reported through error! inside audio_buf_write_all and no
                                    // longer end the thread: a hard error is reported as an event and the block is dropped;
                                    // WriteRejected means the ring rejects writes (shutdown/track change), so skip the block
                                    // and keep consuming control data such as Eof/NoNext.
                                    match Self::audio_buf_write_all(&audio_buf,
                                                                    &out_temp_buf[..len],
                                                                    &event_list) {
                                        Ok(true) => continue,
                                        Ok(false) => {}
                                        Err(_) => {
                                            continue;
                                        }
                                    }
                                    //AI_END===
                                    write_wait_acc.record(t2.elapsed());
                                    blocks += 1;
                                    if blocks % crate::music_play::tools::DURATION_ACCUMULATOR_FLUSH_INTERVAL == 0 {
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
                                                    "AudioData 不连续变体转换失败（逻辑不变量被破坏）: {} / \
                                                    discontinuity AudioData variant conversion \
                                                    failed (logic invariant broken): {}", e, e
                                                )
                                            }
                                        };
                                    let len = effects.on_discontinuity(
                                        &mut out_temp_buf,
                                        discontinuity,
                                    );
                                    //AI_END===
                                    match Self::audio_buf_write_all(&audio_buf,
                                                                    &out_temp_buf[..len],
                                                                    &event_list) {
                                        Ok(true) => continue,
                                        Ok(false) => {}
                                        //AI=== 硬错误不再退出线程：上报事件后跳过本块 / a hard error no longer ends the thread: report and skip
                                        Err(_) => {
                                            continue;
                                        }
                                        //AI_END===
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

    pub fn add_event(&mut self, event: Box<dyn EventSink<PreEffectRuntimeEvent>>) {
        self.event_list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }

    pub fn pre_effect_ring_buffer(&self) -> &ConcurrentRing<AudioData> {
        &self.input_buf
    }
}

#[derive(Debug)]
pub struct PreEffectRuntimeEvent {
    result: Result<PreEffectRuntimeEventType, PreEffectRuntimeEventError>,
}

impl PreEffectRuntimeEvent {
    #[must_use]
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
    #[must_use]
    pub fn new(message: &str, kind: PreEffectRuntimeEventErrorKind) -> Self {
        Self {
            message: message.to_string(),
            kind,
        }
    }

    #[must_use]
    pub fn kind(&self) -> &PreEffectRuntimeEventErrorKind {
        &self.kind
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Debug for PreEffectRuntimeEventError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind() {
            PreEffectRuntimeEventErrorKind::ResetErrors(e) => &format!("ResetError({e:?})"),
            //AI=== 运行期环错误 / runtime ring error
            PreEffectRuntimeEventErrorKind::Runtime(e) => &format!("Runtime({e})"),
            //AI_END===
            PreEffectRuntimeEventErrorKind::Other(e) => &format!("Other({e})"),
            PreEffectRuntimeEventErrorKind::Create(e) => &format!("Create({e})"),
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

impl From<Box<dyn Error + Send + Sync + 'static>> for PreEffectRuntimeEventError {
    fn from(err: Box<dyn Error + Send + Sync + 'static>) -> Self {
        Self {
            message: format!("{err}"),
            kind: PreEffectRuntimeEventErrorKind::Other(err),
        }
    }
}

pub enum PreEffectRuntimeEventErrorKind {
    ResetErrors(Vec<(PreEffectInfo, PreEffectError)>),
    //AI=== 运行期环错误：线程继续存活，事件仅用于上报 / runtime ring error: the thread keeps running, the event is informational
    /// 环形缓冲区/输出写入的运行期错误描述。
    Runtime(String),
    //AI_END===
    Other(Box<dyn Error + Send + Sync + 'static>),
    Create(Box<dyn Error + Send + Sync + 'static>),
}

impl From<Vec<(PreEffectInfo, PreEffectError)>> for PreEffectRuntimeEventErrorKind {
    fn from(value: Vec<(PreEffectInfo, PreEffectError)>) -> Self {
        Self::ResetErrors(value)
    }
}

pub struct PreEffectRuntimeEventChannelSink;

//AI=== 构造器改为 inherent 方法：事件线程创建失败必须返回 io::Result 而非仅记录日志 / Constructor moved to an inherent method: a failed event-thread spawn must be returned as io::Result instead of merely logged
impl PreEffectRuntimeEventChannelSink {
    pub fn create<F>(
        f: F,
    ) -> Result<EventChannelSink<PreEffectRuntimeEvent>, PreEffectRuntimeEventError>
    where
        F: Fn(&PreEffectRuntimeEvent) + Send + 'static,
    {
        EventChannelSink::new("PreEffectRuntimeEventChannelSink", f).map_err(|e| {
            PreEffectRuntimeEventError::new(
                "Create PreEffectRuntimeEventChannelSink failed",
                PreEffectRuntimeEventErrorKind::Create(e),
            )
        })
    }
}
//AI_END===

pub struct PreEffectRuntimeEventDirectSink;

//AI=== 构造器改为 inherent 方法（无失败路径，不需要 Result）/ Constructor moved to an inherent method (infallible, so no Result is needed)
impl PreEffectRuntimeEventDirectSink {
    pub fn create<F>(f: F) -> EventDirectSink<PreEffectRuntimeEvent>
    where
        F: Fn(&PreEffectRuntimeEvent) + Send + 'static,
    {
        EventDirectSink::new("PreEffectRuntimeEventDirectSink", f)
    }
}
//AI_END===
