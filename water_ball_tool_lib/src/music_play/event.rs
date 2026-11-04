use std::error::Error;
use std::sync::mpsc::Sender;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

pub trait EventSink<T>: Send + 'static {
    fn emit(&mut self, event: Arc<T>);
    fn type_of(&self) -> &'static str;
}

pub struct EventChannelSink<T>(&'static str, Option<(Sender<Arc<T>>, JoinHandle<()>)>);

//AI=== 构造函数从 trait 移出为固有方法；线程创建失败必须返回 io::Error / constructors moved out of the trait into inherent methods; a spawn failure must return io::Error
impl<T: Send + Sync + 'static> EventChannelSink<T> {
    pub fn new<F>(
        type_str: &'static str,
        f: F,
    ) -> Result<Self, Box<dyn Error + Sync + Send + 'static>>
    where
        F: Fn(&T) + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel::<Arc<T>>();
        let th = std::thread::Builder::new()
            .name(type_str.to_string())
            .spawn(move || {
                for event in receiver {
                    f(&event);
                }
            })?;
        Ok(Self(type_str, Some((sender, th))))
    }
}
//AI_END===
impl<T> Drop for EventChannelSink<T> {
    fn drop(&mut self) {
        if let Some((s, t)) = self.1.take() {
            drop(s);
            _ = t.join();
        }
    }
}

impl<T: Send + Sync + 'static> EventSink<T> for EventChannelSink<T> {
    fn emit(&mut self, event: Arc<T>) {
        // send 非阻塞；满了就丢弃（或 try_send + 计数）/ non-blocking send; the event is dropped when the channel is full
        self.1.as_mut().map(|(s, _)| s.send(event));
    }

    fn type_of(&self) -> &'static str {
        self.0
    }
}

pub struct EventDirectSink<T>(&'static str, Box<dyn Fn(&T) + Send + 'static>);

//AI=== 直接由解码线程调用的 sink 构造函数 / constructor for the sink invoked directly from the decode thread
impl<T: Send + Sync + 'static> EventDirectSink<T> {
    pub fn new<F>(type_str: &'static str, f: F) -> Self
    where
        F: Fn(&T) + Send + 'static,
    {
        Self(type_str, Box::new(f))
    }
}
//AI_END===

impl<T: Send + Sync + 'static> EventSink<T> for EventDirectSink<T> {
    fn emit(&mut self, event: Arc<T>) {
        self.1(&event); // 就在解码线程里跑 / runs on the decode thread
    }

    fn type_of(&self) -> &'static str {
        self.0
    }
}
