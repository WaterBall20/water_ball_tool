//! try_* 方法、clear、并发综合。

use std::thread;
use std::time::Duration;

use crate::split_ring_buffer::*;

const WAIT: Duration = Duration::from_millis(100);

// ---------- try_* 成功路径 ----------

#[test]
fn try_pop_success() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.push(1).expect("push should succeed");
    assert_eq!(r.try_pop().expect("try_pop should succeed"), Some(1));
    assert_eq!(r.try_pop().expect("try_pop should succeed"), None);
}

#[test]
fn try_read_success() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.write(&[1, 2, 3]);
    let mut buf = [0u8; 4];
    assert_eq!(r.try_read(&mut buf).expect("try_read should succeed"), 3);
    assert_eq!(&buf[..3], &[1, 2, 3]);
}

#[test]
fn try_push_success() {
    let (r, w) = locked_ring_buffer::<u8>(2);
    w.try_push(1).expect("try_push should succeed");
    w.try_push(2).expect("try_push should succeed");
    match w.try_push(3) {
        Err(e) => assert!(matches!(e.kind, Kind::Full(3))),
        Ok(_) => panic!("expected Full"),
    }
    assert_eq!(r.pop(), Some(1));
}

#[test]
fn try_write_success() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let mut n = w
        .try_write(&[1, 2, 3])
        .expect("try_write should succeed");
    assert_eq!(n, 3);
    n = w.try_write(&[4, 5, 6]).expect("try_write should succeed");
    assert_eq!(n, 1);
    assert!(r.is_full());
}

// ---------- try_* 锁冲突 ----------

#[test]
fn try_pop_locked_when_read_lock_held() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let r2 = r.clone();
    let h = thread::spawn(move || {
        let mut buf = [0u8; 1];
        r2.read_exact(&mut buf).expect("read_exact should succeed");
    });
    thread::sleep(WAIT);
    match r.try_pop() {
        Err(e) => assert!(matches!(e.kind, Kind::Locked)),
        Ok(_) => panic!("expected Locked"),
    }
    w.push(1).expect("push should succeed");
    h.join().expect("reader thread should not panic");
}

#[test]
fn try_read_locked_when_read_lock_held() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let r2 = r.clone();
    let h = thread::spawn(move || {
        let mut buf = [0u8; 1];
        r2.read_exact(&mut buf).expect("read_exact should succeed");
    });
    thread::sleep(WAIT);
    let mut buf = [0u8; 2];
    match r.try_read(&mut buf) {
        Err(e) => assert!(matches!(e.kind, Kind::Locked)),
        Ok(_) => panic!("expected Locked"),
    }
    w.push(1).expect("push should succeed");
    h.join().expect("reader thread should not panic");
}

#[test]
fn try_push_locked_when_write_lock_held() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.write_all(&[1, 2, 3, 4]).expect("write_all should succeed");
    let w2 = w.clone();
    let h = thread::spawn(move || w2.write_all(&[5, 6, 7, 8]));
    thread::sleep(WAIT);
    match w.try_push(99) {
        Err(e) => assert!(matches!(e.kind, Kind::Locked)),
        Ok(_) => panic!("expected Locked"),
    }
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf).expect("read_exact should succeed");
    h.join()
        .expect("writer thread should not panic")
        .expect("write_all should succeed");
}

#[test]
fn try_write_locked_when_write_lock_held() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.write_all(&[1, 2, 3, 4]).expect("write_all should succeed");
    let w2 = w.clone();
    let h = thread::spawn(move || w2.write_all(&[5, 6, 7, 8]));
    thread::sleep(WAIT);
    match w.try_write(&[9, 10]) {
        Err(e) => assert!(matches!(e.kind, Kind::Locked)),
        Ok(_) => panic!("expected Locked"),
    }
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf).expect("read_exact should succeed");
    h.join()
        .expect("writer thread should not panic")
        .expect("write_all should succeed");
}

// ---------- clear ----------

#[test]
fn clear_empties_buffer() {
    let (r, w) = locked_ring_buffer::<u8>(8);
    w.write(&[1, 2, 3, 4, 5]);
    assert_eq!(r.len(), 5);
    w.clear();
    assert_eq!(r.len(), 0);
    assert!(r.is_empty());
    w.write(&[10, 20]);
    assert_eq!(r.len(), 2);
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf).expect("read_exact should succeed");
    assert_eq!(buf, [10, 20]);
}

#[test]
fn clear_after_full_allows_write() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.write_all(&[1, 2, 3, 4]).expect("write_all should succeed");
    assert!(r.is_full());
    w.clear();
    assert!(!r.is_full());
    w.write_all(&[5, 6, 7, 8]).expect("write_all should succeed");
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf).expect("read_exact should succeed");
    assert_eq!(buf, [5, 6, 7, 8]);
}

#[test]
fn clear_wakes_write_waiters() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.write_all(&[1, 2, 3, 4]).expect("write_all should succeed");
    let w2 = w.clone();
    let h = thread::spawn(move || w2.push_wait(99));
    thread::sleep(WAIT);
    w.clear();
    h.join()
        .expect("writer thread should not panic")
        .expect("push_wait should succeed");
    assert_eq!(r.len(), 1);
}

// ---------- 并发综合 ----------

#[test]
fn concurrent_write_all_read_exact() {
    let (r, w) = locked_ring_buffer::<u64>(16);
    let total: u64 = 2000;
    let expected: Vec<u64> = (0..total).collect();
    let expected_w = expected.clone();

    let wh = thread::spawn(move || w.write_all(&expected_w));
    let rh = thread::spawn(move || {
        let mut received = vec![0u64; total as usize];
        r.read_exact(&mut received).expect("read_exact should succeed");
        received
    });

    wh.join()
        .expect("writer thread should not panic")
        .expect("write_all should succeed");
    let received = rh.join().expect("reader thread should not panic");
    assert_eq!(received, expected);
}

#[test]
fn concurrent_two_readers_split_data() {
    let (r, w) = locked_ring_buffer::<u32>(8);
    let total: u32 = 1000;

    let r1 = r.clone();
    let r2 = r.clone();

    let wh = thread::spawn(move || {
        let mut written = 0u32;
        while written < total {
            let end = (written + 10).min(total);
            let chunk: Vec<u32> = (written..end).collect();
            let n = w.write(&chunk);
            written += n as u32;
            if n == 0 {
                thread::yield_now();
            }
        }
        drop(w);
    });

    let h1 = thread::spawn(move || {
        let mut data = Vec::new();
        loop {
            match r1.pop_wait() {
                Ok(v) => data.push(v),
                Err(_) => break,
            }
        }
        data
    });
    let h2 = thread::spawn(move || {
        let mut data = Vec::new();
        loop {
            match r2.pop_wait() {
                Ok(v) => data.push(v),
                Err(_) => break,
            }
        }
        data
    });

    wh.join().expect("writer thread should not panic");

    let mut all = h1.join().expect("reader1 thread should not panic");
    all.extend(h2.join().expect("reader2 thread should not panic"));

    assert_eq!(all.len(), total as usize);

    all.sort();
    assert_eq!(all, (0..total).collect::<Vec<_>>());
}

#[test]
fn concurrent_multiple_writers_single_reader() {
    let (r, w) = locked_ring_buffer::<u32>(16);
    let total: u32 = 1000;
    let per_writer = total / 4;

    let mut wh = vec![];
    for t in 0..4u32 {
        let w = w.clone();
        wh.push(thread::spawn(move || {
            let base = t * per_writer;
            for i in 0..per_writer {
                let v = base + i;
                while w.push(v).is_err() {
                    thread::yield_now();
                }
            }
        }));
    }

    let rh = thread::spawn(move || {
        let mut received = vec![];
        loop {
            match r.pop() {
                Some(v) => {
                    received.push(v);
                    if received.len() == total as usize {
                        break;
                    }
                }
                None => thread::yield_now(),
            }
        }
        received
    });

    for h in wh {
        h.join().expect("writer thread should not panic");
    }
    let mut received = rh.join().expect("reader thread should not panic");
    assert_eq!(received.len(), total as usize);
    received.sort();
    assert_eq!(received, (0..total).collect::<Vec<_>>());
}

// ---------- 超容量 + 阻塞混合 ----------

#[test]
fn blocking_roundtrip_large() {
    let (r, w) = locked_ring_buffer::<u32>(4);
    let total: u32 = 500;
    let data: Vec<u32> = (0..total).collect();
    let data_w = data.clone();

    let wh = thread::spawn(move || w.write_all(&data_w));
    let rh = thread::spawn(move || {
        let mut buf = vec![0u32; total as usize];
        r.read_exact(&mut buf).expect("read_exact should succeed");
        buf
    });

    wh.join()
        .expect("writer thread should not panic")
        .expect("write_all should succeed");
    let received = rh.join().expect("reader thread should not panic");
    assert_eq!(received, data);
}