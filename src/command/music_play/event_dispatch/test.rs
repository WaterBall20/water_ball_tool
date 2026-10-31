//! 事件分发线程单元测试 / Unit tests for the player event dispatch thread
//!
//! 这些测试只使用内存对象（通道与 sink），不读写文件、不使用临时目录。
//! These tests only use in-memory objects (channels and sinks); no files or temp dirs.

use super::*;
use crate::command::music_play::player::{PlayerEvent, PlayerEventSink, PlayerEventType};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// 测试 sink：记录事件以及实际执行 `emit` 的线程 ID。
///
/// Test sink recording the event and the id of the thread that ran `emit`.
struct RecordingSink {
    events: mpsc::Sender<(Arc<PlayerEvent>, thread::ThreadId)>,
}

impl PlayerEventSink for RecordingSink {
    fn emit(&mut self, event: Arc<PlayerEvent>) {
        let _ = self.events.send((event, thread::current().id()));
    }

    fn type_of(&self) -> &'static str {
        "RecordingSink"
    }
}

#[test]
fn dispatcher_delivers_event_on_dispatch_thread() {
    let (tx, rx) = mpsc::channel::<(Arc<PlayerEvent>, thread::ThreadId)>();
    let list: Arc<Mutex<Vec<Box<dyn PlayerEventSink>>>> =
        Arc::new(Mutex::new(vec![Box::new(RecordingSink { events: tx })]));
    //AI=== 分发线程创建失败在测试中直接判失败 / a spawn failure fails the test explicitly
    let dispatcher = match EventDispatcher::new(list) {
        Ok(dispatcher) => dispatcher,
        Err(e) => panic!("事件分发线程应能创建 / dispatch thread should be spawned: {e}"),
    };
    //AI_END===

    dispatcher.try_emit_event(PlayerEvent::track_ended());
    let (event, emit_thread) = match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(delivered) => delivered,
        Err(e) => panic!("事件应由分发线程送达 / event should be delivered: {e}"),
    };

    assert!(matches!(event.result(), Ok(PlayerEventType::TrackEnded)));
    assert_ne!(
        emit_thread,
        thread::current().id(),
        "sink 必须在分发线程执行 / the sink must run on the dispatch thread"
    );
    assert_eq!(dispatcher.dropped(), 0);

    // 停止应幂等：第二次 stop 不应 panic，也不会重复 join。
    // Stopping is idempotent: a second `stop` must not panic nor join twice.
    dispatcher.stop();
    dispatcher.stop();
}
