use crate::command::music_play::effect::runtime::PreEffectRuntime;
use crate::command::music_play::effect::{AudioData, AudioFormat, PreEffectError};
use crate::command::music_play::tools;
use crate::command::music_play::tools::{DurationAccumulator, TimeMetrics};
use std::any::Any;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::SeekErrorKind;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{
    FormatInfo, FormatOptions, FormatReader, MediaInfo, SeekMode, SeekTo, TrackType,
};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;
use tracing::{debug, error};
//AI=== 只保留实际使用的类型（不再直接引用 `ConcurrentRingError`）/ keep only the types actually used (no direct `ConcurrentRingError` reference)
use water_ball_tool_lib::concurrent_ring::{ConcurrentRing, ConcurrentRingErrorKind};
//AI_END===

#[derive(Clone)]
pub(super) struct DecoderRuntime {
    thread_handle: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    ring_buf: ConcurrentRing<AudioData>,
    stop: Arc<AtomicBool>,
    resetting: Arc<AtomicBool>,
    //AI=== engine 与 data 合并为单一 session，二者必然成对存在 / merge engine and data into a single session so they always exist as a pair
    session: Arc<Mutex<Option<DecoderSession>>>,
    //AI_END===
    event_list: Arc<Mutex<Vec<Box<dyn DecoderEventSink>>>>,
    effect_runtime: PreEffectRuntime,
    time_metrics: Arc<TimeMetrics>,
    condvar: Arc<Condvar>,
}

struct DecoderEngine {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
}

struct DecoderData {
    track_id: u32,
    path_buf: PathBuf,
    decoder_format: AudioFormat,
    format_info: FormatInfo,
    media_info: MediaInfo,
    ring_buf_write_len: usize,
}

//AI=== 解码器会话：engine 与 data 必须成对出现，避免“一个有、一个没有”的不配对状态 / decoder session: engine and data must exist as a pair, preventing an unpaired "one present, one missing" state
struct DecoderSession {
    engine: DecoderEngine,
    data: DecoderData,
}
//AI_END===

//AI=== 环形缓冲区写入结果：主动拒绝（停止/切曲/寻道）属于正常控制流，必须与真实错误区分 / ring-write outcome: an intentional rejection (stop/track change/seek) is normal control flow and must be distinguished from a real error
enum RingWriteOutcome {
    /// 写入成功 / write succeeded
    Written,
    /// 写入被环主动拒绝（停止/切曲/寻道中），不是错误 / write rejected by the ring (stop/track change/seek), not an error
    Rejected,
    /// 已上报停止事件，调用方应停止解码 / stop event already emitted, the caller should stop decoding
    Stopped,
    /// 真实写入错误 / real write error
    Failed(DecoderEventError),
}
//AI_END===

//AI=== EOF 写入结果 / EOF-push outcome
enum EofPushOutcome {
    /// EOF 已写入环形缓冲区 / EOF pushed into the ring buffer
    Pushed,
    /// 写入被环主动拒绝（停止/切曲中），不是错误 / rejected by the ring (stop/track change), not an error
    Rejected,
    /// 真实错误 / real error
    Failed(DecoderEventError),
}
//AI_END===

impl DecoderRuntime {
    fn emit(event: DecoderEvent, list: &Arc<Mutex<Vec<Box<dyn DecoderEventSink>>>>) {
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
    //AI=== 写入失败一律返回结果（真实错误不再 panic），并区分主动拒绝与真实错误 / write failures always return an outcome (no panic on real errors), distinguishing intentional rejection from real errors
    fn write_all_to_ring_buf(
        ring_buf: &ConcurrentRing<AudioData>,
        stop: &Arc<AtomicBool>,
        event_list: &Arc<Mutex<Vec<Box<dyn DecoderEventSink>>>>,
        write_buf: &[f32],
        write_wait_acc: &mut DurationAccumulator,
    ) -> RingWriteOutcome {
        // 检查停止信号 / check the stop signal
        if stop.load(Ordering::Acquire) {
            Self::emit(
                DecoderEvent {
                    result: Ok(DecoderEventType::Stop),
                },
                event_list,
            );
            return RingWriteOutcome::Stopped;
        }
        let t1 = Instant::now();
        let push_result = ring_buf.push_wait(AudioData::Block(write_buf.to_vec()));
        write_wait_acc.record(t1.elapsed());
        match push_result {
            Ok(()) => RingWriteOutcome::Written,
            // WriteRejected/WriteRejectedPartial 是 reject_waiting_writers 主动拒绝的正常控制流（停止/切曲/寻道），不是运行期错误 / an intentional rejection by reject_waiting_writers during stop/track change/seek, not a runtime error
            Err(e) if matches!(&e.kind, ConcurrentRingErrorKind::WriteRejected(_)) => {
                RingWriteOutcome::Rejected
            }
            Err(e) => RingWriteOutcome::Failed(DecoderEventError::new(
                format!("ring_buf push failed: {e}"),
                DecoderEventErrorKind::RingBufferError(e.to_string()),
            )),
        }
    }
    //AI_END===
}

impl DecoderRuntime {
    pub fn new(
        ring_buf: ConcurrentRing<AudioData>,
        effect_runtime: PreEffectRuntime,
        time_metrics: Arc<TimeMetrics>,
    ) -> Self {
        Self {
            thread_handle: Arc::new(Mutex::new(None)),
            ring_buf,
            stop: Arc::new(AtomicBool::new(false)),
            //AI=== engine/data 两个 Option 合并为单一 session / merge the two engine/data Options into a single session
            session: Arc::new(Mutex::new(None)),
            //AI_END===
            resetting: Arc::new(AtomicBool::new(false)),
            event_list: Arc::new(Mutex::new(Vec::new())),
            effect_runtime,
            time_metrics,
            condvar: Arc::new(Condvar::new()),
        }
    }

    //AI=== run 返回 Result：线程创建失败向上传播；解码循环中的真实运行期错误上报后停止解码 / `run` returns Result: spawn failures propagate upward, and real runtime errors are reported then stop the decode loop
    pub fn run(&mut self) -> DecoderResult<()> {
        let mut thread_handle_lock = self
            .thread_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if thread_handle_lock.is_none() {
            let spawn_result = {
                let ring_buf = self.ring_buf.clone();
                let stop = self.stop.clone();
                let resetting = self.resetting.clone();
                let session = self.session.clone();
                let event_list = self.event_list.clone();
                let time_metrics = self.time_metrics.clone();
                let condvar = self.condvar.clone();
                // 4. 解码循环 / decode loop
                std::thread::Builder::new()
                    .name("decoder-runtime".into())
                    .spawn(move || -> () {
                        debug!("decoder run thread loop start");
                        let mut buf = Vec::new();
                        let mut out_buf = Vec::new();
                        //统计数据 / statistics
                        let mut decode_acc = DurationAccumulator::new();
                        let mut write_wait_acc = DurationAccumulator::new();
                        let mut blocks = 0;

                        loop {
                            // 检查停止信号 / check the stop signal
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            // 获取会话锁 / acquire the session lock
                            //AI=== 锁中毒时恢复内部数据而非 panic / recover from mutex poisoning instead of panicking
                            let mut session_lock = session
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            //AI_END===
                            //AI=== resetting 时在同一把锁内 take()，engine 与 data 必然成对移除 / take() under the same lock while resetting so engine and data are always removed as a pair
                            if resetting.load(Ordering::Acquire) {
                                session_lock.take();
                                resetting.store(false, Ordering::Release);
                            }
                            //AI_END===
                            //AI=== 没有会话时等待 condvar；原先“engine 有、data 无”的不配对状态已不可能出现 / wait on the condvar when there is no session; the previous unpaired "engine present, data missing" state is now impossible
                            let Some(session_ref) = session_lock.as_mut() else {
                                debug!("decoder runtime thread wait start");
                                let _unused = condvar
                                    .wait(session_lock)
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                debug!("decoder runtime thread wait end");
                                continue;
                            };
                            //AI_END===
                            //提前重分配 / pre-reserve
                            if buf.capacity() < session_ref.data.ring_buf_write_len {
                                buf.reserve_exact(session_ref.data.ring_buf_write_len);
                                out_buf.reserve_exact(session_ref.data.ring_buf_write_len);
                            }
                            //解码 / decode one packet
                            let t0 = Instant::now();
                            let packet = match session_ref.engine.format.next_packet() {
                                Ok(Some(p)) => {
                                    decode_acc.record(t0.elapsed());
                                    p
                                }
                                Ok(None) => {
                                    //AI=== EOF 写入失败必须上报错误并停止，且不再发送 Eof 成功事件；主动拒绝（停止/切曲）不算错误 / an EOF push failure must be reported and stop, and no Eof success event may be emitted; an intentional rejection (stop/track change) is not an error
                                    let eof_outcome = match ring_buf.push_wait(AudioData::Eof) {
                                        Ok(()) => EofPushOutcome::Pushed,
                                        Err(e)
                                        if matches!(
                                                &e.kind,
                                                ConcurrentRingErrorKind::WriteRejected(_)
                                            ) =>
                                            {
                                                EofPushOutcome::Rejected
                                            }
                                        Err(e) => EofPushOutcome::Failed(
                                            DecoderEventError::new(
                                                format!("push Eof failed: {e}"),
                                                DecoderEventErrorKind::RingBufferError(
                                                    e.to_string(),
                                                ),
                                            ),
                                        ),
                                    };
                                    if matches!(eof_outcome, EofPushOutcome::Pushed) {
                                        // 取出会话，下轮回到等待 / take the session; the next iteration waits
                                        session_lock.take();
                                    }
                                    drop(session_lock);
                                    match eof_outcome {
                                        EofPushOutcome::Pushed => {
                                            Self::emit(
                                                DecoderEvent {
                                                    result: Ok(DecoderEventType::Eof),
                                                },
                                                &event_list,
                                            );
                                            continue;
                                        }
                                        EofPushOutcome::Rejected => {
                                            // 停止/切曲中的主动拒绝：不发送 Eof 成功事件，交由循环顶部处理 stop/resetting / intentional rejection during stop/track change: no Eof success event; the loop top handles stop/resetting
                                            debug!("push Eof rejected by ring (stop/reset in progress) / 写入 EOF 被环拒绝（正在停止/重置）");
                                            continue;
                                        }
                                        EofPushOutcome::Failed(e) => {
                                            error!("push Eof failed: {e:?} / 写入 EOF 失败: {e:?}");
                                            Self::emit(
                                                DecoderEvent { result: Err(e) },
                                                &event_list,
                                            );
                                            break;
                                        }
                                    }
                                    //AI_END===
                                }
                                Err(e) => {
                                    //AI=== next_packet 失败属于真实运行期错误：上报后停止解码 / a next_packet failure is a real runtime error: report then stop decoding
                                    let event_error = DecoderEventError::new(
                                        format!("Error reading packet: {e}"),
                                        DecoderEventErrorKind::NextPacketError(e.to_string()),
                                    );
                                    drop(session_lock);
                                    error!("Error reading packet: {e}");
                                    Self::emit(
                                        DecoderEvent {
                                            result: Err(event_error),
                                        },
                                        &event_list,
                                    );
                                    break;
                                    //AI_END===
                                }
                            };

                            // 只处理当前音轨的帧 / process packets of the current track only
                            if packet.track_id != session_ref.data.track_id {
                                continue;
                            }

                            match session_ref.engine.decoder.decode(&packet) {
                                Ok(decoded) => {
                                    // 使用新方法进行交错转换 / interleave the decoded samples
                                    decoded.copy_to_vec_interleaved(&mut buf);
                                    out_buf.extend_from_slice(&buf);
                                    if out_buf.len() < session_ref.data.ring_buf_write_len {
                                        continue;
                                    }
                                    // 阻塞写入前释放会话锁（保持原先写入前释放 engine/data 锁的行为）/ release the session lock before the blocking write (same as releasing the engine/data locks before)
                                    drop(session_lock);
                                    //AI=== 写入结果需区分成功/主动拒绝/停止/真实错误；失败时清空 out_buf，不带着旧数据继续 / distinguish written/rejected/stopped/failed; clear out_buf on failure so stale data is not reused
                                    match Self::write_all_to_ring_buf(
                                        &ring_buf,
                                        &stop,
                                        &event_list,
                                        &out_buf,
                                        &mut write_wait_acc,
                                    ) {
                                        RingWriteOutcome::Written => {
                                            out_buf.clear();
                                        }
                                        RingWriteOutcome::Rejected => {
                                            // 停止/切曲/寻道中的主动拒绝：丢弃旧数据，由循环顶部处理 stop/resetting / intentional rejection during stop/track change/seek: discard stale data; the loop top handles stop/resetting
                                            out_buf.clear();
                                            continue;
                                        }
                                        RingWriteOutcome::Stopped => {
                                            // 停止事件已由 write_all_to_ring_buf 上报 / the stop event was already emitted by write_all_to_ring_buf
                                            out_buf.clear();
                                            break;
                                        }
                                        RingWriteOutcome::Failed(e) => {
                                            out_buf.clear();
                                            error!("write to ring buf failed: {e:?} / 写入环形缓冲区失败: {e:?}");
                                            Self::emit(
                                                DecoderEvent { result: Err(e) },
                                                &event_list,
                                            );
                                            break;
                                        }
                                    }
                                    //AI_END===
                                    //统计 / statistics
                                    blocks += 1;
                                    if blocks % tools::DURATION_ACCUMULATOR_FLUSH_INTERVAL == 0 {
                                        decode_acc.flush_into(&time_metrics.decode);
                                        write_wait_acc
                                            .flush_into(&time_metrics.decode_write_wait);
                                    }
                                }
                                Err(symphonia::core::errors::Error::DecodeError(e)) => {
                                    //AI=== 单帧 DecodeError 可恢复：上报后跳过该帧继续，保持原行为 / a single-frame DecodeError is recoverable: report then skip the frame, preserving the old behavior
                                    let event_error = DecoderEventError::new(
                                        format!("Decode error (skipping frame): {e}"),
                                        DecoderEventErrorKind::DecodeError(e),
                                    );
                                    drop(session_lock);
                                    error!("Decode error (skipping frame): {e}");
                                    Self::emit(
                                        DecoderEvent {
                                            result: Err(event_error),
                                        },
                                        &event_list,
                                    );
                                    continue;
                                    //AI_END===
                                }
                                Err(e) => {
                                    //AI=== 致命解码错误（非帧级 DecodeError）：上报后停止解码线程 / fatal decode error (not a frame-level DecodeError): report then stop the decode thread
                                    let message = format!("Fatal decode error, err:{e}");
                                    error!("Fatal decode error: {e}");
                                    let event_error =
                                        DecoderEventError::new(message, DecoderEventErrorKind::from(e));
                                    drop(session_lock);
                                    Self::emit(
                                        DecoderEvent {
                                            result: Err(event_error),
                                        },
                                        &event_list,
                                    );
                                    break;
                                    //AI_END===
                                }
                            }
                        }
                        debug!("decoder run thread loop end");
                    })
            };
            //AI=== 线程创建失败必须返回错误，不能记录后继续 / a spawn failure must be returned instead of logged and ignored
            let thread_handle = spawn_result.map_err(|e| DecoderError {
                message: format!("create decoder thread failed: {e}"),
                kind: DecoderErrorKind::ThreadSpawnError(e),
            })?;
            //AI_END===
            thread_handle_lock.replace(thread_handle);
        }
        Ok(())
    }
    //AI_END===

    pub fn set_resetting(&self) {
        self.resetting.store(true, Ordering::Release);
    }

    pub fn set_decoder_path(&mut self, path: &Path) -> DecoderResult<()> {
        // 1. 打开文件 & 创建 MSS
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

        // 2. 建议探测格式
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| DecoderError {
                message: format!("get_probe error: {e}"),
                kind: DecoderErrorKind::GetProbeError(e),
            })?;

        // 3. 创建解码器
        let track = format.default_track(TrackType::Audio).ok_or(DecoderError {
            message: "No audio track".to_string(),
            kind: DecoderErrorKind::NoAudioTrack,
        })?;
        let track_id = track.id;
        let format_info = *format.format_info();
        let media_info = *format.media_info();
        let codec_params =
            track
                .codec_params
                .as_ref()
                .and_then(|p| p.audio())
                .ok_or(DecoderError {
                    message: "No audio codec params".to_string(),
                    kind: DecoderErrorKind::NoAudioCodecParams,
                })?;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(codec_params, &AudioDecoderOptions::default())
            .map_err(|e| DecoderError {
                message: format!("make audio decoder error: {e}"),
                kind: DecoderErrorKind::MakeAudioDecoderError(e),
            })?;
        let decoder_sample_rate = codec_params.sample_rate.ok_or(DecoderError {
            message: "sample rate not available".to_string(),
            kind: DecoderErrorKind::NoSampleRate,
        })?;

        let channels = codec_params
            .channels
            .as_ref()
            .map(symphonia::core::audio::Channels::count)
            .ok_or(DecoderError {
                message: "channels not available".to_string(),
                kind: DecoderErrorKind::NoChannels,
            })?;

        let decoder_format = AudioFormat::new(channels as u16, decoder_sample_rate);
        let ring_buf_write_len = (decoder_sample_rate / 50) as usize;
        let data = DecoderData {
            track_id,
            path_buf: path.to_path_buf(),
            decoder_format,
            format_info,
            media_info,
            ring_buf_write_len,
        };

        let engine = DecoderEngine { format, decoder };

        //更改解码器 / replace the decoder
        //AI=== 在同一把锁内成对替换 engine 与 data，避免再出现不配对状态 / replace engine and data as a pair under a single lock, so no unpaired state can occur
        let session = DecoderSession { engine, data };
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(session);
        //AI_END===
        self.effect_runtime.reset(decoder_format)?;
        self.resetting.store(false, Ordering::Release);
        self.condvar.notify_one(); //唤醒
        Ok(())
    }

    pub fn decoder_format(&self) -> Option<AudioFormat> {
        //AI=== 从合并后的 session 读取 / read from the merged session
        let session = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        session.as_ref().map(|session| session.data.decoder_format)
        //AI_END===
    }

    pub fn add_event(&mut self, event: Box<dyn DecoderEventSink>) {
        self.event_list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }

    pub fn add_event_insert(&mut self, index: usize, event: Box<dyn DecoderEventSink>) {
        self.event_list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(index, event);
    }

    pub fn stop(&mut self) -> DecoderResult<()> {
        self.stop.store(true, Ordering::Release);
        self.condvar.notify_one();
        //AI=== send_skip 失败也必须返回错误：先尝试发送 skip，始终执行 join，最后返回第一个错误 / a send_skip failure must be returned too: try skipping first, always join, then return the first error
        let skip_result = self.effect_runtime.send_skip().map_err(DecoderError::from);
        let join_result = self.join();
        skip_result?;
        join_result
        //AI_END===
    }

    pub fn join(&mut self) -> DecoderResult<()> {
        let mut thread_handle = self
            .thread_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(thread) = thread_handle.take()
            && let Err(e) = thread.join()
        {
            return Err(DecoderError {
                message: format!("Thread join error: {e:?}"),
                kind: DecoderErrorKind::ThreadJoinError(e),
            });
        }
        Ok(())
    }

    pub fn try_seek(&mut self, d: Duration) -> DecoderResult<symphonia::core::formats::SeekedTo> {
        self.ring_buf.reject_waiting_writers();
        //AI=== 从合并后的 session 同时取 engine/data / take engine and data from the merged session together
        let mut session = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = session.as_mut() else {
            return Err(DecoderError::new("no decoder", DecoderErrorKind::NoDecoder));
        };
        let track_id = session.data.track_id;
        let r = session.engine.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from_millis(i64::try_from(d.as_millis()).unwrap_or(0)),
                track_id: Some(track_id),
            },
        )?;
        session.engine.decoder.reset();
        self.effect_runtime.reset(session.data.decoder_format)?;
        Ok(r)
        //AI_END===
    }

    pub fn format_info(&self) -> Option<FormatInfo> {
        //AI=== 从合并后的 session 读取 / read from the merged session
        let session = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        session.as_ref().map(|session| session.data.format_info)
        //AI_END===
    }

    pub fn media_info(&self) -> Option<MediaInfo> {
        //AI=== 从合并后的 session 读取 / read from the merged session
        let session = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        session.as_ref().map(|session| session.data.media_info)
        //AI_END===
    }
}

pub trait DecoderEventSink: Send + 'static {
    fn emit(&mut self, event: Arc<DecoderEvent>);
    fn type_of(&self) -> &'static str;
}

pub struct DecoderEventChannelSink(Sender<Arc<DecoderEvent>>);

//AI=== 构造函数从 trait 移出为固有方法；线程创建失败必须返回 io::Error / constructors moved out of the trait into inherent methods; a spawn failure must return io::Error
impl DecoderEventChannelSink {
    pub fn new<F>(f: F) -> std::io::Result<Self>
    where
        F: Fn(&DecoderEvent) + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel::<Arc<DecoderEvent>>();
        std::thread::Builder::new()
            .name("decoder_event_channel_sink".to_string())
            .spawn(move || {
                for event in receiver {
                    f(&event);
                }
            })?;
        Ok(Self(sender))
    }
}
//AI_END===

impl DecoderEventSink for DecoderEventChannelSink {
    fn emit(&mut self, event: Arc<DecoderEvent>) {
        // send 非阻塞；满了就丢弃（或 try_send + 计数）/ non-blocking send; the event is dropped when the channel is full
        let _ = self.0.send(event);
    }

    fn type_of(&self) -> &'static str {
        "DecoderEventChannelSink"
    }
}

pub struct DecoderEventDirectSink(Box<dyn Fn(&DecoderEvent) + Send + 'static>);

//AI=== 直接由解码线程调用的 sink 构造函数 / constructor for the sink invoked directly from the decode thread
impl DecoderEventDirectSink {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&DecoderEvent) + Send + 'static,
    {
        Self(Box::new(f))
    }
}
//AI_END===

impl DecoderEventSink for DecoderEventDirectSink {
    fn emit(&mut self, event: Arc<DecoderEvent>) {
        self.0(&event); // 就在解码线程里跑 / runs on the decode thread
    }

    fn type_of(&self) -> &'static str {
        "DecoderEventDirectSink"
    }
}

#[derive(Debug, Clone)]
pub(super) struct DecoderEvent {
    result: Result<DecoderEventType, DecoderEventError>,
}

impl DecoderEvent {
    pub fn result(&self) -> &Result<DecoderEventType, DecoderEventError> {
        &self.result
    }
    pub fn get_result(self) -> Result<DecoderEventType, DecoderEventError> {
        self.result
    }
}

#[derive(Debug, Clone)]
pub(super) enum DecoderEventType {
    Eof,
    Stop,
}

#[derive(Debug, Clone)]
pub(super) struct DecoderEventError {
    message: String,
    kind: DecoderEventErrorKind,
}

//AI=== 统一的错误事件构造入口 / single construction entry for error events
impl DecoderEventError {
    fn new(message: impl Into<String>, kind: DecoderEventErrorKind) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }
}
//AI_END===

#[derive(Debug)]
pub(super) enum DecoderEventErrorKind {
    //AI=== 改为保存错误描述：原先的 `ConcurrentRingError<f32>` 与 `ConcurrentRing<AudioData>` 的推送错误类型不匹配且从未被构造 / store the error description: the old `ConcurrentRingError<f32>` did not match the AudioData push error type and was never constructed
    RingBufferError(String),
    //AI_END===
    /// An IO error occured while reading, writing, or seeking the stream.
    IoError(std::io::Error),
    /// The stream contained malformed data and could not be decoded or demuxed.
    DecodeError(&'static str),
    /// The stream could not be seeked.
    SeekError(SeekErrorKind),
    /// An unsupported container or codec feature was encounted.
    Unsupported(&'static str),
    /// A default or user-defined limit was reached while decoding or demuxing the stream. Limits
    /// are used to prevent denial-of-service attacks from malicious streams.
    LimitError(&'static str),
    /// The demuxer or decoder needs to be reset before continuing.
    ResetRequired,

    /// 获取下一帧错误 / error while reading the next packet
    NextPacketError(String),
    /// 解码停止 / decode stopped
    DecoderStop,
    //AI=== 外部非穷尽枚举的未知错误兜底，避免 panic / fallback for unknown variants of external non-exhaustive enums, avoiding panic
    /// 其他未定义错误 / other undefined error
    Other(String),
    //AI_END===
}
impl Clone for DecoderEventErrorKind {
    fn clone(&self) -> Self {
        match &self {
            Self::RingBufferError(e) => Self::RingBufferError(e.clone()),
            Self::IoError(e) => Self::IoError(std::io::Error::new(e.kind(), e.to_string())),
            Self::DecodeError(e) => Self::DecodeError(e),
            //AI=== `SeekErrorKind` 是非穷尽外部枚举：已知变体逐个重建，未知变体退化为 Other 而不是 panic / `SeekErrorKind` is a non-exhaustive external enum: rebuild known variants, degrade unknown ones to Other instead of panicking
            Self::SeekError(e) => Self::SeekError(match e {
                SeekErrorKind::Unseekable => SeekErrorKind::Unseekable,
                SeekErrorKind::ForwardOnly => SeekErrorKind::ForwardOnly,
                SeekErrorKind::OutOfRange => SeekErrorKind::OutOfRange,
                SeekErrorKind::InvalidTrack => SeekErrorKind::InvalidTrack,
                other => {
                    return Self::Other(format!("unknown symphonia SeekErrorKind: {other:?}"));
                }
            }),
            //AI_END===
            Self::Unsupported(e) => Self::Unsupported(e),
            Self::LimitError(e) => Self::LimitError(e),
            Self::ResetRequired => Self::ResetRequired,
            Self::NextPacketError(e) => Self::NextPacketError(e.clone()),
            Self::DecoderStop => Self::DecoderStop,
            //AI=== 未知错误兜底 / fallback for undefined errors
            Self::Other(e) => Self::Other(e.clone()),
            //AI_END===
        }
    }
}

impl From<symphonia::core::errors::Error> for DecoderEventErrorKind {
    fn from(value: symphonia::core::errors::Error) -> Self {
        match value {
            symphonia::core::errors::Error::IoError(e) => Self::IoError(e),
            symphonia::core::errors::Error::DecodeError(e) => Self::DecodeError(e),
            symphonia::core::errors::Error::SeekError(e) => Self::SeekError(e),
            symphonia::core::errors::Error::Unsupported(e) => Self::Unsupported(e),
            symphonia::core::errors::Error::LimitError(e) => Self::LimitError(e),
            symphonia::core::errors::Error::ResetRequired => Self::ResetRequired,
            //AI=== 非穷尽外部枚举的未知变体是可能发生的运行期错误，映射为 Other 而不是 panic / unknown variants of a non-exhaustive external enum are possible runtime errors; map them to Other instead of panicking
            other => Self::Other(format!("unknown symphonia error: {other:?}")),
            //AI_END===
        }
    }
}

pub(super) type DecoderResult<T> = Result<T, DecoderError>;

#[derive(Debug)]
pub(super) struct DecoderError {
    message: String,
    kind: DecoderErrorKind,
}

impl Display for DecoderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.kind {
            DecoderErrorKind::IoError(e) => &format!("IOError({e})"),
            DecoderErrorKind::ThreadJoinError(e) => &format!("ThreadJoinError({e:?})"),
            //AI=== 解码线程创建失败 / decode thread creation failure
            DecoderErrorKind::ThreadSpawnError(e) => &format!("ThreadSpawnError({e})"),
            //AI_END===
            DecoderErrorKind::NoAudioTrack => "NoAudioTrack",
            DecoderErrorKind::NoAudioCodecParams => "NoAudioCodecParams",
            DecoderErrorKind::MakeAudioDecoderError(e) => &format!("MakeAudioDecoderError({e})"),
            DecoderErrorKind::GetProbeError(e) => &format!("GetProbeError({e})"),
            DecoderErrorKind::NoSampleRate => "NoSampleRate",
            DecoderErrorKind::NoChannels => "NoChannels",
            DecoderErrorKind::NoDecoder => "NoDecoder",
            DecoderErrorKind::ProEffectError(e) => &format!("ProEffectError({e})"),
            DecoderErrorKind::DecodeError(e) => &format!("DecodeError({e})"),
            DecoderErrorKind::SeekError(e) => &format!("SeekError({e:?})"),
            DecoderErrorKind::Unsupported(e) => &format!("Unsupported({e})"),
            DecoderErrorKind::LimitError(e) => &format!("LimitError({e})"),
            DecoderErrorKind::ResetRequired => "ResetRequired",
            DecoderErrorKind::Other(e) => &format!("Other({e})"),
        };
        write!(f, "DecoderError: Kind: {kind}, message: {}", self.message)
    }
}

impl From<symphonia::core::errors::Error> for DecoderError {
    fn from(value: symphonia::core::errors::Error) -> Self {
        Self {
            message: format!("symphonia error: {value}"),
            kind: DecoderErrorKind::from(value),
        }
    }
}

impl From<PreEffectError> for DecoderError {
    fn from(value: PreEffectError) -> Self {
        Self {
            message: format!("PreeffectError, err:{value}"),
            kind: DecoderErrorKind::from(value),
        }
    }
}

impl Error for DecoderError {}

impl DecoderError {
    fn new(message: &str, kind: DecoderErrorKind) -> Self {
        Self {
            message: message.to_string(),
            kind,
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn kind(&self) -> &DecoderErrorKind {
        &self.kind
    }

    pub fn get_kind(self) -> DecoderErrorKind {
        self.kind
    }
}

#[derive(Debug)]
pub enum DecoderErrorKind {
    IoError(std::io::Error),
    /// 线程等待发生错误 / error while waiting on the thread
    ThreadJoinError(Box<dyn Any + Send + 'static>),
    //AI=== 解码线程创建失败 / decode thread creation failure
    /// 解码线程创建失败 / decode thread creation failure
    ThreadSpawnError(std::io::Error),
    //AI_END===
    /// 没有音轨 / no audio track
    NoAudioTrack,
    /// 没有声音编码参数
    NoAudioCodecParams,
    /// 创建解码器失败
    MakeAudioDecoderError(symphonia::core::errors::Error),
    /// 获取解码帧错误
    GetProbeError(symphonia::core::errors::Error),
    /// 没有采样率
    NoSampleRate,
    /// 没有声道
    NoChannels,
    ///没有解码器
    NoDecoder,
    /// 重置后效果器错误
    ProEffectError(PreEffectError),

    /// The stream contained malformed data and could not be decoded or demuxed.
    DecodeError(&'static str),
    /// The stream could not be seeked.
    SeekError(SeekErrorKind),
    /// An unsupported container or codec feature was encounted.
    Unsupported(&'static str),
    /// A default or user-defined limit was reached while decoding or demuxing the stream. Limits
    /// are used to prevent denial-of-service attacks from malicious streams.
    LimitError(&'static str),
    /// The demuxer or decoder needs to be reset before continuing.
    ResetRequired,

    /// 其他错误
    Other(Box<dyn Error + 'static>),
}

impl From<std::io::Error> for DecoderError {
    fn from(e: std::io::Error) -> Self {
        Self {
            message: e.to_string(),
            kind: DecoderErrorKind::IoError(e),
        }
    }
}

impl From<Box<dyn Error + 'static>> for DecoderError {
    fn from(e: Box<dyn Error + 'static>) -> Self {
        Self {
            message: e.to_string(),
            kind: DecoderErrorKind::Other(e),
        }
    }
}

impl From<symphonia::core::errors::Error> for DecoderErrorKind {
    fn from(value: symphonia::core::errors::Error) -> Self {
        match value {
            symphonia::core::errors::Error::IoError(e) => Self::IoError(e),
            symphonia::core::errors::Error::DecodeError(e) => Self::DecodeError(e),
            symphonia::core::errors::Error::SeekError(e) => Self::SeekError(e),
            symphonia::core::errors::Error::Unsupported(e) => Self::Unsupported(e),
            symphonia::core::errors::Error::LimitError(e) => Self::LimitError(e),
            symphonia::core::errors::Error::ResetRequired => Self::ResetRequired,
            //AI=== 非穷尽外部枚举的未知变体映射为 Other，避免 panic / map unknown variants of the non-exhaustive external enum to Other instead of panicking
            other => Self::Other(Box::new(other)),
            //AI_END===
        }
    }
}

impl From<PreEffectError> for DecoderErrorKind {
    fn from(value: PreEffectError) -> Self {
        Self::ProEffectError(value)
    }
}
