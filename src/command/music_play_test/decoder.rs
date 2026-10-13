use std::any::Any;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use tracing::{debug, error};
use water_ball_tool_lib::ring_buffer_wait::RingBufferWait;

pub(super) struct Decoder {
    thread_handle: Option<std::thread::JoinHandle<DecoderResult<()>>>, //
    ring_buf: RingBufferWait<f32>,
    stop: Arc<AtomicBool>,
    format: Option<Box<dyn FormatReader>>,
    track_id: u32,
    decoder: Option<Box<dyn AudioDecoder>>,
}
impl Drop for Decoder {
    fn drop(&mut self) {
        _ = self.stop_inner();
    }
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
        })
    }

    pub fn run(&mut self) -> DecoderResult<()> {
        if self.thread_handle.is_none() {
            let ring_buf = self.ring_buf.clone();
            let stop = self.stop.clone();
            let mut format = self.format.take().expect("self.format is None");
            let track_id = self.track_id;
            let mut decoder = self.decoder.take().expect("decoder is None");
            // 4. 解码循环
            let mut buf = Vec::with_capacity(ring_buf.len());
            let thread_handle = {
                let mut ring_buf = ring_buf.clone();
                let stop = stop.clone();
                std::thread::Builder::new()
                    .name("decoder".to_string())
                    .spawn(move || -> DecoderResult<()> {
                        loop {
                            debug!("decoder run thread loop");
                            // 检查停止信号
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            // 检查暂停信号，若暂停则等待
                            // while paused.load(Ordering::Relaxed) { std::thread::yield_now(); }

                            let packet = match format.next_packet() {
                                Ok(Some(p)) => p,
                                Ok(None) => break,
                                Err(e) => {
                                    eprintln!("Error reading packet: {e}");
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

                                    // 写入环形缓冲区（你的 write_all 会阻塞等待空间）
                                    ring_buf.write_all(&buf).expect("Ring buffer write failed");
                                }
                                Err(symphonia::core::errors::Error::DecodeError(e)) => {
                                    error!("Decode error (skipping frame): {e}");
                                    continue;
                                }
                                Err(e) => {
                                    error!("Fatal decode error: {e}");
                                    break;
                                }
                            }
                        }
                        Ok(())
                    })
            }?;
            self.thread_handle = Some(thread_handle);
            Ok(())
        } else {
            Err(DecoderError::new("Running", DecoderErrorKind::Running))
        }
    }

    fn stop_inner(&mut self) -> DecoderResult<()> {
        self.ring_buf.clear();
        self.stop.store(true, Ordering::Release);
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
