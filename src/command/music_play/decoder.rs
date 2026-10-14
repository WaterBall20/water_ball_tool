use rubato::audioadapter_buffers::SizeError;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, ResampleError, Resampler, ResamplerConstructionError};
use std::any::Any;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::errors::SeekErrorKind;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;
use tracing::{debug, error};
use water_ball_tool_lib::ring_buffer_wait::{RingBufferWait, RingBufferWaitError};

const RESAMPLER_CHUNK_IN: usize = 1024;

pub(super) struct Decoder {
    thread_handle: Option<std::thread::JoinHandle<()>>, //
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
    resampler: Option<DecoderResampler>,
}

struct DecoderResampler {
    fft_resampler: Fft<f32>,
    in_buf: Vec<f32>,
}

struct DecoderData {
    channels: usize,
    resampler_max_out_frames: usize,
}

impl Decoder {
    pub fn new(
        ring_buf: RingBufferWait<f32>,
        path: PathBuf,
        output_sample_rate: u32,
    ) -> DecoderResult<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        // 1. 打开文件 & 创建 MSS
        let file = File::open(&path)?;
        let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

        // 2. 探测格式
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
            .make_audio_decoder(codec_params, &Default::default())
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

        let needs_resampling = decoder_sample_rate != output_sample_rate;
        let (resampler, resampler_max_out_frames) = if needs_resampling {
            // 预分配输出缓冲区（容量取输入帧数按比例放大再留余量）
            let fft_resampler = Fft::<f32>::new(
                decoder_sample_rate as usize,
                output_sample_rate as usize,
                RESAMPLER_CHUNK_IN, // 如 1024
                channels,
                FixedSync::Input,
            )?;
            let max_out_frames = fft_resampler.output_frames_max();
            (
                Some(DecoderResampler {
                    fft_resampler,
                    in_buf: Vec::with_capacity(match ring_buf.max_len() {
                        Some(v) => v,
                        None => {
                            return Err(DecoderError::new(
                                "ring_buf no max len",
                                DecoderErrorKind::RingBufferNoMaxLen,
                            ));
                        }
                    }),
                }),
                max_out_frames,
            )
        } else {
            (None, 0)
        };

        let engine = DecoderEngine {
            format,
            track_id,
            decoder,
            resampler,
        };

        let data = DecoderData {
            channels,
            resampler_max_out_frames,
        };

        Ok(Self {
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
                let engine = self.engine.clone();
                let event_list = self.event_list.clone();
                let mut ring_buf = self.ring_buf.clone();
                let stop = self.stop.clone();
                let channels = self.data.channels;
                let max_out_frames = self.data.resampler_max_out_frames;
                // 4. 解码循环
                std::thread::Builder::new()
                    .name("decoder".to_string())
                    .spawn(move || -> () {
                        debug!("decoder run thread loop start");
                        //初始化
                        let ring_buf_max_len = ring_buf.max_len().expect(
                            "ring_buf max_len is \
                        None",
                        );
                        let max_write_len = ring_buf_max_len / 4;
                        let mut buf = Vec::with_capacity(ring_buf_max_len);
                        let mut out_buffer = vec![0.0f32; max_out_frames * channels];
                        'root: loop {
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

                                    // 重采样处理
                                    if let Some(rs) = &mut engine.resampler {
                                        rs.in_buf.extend_from_slice(&buf);
                                        let chunk_len = channels * RESAMPLER_CHUNK_IN;
                                        while rs.in_buf.len() >= chunk_len {
                                            let Ok(frames_out) = Self::resampler_inner(
                                                &event_list,
                                                rs,
                                                channels,
                                                &mut out_buffer,
                                                max_out_frames,
                                            ) else {
                                                break 'root;
                                            };
                                            rs.in_buf.drain(..chunk_len);
                                            // 写入环形缓冲区
                                            if Self::write_all_to_ring_buf(
                                                &mut ring_buf,
                                                &stop,
                                                max_write_len,
                                                &event_list,
                                                &out_buffer[..frames_out * channels],
                                            )
                                            .is_err()
                                            {
                                                break 'root;
                                            }
                                        }
                                    } else {
                                        // 写入环形缓冲区
                                        if Self::write_all_to_ring_buf(
                                            &mut ring_buf,
                                            &stop,
                                            max_write_len,
                                            &event_list,
                                            &buf,
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

    fn resampler_inner(
        event_list: &Arc<Mutex<Vec<Box<dyn DecoderEventSink>>>>,
        rs: &mut DecoderResampler,
        channels: usize,
        out_buffer: &mut [f32],
        max_out_frames: usize,
    ) -> Result<usize, DecoderEventError> {
        let input_buf = &rs.in_buf[..RESAMPLER_CHUNK_IN * channels];
        let max_out_frames = max_out_frames;
        let input_adapter = match InterleavedSlice::new(input_buf, channels, RESAMPLER_CHUNK_IN) {
            Ok(i) => i,
            Err(e) => {
                let e = DecoderEventError {
                    message: format!("InterleavedSlice::new, err:{e}"),
                    kind: DecoderEventErrorKind::InterleavedSliceSizeError(e),
                };
                Self::emit(
                    DecoderEvent {
                        result: Err(e.clone()),
                    },
                    event_list,
                );
                return Err(e);
            }
        };
        let mut output_adapter =
            match InterleavedSlice::new_mut(out_buffer, channels, max_out_frames) {
                Ok(i) => i,
                Err(e) => {
                    let e = DecoderEventError {
                        message: format!("InterleavedSlice::new_mut, err:{e}"),
                        kind: DecoderEventErrorKind::InterleavedSliceSizeError(e),
                    };
                    Self::emit(
                        DecoderEvent {
                            result: Err(e.clone()),
                        },
                        event_list,
                    );
                    return Err(e);
                }
            };
        let rs = &mut rs.fft_resampler;
        let (_, frames_out) =
            match rs.process_into_buffer(&input_adapter, &mut output_adapter, None) {
                Ok(i) => i,
                Err(e) => {
                    let e = DecoderEventError {
                        message: format!("process_into_buffer, err:{e}"),
                        kind: DecoderEventErrorKind::ResampleError(e),
                    };
                    Self::emit(
                        DecoderEvent {
                            result: Err(e.clone()),
                        },
                        event_list,
                    );
                    return Err(e);
                }
            };
        Ok(frames_out)
    }

    fn emit(event: DecoderEvent, list: &Arc<Mutex<Vec<Box<dyn DecoderEventSink>>>>) {
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
                break;
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

    pub fn add_event(&mut self, event: Box<dyn DecoderEventSink>) {
        self.event_list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }

    fn stop_inner(&mut self) -> DecoderResult<()> {
        self.stop.store(true, Ordering::Release);
        self.ring_buf.clear();
        self.join()
    }

    pub fn stop(self) -> DecoderResult<()> {
        let mut s = self;
        s.stop_inner()
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
        if let Some(r) = &mut engine.resampler {
            r.in_buf.clear();
        }
        Ok(r)
    }
}

pub trait DecoderEventSink: Send + 'static {
    fn emit(&mut self, event: DecoderEvent);
    fn new<F>(f: F) -> Self
    where
        F: Fn(DecoderEvent) + Send + 'static,
        Self: Sized;
}

pub struct DecoderEventChannelSink(Sender<DecoderEvent>);

impl DecoderEventSink for DecoderEventChannelSink {
    fn emit(&mut self, event: DecoderEvent) {
        // send 非阻塞；满了就丢弃（或 try_send + 计数）
        let _ = self.0.send(event);
    }

    fn new<F>(f: F) -> Self
    where
        F: Fn(DecoderEvent) + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("decoder_event_channel_sink".to_string())
            .spawn(move || {
                for event in receiver {
                    f(event);
                }
            })
            .expect("创建线程失败");
        Self(sender)
    }
}

pub struct DecoderEventDirectSink(Box<dyn Fn(DecoderEvent) + Send + 'static>);

impl DecoderEventSink for DecoderEventDirectSink {
    fn emit(&mut self, event: DecoderEvent) {
        self.0(event); // 就在解码线程里跑
    }

    fn new<F>(f: F) -> Self
    where
        F: Fn(DecoderEvent) + Send + 'static,
    {
        Self(Box::new(f))
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
        };
        write!(f, "DecoderError: Kind: {kind}, message: {}", self.message)
    }
}

impl std::error::Error for DecoderError {}

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
enum DecoderErrorKind {
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
}

impl From<std::io::Error> for DecoderError {
    fn from(e: std::io::Error) -> Self {
        Self {
            message: e.to_string(),
            kind: DecoderErrorKind::IOError(e),
        }
    }
}
