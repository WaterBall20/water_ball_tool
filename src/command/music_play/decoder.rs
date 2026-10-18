use crate::command::music_play::effect::resampler::ResamplerEffect;
use crate::command::music_play::effect::{AudioFormat, PreEffect, PreEffectChain};
use rubato::audioadapter_buffers::SizeError;
use rubato::{ResampleError, ResamplerConstructionError};
use std::any::Any;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
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
use water_ball_tool_lib::ring_buffer_wait::{RingBufferWait, RingBufferWaitError};

pub(super) struct Decoder {
    path_buf: PathBuf,
    thread_handle: Option<std::thread::JoinHandle<()>>,
    ring_buf: RingBufferWait<f32>,
    stop: Arc<AtomicBool>,
    engine: Arc<Mutex<DecoderEngine>>,
    event_list: Arc<Mutex<Vec<Box<dyn DecoderEventSink>>>>,
    data: DecoderData,
}

struct DecoderEngine {
    format: Box<dyn FormatReader>,
    track_id: u32,
    decoder: Box<dyn AudioDecoder>,
    effect_chain: PreEffectChain,
}

#[derive(Copy, Clone)]
struct DecoderData {
    input_format: AudioFormat,
    format_info: FormatInfo,
    media_info: MediaInfo,
    max_ring_buf_write_len: usize,
}

impl Decoder {
    fn emit(event: DecoderEvent, list: &Arc<Mutex<Vec<Box<dyn DecoderEventSink>>>>) {
        let event = Arc::new(event);
        let mut list = list.lock().expect("list is poisoned");
        for sink in list.iter_mut() {
            sink.emit(event.clone());
        }
    }
    fn write_all_to_ring_buf(
        ring_buf: &mut RingBufferWait<f32>,
        stop: &Arc<AtomicBool>,
        max_write_len: usize,
        event_list: &Arc<Mutex<Vec<Box<dyn DecoderEventSink>>>>,
        write_buf: &[f32],
    ) -> Result<(), DecoderEventError> {
        for chunk in write_buf.chunks(max_write_len) {
            // 检查停止信号
            if stop.load(Ordering::Acquire) {
                return Err(DecoderEventError {
                    message: "Decoder stop".to_string(),
                    kind: DecoderEventErrorKind::DecoderStop,
                });
            }
            // 分块后，每一块的长度都 ≤ CHUNK_SIZE ≤ ring 容量
            // 因此 write_all 不会返回 WriteSizeTooLarge
            if let Err(e) = ring_buf.write_all(chunk) {
                // 只可能是致命错误（锁异常、溢出等），上报后退出
                let e = DecoderEventError {
                    message: format!("write_all failed: {e}"),
                    kind: DecoderEventErrorKind::RingBufferError(e),
                };
                let event = DecoderEvent {
                    result: Err(e.clone()),
                };
                Self::emit(event, event_list);
                return Err(e);
            }
        }
        Ok(())
    }
}

impl Decoder {
    pub fn new(
        ring_buf: RingBufferWait<f32>,
        path: &Path,
        output_sample_rate: u32,
    ) -> DecoderResult<Self> {
        let stop = Arc::new(AtomicBool::new(false));
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
        //重采样
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

        let input_format = AudioFormat::new(channels as u16, decoder_sample_rate);
        let max_ring_buf_write_len = (decoder_sample_rate / 50) as usize;
        let mut effect_chain = PreEffectChain::new(input_format);

        let needs_resampling = decoder_sample_rate != output_sample_rate;
        if needs_resampling {
            //创建采样器
            effect_chain.push(ResamplerEffect::new(AudioFormat::new(
                u16::try_from(channels).expect("Too many channels"),
                output_sample_rate,
            )))?;
        }

        let engine = DecoderEngine {
            format,
            track_id,
            decoder,
            effect_chain,
        };

        let data = DecoderData {
            input_format,
            format_info,
            media_info,
            max_ring_buf_write_len,
        };

        Ok(Self {
            path_buf: path.to_path_buf(),
            thread_handle: None,
            ring_buf,
            stop,
            engine: Arc::new(Mutex::new(engine)),
            event_list: Arc::new(Mutex::new(Vec::new())),
            data,
        })
    }

    pub fn run(&mut self) -> DecoderResult<()> {
        if self.thread_handle.is_none() {
            let thread_handle = {
                let file_stem = self
                    .path_buf
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Unknown");
                let engine = self.engine.clone();
                let event_list = self.event_list.clone();
                let mut ring_buf = self.ring_buf.clone();
                let stop = self.stop.clone();
                let data = self.data;
                // 4. 解码循环
                std::thread::Builder::new()
                    .name(format!("decoder-{file_stem}"))
                    .spawn(move || -> () {
                        debug!("decoder run thread loop start");
                        //初始化
                        let ring_buf_max_len =
                            ring_buf.max_len().expect("ring_buf max_len is None");
                        let mut buf = Vec::with_capacity(ring_buf_max_len);
                        loop {
                            // 检查停止信号
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            // 获取锁
                            let mut engine = engine.lock().expect("engine is poisoned");
                            //事件列表
                            let packet = match engine.format.next_packet() {
                                Ok(Some(p)) => p,
                                Ok(None) => {
                                    Self::emit(
                                        DecoderEvent {
                                            result: Ok(DecoderEventType::Eof),
                                        },
                                        &event_list,
                                    );
                                    break;
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
                                    break;
                                }
                            };

                            if packet.track_id != engine.track_id {
                                continue;
                            }

                            match engine.decoder.decode(&packet) {
                                Ok(decoded) => {
                                    // 使用新方法进行交错转换
                                    decoded.copy_to_vec_interleaved(&mut buf);

                                    // 前效果处理
                                    if engine.effect_chain.is_empty() {
                                        // 写入环形缓冲区
                                        if Self::write_all_to_ring_buf(
                                            &mut ring_buf,
                                            &stop,
                                            data.max_ring_buf_write_len,
                                            &event_list,
                                            &buf,
                                        )
                                        .is_err()
                                        {
                                            break;
                                        }
                                    } else {
                                        let mut out_buf = Vec::new();
                                        let len = engine.effect_chain.process(&buf, &mut out_buf);
                                        if len == 0 {
                                            continue;
                                        }
                                        // 写入环形缓冲区
                                        if Self::write_all_to_ring_buf(
                                            &mut ring_buf,
                                            &stop,
                                            data.max_ring_buf_write_len,
                                            &event_list,
                                            &out_buf[..len],
                                        )
                                        .is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                                Err(symphonia::core::errors::Error::DecodeError(e)) => {
                                    error!("Decode error (skipping frame): {e}");
                                }
                                Err(e) => {
                                    error!("Fatal decode error: {e}");
                                    break;
                                }
                            }
                        }
                        debug!("decoder run thread loop end");
                    })
            }?;
            self.thread_handle = Some(thread_handle);
            Ok(())
        } else {
            Err(DecoderError::new("Running", DecoderErrorKind::Running))
        }
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

    pub fn stop(self) -> DecoderResult<()> {
        let mut s = self;
        s.stop.store(true, Ordering::Release);
        s.ring_buf.clear();
        s.join()
    }

    pub fn join(&mut self) -> DecoderResult<()> {
        if let Some(thread) = self.thread_handle.take()
            && let Err(e) = thread.join()
        {
            return Err(DecoderError {
                message: format!("Thread join error: {e:?}"),
                kind: DecoderErrorKind::ThreadJoinError(e),
            });
        }
        Ok(())
    }

    pub fn try_seek(
        &mut self,
        d: Duration,
    ) -> Result<symphonia::core::formats::SeekedTo, symphonia::core::errors::Error> {
        self.ring_buf.clear();
        let mut engine = self.engine.lock().expect("engine is poisoned");
        let track_id = engine.track_id;
        let r = engine.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from_millis(i64::try_from(d.as_millis()).unwrap_or(0)),
                track_id: Some(track_id),
            },
        )?;
        engine.decoder.reset();
        engine.effect_chain.reset(self.data.input_format);
        Ok(r)
    }

    pub fn format_info(&self) -> &FormatInfo {
        &self.data.format_info
    }

    pub fn media_info(&self) -> &MediaInfo {
        &self.data.media_info
    }

    pub fn add_audio_pre_effect<E>(&mut self, effect: E) -> Result<(), Box<dyn Error>>
    where
        E: PreEffect + Send + 'static,
    {
        self.engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .effect_chain
            .push(effect)
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
}

#[derive(Debug, Clone)]
pub(super) struct DecoderEventError {
    message: String,
    kind: DecoderEventErrorKind,
}
impl From<SizeError> for DecoderEventError {
    fn from(value: SizeError) -> Self {
        Self {
            message: value.to_string(),
            kind: DecoderEventErrorKind::InterleavedSliceSizeError(value),
        }
    }
}

impl From<ResampleError> for DecoderEventError {
    fn from(value: ResampleError) -> Self {
        Self {
            message: value.to_string(),
            kind: DecoderEventErrorKind::ResampleError(value),
        }
    }
}
#[derive(Debug)]
pub(super) enum DecoderEventErrorKind {
    RingBufferError(RingBufferWaitError<f32>),
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

    InterleavedSliceSizeError(SizeError),
    ResampleError(ResampleError),
    NextPacketError(String),
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
                _ => panic!("克隆失败，存在没有列举的类型, kind: {e:?}"),
            }),
            Self::Unsupported(e) => Self::Unsupported(e),
            Self::LimitError(e) => Self::LimitError(e),
            Self::ResetRequired => Self::ResetRequired,
            Self::InterleavedSliceSizeError(e) => Self::InterleavedSliceSizeError(match e {
                SizeError::ChannelsContainer { actual, required } => SizeError::ChannelsContainer {
                    actual: *actual,
                    required: *required,
                },
                SizeError::FramesContainer { actual, required } => SizeError::FramesContainer {
                    actual: *actual,
                    required: *required,
                },
                SizeError::ChannelBuffer {
                    index,
                    actual,
                    required,
                } => SizeError::ChannelBuffer {
                    index: *index,
                    actual: *actual,
                    required: *required,
                },
                SizeError::FrameBuffer {
                    index,
                    actual,
                    required,
                } => SizeError::FrameBuffer {
                    index: *index,
                    actual: *actual,
                    required: *required,
                },
                SizeError::Total { actual, required } => SizeError::Total {
                    actual: *actual,
                    required: *required,
                },
                SizeError::Mask { actual, required } => SizeError::Mask {
                    actual: *actual,
                    required: *required,
                },
            }),
            Self::ResampleError(e) => Self::ResampleError(*e),
            Self::NextPacketError(e) => Self::NextPacketError(e.clone()),
            Self::DecoderStop => Self::DecoderStop,
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
            DecoderErrorKind::Running => "Running",
            DecoderErrorKind::IOError(e) => &format!("IOError({e})"),
            DecoderErrorKind::ThreadJoinError(e) => &format!("ThreadJoinError({e:?})"),
            DecoderErrorKind::NoAudioTrack => "NoAudioTrack",
            DecoderErrorKind::NoAudioCodecParams => "NoAudioCodecParams",
            DecoderErrorKind::MakeAudioDecoderError(e) => &format!("MakeAudioDecoderError({e})"),
            DecoderErrorKind::GetProbeError(e) => &format!("GetProbeError({e})"),
            DecoderErrorKind::NoSampleRate => "NoSampleRate",
            DecoderErrorKind::NoChannels => "NoChannels",
            DecoderErrorKind::ResamplerConstructionError(e) => {
                &format!("ResamplerConstructionError({e})")
            }
            DecoderErrorKind::RingBufferNoMaxLen => "RingBufferNoMaxLen",
            DecoderErrorKind::Other(e) => &format!("Other({e})"),
        };
        write!(f, "DecoderError: Kind: {kind}, message: {}", self.message)
    }
}

impl Error for DecoderError {}

impl From<ResamplerConstructionError> for DecoderError {
    fn from(value: ResamplerConstructionError) -> Self {
        Self {
            message: value.to_string(),
            kind: DecoderErrorKind::ResamplerConstructionError(value),
        }
    }
}

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
    RingBufferNoMaxLen,
    Running,
    IOError(std::io::Error),
    ThreadJoinError(Box<dyn Any + Send + 'static>),
    NoAudioTrack,
    NoAudioCodecParams,
    MakeAudioDecoderError(symphonia::core::errors::Error),
    GetProbeError(symphonia::core::errors::Error),
    NoSampleRate,
    NoChannels,
    ResamplerConstructionError(ResamplerConstructionError),
    Other(Box<dyn Error + 'static>),
}

impl From<std::io::Error> for DecoderError {
    fn from(e: std::io::Error) -> Self {
        Self {
            message: e.to_string(),
            kind: DecoderErrorKind::IOError(e),
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
