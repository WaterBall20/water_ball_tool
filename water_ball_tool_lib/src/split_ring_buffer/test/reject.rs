//! `reject_pending_writers` 测试：拒绝 pending 写端、不影响之后的写入阻塞。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crate::split_ring_buffer::*;

const WAIT: Duration = Duration::from_millis(100);

// ---------- push_wait 被拒绝 ----------

#[test]
fn push_wait_rejected_when_blocked() {
    let (reader, writer) = locked_ring_buffer::<u8>(2);
    writer.push(1).expect("push");
    writer.push(2).expect("push"); // 满

    let w2 = writer.clone();
    let h = thread::spawn(move || w2.push_wait(99));
    thread::sleep(WAIT);

    writer.reject_pending_writers();

    match h.join().expect("thread should not panic") {
        Err(e) => match e.kind {
            Kind::WriteRejectedValue(99) => {}
            other => panic!("expected WriteRejectedValue(99), got {:?}", other),
        },
        Ok(()) => panic!("expected WriteRejectedValue"),
    }

    // reject 之后缓冲区已空
    assert_eq!(reader.len(), 0);
}

#[test]
fn push_wait_after_reject_normal() {
    let (_reader, writer) = locked_ring_buffer::<u8>(2);
    writer.push(1).expect("push");
    writer.push(2).expect("push");
    writer.reject_pending_writers();

    // reject 之后的新写端不受影响
    writer.push_wait(3).expect("push_wait after reject");
    assert_eq!(writer.len(), 1);
}

#[test]
fn push_after_reject_normal() {
    let (_reader, writer) = locked_ring_buffer::<u8>(4);
    writer.reject_pending_writers();
    // 非阻塞 push 也不受影响
    writer.push(1).expect("push after reject");
    writer.push(2).expect("push after reject");
    assert_eq!(writer.len(), 2);
}

// ---------- write_wait / write_all 被拒绝 ----------

#[test]
fn write_wait_rejected_when_blocked() {
    let (reader, writer) = locked_ring_buffer::<u8>(2);
    writer.write_all(&[1, 2]).expect("write_all");
    let w2 = writer.clone();
    let h = thread::spawn(move || w2.write_wait(&[3, 4, 5]));
    thread::sleep(WAIT);

    writer.reject_pending_writers();

    match h.join().expect("thread should not panic") {
        Err(e) => assert!(matches!(e.kind, Kind::WriteRejected)),
        Ok(_) => panic!("expected WriteRejected"),
    }
    assert_eq!(reader.len(), 0);
}

#[test]
fn write_all_rejected_when_blocked() {
    let (reader, writer) = locked_ring_buffer::<u8>(2);
    writer.write_all(&[1, 2]).expect("write_all");
    let w2 = writer.clone();
    let h = thread::spawn(move || w2.write_all(&[3, 4, 5]));
    thread::sleep(WAIT);

    writer.reject_pending_writers();

    match h.join().expect("thread should not panic") {
        Err(e) => assert!(matches!(e.kind, Kind::WriteRejected)),
        Ok(_) => panic!("expected WriteRejected"),
    }
    assert_eq!(reader.len(), 0);
}

#[test]
fn write_all_after_reject_normal() {
    let (_reader, writer) = locked_ring_buffer::<u8>(2);
    writer.write_all(&[1, 2]).expect("write_all");
    writer.reject_pending_writers();

    // reject 之后 write_all 正常
    writer.write_all(&[3, 4]).expect("write_all after reject");
    assert_eq!(writer.len(), 2);
}

// ---------- 多次 reject 累积 ----------

#[test]
fn reject_multiple_times() {
    let (_reader, writer) = locked_ring_buffer::<u8>(2);
    writer.push(1).expect("push");
    writer.push(2).expect("push");

    // 第一个等待者
    let w1 = writer.clone();
    let h1 = thread::spawn(move || w1.push_wait(10));
    thread::sleep(WAIT);

    writer.reject_pending_writers();
    let r1 = h1.join().expect("thread should not panic");
    assert!(matches!(r1, Err(ref e) if matches!(e.kind, Kind::WriteRejectedValue(10))));

    // 第二个等待者，在第二次 reject 之前进入
    writer.push(3).expect("push");
    writer.push(4).expect("push");
    let w2 = writer.clone();
    let h2 = thread::spawn(move || w2.push_wait(20));
    thread::sleep(WAIT);

    writer.reject_pending_writers();
    let r2 = h2.join().expect("thread should not panic");
    assert!(matches!(r2, Err(ref e) if matches!(e.kind, Kind::WriteRejectedValue(20))));
}

// ---------- 无等待者时调用 ----------

#[test]
fn reject_with_no_waiters() {
    let (reader, writer) = locked_ring_buffer::<u8>(4);
    writer.push(1).expect("push");
    writer.push(2).expect("push");
    writer.reject_pending_writers();
    assert_eq!(reader.len(), 0);
    // 之后正常
    writer.push(3).expect("push after reject");
    assert_eq!(reader.pop(), Some(3));
}

// ---------- 非 Copy 元素 ----------

#[test]
fn reject_drops_elements() {
    let counter = Arc::new(AtomicUsize::new(0));

    #[derive(Debug)]
    struct DropTracker {
        counter: Arc<AtomicUsize>,
    }
    impl Drop for DropTracker {
        fn drop(&mut self) {
            self.counter.fetch_add(1, Ordering::SeqCst);
        }
    }

    let (reader, writer) = locked_ring_buffer::<DropTracker>(4);
    writer
        .push(DropTracker {
            counter: counter.clone(),
        })
        .expect("push");
    writer
        .push(DropTracker {
            counter: counter.clone(),
        })
        .expect("push");
    writer.reject_pending_writers();
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    assert_eq!(reader.len(), 0);
}

// ---------- 与 clear 互斥 ----------

#[test]
fn clear_then_reject() {
    let (reader, writer) = locked_ring_buffer::<u8>(4);
    writer.push(1).expect("push");
    writer.push(2).expect("push");
    writer.clear();
    assert_eq!(reader.len(), 0);
    writer.reject_pending_writers();
    assert_eq!(reader.len(), 0);
}

#[test]
fn reject_then_clear() {
    let (reader, writer) = locked_ring_buffer::<u8>(4);
    writer.push(1).expect("push");
    writer.reject_pending_writers();
    assert_eq!(reader.len(), 0);
    writer.push(2).expect("push");
    writer.clear();
    assert_eq!(reader.len(), 0);
}

// ---------- 无锁端 ----------

#[test]
fn unlocked_reject_pending_writers() {
    let (mut reader, mut writer) = unlocked_ring_buffer::<u8>(4);
    writer.push(1).expect("push");
    writer.push(2).expect("push");
    writer.reject_pending_writers();
    assert_eq!(reader.len(), 0);
    writer.push(3).expect("push after reject");
    assert_eq!(reader.pop(), Some(3));
}

// ---------- 混合模式 ----------

#[test]
fn mixed_locked_write_reject() {
    let (reader, writer) = locked_read_unlocked_write_ring_buffer::<u8>(2);
    let mut writer = writer;
    writer.push(1).expect("push");
    writer.push(2).expect("push");
    writer.reject_pending_writers();
    assert_eq!(reader.len(), 0);
    writer.push(3).expect("push after reject");
    assert_eq!(reader.pop(), Some(3));
}

#[test]
fn mixed_unlocked_read_locked_write_reject() {
    let (mut reader, writer) = unlocked_read_locked_write_ring_buffer::<u8>(2);
    writer.push(1).expect("push");
    writer.push(2).expect("push");
    writer.reject_pending_writers();
    assert_eq!(reader.len(), 0);
    writer.push(3).expect("push after reject");
    assert_eq!(reader.pop(), Some(3));
}

// ---------- 读端不受影响 ----------

#[test]
fn reader_unaffected_by_reject() {
    let (reader, writer) = locked_ring_buffer::<u8>(4);

    // 让 reader 在空缓冲区上阻塞，真正进入 write_cvar 等待
    let r2 = reader.clone();
    let h = thread::spawn(move || r2.pop_wait());
    thread::sleep(WAIT);

    // reject：清空（已空）+ 递增 epoch；读端不在 epoch 影响范围内
    writer.reject_pending_writers();

    // 读端仍阻塞（epoch 不影响读端）；push 后唤醒
    writer.push(42).expect("push to wake reader");

    let v = h
        .join()
        .expect("reader thread should not panic")
        .expect("pop_wait should succeed");
    assert_eq!(v, 42);
}
