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
use thread_priority::ThreadPriority;
use tracing::{debug, error};
use water_ball_tool_lib::concurrent_ring::ConcurrentRing;
use water_ball_tool_lib::split_ring_buffer::LockedWriter;

#[derive(Clone)]
pub struct PreEffectRuntime {
    effects: Arc<Mutex<PreEffectChain>>,
    input_buf: Arc<ConcurrentRing<AudioData>>,
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
        let mut list = list.lock().expect("list is poisoned");
        for sink in list.iter_mut() {
            sink.emit(event.clone());
        }
    }
}

impl PreEffectRuntime {
    fn send_input_and_clear_buf_inner(&self, audio_data: AudioData) {
        self.input_buf.clear();
        self.input_buf.push_wait(audio_data);
    }
}

impl PreEffectRuntime {
    pub fn create(
        input_buf: Arc<ConcurrentRing<AudioData>>,
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

    pub fn run(&mut self) {
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
            *effect_thread = Some({
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
                            let data = input_buf.pop_wait();
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
                                    if let Err(e) = audio_buf.write_all(&out_temp_buf[..len]) {
                                        error!("Failed to write all audio data: {}", e);
                                        break;
                                    }
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
                                    let len = effects.on_discontinuity(
                                        &mut out_temp_buf,
                                        PreEffectDiscontinuity::try_from(&data).expect("逻辑错误"),
                                    );
                                    if len == 0 {
                                        continue;
                                    }
                                    if let Err(e) = audio_buf.write_all(&out_temp_buf[..len]) {
                                        error!("Failed to write all audio data: {}", e);
                                        break;
                                    }
                                    if let AudioData::Eof = &data {
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
                                        Self::emit(
                                            PreEffectRuntimeEvent {
                                                result: Err(PreEffectRuntimeEventError::new(
                                                    "Reset pre effect err",
                                                    PreEffectRuntimeEventErrorKind::from(e),
                                                )),
                                            },
                                            &event_list,
                                        );
                                    }
                                }
                            }
                        }
                        debug!("Pre effect thread end");
                    })
                    .expect("pre-effect-thread spawn failed")
            });
        }
    }

    pub fn stop(&self) -> Result<(), Box<dyn Any + Send + 'static>> {
        self.stop.store(true, Ordering::Release);
        self.input_buf.clear();
        self.audio_buf.clear();
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

    pub fn send_skip(&self) {
        self.send_input_and_clear_buf_inner(AudioData::Skip)
    }

    pub fn send_no_next(&self) {
        self.send_input_and_clear_buf_inner(AudioData::NoNext)
    }

    pub fn reset(&self, format: AudioFormat) {
        self.input_buf.push_wait(AudioData::Reset(format));
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

pub struct PreEffectRuntimeEvent {
    result: Result<PreEffectRuntimeEventType, PreEffectRuntimeEventError>,
}

impl PreEffectRuntimeEvent {
    pub fn result(&self) -> &Result<PreEffectRuntimeEventType, PreEffectRuntimeEventError> {
        &self.result
    }
}

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
    fn new<F>(f: F) -> Self
    where
        F: Fn(&PreEffectRuntimeEvent) + Send + 'static,
        Self: Sized;
    fn type_of(&self) -> &'static str;
}

pub struct PreEffectRuntimeEventChannelSink(Sender<Arc<PreEffectRuntimeEvent>>);

impl PreEffectRuntimeEventSink for PreEffectRuntimeEventChannelSink {
    fn emit(&mut self, event: Arc<PreEffectRuntimeEvent>) {
        // send 非阻塞；满了就丢弃（或 try_send + 计数）
        let _ = self.0.send(event);
    }

    fn new<F>(f: F) -> Self
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
            })
            .expect("创建线程失败");
        Self(sender)
    }

    fn type_of(&self) -> &'static str {
        "PreEffectRuntimeEventChannelSink"
    }
}

pub struct PreEffectRuntimeEventDirectSink(Box<dyn Fn(&PreEffectRuntimeEvent) + Send + 'static>);

impl PreEffectRuntimeEventSink for PreEffectRuntimeEventDirectSink {
    fn emit(&mut self, event: Arc<PreEffectRuntimeEvent>) {
        self.0(&event); // 就在解码线程里跑
    }

    fn new<F>(f: F) -> Self
    where
        F: Fn(&PreEffectRuntimeEvent) + Send + 'static,
    {
        Self(Box::new(f))
    }

    fn type_of(&self) -> &'static str {
        "PreEffectRuntimeEventDirectSink"
    }
}
