use crate::command::music_play::effect::{AudioFormat, PreEffect, PreEffectChain};
use std::any::Any;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, mpsc};
use tracing::{debug, error};
use water_ball_tool_lib::concurrent_ring::ConcurrentRing;

#[derive(Clone)]
pub struct PreEffectRuntime {
    effects: Arc<Mutex<PreEffectChain>>,
    input_buf: Arc<ConcurrentRing<AudioData>>,
    audio_buf: Arc<ConcurrentRing<f32>>,
    stop: Arc<AtomicBool>,
    effect_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    event_list: Arc<Mutex<Vec<Box<dyn PreEffectRuntimeEventSink>>>>,
}

pub enum AudioData {
    Block(Vec<f32>),
    Eof,
    Reset(AudioFormat),
    None,
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
    pub fn create(
        input_buf: Arc<ConcurrentRing<AudioData>>,
        audio_buf: Arc<ConcurrentRing<f32>>,
    ) -> Self {
        Self {
            effects: Arc::new(Mutex::new(PreEffectChain::new())),
            input_buf,
            audio_buf,
            stop: Arc::new(AtomicBool::new(false)),
            effect_thread: Arc::new(Mutex::new(None)),
            event_list: Arc::new(Mutex::new(Vec::new())),
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
            let mut out_temp_buf = Vec::new();
            *effect_thread = Some({
                std::thread::Builder::new()
                    .name("pre-effect-running".to_string())
                    .spawn(move || {
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
                            let data = input_buf.pop_wait();
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            match data {
                                AudioData::Block(data) => {
                                    let len = effects.process(&data, &mut out_temp_buf);
                                    if len == 0 {
                                        continue;
                                    }
                                    if let Err(e) = audio_buf.write_all(&out_temp_buf[..len]) {
                                        error!("Failed to write all audio data: {}", e);
                                        break;
                                    }
                                }
                                AudioData::Eof => {
                                    let len = effects.input_eof(&mut out_temp_buf);
                                    if len == 0 {
                                        continue;
                                    }
                                    if let Err(e) = audio_buf.write_all(&out_temp_buf[..len]) {
                                        error!("Failed to write all audio data: {}", e);
                                        break;
                                    }
                                    Self::emit(
                                        PreEffectRuntimeEvent {
                                            result: Ok(PreEffectRuntimeEventType::Eof),
                                        },
                                        &event_list,
                                    );
                                }
                                AudioData::Reset(input_format) => effects.reset(input_format),
                                AudioData::None => {
                                    effects.input_none();
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
        _ = self.input_buf.push(AudioData::None);
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

    pub fn reset(&self, format: AudioFormat) {
        self.input_buf.push_wait(AudioData::Reset(format));
    }

    pub fn input_none(&self) {
        self.input_buf.push_wait(AudioData::None);
    }

    pub fn add_effect<E>(&mut self, effect: E) -> Result<(), Box<dyn Error>>
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

pub enum PreEffectRuntimeEventErrorKind {}

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
