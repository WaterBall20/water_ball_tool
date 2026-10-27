//! 阻塞等待、对端关闭、CannotWait、read_exact / write_all。

use std::thread;
use std::time::Duration;

use crate::split_ring_buffer::*;

const WAIT: Duration = Duration::from_millis(100);

// ---------- 基本阻塞 ----------

#[test]
fn pop_wait_blocks_until_data() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let h = thread::spawn(move || r.pop_wait());
    thread::sleep(WAIT);
    w.push(42).unwrap();
    let v = h.join().unwrap().unwrap();
    assert_eq!(v, 42);
}

#[test]
fn push_wait_blocks_until_space() {
    let (r, w) = locked_ring_buffer::<u8>(2);
    w.write_all(&[1, 2]).unwrap(); // 满
    let w2 = w.clone();
    let h = thread::spawn(move || w2.push_wait(3));
    thread::sleep(WAIT);
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf).unwrap();
    assert_eq!(buf, [1, 2]);
    h.join().unwrap().unwrap();
    // 剩余的 3 应在缓冲区
    assert_eq!(r.len(), 1);
}

#[test]
fn read_wait_blocks_until_data() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let h = thread::spawn(move || {
        let mut buf = [0u8; 4];
        let n = r.read_wait(&mut buf).unwrap();
        (n, buf)
    });
    thread::sleep(WAIT);
    w.write(&[1, 2, 3]);
    let (n, buf) = h.join().unwrap();
    assert_eq!(n, 3);
    assert_eq!(&buf[..3], &[1, 2, 3]);
}

#[test]
fn write_wait_blocks_until_space() {
    let (r, w) = locked_ring_buffer::<u8>(2);
    w.write_all(&[1, 2]).unwrap();
    let w2 = w.clone();
    let h = thread::spawn(move || {
        let n = w2.write_wait(&[3, 4, 5]).unwrap();
        n
    });
    thread::sleep(WAIT);
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf).unwrap();
    let n = h.join().unwrap();
    assert_eq!(n, 2); // 只写了 3, 4
}

// ---------- read_exact / write_all 跨容量 ----------

#[test]
fn write_all_exceeds_capacity_with_reader() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let total: u8 = 20;
    let data: Vec<u8> = (0..total).collect();

    let h = thread::spawn(move || {
        let mut received = vec![0u8; total as usize];
        r.read_exact(&mut received).unwrap();
        received
    });

    w.write_all(&data).unwrap();
    let received = h.join().unwrap();
    assert_eq!(received, data);
}

#[test]
fn read_exact_exceeds_capacity_with_writer() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let total: u8 = 20;
    let data: Vec<u8> = (0..total).collect();

    // `data` 稍后还要用于断言，因此给写线程一份副本。
    let data_for_writer = data.clone();

    let h = thread::spawn(move || {
        w.write_all(&data_for_writer).unwrap();
    });

    let mut received = vec![0u8; total as usize];
    r.read_exact(&mut received).unwrap();
    h.join().unwrap();
    assert_eq!(received, data);
}

// ---------- 对端关闭 ----------

#[test]
fn pop_wait_pending_data_after_writer_drop() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.push(1).unwrap();
    w.push(2).unwrap();
    drop(w);
    assert_eq!(r.pop_wait().unwrap(), 1);
    assert_eq!(r.pop_wait().unwrap(), 2);
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
    match h.join().unwrap() {
        Err(e) => assert!(matches!(e.kind, Kind::CannotWait)),
        Ok(_) => panic!("expected CannotWait"),
    }
}

#[test]
fn write_all_returns_cannot_wait_on_reader_drop() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.write_all(&[1, 2, 3, 4]).unwrap();
    let w2 = w.clone();
    let h = thread::spawn(move || w2.write_all(&[5, 6, 7, 8]));
    thread::sleep(WAIT);
    drop(r);
    match h.join().unwrap() {
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
    match h.join().unwrap() {
        Err(e) => assert!(matches!(e.kind, Kind::CannotWait)),
        Ok(_) => panic!("expected CannotWait"),
    }
}

#[test]
fn push_wait_returns_cannot_wait_on_reader_drop() {
    let (r, w) = locked_ring_buffer::<u8>(2);
    w.write_all(&[1, 2]).unwrap();
    let w2 = w.clone();
    let h = thread::spawn(move || w2.push_wait(3));
    thread::sleep(WAIT);
    drop(r);
    match h.join().unwrap() {
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
    let res1 = h1.join().unwrap();
    let res2 = h2.join().unwrap();
    assert!(matches!(res1, Err(ref e) if matches!(e.kind, Kind::CannotWait)));
    assert!(matches!(res2, Err(ref e) if matches!(e.kind, Kind::CannotWait)));
}