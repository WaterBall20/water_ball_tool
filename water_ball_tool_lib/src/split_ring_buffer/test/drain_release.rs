//! drain / release / truncate_back 测试。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::split_ring_buffer::*;

#[derive(Debug)]
struct DropTracker {
    counter: Arc<AtomicUsize>,
}

impl Drop for DropTracker {
    fn drop(&mut self) {
        self.counter.fetch_add(1, Ordering::SeqCst);
    }
}

// ---------- drain 前缀 ----------

#[test]
fn drain_prefix_partial() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    writer.push(String::from("a")).expect("push");
    writer.push(String::from("b")).expect("push");
    writer.push(String::from("c")).expect("push");
    let v = reader.drain(0..2).expect("drain");
    assert_eq!(v, vec![String::from("a"), String::from("b")]);
    assert_eq!(reader.len(), 1);
    assert_eq!(reader.pop().as_deref(), Some("c"));
}

#[test]
fn drain_full() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    writer.push(String::from("a")).expect("push");
    writer.push(String::from("b")).expect("push");
    let v = reader.drain(0..2).expect("drain");
    assert_eq!(v, vec![String::from("a"), String::from("b")]);
    assert!(reader.is_empty());
}

#[test]
fn drain_more_than_available() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    writer.push(String::from("a")).expect("push");
    let v = reader.drain(0..100).expect("drain");
    assert_eq!(v, vec![String::from("a")]);
    assert!(reader.is_empty());
}

#[test]
fn drain_invalid_range() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    writer.push(String::from("a")).expect("push");
    match reader.drain(1..2) {
        Err(e) => assert!(matches!(e.kind, Kind::InvalidRange)),
        Ok(_) => panic!("expected InvalidRange"),
    }
}

// ---------- release 前缀 ----------

#[test]
fn release_prefix_drops() {
    let counter = Arc::new(AtomicUsize::new(0));
    let (reader, writer) = locked_ring_buffer::<DropTracker>(4);
    for _ in 0..3 {
        writer
            .push(DropTracker {
                counter: counter.clone(),
            })
            .expect("push");
    }
    reader.release(0..2).expect("release");
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    assert_eq!(reader.len(), 1);
}

#[test]
fn release_all() {
    let counter = Arc::new(AtomicUsize::new(0));
    let (reader, writer) = locked_ring_buffer::<DropTracker>(4);
    for _ in 0..3 {
        writer
            .push(DropTracker {
                counter: counter.clone(),
            })
            .expect("push");
    }
    reader.release(0..3).expect("release");
    assert_eq!(counter.load(Ordering::SeqCst), 3);
    assert!(reader.is_empty());
}

// ---------- truncate_back ----------

#[test]
fn truncate_back_partial() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    writer.push(String::from("a")).expect("push");
    writer.push(String::from("b")).expect("push");
    writer.push(String::from("c")).expect("push");
    writer.truncate_back(2);
    assert_eq!(reader.len(), 2);
    assert_eq!(reader.pop().as_deref(), Some("a"));
    assert_eq!(reader.pop().as_deref(), Some("b"));
    assert!(reader.pop().is_none());
}

#[test]
fn truncate_back_drops() {
    let counter = Arc::new(AtomicUsize::new(0));
    let (reader, writer) = locked_ring_buffer::<DropTracker>(4);
    for _ in 0..3 {
        writer
            .push(DropTracker {
                counter: counter.clone(),
            })
            .expect("push");
    }
    writer.truncate_back(1);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    assert_eq!(reader.len(), 1);
    drop(reader.pop());
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[test]
fn truncate_back_zero() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    writer.push(String::from("a")).expect("push");
    writer.push(String::from("b")).expect("push");
    writer.truncate_back(0);
    assert!(reader.is_empty());
}

#[test]
fn truncate_back_more_than_len() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    writer.push(String::from("a")).expect("push");
    writer.push(String::from("b")).expect("push");
    writer.truncate_back(10);
    assert_eq!(reader.len(), 2);
}

#[test]
fn truncate_back_then_push() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    writer.push(String::from("a")).expect("push");
    writer.push(String::from("b")).expect("push");
    writer.truncate_back(1);
    writer.push(String::from("c")).expect("push");
    assert_eq!(reader.pop().as_deref(), Some("a"));
    assert_eq!(reader.pop().as_deref(), Some("c"));
}

// ---------- drain / truncate 并发 ----------

#[test]
fn drain_then_truncate() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    for i in 0..4 {
        writer.push(format!("v{}", i)).expect("push");
    }
    let _ = reader.drain(0..2).expect("drain");
    writer.truncate_back(1);
    assert_eq!(reader.len(), 1);
    assert_eq!(reader.pop().as_deref(), Some("v2"));
}

// ---------- 无锁端 ----------

#[test]
fn unlocked_drain_release_truncate() {
    let (mut reader, mut writer) = unlocked_ring_buffer::<String>(4);
    writer.push(String::from("a")).expect("push");
    writer.push(String::from("b")).expect("push");
    writer.push(String::from("c")).expect("push");

    let v = reader.drain(0..1).expect("drain");
    assert_eq!(v, vec![String::from("a")]);

    reader.release(0..1).expect("release");
    assert_eq!(reader.len(), 1);

    writer.truncate_back(0);
    assert!(reader.is_empty());
}

// ---------- clear 与 truncate 互斥 ----------

#[test]
fn clear_then_truncate() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    writer.push(String::from("a")).expect("push");
    writer.push(String::from("b")).expect("push");
    writer.clear();
    assert_eq!(reader.len(), 0);
    writer.truncate_back(1);
    assert_eq!(reader.len(), 0);
}
