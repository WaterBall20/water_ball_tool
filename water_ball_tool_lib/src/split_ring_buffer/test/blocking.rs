//! 阻塞等待、对端关闭、CannotWait、read_exact / write_all。

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::split_ring_buffer::*;

const WAIT: Duration = Duration::from_millis(100);

// ---------- 基本阻塞 ----------

#[test]
fn pop_wait_blocks_until_data() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let h = thread::spawn(move || r.pop_wait());
    thread::sleep(WAIT);
    w.push(42).expect("push should succeed");
    let v = h
        .join()
        .expect("reader thread should not panic")
        .expect("pop_wait should succeed");
    assert_eq!(v, 42);
}

#[test]
fn push_wait_blocks_until_space() {
    let (r, w) = locked_ring_buffer::<u8>(2);
    w.write_all(&[1, 2]).expect("write_all should succeed"); // 满
    let w2 = w.clone();
    let h = thread::spawn(move || w2.push_wait(3));
    thread::sleep(WAIT);
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf).expect("read_exact should succeed");
    assert_eq!(buf, [1, 2]);
    h.join()
        .expect("writer thread should not panic")
        .expect("push_wait should succeed");
    assert_eq!(r.len(), 1);
}

#[test]
fn read_wait_blocks_until_data() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let h = thread::spawn(move || {
        let mut buf = [0u8; 4];
        let n = r.read_wait(&mut buf).expect("read_wait should succeed");
        (n, buf)
    });
    thread::sleep(WAIT);
    w.write(&[1, 2, 3]);
    let (n, buf) = h.join().expect("reader thread should not panic");
    assert_eq!(n, 3);
    assert_eq!(&buf[..3], &[1, 2, 3]);
}

#[test]
fn write_wait_blocks_until_space() {
    let (r, w) = locked_ring_buffer::<u8>(2);
    w.write_all(&[1, 2]).expect("write_all should succeed");
    let w2 = w.clone();
    let h = thread::spawn(move || w2.write_wait(&[3, 4, 5]).expect("write_wait should succeed"));
    thread::sleep(WAIT);
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf).expect("read_exact should succeed");
    let n = h.join().expect("writer thread should not panic");
    assert_eq!(n, 2);
}

// ---------- read_exact / write_all 跨容量 ----------

#[test]
fn write_all_exceeds_capacity_with_reader() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let total: u8 = 20;
    let data: Vec<u8> = (0..total).collect();

    let h = thread::spawn(move || {
        let mut received = vec![0u8; total as usize];
        r.read_exact(&mut received).expect("read_exact should succeed");
        received
    });

    w.write_all(&data).expect("write_all should succeed");
    let received = h.join().expect("reader thread should not panic");
    assert_eq!(received, data);
}

#[test]
fn read_exact_exceeds_capacity_with_writer() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let total: u8 = 20;
    let data: Vec<u8> = (0..total).collect();
    let data_for_writer = data.clone();

    let h = thread::spawn(move || {
        w.write_all(&data_for_writer).expect("write_all should succeed");
    });

    let mut received = vec![0u8; total as usize];
    r.read_exact(&mut received).expect("read_exact should succeed");
    h.join().expect("writer thread should not panic");
    assert_eq!(received, data);
}

// ---------- 对端关闭 ----------

#[test]
fn pop_wait_pending_data_after_writer_drop() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.push(1).expect("push should succeed");
    w.push(2).expect("push should succeed");
    drop(w);
    assert_eq!(r.pop_wait().expect("pop_wait should return 1"), 1);
    assert_eq!(r.pop_wait().expect("pop_wait should return 2"), 2);
    match r.pop_wait() {
        Err(e) => assert!(matches!(e.kind, Kind::CannotWait)),
        Ok(_) => panic!("expected CannotWait"),
    }
}

#[test]
fn pop_wait_returns_cannot_wait_on_writer_drop() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let h = thread::spawn(move || r.pop_wait());
    thread::sleep(WAIT);
    drop(w);
    match h.join().expect("reader thread should not panic") {
        Err(e) => assert!(matches!(e.kind, Kind::CannotWait)),
        Ok(_) => panic!("expected CannotWait"),
    }
}

#[test]
fn write_all_returns_cannot_wait_on_reader_drop() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.write_all(&[1, 2, 3, 4]).expect("write_all should succeed");
    let w2 = w.clone();
    let h = thread::spawn(move || w2.write_all(&[5, 6, 7, 8]));
    thread::sleep(WAIT);
    drop(r);
    match h.join().expect("writer thread should not panic") {
        Err(e) => assert!(matches!(e.kind, Kind::CannotWait)),
        Ok(_) => panic!("expected CannotWait"),
    }
}

#[test]
fn read_wait_returns_cannot_wait_on_writer_drop() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let h = thread::spawn(move || {
        let mut buf = [0u8; 4];
        r.read_wait(&mut buf)
    });
    thread::sleep(WAIT);
    drop(w);
    match h.join().expect("reader thread should not panic") {
        Err(e) => assert!(matches!(e.kind, Kind::CannotWait)),
        Ok(_) => panic!("expected CannotWait"),
    }
}

#[test]
fn push_wait_returns_cannot_wait_on_reader_drop() {
    let (r, w) = locked_ring_buffer::<u8>(2);
    w.write_all(&[1, 2]).expect("write_all should succeed");
    let w2 = w.clone();
    let h = thread::spawn(move || w2.push_wait(3));
    thread::sleep(WAIT);
    drop(r);
    match h.join().expect("writer thread should not panic") {
        Err(e) => assert!(matches!(e.kind, Kind::CannotWait)),
        Ok(_) => panic!("expected CannotWait"),
    }
}

// ---------- 多 clone 同时被唤醒 ----------

#[test]
fn writer_drop_wakes_multiple_readers() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let r1 = r.clone();
    let r2 = r.clone();
    let h1 = thread::spawn(move || r1.pop_wait());
    let h2 = thread::spawn(move || r2.pop_wait());
    thread::sleep(WAIT);
    drop(w);
    let res1 = h1.join().expect("reader1 thread should not panic");
    let res2 = h2.join().expect("reader2 thread should not panic");
    assert!(matches!(res1, Err(ref e) if matches!(e.kind, Kind::CannotWait)));
    assert!(matches!(res2, Err(ref e) if matches!(e.kind, Kind::CannotWait)));
}

// ---------- truncate_back 与阻塞写端 ----------

#[test]
fn truncate_back_unblocks_waiting_write_all() {
    let (reader, writer) = locked_ring_buffer::<u8>(4);
    writer.write_all(&[1, 2, 3, 4]).expect("write_all should succeed"); // 满
    let w2 = writer.clone();
    let h = thread::spawn(move || w2.write_all(&[5]));

    //AI=== 等到写线程持写端锁（try_push 返回 Locked）：此时它正阻塞等待空位。
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match writer.try_push(0) {
            Err(e) if matches!(e.kind, Kind::Locked) => break,
            _ if Instant::now() > deadline => panic!("write_all never held the write lock"),
            _ => thread::yield_now(),
        }
    }
    //AI_END===

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        writer.truncate_back(2);
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(5))
        .expect("truncate_back must not wait for the blocked write_all");

    // 截断保留前 2 个元素，被阻塞的写端随后追加 5。
    let mut buf = [0u8; 3];
    reader.read_exact(&mut buf).expect("read_exact should succeed");
    assert_eq!(buf, [1, 2, 5]);
    h.join()
        .expect("writer thread should not panic")
        .expect("write_all should succeed");
}