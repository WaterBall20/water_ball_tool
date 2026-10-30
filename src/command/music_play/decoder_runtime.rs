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
use water_ball_tool_lib::concurrent_ring::{
    ConcurrentRing, ConcurrentRingError, ConcurrentRingErrorKind,
};

#[derive(Clone)]
pub(super) struct DecoderRuntime {
    thread_handle: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    ring_buf: ConcurrentRing<AudioData>,
    stop: Arc<AtomicBool>,
    resetting: Arc<AtomicBool>,
    engine: Arc<Mutex<Option<DecoderEngine>>>,
    data: Arc<Mutex<Option<DecoderData>>>,
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

impl DecoderRuntime {
    fn emit(event: DecoderEvent, list: &Arc<Mutex<Vec<Box<dyn DecoderEventSink>>>>) {
        let event = Arc::new(event);
        let mut list = list.lock().expect("list is poisoned");
        for sink in list.iter_mut() {
            sink.emit(event.clone());
        }
    }
    fn write_all_to_ring_buf(
        ring_buf: &ConcurrentRing<AudioData>,
        stop: &Arc<AtomicBool>,
        event_list: &Arc<Mutex<Vec<Box<dyn DecoderEventSink>>>>,
        write_buf: &[f32],
        write_wait_acc: &mut DurationAccumulator,
    ) -> Result<(), DecoderEventError> {
        // 检查停止信号
        if stop.load(Ordering::Acquire) {
            let e = DecoderEventError {
                message: "Decoder stop".to_string(),
                kind: DecoderEventErrorKind::DecoderStop,
            };
            Self::emit(
                DecoderEvent {
                    result: Ok(DecoderEventType::Stop),
                },
                event_list,
            );
            return Err(e);
        }
        let t1 = Instant::now();
        if let Err(e) = ring_buf.push_wait(AudioData::Block(write_buf.to_vec())) {
            if let ConcurrentRingErrorKind::WriteRejected(_) = e.kind {
            } else {
                panic!("ring_buf push failed, err: {e:?}");
            }
        }
        write_wait_acc.record(t1.elapsed());
        Ok(())
    }
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
            engine: Arc::new(Mutex::new(None)),
            data: Arc::new(Mutex::new(None)),
            resetting: Arc::new(AtomicBool::new(false)),
            event_list: Arc::new(Mutex::new(Vec::new())),
            effect_runtime,
            time_metrics,
            condvar: Arc::new(Condvar::new()),
        }
    }

    pub fn run(&mut self) {
        let mut thread_handle_lock = self
            .thread_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if thread_handle_lock.is_none() {
            let thread_handle = {
                let ring_buf = self.ring_buf.clone();
                let stop = self.stop.clone();
                let resetting = self.resetting.clone();
                let engine = self.engine.clone();
                let data = self.data.clone();
                let event_list = self.event_list.clone();
                let time_metrics = self.time_metrics.clone();
                let condvar = self.condvar.clone();
                // 4. 解码循环
                std::thread::Builder::new()
                    .name("decoder-runtime".into())
                    .spawn(move || -> () {
                        debug!("decoder run thread loop start");
                        let mut buf = Vec::new();
                        let mut out_buf = Vec::new();
                        //统计数据
                        let mut decode_acc = DurationAccumulator::new();
                        let mut write_wait_acc = DurationAccumulator::new();
                        let mut blocks = 0;

                        loop {
                            // 检查停止信号
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            // 获取锁
                            let mut engine_lock = engine.lock().expect("engine is poisoned");
                            if resetting.load(Ordering::Acquire) {
                                engine_lock.take();
                                resetting.store(false, Ordering::Release);
                            }
                            if let Some(engine) = &mut *engine_lock {
                                let data_lock = data
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                let data = data_lock.as_ref().expect("data is none");
                                //提前重分配
                                if buf.capacity() < data.ring_buf_write_len {
                                    buf.reserve_exact(data.ring_buf_write_len);
                                    out_buf.reserve_exact(data.ring_buf_write_len);
                                }
                                //解码
                                let t0 = Instant::now();
                                let packet = match engine.format.next_packet() {
                                    Ok(e) => {
                                        decode_acc.record(t0.elapsed());
                                        if let Some(p) = e {
                                            p
                                        } else {
                                            debug!("decoder next_packet none");
                                            ring_buf.push_wait(AudioData::Eof).expect(
                                                "push Eof \
                                        failed",
                                            );
                                            engine_lock.take();
                                            drop(engine_lock);
                                            drop(data_lock);
                                            Self::emit(
                                                DecoderEvent {
                                                    result: Ok(DecoderEventType::Eof),
                                                },
                                                &event_list,
                                            );
                                            continue;
                                        }
                                    }
                                    Err(e) => {
                                        error!("Error reading packet: {e}");
                                        Self::emit(
                                            DecoderEvent {
                                                result: Err(DecoderEventError {
                                                    message: format!("Error reading packet: {e}"),
                                                    kind: DecoderEventErrorKind::NextPacketError(
                                                        e.to_string(),
                                                    ),
                                                }),
                                            },
                                            &event_list,
                                        );
                                        continue;
                                    }
                                };

                                if packet.track_id != data.track_id {
                                    continue;
                                }

                                match engine.decoder.decode(&packet) {
                                    Ok(decoded) => {
                                        // 使用新方法进行交错转换
                                        decoded.copy_to_vec_interleaved(&mut buf);
                                        out_buf.extend_from_slice(&buf);
                                        if out_buf.len() < data.ring_buf_write_len {
                                            continue;
                                        }

                                        // 写入环形缓冲区
                                        drop(engine_lock);
                                        drop(data_lock);
                                        if Self::write_all_to_ring_buf(
                                            &ring_buf,
                                            &stop,
                                            &event_list,
                                            &out_buf,
                                            &mut write_wait_acc,
                                        )
                                        .is_err()
                                        {
                                            continue;
                                        }
                                        out_buf.clear();
                                        //统计
                                        blocks += 1;
                                        if blocks % tools::DURATION_ACCUMULATOR_FLUSH_INTERVAL == 0
                                        {
                                            decode_acc.flush_into(&time_metrics.decode);
                                            write_wait_acc
                                                .flush_into(&time_metrics.decode_write_wait);
                                        }
                                    }
                                    Err(symphonia::core::errors::Error::DecodeError(e)) => {
                                        error!("Decode error (skipping frame): {e}");
                                        Self::emit(
                                            DecoderEvent {
                                                result: Err(DecoderEventError {
                                                    message: format!(
                                                        "Decode error (skipping frame): {e}"
                                                    ),
                                                    kind: DecoderEventErrorKind::DecodeError(e),
                                                }),
                                            },
                                            &event_list,
                                        );
                                    }
                                    Err(e) => {
                                        error!("Fatal decode error: {e}");
                                        Self::emit(
                                            DecoderEvent {
                                                result: Err(DecoderEventError {
                                                    message: format!("Fatal decode error, err:{e}"),
                                                    kind: DecoderEventErrorKind::from(e),
                                                }),
                                            },
                                            &event_list,
                                        );
                                    }
                                }
                            } else {
                                debug!("decoder runtime thread wait start");
                                let _unused = condvar
                                    .wait(engine_lock)
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                debug!("decoder runtime thread wait end");
                            }
                        }
                        debug!("decoder run thread loop end");
                    })
            }
            .expect("创建线程错误");
            thread_handle_lock.replace(thread_handle);
        }
    }

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

        //更改解码器
        let mut engine_lock = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        engine_lock.replace(engine);
        self.data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(data);
        self.effect_runtime.reset(decoder_format)?;
        self.resetting.store(false, Ordering::Release);
        self.condvar.notify_one(); //唤醒
        Ok(())
    }

    pub fn decoder_format(&self) -> Option<AudioFormat> {
        let data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (*data).as_ref().map(|data| data.decoder_format)
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
        self.effect_runtime.send_skip().expect("send skip failed");
        self.join()
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
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(engine) = &mut *engine
            && let Some(data) = &*data
        {
            let track_id = data.track_id;
            let r = engine.format.seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from_millis(i64::try_from(d.as_millis()).unwrap_or(0)),
                    track_id: Some(track_id),
                },
            )?;
            engine.decoder.reset();
            self.effect_runtime.reset(data.decoder_format)?;
            Ok(r)
        } else {
            Err(DecoderError::new("no decoder", DecoderErrorKind::NoDecoder))
        }
    }

    pub fn format_info(&self) -> Option<FormatInfo> {
        let data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (*data).as_ref().map(|data| data.format_info)
    }

    pub fn media_info(&self) -> Option<MediaInfo> {
        let data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (*data).as_ref().map(|data| data.media_info)
    }
}

pub trait DecoderEventSink: Send + 'static {
    fn emit(&mut self, event: Arc<DecoderEvent>);
    fn new<F>(f: F) -> Self
    where
        F: Fn(&DecoderEvent) + Send + 'static,
        Self: Sized;
    fn type_of(&self) -> &'static str;
}

pub struct DecoderEventChannelSink(Sender<Arc<DecoderEvent>>);

impl DecoderEventSink for DecoderEventChannelSink {
    fn emit(&mut self, event: Arc<DecoderEvent>) {
        // send 非阻塞；满了就丢弃（或 try_send + 计数）
        let _ = self.0.send(event);
    }

    fn new<F>(f: F) -> Self
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
            })
            .expect("创建线程失败");
        Self(sender)
    }

    fn type_of(&self) -> &'static str {
        "DecoderEventChannelSink"
    }
}

pub struct DecoderEventDirectSink(Box<dyn Fn(&DecoderEvent) + Send + 'static>);

impl DecoderEventSink for DecoderEventDirectSink {
    fn emit(&mut self, event: Arc<DecoderEvent>) {
        self.0(&event); // 就在解码线程里跑
    }

    fn new<F>(f: F) -> Self
    where
        F: Fn(&DecoderEvent) + Send + 'static,
    {
        Self(Box::new(f))
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

#[derive(Debug)]
pub(super) enum DecoderEventErrorKind {
    RingBufferError(ConcurrentRingError<f32>),
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

    /// 获取下一帧错误
    NextPacketError(String),
    /// 解码停止
    DecoderStop,
}
impl Clone for DecoderEventErrorKind {
    fn clone(&self) -> Self {
        match &self {
            Self::RingBufferError(e) => Self::RingBufferError(e.clone()),
            Self::IoError(e) => Self::IoError(std::io::Error::new(e.kind(), e.to_string())),
            Self::DecodeError(e) => Self::DecodeError(e),
            Self::SeekError(e) => Self::SeekError(match e {
                SeekErrorKind::Unseekable => SeekErrorKind::Unseekable,
                SeekErrorKind::ForwardOnly => SeekErrorKind::ForwardOnly,
                SeekErrorKind::OutOfRange => SeekErrorKind::OutOfRange,
                SeekErrorKind::InvalidTrack => SeekErrorKind::InvalidTrack,
                _ => panic!("未定义错误, kind: {e:?}"),
            }),
            Self::Unsupported(e) => Self::Unsupported(e),
            Self::LimitError(e) => Self::LimitError(e),
            Self::ResetRequired => Self::ResetRequired,
            Self::NextPacketError(e) => Self::NextPacketError(e.clone()),
            Self::DecoderStop => Self::DecoderStop,
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
            _ => panic!("未定义错误, kind: {value:?}"),
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
    /// 线程等待发生错误
    ThreadJoinError(Box<dyn Any + Send + 'static>),
    /// 没有音轨
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
            _ => panic!("未定义错误, kind: {value:?}"),
        }
    }
}

impl From<PreEffectError> for DecoderErrorKind {
    fn from(value: PreEffectError) -> Self {
        Self::ProEffectError(value)
    }
}
