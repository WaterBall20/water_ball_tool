//! 播放器事件分发线程 / Player event dispatch thread
//!
//! cpal 的音频回调运行在实时上下文：不能在回调里加锁、调用用户回调或写日志，否则会
//! 引入调度抖动、优先级反转甚至欠载。本模块提供一个独立线程：实时回调只把事件 / 日志
//! 通过 `try_send` 非阻塞地投递到有界通道，真正的工作（加锁、调用 `PlayerEventSink`、
//! 输出日志）全部在该线程完成。
//!
//! The cpal audio callback runs in a realtime context: it must not take locks, invoke user
//! callbacks or write logs, otherwise it risks scheduling jitter, priority inversion and
//! underruns. This module owns a dedicated thread: the realtime callback only posts events and
//! log lines to a bounded channel with non-blocking `try_send`, while the actual work (locking,
//! invoking `PlayerEventSink`, writing logs) happens on this thread.

use super::player::{PlayerEvent, PlayerEventSink};
use crossbeam_channel::{Sender, bounded};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use tracing::{debug, error, info, warn};

/// 事件分发通道容量（满时丢弃并计数，绝不阻塞实时线程）。
/// Dispatch channel capacity (drop and count when full; the realtime thread never blocks).
const DISPATCH_CHANNEL_CAPACITY: usize = 1024;

/// 日志级别（与 `tracing::Level` 解耦，避免实时线程依赖 tracing 内部结构）。
/// Log level, decoupled from `tracing::Level` so the realtime thread stays allocation-light.
#[derive(Debug, Clone, Copy)]
pub(super) enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// 投递给事件线程的消息 / Message posted to the dispatch thread.
enum DispatchMsg {
    /// 播放器事件 / Player event
    Event(PlayerEvent),
    /// 需要在非实时线程输出的日志 / Log line to emit outside the realtime thread
    Log(LogLevel, String),
    /// 停止事件线程 / Stop the dispatch thread
    Stop,
}

/// 事件分发器：所有克隆共享同一个分发线程与同一条通道。
///
/// Event dispatcher: every clone shares the same dispatch thread and channel.
#[derive(Clone)]
pub(super) struct EventDispatcher {
    tx: Sender<DispatchMsg>,
    handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    dropped: Arc<AtomicU64>,
}

impl EventDispatcher {
    //AI=== 线程创建失败返回错误（由 Player::new 传播），不再记录后继续 / return spawn errors instead of logging
    /// 创建并启动事件分发线程。
    ///
    /// 线程创建失败时返回错误，由调用方（播放器初始化）中止；不再"记录日志后继续"。
    ///
    /// Spawn the dispatch thread. A spawn failure is returned so player initialization can abort; it is
    /// no longer logged-and-ignored.
    pub(super) fn new(
        event_list: Arc<Mutex<Vec<Box<dyn PlayerEventSink>>>>,
    ) -> std::io::Result<Self> {
        let (tx, rx) = bounded::<DispatchMsg>(DISPATCH_CHANNEL_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let handle = thread::Builder::new()
            .name("player-event-dispatch".to_string())
            .spawn(move || {
                for msg in rx {
                    match msg {
                        DispatchMsg::Event(event) => {
                            // 事件线程内加锁 + 调用 sink / lock and dispatch sinks on this thread
                            let event = Arc::new(event);
                            let mut list = event_list
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            for sink in list.iter_mut() {
                                sink.emit(event.clone());
                            }
                        }
                        DispatchMsg::Log(level, message) => log_line(level, &message),
                        DispatchMsg::Stop => break,
                    }
                }
                debug!("播放器事件分发线程退出 / player event dispatch thread stopped");
            })?;
        Ok(Self {
            tx,
            handle: Arc::new(Mutex::new(Some(handle))),
            dropped,
        })
    }
    //AI_END===

    /// 从实时线程投递事件（非阻塞，通道满则丢弃并计数）。
    ///
    /// Post an event from the realtime thread (non-blocking; dropped and counted when full).
    pub(super) fn try_emit_event(&self, event: PlayerEvent) {
        if self.tx.try_send(DispatchMsg::Event(event)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 从实时线程投递日志（非阻塞，通道满则丢弃并计数）。
    ///
    /// Post a log line from the realtime thread (non-blocking; dropped and counted when full).
    pub(super) fn try_log(&self, level: LogLevel, message: String) {
        if self.tx.try_send(DispatchMsg::Log(level, message)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 已被丢弃的消息数量（通道满或线程未启动）。
    ///
    /// Number of messages dropped so far (channel full or thread never started).
    pub(super) fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 停止并等待事件线程退出；可从任意克隆调用，幂等。
    ///
    /// 调用方不是实时线程，因此这里使用阻塞 `send` 确保 `Stop` 一定入队（通道已断开时
    /// 立即返回错误）。
    ///
    /// Stop and join the dispatch thread; safe to call from any clone and idempotent.
    /// The caller is not a realtime thread, so a blocking `send` guarantees `Stop` is
    /// enqueued (it fails fast once the channel is disconnected).
    pub(super) fn stop(&self) {
        if let Err(e) = self.tx.send(DispatchMsg::Stop) {
            debug!("事件分发通道已关闭 / dispatch channel already closed: {e}");
        }
        let handle = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(handle) = handle
            && let Err(e) = handle.join()
        {
            error!("事件分发线程异常退出 / event dispatch thread panicked: {e:?}");
        }
    }
}

/// 在事件线程上输出日志 / Emit a log line on the dispatch thread.
fn log_line(level: LogLevel, message: &str) {
    match level {
        LogLevel::Error => error!("{message}"),
        LogLevel::Warn => warn!("{message}"),
        LogLevel::Info => info!("{message}"),
        LogLevel::Debug => debug!("{message}"),
        LogLevel::Trace => tracing::trace!("{message}"),
    }
}

#[cfg(test)]
mod test;
