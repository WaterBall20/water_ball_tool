use crate::concurrent_ring::{ConcurrentRing, ConcurrentRingErrorKind};
use crate::music_play::effect::{
    AudioData, AudioFormat, EffectSummary, EffectToggleError, PreEffect, PreEffectChain,
    PreEffectDiscontinuity, PreEffectError, PreEffectInfo, ResetReason,
};
use crate::music_play::event::{EventChannelSink, EventDirectSink, EventSink};
use crate::music_play::tools::{DurationAccumulator, TimeMetrics};
//AI=== 看门狗线程退出通知 / watchdog thread-exit notification
use crate::music_play::watchdog::{SLOT_PRE_EFFECT, ThreadExitGuard, WatchNotify};
use crate::split_ring_buffer::{LockedWriter, WriteAllErrorKind};
//AI=== 独立效果器命令通道：不经过音频环，支持主动唤醒阻塞读者 / separate effect command channel with active reader wake
use crossbeam_channel::{Receiver, Sender};
//AI_END===
use std::any::Any;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, error};
//AI_END===

//AI=== 效果器开关命令 / effect toggle command
#[derive(Clone, Copy, Debug)]
struct EffectCommand {
    index: usize,
    enabled: bool,
}

/// 一次输出写入的结果 / outcome of one output write
enum AudioWriteOutcome {
    Complete,
    Interrupted(usize),
    Discarded,
}
//AI_END===

pub struct PreEffectRuntime {
    effects: Arc<Mutex<PreEffectChain>>,
    input_buf: ConcurrentRing<AudioData>,
    audio_buf: LockedWriter<f32>,
    //AI=== 开关命令走独立通道，由效果器线程应用 / toggle commands use a separate channel applied by the effect thread
    effect_cmd_tx: Sender<EffectCommand>,
    effect_cmd_rx: Receiver<EffectCommand>,
    //AI_END===
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
            effect_cmd_tx: self.effect_cmd_tx.clone(),
            effect_cmd_rx: self.effect_cmd_rx.clone(),
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
            //AI=== 停止标志本身不能唤醒阻塞的 pop_wait；输出环满时还要拒绝在途写者 / the stop flag alone cannot wake a blocked pop_wait; reject in-flight writers too
            self.input_buf.wake_blocked_readers();
            self.input_buf.reject_waiting_writers();
            self.audio_buf.reject_pending_writers();
            //AI_END===
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

    //AI=== 效果线程算延迟并存原子值，UI 不锁效果链 / compute on the effect thread; the UI reads an atomic and never locks the chain
    fn store_effect_delay(effects: &PreEffectChain, time_metrics: &TimeMetrics) {
        let ns = u64::try_from(effects.buffered_duration().as_nanos()).unwrap_or(u64::MAX);
        time_metrics.effect_delay_ns.store(ns, Ordering::Relaxed);
    }
    //AI_END===

    fn send_input_and_clear_buf_inner(&self, audio_data: AudioData) -> Result<(), PreEffectError> {
        self.input_buf.reject_waiting_writers();
        //AI=== 输入环被清空：丢弃的块不会再被效果线程取走，排队延迟必须同步归零
        // The input ring was cleared: discarded blocks will never be popped, so the queue gauge must reset too.
        self.time_metrics
            .pre_effect_queue_samples
            .store(0, Ordering::Relaxed);
        //AI_END===
        self.input_buf.push_wait(audio_data)?;
        Ok(())
    }

    //AI=== 输出缓存：被非破坏性唤醒时保留未写数据，稍后按顺序重试 / output cache: keep unwritten data on a non-destructive wake
    fn write_audio_once(
        buf: &LockedWriter<f32>,
        data: &[f32],
        event_list: &Arc<Mutex<Vec<Box<dyn EventSink<PreEffectRuntimeEvent>>>>>,
    ) -> AudioWriteOutcome {
        match buf.write_all(data) {
            Ok(()) => AudioWriteOutcome::Complete,
            Err(e) => match e.kind {
                WriteAllErrorKind::Interrupted => AudioWriteOutcome::Interrupted(e.written),
                WriteAllErrorKind::Rejected => AudioWriteOutcome::Discarded,
                WriteAllErrorKind::PeerClosed => {
                    Self::emit(
                        PreEffectRuntimeEvent {
                            result: Err(PreEffectRuntimeEventError::new(
                                "Pre effect runtime write error",
                                PreEffectRuntimeEventErrorKind::Runtime(e.to_string()),
                            )),
                        },
                        &event_list,
                    );
                    error!("Failed to write all audio data: {}", e);
                    AudioWriteOutcome::Discarded
                }
            },
        }
    }

    /// 把新输出放到待写缓存末尾；缓存为空时先尝试直接写。
    /// Returns true when the pending buffer is still non-empty.
    fn queue_audio_output(
        buf: &LockedWriter<f32>,
        pending: &mut Vec<f32>,
        data: &[f32],
        event_list: &Arc<Mutex<Vec<Box<dyn EventSink<PreEffectRuntimeEvent>>>>>,
    ) -> bool {
        if pending.is_empty() {
            match Self::write_audio_once(buf, data, event_list) {
                AudioWriteOutcome::Complete | AudioWriteOutcome::Discarded => false,
                AudioWriteOutcome::Interrupted(written) => {
                    pending.extend_from_slice(&data[written..]);
                    !pending.is_empty()
                }
            }
        } else {
            pending.extend_from_slice(data);
            true
        }
    }

    /// 尝试写出缓存；返回 true 表示仍有未写数据。
    fn flush_pending_audio(
        buf: &LockedWriter<f32>,
        pending: &mut Vec<f32>,
        event_list: &Arc<Mutex<Vec<Box<dyn EventSink<PreEffectRuntimeEvent>>>>>,
    ) -> bool {
        if pending.is_empty() {
            return false;
        }
        match Self::write_audio_once(buf, pending, event_list) {
            AudioWriteOutcome::Complete | AudioWriteOutcome::Discarded => {
                pending.clear();
                false
            }
            AudioWriteOutcome::Interrupted(written) => {
                pending.drain(..written);
                !pending.is_empty()
            }
        }
    }

    //AI_END===
}

impl PreEffectRuntime {
    pub fn create(
        input_buf: ConcurrentRing<AudioData>,
        audio_buf: LockedWriter<f32>,
        time_metrics: Arc<TimeMetrics>,
    ) -> Self {
        //AI=== 命令通道不丢消息，容量不设上限 / command channel must not drop messages, so it is unbounded
        let (effect_cmd_tx, effect_cmd_rx) = crossbeam_channel::unbounded();
        //AI_END===
        Self {
            effects: Arc::new(Mutex::new(PreEffectChain::new())),
            input_buf,
            audio_buf,
            effect_cmd_tx,
            effect_cmd_rx,
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
                //AI=== 环已清空，排队延迟归零 / the ring was cleared, so the queue gauge resets
                self.time_metrics
                    .pre_effect_queue_samples
                    .store(0, Ordering::Relaxed);
                //AI_END===
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
            let effect_cmd_rx = self.effect_cmd_rx.clone();
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
                        //AI=== 待写输出缓存：非破坏性唤醒后保留未写数据 / pending output cache retained across non-destructive wakes
                        let mut pending_audio = Vec::new();
                        let mut pending_eof = false;
                        //AI=== 开关排空缓冲复用，避免每条命令分配 / reuse the drained buffer across commands
                        let mut drained = Vec::new();
                        //AI_END===
                        let mut read_wait_acc = DurationAccumulator::new();
                        let mut process_acc = DurationAccumulator::new();
                        let mut write_wait_acc = DurationAccumulator::new();
                        let mut blocks = 0;

                        debug!("Pre effect thread started");
                        loop {
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            //AI=== 先应用开关命令，再阻塞取音频；与音频回调“先命令后数据”的顺序一致
                            // Apply toggle commands before blocking for audio, mirroring the audio callback.
                            while let Ok(command) = effect_cmd_rx.try_recv() {
                                drained.clear();
                                let result = {
                                    let mut effects = effects.lock().unwrap_or_else(|e| {
                                        effects.clear_poison();
                                        e.into_inner()
                                    });
                                    let result = effects.set_effect_enabled(
                                        command.index,
                                        command.enabled,
                                        &mut drained,
                                    );
                                    Self::store_effect_delay(&effects, &time_metrics);
                                    result
                                };
                                match result {
                                    Ok(()) => {}
                                    Err(EffectToggleError::Reset(errors)) => {
                                        error!(
                                            "效果器开关 reset 失败，相关效果器已降级 / effect toggle reset failed, effects degraded: {errors:?}"
                                        );
                                        Self::emit(
                                            PreEffectRuntimeEvent {
                                                result: Err(PreEffectRuntimeEventError::new(
                                                    "Reset pre effect err",
                                                    PreEffectRuntimeEventErrorKind::from(errors),
                                                )),
                                            },
                                            &event_list,
                                        );
                                    }
                                    Err(
                                        EffectToggleError::InvalidIndex
                                        | EffectToggleError::Forbidden,
                                    ) => {
                                        debug!(
                                            "效果器开关被拒绝 / effect toggle rejected: index={} enabled={}",
                                            command.index, command.enabled
                                        );
                                    }
                                }
                                if !drained.is_empty() {
                                    // 排空数据追加到待写缓存，保持旧块 → 排空数据 → 新块顺序
                                    // Append drained output after any pending block to preserve ordering.
                                    Self::queue_audio_output(
                                        &audio_buf,
                                        &mut pending_audio,
                                        &drained,
                                        &event_list,
                                    );
                                }
                            }
                            //AI_END===

                            //AI=== 先写完缓存再取新数据；被再次唤醒就回到顶部处理命令 / flush before new data; on another wake go back to commands
                            if !pending_audio.is_empty() {
                                Self::flush_pending_audio(
                                    &audio_buf,
                                    &mut pending_audio,
                                    &event_list,
                                );
                                if !pending_audio.is_empty() {
                                    continue;
                                }
                            }
                            if pending_eof && pending_audio.is_empty() {
                                Self::emit(
                                    PreEffectRuntimeEvent {
                                        result: Ok(PreEffectRuntimeEventType::Eof),
                                    },
                                    &event_list,
                                );
                                pending_eof = false;
                            }
                            //AI_END===

                            //数据处理
                            let t0 = Instant::now();
                            let pop_r = input_buf.pop_wait();
                            let mut effects = effects.lock().unwrap_or_else(|e| {
                                effects.clear_poison();
                                e.into_inner()
                            });
                            let data = match pop_r {
                                Ok(v) => v,
                                //AI=== 主动唤醒只表示有命令待处理：不当作错误，也不复位效果器
                                // An active wake only means commands are pending: not an error, no effect reset.
                                Err(e)
                                if matches!(&e.kind, ConcurrentRingErrorKind::ReadWoken) =>
                                    {
                                        drop(effects);
                                        continue;
                                    }
                                //AI_END===
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
                                    //AI=== 复位后延迟线为空 / the delay line is empty after the reset
                                    Self::store_effect_delay(&effects, &time_metrics);
                                    //AI_END===
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
                                    //AI=== 该块已离开输入环，先扣减排队样本 / this block has left the input ring, so remove it from the queue gauge
                                    time_metrics.take_pre_effect_queue_samples(data.len());
                                    //AI_END===
                                    let t1 = Instant::now();
                                    let len = effects.process(&data, &mut out_temp_buf);
                                    process_acc.record(t1.elapsed());
                                    //AI=== 处理完更新延迟线时长 / update the delay-line duration after processing
                                    Self::store_effect_delay(&effects, &time_metrics);
                                    //AI_END===
                                    if len == 0 {
                                        continue;
                                    }
                                    //AI=== 写输出环可能阻塞，先释放效果链锁让 UI 能读取列表 / releasing the output ring can block, so drop the chain lock first
                                    drop(effects);
                                    //AI_END===
                                    let t2 = Instant::now();
                                    //AI=== 未写完的数据进入缓存，回到顶部先处理命令再续写 / keep unwritten data and handle commands before retrying
                                    Self::queue_audio_output(
                                        &audio_buf,
                                        &mut pending_audio,
                                        &out_temp_buf[..len],
                                        &event_list,
                                    );
                                    if !pending_audio.is_empty() {
                                        continue;
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
                                    //AI=== seek 先丢弃已写出的旧位置音频，effect 线程是输出环唯一写者
                                    // Seek drops audio already written for the old position; this thread is the ring's only writer.
                                    if matches!(&data, AudioData::Seek) {
                                        audio_buf.clear();
                                        // 旧位置的待写数据随输出环一起丢弃 / old-position pending data is dropped with the output ring
                                        pending_audio.clear();
                                    }
                                    //AI_END===
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
                                    //AI=== 不连续处理会清空或排空延迟线 / a discontinuity clears or drains the delay line
                                    Self::store_effect_delay(&effects, &time_metrics);
                                    //AI_END===
                                    //AI=== 写输出环可能阻塞，先释放效果链锁 / releasing the output ring can block, so drop the chain lock first
                                    drop(effects);
                                    //AI_END===
                                    //AI=== 边界数据必须完整写出后再发 Eof；被唤醒则回到顶部，缓存写完再发
                                    // Finish the boundary output before emitting Eof; on a wake, return to the top and emit after the cache drains.
                                    Self::queue_audio_output(
                                        &audio_buf,
                                        &mut pending_audio,
                                        &out_temp_buf[..len],
                                        &event_list,
                                    );
                                    if matches!(&data, AudioData::Eof) {
                                        pending_eof = true;
                                    }
                                    if !pending_audio.is_empty() {
                                        continue;
                                    }
                                    if pending_eof {
                                        Self::emit(
                                            PreEffectRuntimeEvent {
                                                result: Ok(PreEffectRuntimeEventType::Eof),
                                            },
                                            &event_list,
                                        );
                                        pending_eof = false;
                                    }
                                    //AI_END===
                                }
                                AudioData::Reset(input_format) => {
                                    if let Err(e) =
                                        effects.reset(input_format, ResetReason::TrackStart)
                                    {
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
                                    //AI=== reset 后重新登记格式，延迟线已清空 / formats are registered again and the delay line is empty after reset
                                    Self::store_effect_delay(&effects, &time_metrics);
                                    //AI_END===
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
        //AI=== 主动唤醒阻塞读者，保证 stop 不依赖后续 push / wake blocked readers so stop does not rely on a later push
        self.input_buf.wake_blocked_readers();
        //AI_END===
        self.input_buf.reject_waiting_writers();
        //AI=== 停止时清空排队延迟 / clear the queue gauge on stop
        self.time_metrics
            .pre_effect_queue_samples
            .store(0, Ordering::Relaxed);
        //AI_END===
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

    //AI=== seek：清空输入环并让效果链做基本复位 / seek: clear the input ring and basic-reset the effect chain
    pub fn send_seek(&self) -> Result<(), PreEffectError> {
        self.send_input_and_clear_buf_inner(AudioData::Seek)
    }
    //AI_END===

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

    pub fn min_input_len_work(&self) -> usize {
        self.effects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .min_input_len_work()
    }

    //AI=== 效果器窗口数据：try_lock 避免 UI 阻塞效果线程 / effects-window data: try_lock so the UI never blocks the effect thread
    /// 前效果器展示快照；效果线程正忙时返回 None / pre-effect display snapshot; None while the effect thread is busy
    #[must_use]
    pub fn effect_summaries(&self) -> Option<Vec<EffectSummary>> {
        self.effects
            .try_lock()
            .ok()
            .map(|effects| effects.effect_summaries())
    }

    //AI=== 启用/禁用前效果器：投递到效果器线程，由它排空并通知下游 / queue the toggle for the effect thread
    /// 请求设置指定前效果器的启用状态。
    ///
    /// 命令进入独立通道后主动唤醒阻塞的效果器线程；实际应用发生在下一个
    /// 数据块边界之前。返回值只表示请求已入队，最终状态以快照为准。
    ///
    /// Request a pre-effect toggle. The command is queued on the dedicated channel
    /// and the blocked effect thread is woken; it is applied before the next block.
    /// The return value only says the request was queued.
    pub fn request_effect_enabled(&self, index: usize, enabled: bool) -> bool {
        if self
            .effect_cmd_tx
            .send(EffectCommand { index, enabled })
            .is_err()
        {
            return false;
        }
        self.input_buf.wake_blocked_readers();
        //AI=== 写端可能阻塞在满输出环上，非破坏性唤醒让它回到命令处理 / the writer may be blocked on a full output ring
        self.audio_buf.wake_blocked_writers();
        //AI_END===
        true
    }
    //AI_END===
    //AI_END===
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
