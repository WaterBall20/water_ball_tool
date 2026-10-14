use std::any::Any;
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
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;
use tracing::{debug, error};
use water_ball_tool_lib::ring_buffer_wait::{RingBufferWait, RingBufferWaitError};

pub(super) struct Decoder {
    thread_handle: Option<std::thread::JoinHandle<DecoderResult<()>>>, //
    ring_buf: RingBufferWait<f32>,
    stop: Arc<AtomicBool>,
    format: Option<Box<dyn FormatReader>>,
    track_id: u32,
    decoder: Option<Box<dyn AudioDecoder>>,
    event_list: Arc<Mutex<Vec<Box<dyn DecoderEventSink>>>>,
}
impl Decoder {
    pub fn new(ring_buf: RingBufferWait<f32>, path: PathBuf) -> DecoderResult<Self> {
        let ring_buf = ring_buf;
        let stop = Arc::new(AtomicBool::new(false)); // 1. 打开文件 & 创建 MSS
        let file = File::open(&path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

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
        Ok(Self {
            thread_handle: None,
            ring_buf,
            stop,
            format: Some(format),
            track_id,
            decoder: Some(decoder),
            event_list: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn run(&mut self) -> DecoderResult<()> {
        fn emit(event: DecoderEvent, list: &mut Vec<Box<dyn DecoderEventSink>>) {
            for sink in list.iter_mut() {
                sink.emit(event.clone());
            }
        }
        if self.thread_handle.is_none() {
            let stop = self.stop.clone();
            let mut format = self.format.take().expect("self.format is None");
            let track_id = self.track_id;
            let mut decoder = self.decoder.take().expect("decoder is None");
            let event_list = self.event_list.clone();
            // 4. 解码循环
            let write_len = self.ring_buf.max_len() / 4;
            let mut buf = Vec::with_capacity(self.ring_buf.max_len());
            let thread_handle = {
                let mut ring_buf = self.ring_buf.clone();
                let stop = stop.clone();
                std::thread::Builder::new()
                    .name("decoder".to_string())
                    .spawn(move || -> DecoderResult<()> {
                        debug!("decoder run thread loop start");
                        loop {
                            // 检查停止信号
                            if stop.load(Ordering::Acquire) {
                                break;
                            }

                            let packet = match format.next_packet() {
                                Ok(Some(p)) => p,
                                Ok(None) => {
                                    let mut event_list = event_list
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                                    emit(
                                        DecoderEvent {
                                            result: Ok(DecoderEventType::Eof),
                                        },
                                        &mut event_list,
                                    );
                                    break;
                                }
                                Err(e) => {
                                    error!("Error reading packet: {e}");
                                    break;
                                }
                            };

                            if packet.track_id != track_id {
                                continue;
                            }

                            match decoder.decode(&packet) {
                                Ok(decoded) => {
                                    // 使用新方法进行交错转换
                                    decoded.copy_to_vec_interleaved(&mut buf);

                                    // 写入环形缓冲区
                                    for chunk in buf.chunks(write_len) {
                                        // 检查停止信号
                                        if stop.load(Ordering::Acquire) {
                                            break;
                                        }
                                        // 分块后，每一块的长度都 ≤ CHUNK_SIZE ≤ ring 容量
                                        // 因此 write_all 不会返回 WriteSizeTooLarge
                                        if let Err(e) = ring_buf.write_all(chunk) {
                                            // 只可能是致命错误（锁异常、溢出等），上报后退出
                                            let event = DecoderEvent {
                                                result: Err(DecoderEventError {
                                                    message: format!("write_all failed: {e}"),
                                                    kind: DecoderEventErrorKind::RingBufferError(e),
                                                }),
                                            };
                                            let mut list = event_list
                                                .lock()
                                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                                            emit(event, &mut list);
                                            return Ok(());
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
                        Ok(())
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
        if let Some(thread) = self.thread_handle.take() {
            match thread.join() {
                Ok(result) => result?,
                Err(e) => {
                    return Err(DecoderError {
                        message: format!("Thread join error: {e:?}"),
                        kind: DecoderErrorKind::ThreadJoinError(e),
                    });
                }
            }
        }
        Ok(())
    }

    pub fn try_seek(
        &mut self,
        d: Duration,
    ) -> Result<symphonia::core::formats::SeekedTo, symphonia::core::errors::Error> {
        if let Some(format) = self.format.as_mut()
            && let Some(decoder) = self.decoder.as_mut()
        {
            let r = format.seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from_millis(i64::try_from(d.as_millis()).unwrap_or(0)),
                    track_id: Some(self.track_id),
                },
            )?;
            decoder.reset();
            self.ring_buf.clear();
            Ok(r)
        } else {
            panic!("逻辑错误")
        }
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
        std::thread::spawn(move || {
            for event in receiver {
                f(event);
            }
        });
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
                _ => panic!("无法处理的错误, error: {e:?}"),
            }),
            Self::Unsupported(e) => Self::Unsupported(e),
            Self::LimitError(e) => Self::LimitError(e),
            Self::ResetRequired => Self::ResetRequired,
        }
    }
}

pub(super) type DecoderResult<T> = Result<T, DecoderError>;

#[derive(Debug)]
pub(super) struct DecoderError {
    message: String,
    kind: DecoderErrorKind,
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
    Running,
    IOError(std::io::Error),
    ThreadJoinError(Box<dyn Any + Send + 'static>),
    NoAudioTrack,
    NoAudioCodecParams,
    MakeAudioDecoderError(symphonia::core::errors::Error),
    GetProbeError(symphonia::core::errors::Error),
}

impl From<std::io::Error> for DecoderError {
    fn from(e: std::io::Error) -> Self {
        Self {
            message: e.to_string(),
            kind: DecoderErrorKind::IOError(e),
        }
    }
}
