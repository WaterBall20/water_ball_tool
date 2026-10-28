//! 非 Copy 类型测试：pop / push / clear / Drop 语义 / 并发。
//!
//! 非 Copy 只提供 pop / push / pop_wait / push_wait / try_pop / try_push /
//! clear / len / is_empty / is_full；借用切片方法（read / write 系列）仅
//! Copy 类型可用，不在本文件测试。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crate::split_ring_buffer::*;

const WAIT: Duration = Duration::from_millis(100);

/// Drop 计数元素，用于验证元素是否被正确 drop。
#[derive(Debug)]
struct DropTracker {
    counter: Arc<AtomicUsize>,
}

impl DropTracker {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        Self { counter }
    }
}

impl Drop for DropTracker {
    fn drop(&mut self) {
        self.counter.fetch_add(1, Ordering::SeqCst);
    }
}

// ---------- 基本非 Copy pop / push ----------

#[test]
fn non_copy_basic_push_pop_string() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    assert!(reader.pop().is_none());
    writer
        .push(String::from("hello"))
        .expect("push should succeed");
    assert_eq!(reader.pop().as_deref(), Some("hello"));
    assert!(reader.pop().is_none());
}

#[test]
fn non_copy_basic_push_pop_box() {
    let (mut reader, mut writer) = unlocked_ring_buffer::<Box<u32>>(4);
    assert!(reader.pop().is_none());
    writer.push(Box::new(42)).expect("push should succeed");
    assert_eq!(reader.pop().as_deref().copied(), Some(42));
}

#[test]
fn non_copy_push_full_returns_element() {
    let (_reader, writer) = locked_ring_buffer::<String>(2);
    writer.push(String::from("a")).expect("push should succeed");
    writer.push(String::from("b")).expect("push should succeed");
    match writer.push(String::from("c")) {
        Err(e) => match e.kind {
            Kind::Full(s) => assert_eq!(s, "c"),
            _ => panic!("expected Full"),
        },
        Ok(()) => panic!("expected Full"),
    }
}

// ---------- Drop 语义 ----------

#[test]
fn non_copy_clear_drops_elements() {
    let counter = Arc::new(AtomicUsize::new(0));
    let (reader, writer) = locked_ring_buffer::<DropTracker>(4);
    for _ in 0..3 {
        writer
            .push(DropTracker::new(counter.clone()))
            .expect("push should succeed");
    }
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    assert_eq!(reader.len(), 3);

    writer.clear();
    // clear 内部交换 a/b，然后同步 drop b 中 3 个元素
    assert_eq!(counter.load(Ordering::SeqCst), 3);
    assert_eq!(reader.len(), 0);
}

#[test]
fn non_copy_inner_drop_drops_unread_elements() {
    let counter = Arc::new(AtomicUsize::new(0));
    {
        let (reader, writer) = locked_ring_buffer::<DropTracker>(4);
        for _ in 0..3 {
            writer
                .push(DropTracker::new(counter.clone()))
                .expect("push should succeed");
        }
        drop(reader);
        drop(writer);
    }
    // 最后一个 Arc 释放触发 Inner::drop，a 的 [r, w) 中 3 个元素被 drop
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[test]
fn non_copy_pop_transfers_ownership() {
    let counter = Arc::new(AtomicUsize::new(0));
    let (reader, writer) = locked_ring_buffer::<DropTracker>(4);
    writer
        .push(DropTracker::new(counter.clone()))
        .expect("push should succeed");
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    let v = reader.pop().expect("pop should return");
    assert_eq!(counter.load(Ordering::SeqCst), 0); // 所有权在 v 中

    drop(v);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

// ---------- clear 后再用 ----------

#[test]
fn non_copy_clear_then_reuse() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    writer
        .push(String::from("old"))
        .expect("push should succeed");
    writer.clear();
    assert_eq!(reader.len(), 0);
    writer
        .push(String::from("new"))
        .expect("push should succeed");
    assert_eq!(reader.pop().as_deref(), Some("new"));
}

#[test]
fn non_copy_clear_after_full() {
    let (reader, writer) = locked_ring_buffer::<String>(2);
    writer.push(String::from("a")).expect("push should succeed");
    writer.push(String::from("b")).expect("push should succeed");
    assert!(reader.is_full());
    writer.clear();
    assert!(!reader.is_full());
    writer.push(String::from("c")).expect("push should succeed");
    writer.push(String::from("d")).expect("push should succeed");
    assert_eq!(reader.pop().as_deref(), Some("c"));
    assert_eq!(reader.pop().as_deref(), Some("d"));
}

#[test]
fn non_copy_double_clear() {
    let counter = Arc::new(AtomicUsize::new(0));
    let (reader, writer) = locked_ring_buffer::<DropTracker>(4);
    writer
        .push(DropTracker::new(counter.clone()))
        .expect("push should succeed");
    writer
        .push(DropTracker::new(counter.clone()))
        .expect("push should succeed");
    writer.clear();
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    writer
        .push(DropTracker::new(counter.clone()))
        .expect("push should succeed");
    writer
        .push(DropTracker::new(counter.clone()))
        .expect("push should succeed");
    writer
        .push(DropTracker::new(counter.clone()))
        .expect("push should succeed");
    writer.clear();
    assert_eq!(counter.load(Ordering::SeqCst), 5);
    assert_eq!(reader.len(), 0);
}

#[test]
fn non_copy_clear_drops_remaining() {
    let counter = Arc::new(AtomicUsize::new(0));
    let (reader, writer) = locked_ring_buffer::<DropTracker>(8);
    for _ in 0..5 {
        writer
            .push(DropTracker::new(counter.clone()))
            .expect("push should succeed");
    }
    let v = reader.pop().expect("pop should succeed");
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    drop(v);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(reader.len(), 4);

    writer.clear();
    assert_eq!(counter.load(Ordering::SeqCst), 5);
    assert_eq!(reader.len(), 0);
}

// ---------- 阻塞 ----------

#[test]
fn non_copy_pop_wait_blocks() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    let h = thread::spawn(move || reader.pop_wait());
    thread::sleep(WAIT);
    writer
        .push(String::from("data"))
        .expect("push should succeed");
    let v = h
        .join()
        .expect("reader thread should not panic")
        .expect("pop_wait should succeed");
    assert_eq!(v, "data");
}

#[test]
fn non_copy_push_wait_blocks() {
    let (reader, writer) = locked_ring_buffer::<String>(2);
    writer.push(String::from("a")).expect("push should succeed");
    writer.push(String::from("b")).expect("push should succeed");
    let w2 = writer.clone();
    let h = thread::spawn(move || w2.push_wait(String::from("c")));
    thread::sleep(WAIT);
    assert_eq!(reader.pop().as_deref(), Some("a"));
    h.join()
        .expect("writer thread should not panic")
        .expect("push_wait should succeed");
    assert_eq!(reader.pop().as_deref(), Some("b"));
    assert_eq!(reader.pop().as_deref(), Some("c"));
}

// ---------- 对端关闭 ----------

#[test]
fn non_copy_pop_wait_cannot_wait_on_writer_drop() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    let h = thread::spawn(move || reader.pop_wait());
    thread::sleep(WAIT);
    drop(writer);
    match h.join().expect("reader thread should not panic") {
        Err(e) => assert!(matches!(e.kind, Kind::CannotWait)),
        Ok(_) => panic!("expected CannotWait"),
    }
}

#[test]
fn non_copy_push_wait_cannot_wait_on_reader_drop() {
    let (reader, writer) = locked_ring_buffer::<String>(2);
    writer.push(String::from("a")).expect("push should succeed");
    writer.push(String::from("b")).expect("push should succeed");
    let w2 = writer.clone();
    let h = thread::spawn(move || w2.push_wait(String::from("c")));
    thread::sleep(WAIT);
    drop(reader);
    match h.join().expect("writer thread should not panic") {
        Err(e) => assert!(matches!(e.kind, Kind::CannotWait)),
        Ok(_) => panic!("expected CannotWait"),
    }
}

// ---------- try_pop / try_push ----------

#[test]
fn non_copy_try_pop_try_push() {
    let (reader, writer) = locked_ring_buffer::<String>(2);
    writer
        .try_push(String::from("a"))
        .expect("try_push should succeed");
    match reader.try_pop() {
        Ok(Some(s)) => assert_eq!(s, "a"),
        Ok(None) => panic!("expected Some"),
        Err(_) => panic!("unexpected Locked"),
    }
}

#[test]
fn non_copy_try_push_full() {
    let (_reader, writer) = locked_ring_buffer::<String>(2);
    writer
        .try_push(String::from("a"))
        .expect("try_push should succeed");
    writer
        .try_push(String::from("b"))
        .expect("try_push should succeed");
    match writer.try_push(String::from("c")) {
        Err(e) => match e.kind {
            Kind::Full(s) => assert_eq!(s, "c"),
            _ => panic!("expected Full"),
        },
        Ok(()) => panic!("expected Full"),
    }
}

// ---------- 四模式 ----------

#[test]
fn non_copy_mode_locked_locked() {
    let (reader, writer) = locked_ring_buffer::<String>(4);
    writer.push(String::from("x")).expect("push should succeed");
    assert_eq!(reader.pop().as_deref(), Some("x"));
}

#[test]
fn non_copy_mode_unlocked_unlocked() {
    let (mut reader, mut writer) = unlocked_ring_buffer::<String>(4);
    writer.push(String::from("x")).expect("push should succeed");
    assert_eq!(reader.pop().as_deref(), Some("x"));
}

#[test]
fn non_copy_mode_locked_read_unlocked_write() {
    let (reader, mut writer) = locked_read_unlocked_write_ring_buffer::<String>(4);
    writer.push(String::from("x")).expect("push should succeed");
    assert_eq!(reader.pop().as_deref(), Some("x"));
}

#[test]
fn non_copy_mode_unlocked_read_locked_write() {
    let (mut reader, writer) = unlocked_read_locked_write_ring_buffer::<String>(4);
    writer.push(String::from("x")).expect("push should succeed");
    assert_eq!(reader.pop().as_deref(), Some("x"));
}

// ---------- 并发 ----------

#[test]
fn non_copy_concurrent_producer_consumer() {
    let (reader, writer) = locked_ring_buffer::<Box<u32>>(16);
    let total = 100u32;
    let w = writer;
    let ph = thread::spawn(move || {
        for i in 0..total {
            w.push_wait(Box::new(i)).expect("push_wait should succeed");
        }
        // w 在此 drop，触发 close_writer
    });

    let mut received = Vec::new();
    loop {
        match reader.pop_wait() {
            Ok(v) => received.push(*v),
            Err(e) if matches!(e.kind, Kind::CannotWait) => break,
            Err(_) => panic!("unexpected error"),
        }
    }
    ph.join().expect("producer thread should not panic");
    assert_eq!(received.len(), total as usize);
    assert_eq!(received, (0..total).collect::<Vec<_>>());
}

#[test]
fn non_copy_concurrent_multiple_writers() {
    let (reader, writer) = locked_ring_buffer::<String>(8);
    let per_writer = 25usize;
    let n_writers = 4usize;
    let total = per_writer * n_writers;

    let mut handles = Vec::new();
    for t in 0..n_writers {
        let w = writer.clone();
        handles.push(thread::spawn(move || {
            for i in 0..per_writer {
                let s = format!("{}_{}", t, i);
                w.push_wait(s).expect("push_wait should succeed");
            }
        }));
    }

    let mut received = Vec::new();
    for _ in 0..total {
        received.push(reader.pop_wait().expect("pop_wait should succeed"));
    }

    for h in handles {
        h.join().expect("writer thread should not panic");
    }
    assert_eq!(received.len(), total);
}
