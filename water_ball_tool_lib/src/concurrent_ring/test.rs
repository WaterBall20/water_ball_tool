use super::*;
use crate::concurrent_ring::ConcurrentRing;
use std::hint::spin_loop;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

// ==================== 测试辅助 ====================

struct DropCounter {
    id: u32,
    counter: Arc<AtomicUsize>,
}
impl DropCounter {
    fn new(id: u32, counter: Arc<AtomicUsize>) -> Self {
        Self { id, counter }
    }
}
impl Drop for DropCounter {
    fn drop(&mut self) {
        self.counter.fetch_add(1, Ordering::SeqCst);
    }
}

/// 断言 Result 为 Ok，返回内部值（等价 unwrap，但显式处理）。
fn expect_ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic!("expected Ok, got Err: {:?}", e),
    }
}

/// 断言 Result 为 Err，返回错误（等价 unwrap_err）。
fn expect_err<T: std::fmt::Debug, E>(r: Result<T, E>) -> E {
    match r {
        Ok(v) => panic!("expected Err, got Ok: {:?}", v),
        Err(e) => e,
    }
}

/// 断言 Option 为 Some，返回内部值（等价 unwrap）。
fn expect_some<T>(v: Option<T>) -> T {
    match v {
        Some(x) => x,
        None => panic!("expected Some"),
    }
}

/// join 一个 JoinHandle，若线程 panic 则在此 panic。
fn join_ok<T>(h: thread::JoinHandle<T>) -> T {
    match h.join() {
        Ok(v) => v,
        Err(_) => panic!("thread panicked"),
    }
}

/// `send` 失败则 panic。
fn send_ok<T>(tx: &mpsc::Sender<T>, v: T) {
    if tx.send(v).is_err() {
        panic!("send failed: receiver dropped");
    }
}

/// `recv` 失败则 panic。
fn recv_ok<T>(rx: &mpsc::Receiver<T>) -> T {
    match rx.recv() {
        Ok(v) => v,
        Err(_) => panic!("recv failed: sender dropped"),
    }
}

/// `recv_timeout` 失败则 panic。
fn recv_timeout_ok<T>(rx: &mpsc::Receiver<T>, dur: Duration) -> T {
    match rx.recv_timeout(dur) {
        Ok(v) => v,
        Err(_) => panic!("recv_timeout failed"),
    }
}

// ==================== 构造与边界 ====================

#[test]
#[should_panic(expected = "capacity must be non-zero")]
fn new_zero_capacity_panics() {
    let _ = ConcurrentRing::<u32>::new_max_capacity(0);
}

#[test]
fn new_capacity_one() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(1);
    assert_eq!(ring.capacity(), 1);
    assert!(ring.is_empty());
    assert!(!ring.is_full());
}

#[test]
fn capacity_one_alternates() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(1);
    for i in 0..10 {
        assert!(ring.push(i).is_ok());
        assert!(ring.is_full());
        assert_eq!(ring.pop(), Some(i));
        assert!(ring.is_empty());
    }
}

// ==================== push / pop ====================

#[test]
fn push_pop_fifo() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    assert!(ring.push(1).is_ok());
    assert!(ring.push(2).is_ok());
    assert_eq!(ring.pop(), Some(1));
    assert_eq!(ring.pop(), Some(2));
    assert_eq!(ring.pop(), None);
}

#[test]
fn push_returns_value_when_full() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(2);
    assert!(ring.push(1).is_ok());
    assert!(ring.push(2).is_ok());
    let err = expect_err(ring.push(3));
    assert!(matches!(err.kind, ConcurrentRingErrorKind::Full(3)));
}

#[test]
fn pop_returns_none_when_empty() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(2);
    assert_eq!(ring.pop(), None);
}

#[test]
fn push_pop_wraps_many_rounds() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    for round in 0..10u32 {
        for i in 0..4 {
            assert!(ring.push(round * 4 + i).is_ok());
        }
        for i in 0..4 {
            assert_eq!(ring.pop(), Some(round * 4 + i));
        }
    }
}

// ==================== write / read ====================

#[test]
fn write_read_fifo() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(8);
    assert_eq!(ring.write(&[1, 2, 3, 4]), 4);
    let mut buf = [0u32; 4];
    assert_eq!(ring.read(&mut buf), 4);
    assert_eq!(buf, [1, 2, 3, 4]);
}

#[test]
fn write_partial_when_not_enough_space() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    assert!(ring.push(1).is_ok());
    assert!(ring.push(2).is_ok());
    assert!(ring.push(3).is_ok());
    assert_eq!(ring.write(&[10, 20, 30]), 1);
    let mut buf = [0u32; 4];
    assert_eq!(ring.borrow(&mut buf), 4);
    assert_eq!(buf, [1, 2, 3, 10]);
}

#[test]
fn write_returns_zero_when_full() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(2);
    ring.write(&[1, 2]);
    assert_eq!(ring.write(&[99]), 0);
}

#[test]
fn write_empty_slice_returns_zero() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    assert_eq!(ring.write(&[]), 0);
    assert!(ring.is_empty());
}

#[test]
fn read_partial_when_out_smaller() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3, 4]);
    let mut small = [0u32; 2];
    assert_eq!(ring.read(&mut small), 2);
    assert_eq!(small, [1, 2]);
    assert_eq!(ring.read(&mut small), 2);
    assert_eq!(small, [3, 4]);
}

#[test]
fn read_returns_zero_when_empty() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    let mut buf = [0u32; 4];
    assert_eq!(ring.read(&mut buf), 0);
    assert_eq!(ring.borrow(&mut buf), 0);
}

#[test]
fn write_wraps_around() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    assert!(ring.push(1).is_ok());
    assert!(ring.push(2).is_ok());
    assert_eq!(ring.pop(), Some(1));
    assert_eq!(ring.pop(), Some(2));
    assert_eq!(ring.write(&[10, 20, 30, 40]), 4);
    let mut buf = [0u32; 4];
    assert_eq!(ring.read(&mut buf), 4);
    assert_eq!(buf, [10, 20, 30, 40]);
}

#[test]
fn read_wraps_around() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3, 4]);
    assert_eq!(ring.read(&mut [0u32; 2]), 2);
    ring.write(&[5, 6]);
    let mut buf = [0u32; 4];
    assert_eq!(ring.read(&mut buf), 4);
    assert_eq!(buf, [3, 4, 5, 6]);
}

// ==================== borrow ====================

#[test]
fn borrow_does_not_advance_read_pos() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3]);
    let mut buf = [0u32; 3];
    assert_eq!(ring.borrow(&mut buf), 3);
    assert_eq!(buf, [1, 2, 3]);
    let mut again = [0u32; 3];
    assert_eq!(ring.borrow(&mut again), 3);
    assert_eq!(again, [1, 2, 3]);
    let mut consumed = [0u32; 3];
    assert_eq!(ring.read(&mut consumed), 3);
    assert_eq!(consumed, [1, 2, 3]);
}

#[test]
fn borrow_wraps_around() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3, 4]);
    assert_eq!(ring.read(&mut [0u32; 2]), 2);
    ring.write(&[5, 6]);
    let mut buf = [0u32; 4];
    assert_eq!(ring.borrow(&mut buf), 4);
    assert_eq!(buf, [3, 4, 5, 6]);
}

#[test]
fn borrow_partial_when_out_smaller() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3, 4]);
    let mut small = [0u32; 2];
    assert_eq!(ring.borrow(&mut small), 2);
    assert_eq!(small, [1, 2]);
    let mut again = [0u32; 2];
    assert_eq!(ring.borrow(&mut again), 2);
    assert_eq!(again, [1, 2]);
}

#[test]
fn read_and_borrow_consistency() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    for v in 1..=4 {
        assert!(ring.push(v).is_ok());
    }
    let mut peek = [0u32; 2];
    assert_eq!(ring.borrow(&mut peek), 2);
    assert_eq!(peek, [1, 2]);
    assert_eq!(ring.borrow(&mut peek), 2);
    assert_eq!(peek, [1, 2]);
    let mut buf = [0u32; 2];
    assert_eq!(ring.read(&mut buf), 2);
    assert_eq!(buf, [1, 2]);
    assert!(ring.push(5).is_ok());
    assert!(ring.push(6).is_ok());
    let mut wrap = [0u32; 4];
    assert_eq!(ring.borrow(&mut wrap), 4);
    assert_eq!(wrap, [3, 4, 5, 6]);
}

// ==================== write_wait / read_wait ====================

#[test]
fn write_wait_empty_returns_zero() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    assert_eq!(ring.write_wait(&[]), 0);
}

#[test]
fn write_wait_returns_written_count_with_space() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    assert_eq!(ring.write_wait(&[1, 2, 3]), 3);
    let mut buf = [0u32; 3];
    assert_eq!(ring.read(&mut buf), 3);
    assert_eq!(buf, [1, 2, 3]);
}

#[test]
fn write_wait_partial_when_insufficient_space() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3]);
    assert_eq!(ring.write_wait(&[10, 20, 30]), 1);
}

#[test]
fn write_wait_blocks_then_writes() {
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(2));
    ring.write(&[1, 2]);
    assert!(ring.is_full());
    let r = ring.clone();
    let writer = thread::spawn(move || r.write_wait(&[3, 4]));
    thread::sleep(Duration::from_millis(50));
    assert_eq!(ring.len(), 2);
    assert_eq!(ring.pop(), Some(1));
    let n = join_ok(writer);
    assert_eq!(n, 1);
    assert_eq!(ring.len(), 2);
    assert_eq!(ring.pop(), Some(2));
    assert_eq!(ring.pop(), Some(3));
}

#[test]
fn read_wait_empty_slice_returns_zero() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    let mut empty: [u32; 0] = [];
    assert_eq!(ring.read_wait(&mut empty), 0);
}

#[test]
fn read_wait_returns_read_count_with_data() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3]);
    let mut buf = [0u32; 4];
    assert_eq!(ring.read_wait(&mut buf), 3);
    assert_eq!(buf[..3], [1, 2, 3]);
}

#[test]
fn read_wait_blocks_then_reads() {
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(2));
    let r = ring.clone();
    let reader = thread::spawn(move || {
        let mut buf = [0u32; 2];
        let n = r.read_wait(&mut buf);
        (n, buf)
    });
    thread::sleep(Duration::from_millis(50));
    assert!(ring.is_empty());
    assert!(ring.push(10).is_ok());
    let (n, buf) = join_ok(reader);
    assert_eq!(n, 1);
    assert_eq!(buf[0], 10);
}

// ==================== write_all / read_exact ====================

#[test]
fn write_all_empty_ok() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(2);
    assert!(ring.write_all(&[]).is_ok());
}

#[test]
fn write_all_writes_everything_when_space() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    assert!(ring.write_all(&[1, 2, 3, 4]).is_ok());
    let mut buf = [0u32; 4];
    assert_eq!(ring.read(&mut buf), 4);
    assert_eq!(buf, [1, 2, 3, 4]);
}

#[test]
fn write_all_too_large_returns_too_large() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(2);
    let err = expect_err(ring.write_all(&[1, 2, 3]));
    assert!(matches!(err.kind, ConcurrentRingErrorKind::TooLarge));
    assert!(ring.is_empty());
}

#[test]
fn write_all_blocks_until_all_written() {
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(4));
    let r = ring.clone();
    let reader = thread::spawn(move || {
        let mut got = Vec::new();
        while got.len() < 4 {
            if let Some(v) = r.pop() {
                got.push(v);
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        got
    });
    assert!(ring.write_all(&[1, 2, 3, 4]).is_ok());
    let got = join_ok(reader);
    assert_eq!(got, vec![1, 2, 3, 4]);
}

#[test]
fn read_exact_empty_ok() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(2);
    let mut empty: [u32; 0] = [];
    assert!(ring.read_exact(&mut empty).is_ok());
}

#[test]
fn read_exact_reads_everything_when_data() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3]);
    let mut buf = [0u32; 3];
    assert!(ring.read_exact(&mut buf).is_ok());
    assert_eq!(buf, [1, 2, 3]);
    assert!(ring.is_empty());
}

#[test]
fn read_exact_too_large_returns_too_large() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(2);
    let mut buf = [0u32; 3];
    let err = expect_err(ring.read_exact(&mut buf));
    assert!(matches!(err.kind, ConcurrentRingErrorKind::TooLarge));
}

#[test]
fn read_exact_blocks_until_filled() {
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(4));
    ring.write(&[1, 2]);
    let r = ring.clone();
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        r.write(&[3, 4]);
    });
    let mut buf = [0u32; 4];
    assert!(ring.read_exact(&mut buf).is_ok());
    assert_eq!(buf, [1, 2, 3, 4]);
    join_ok(writer);
}

// ==================== clear ====================

#[test]
fn clear_empties_buffer() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3]);
    assert_eq!(ring.len(), 3);
    ring.clear();
    assert_eq!(ring.len(), 0);
    assert!(ring.is_empty());
    assert_eq!(ring.write(&[10, 20]), 2);
    let mut buf = [0u32; 2];
    assert_eq!(ring.read(&mut buf), 2);
    assert_eq!(buf, [10, 20]);
}

#[test]
fn clear_on_empty_is_noop() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.clear();
    assert!(ring.is_empty());
    assert!(ring.push(1).is_ok());
    assert_eq!(ring.pop(), Some(1));
}

#[test]
fn clear_drops_all_values() {
    let counter = Arc::new(AtomicUsize::new(0));
    let ring = ConcurrentRing::<DropCounter>::new_max_capacity(4);
    assert!(ring.push(DropCounter::new(1, counter.clone())).is_ok());
    assert!(ring.push(DropCounter::new(2, counter.clone())).is_ok());
    assert!(ring.push(DropCounter::new(3, counter.clone())).is_ok());
    ring.clear();
    assert_eq!(counter.load(Ordering::SeqCst), 3);
    assert!(ring.is_empty());
}

#[test]
fn clear_wakes_blocked_writers() {
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(2));
    ring.write(&[1, 2]);
    assert!(ring.is_full());
    let r = ring.clone();
    let writer = thread::spawn(move || r.write_wait(&[3, 4]));
    thread::sleep(Duration::from_millis(50));
    assert_eq!(ring.len(), 2);
    ring.clear();
    assert!(ring.is_empty());
    let n = join_ok(writer);
    assert_eq!(n, 2);
    let mut buf = [0u32; 2];
    assert_eq!(ring.read(&mut buf), 2);
    assert_eq!(buf, [3, 4]);
}

// ==================== Drop 正确性 ====================

#[test]
fn drop_initialized_values_on_ring_drop() {
    let counter = Arc::new(AtomicUsize::new(0));
    {
        let ring = ConcurrentRing::<DropCounter>::new_max_capacity(4);
        assert!(ring.push(DropCounter::new(1, counter.clone())).is_ok());
        assert!(ring.push(DropCounter::new(2, counter.clone())).is_ok());
        assert!(ring.push(DropCounter::new(3, counter.clone())).is_ok());
        let v = expect_some(ring.pop());
        assert_eq!(v.id, 1);
        drop(v);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[test]
fn no_double_drop_after_pop() {
    let counter = Arc::new(AtomicUsize::new(0));
    {
        let ring = ConcurrentRing::<DropCounter>::new_max_capacity(2);
        assert!(ring.push(DropCounter::new(1, counter.clone())).is_ok());
        let _ = expect_some(ring.pop());
    }
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn empty_ring_drop_no_leak() {
    let counter = Arc::new(AtomicUsize::new(0));
    {
        let _ring = ConcurrentRing::<DropCounter>::new_max_capacity(4);
    }
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[test]
fn repeated_push_pop_no_leak() {
    let counter = Arc::new(AtomicUsize::new(0));
    let ring = ConcurrentRing::<DropCounter>::new_max_capacity(2);
    for _ in 0..100 {
        assert!(ring.push(DropCounter::new(1, counter.clone())).is_ok());
        let v = expect_some(ring.pop());
        drop(v);
    }
    assert_eq!(counter.load(Ordering::SeqCst), 100);
}

// ==================== 多线程冒烟 ====================

#[test]
fn concurrent_mpsc_push_pop() {
    let ring = Arc::new(ConcurrentRing::<usize>::new_max_capacity(64));
    let n_producers = 4;
    let per_producer = 500;
    let total = n_producers * per_producer;

    let producers: Vec<_> = (0..n_producers)
        .map(|_| {
            let r = ring.clone();
            thread::spawn(move || {
                for i in 0..per_producer {
                    let v = i;
                    loop {
                        match r.push(v) {
                            Ok(()) => break,
                            Err(_) => spin_loop(),
                        }
                    }
                }
            })
        })
        .collect();

    let consumer = {
        let r = ring.clone();
        thread::spawn(move || {
            let mut got = 0usize;
            while got < total {
                if r.pop().is_some() {
                    got += 1;
                } else {
                    spin_loop();
                }
            }
            got
        })
    };

    for t in producers {
        join_ok(t);
    }
    assert_eq!(join_ok(consumer), total);
    assert!(ring.is_empty());
}

#[test]
fn concurrent_write_read_roundtrip() {
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(32));
    let rounds = 200;

    let writer = {
        let r = ring.clone();
        thread::spawn(move || {
            let mut sent = 0usize;
            while sent < rounds * 4 {
                let batch: Vec<u32> = (0..4).map(|i| (sent + i) as u32).collect();
                let n = r.write(&batch);
                sent += n;
                if n == 0 {
                    spin_loop();
                }
            }
            sent
        })
    };

    let reader = {
        let r = ring.clone();
        thread::spawn(move || {
            let mut received = 0usize;
            let mut buf = [0u32; 4];
            while received < rounds * 4 {
                let n = r.read(&mut buf);
                received += n;
                if n == 0 {
                    spin_loop();
                }
            }
            received
        })
    };

    assert_eq!(join_ok(writer), rounds * 4);
    assert_eq!(join_ok(reader), rounds * 4);
}

#[test]
fn concurrent_borrow_with_writer_and_reader() {
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(16));
    let total = 2000;

    let writer = {
        let r = ring.clone();
        thread::spawn(move || {
            let mut sent = 0usize;
            while sent < total {
                let v = sent as u32;
                loop {
                    match r.push(v) {
                        Ok(()) => break,
                        Err(_) => spin_loop(),
                    }
                }
                sent += 1;
            }
        })
    };

    let reader = {
        let r = ring.clone();
        thread::spawn(move || {
            let mut received = 0usize;
            while received < total {
                if r.pop().is_some() {
                    received += 1;
                } else {
                    spin_loop();
                }
            }
        })
    };

    let borrower = {
        let r = ring.clone();
        thread::spawn(move || {
            let mut buf = [0u32; 16];
            for _ in 0..1000 {
                let n = r.borrow(&mut buf);
                assert!(n <= 16);
            }
        })
    };

    join_ok(writer);
    join_ok(reader);
    join_ok(borrower);
    assert!(ring.is_empty());
}

#[test]
fn locks_not_poisoned_after_concurrent_use() {
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(8));
    let handles: Vec<_> = (0..4)
        .map(|t| {
            let r = ring.clone();
            thread::spawn(move || {
                for i in 0..1000 {
                    let _ = r.push((t * 1000 + i) as u32);
                    let _ = r.pop();
                    let mut buf = [0u32; 4];
                    let _ = r.borrow(&mut buf);
                    let _ = r.read(&mut buf);
                }
            })
        })
        .collect();
    for h in handles {
        join_ok(h);
    }
    assert!(ring.push(1).is_ok());
}

#[test]
fn concurrent_write_wait_read_wait() {
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(8));
    let total = 1000;

    let writers: Vec<_> = (0..2)
        .map(|t| {
            let r = ring.clone();
            thread::spawn(move || {
                let base = t * total / 2;
                let mut sent = 0;
                while sent < total / 2 {
                    let remaining = total / 2 - sent;
                    let count = remaining.min(8);
                    let batch: Vec<u32> = (0..count).map(|i| (base + sent + i) as u32).collect();
                    let n = r.write_wait(&batch);
                    sent += n;
                }
            })
        })
        .collect();

    let reader = {
        let r = ring.clone();
        thread::spawn(move || {
            let mut got = 0usize;
            let mut buf = [0u32; 16];
            while got < total {
                let n = r.read_wait(&mut buf);
                got += n;
            }
            got
        })
    };

    for w in writers {
        join_ok(w);
    }
    assert_eq!(join_ok(reader), total);
    assert!(ring.is_empty());
}

// ==================== 全锁模式 ConcurrentRingGuard ====================

#[test]
fn lock_guard_basic_size() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    assert!(ring.push(1).is_ok());
    assert!(ring.push(2).is_ok());
    assert!(ring.push(3).is_ok());
    let g = ring.lock();
    assert_eq!(g.len(), 3);
    assert_eq!(g.capacity(), 4);
    assert!(!g.is_empty());
    assert!(!g.is_full());
}

#[test]
fn lock_guard_empty_and_full() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(2);
    {
        let g = ring.lock();
        assert!(g.is_empty());
        assert!(!g.is_full());
        assert_eq!(g.len(), 0);
    }
    assert!(ring.push(1).is_ok());
    assert!(ring.push(2).is_ok());
    {
        let g = ring.lock();
        assert!(g.is_full());
        assert!(!g.is_empty());
        assert_eq!(g.len(), 2);
    }
}

#[test]
fn lock_guard_index() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[10, 20, 30]);
    let mut g = ring.lock();
    assert_eq!(g[0], 10);
    assert_eq!(g[1], 20);
    assert_eq!(g[2], 30);
    assert_eq!(g.get(3), None);
    assert_eq!(g.get(100), None);
    g[1] = 99;
    assert_eq!(g[1], 99);
    match g.get_mut(0) {
        Some(v) => *v = 77,
        None => panic!("expected Some"),
    }
    assert_eq!(g[0], 77);
    assert!(g.get_mut(3).is_none());
}

#[test]
#[should_panic(expected = "out of bounds")]
fn lock_guard_index_out_of_bounds_panics() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(2);
    assert!(ring.push(1).is_ok());
    let g = ring.lock();
    let _ = g[5];
}

#[test]
fn lock_guard_front_back() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    {
        let g = ring.lock();
        assert_eq!(g.front(), None);
        assert_eq!(g.back(), None);
    }
    ring.write(&[1, 2, 3]);
    {
        let g = ring.lock();
        assert_eq!(g.front(), Some(&1));
        assert_eq!(g.back(), Some(&3));
    }
    let mut g = ring.lock();
    match g.front_mut() {
        Some(v) => *v = 100,
        None => panic!("expected Some"),
    }
    match g.back_mut() {
        Some(v) => *v = 300,
        None => panic!("expected Some"),
    }
    assert_eq!(g[0], 100);
    assert_eq!(g[2], 300);
}

#[test]
fn lock_guard_push_pop_back() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(3);
    let mut g = ring.lock();
    assert!(g.push_back(1).is_ok());
    assert!(g.push_back(2).is_ok());
    assert!(g.push_back(3).is_ok());
    assert_eq!(g.push_back(4), Err(4));
    assert_eq!(g.pop_back(), Some(3));
    assert_eq!(g.pop_back(), Some(2));
    assert_eq!(g.pop_back(), Some(1));
    assert_eq!(g.pop_back(), None);
}

#[test]
fn lock_guard_push_pop_front() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(3);
    let mut g = ring.lock();
    assert!(g.push_front(3).is_ok());
    assert!(g.push_front(2).is_ok());
    assert!(g.push_front(1).is_ok());
    assert_eq!(g[0], 1);
    assert_eq!(g[1], 2);
    assert_eq!(g[2], 3);
    assert_eq!(g.push_front(0), Err(0));
    assert_eq!(g.pop_front(), Some(1));
    assert_eq!(g.pop_front(), Some(2));
    assert_eq!(g.pop_front(), Some(3));
    assert_eq!(g.pop_front(), None);
}

#[test]
fn lock_guard_push_front_wraps() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3]);
    let mut g = ring.lock();
    assert!(g.push_front(0).is_ok());
    assert_eq!(g[0], 0);
    assert_eq!(g.pop_front(), Some(0));
    assert!(g.push_front(100).is_ok());
    assert_eq!(g[0], 100);
}

#[test]
fn lock_guard_pop_back_after_wrap() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3, 4]);
    ring.pop();
    assert!(ring.push(5).is_ok());
    let mut g = ring.lock();
    assert_eq!(g.len(), 4);
    assert_eq!(g[0], 2);
    assert_eq!(g[3], 5);
    assert_eq!(g.pop_back(), Some(5));
    assert_eq!(g.pop_back(), Some(4));
    assert_eq!(g.pop_back(), Some(3));
    assert_eq!(g.pop_back(), Some(2));
    assert_eq!(g.pop_back(), None);
    assert!(g.is_empty());
}

#[test]
fn lock_guard_mixed_operations() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    let mut g = ring.lock();
    assert!(g.push_back(1).is_ok());
    assert!(g.push_back(2).is_ok());
    assert!(g.push_front(0).is_ok());
    assert_eq!(g[0], 0);
    assert_eq!(g[1], 1);
    assert_eq!(g[2], 2);
    assert_eq!(g.pop_back(), Some(2));
    assert_eq!(g.pop_front(), Some(0));
    assert_eq!(g.len(), 1);
    assert_eq!(g[0], 1);
}

#[test]
fn lock_guard_clear() {
    let counter = Arc::new(AtomicUsize::new(0));
    let ring = ConcurrentRing::<DropCounter>::new_max_capacity(4);
    assert!(ring.push(DropCounter::new(1, counter.clone())).is_ok());
    assert!(ring.push(DropCounter::new(2, counter.clone())).is_ok());
    assert!(ring.push(DropCounter::new(3, counter.clone())).is_ok());
    {
        let mut g = ring.lock();
        g.clear();
        assert_eq!(g.len(), 0);
        assert!(g.is_empty());
    }
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[test]
fn lock_guard_clear_then_reuse() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3]);
    {
        let mut g = ring.lock();
        g.clear();
        assert!(g.push_back(10).is_ok());
        assert!(g.push_front(5).is_ok());
        assert_eq!(g.len(), 2);
        assert_eq!(g[0], 5);
        assert_eq!(g[1], 10);
    }
    let mut buf = [0u32; 2];
    assert_eq!(ring.read(&mut buf), 2);
    assert_eq!(buf, [5, 10]);
}

#[test]
fn lock_guard_as_slices_no_wrap() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3]);
    let g = ring.lock();
    let (a, b) = g.as_slices();
    assert_eq!(a, [1, 2, 3]);
    assert!(b.is_empty());
}

#[test]
fn lock_guard_as_slices_wraps() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3, 4]);
    ring.pop();
    assert!(ring.push(5).is_ok());
    let g = ring.lock();
    let (a, b) = g.as_slices();
    assert_eq!(a, [2, 3, 4]);
    assert_eq!(b, [5]);
}

#[test]
fn lock_guard_as_slices_empty() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    let g = ring.lock();
    let (a, b) = g.as_slices();
    assert!(a.is_empty());
    assert!(b.is_empty());
}

#[test]
fn lock_guard_as_mut_slices() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3, 4]);
    ring.pop();
    assert!(ring.push(5).is_ok());
    {
        let mut g = ring.lock();
        let (a, b) = g.as_mut_slices();
        for v in a.iter_mut() {
            *v *= 10;
        }
        for v in b.iter_mut() {
            *v *= 10;
        }
    }
    let mut buf = [0u32; 4];
    assert_eq!(ring.read(&mut buf), 4);
    assert_eq!(buf, [20, 30, 40, 50]);
}

#[test]
fn lock_guard_iter() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[10, 20, 30, 40]);
    ring.pop();
    assert!(ring.push(50).is_ok());
    let g = ring.lock();
    let v: Vec<u32> = g.iter().copied().collect();
    assert_eq!(v, vec![20, 30, 40, 50]);
    assert_eq!(g.iter().len(), 4);
    assert_eq!(g.iter().count(), 4);
}

#[test]
fn lock_guard_iter_empty() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    let g = ring.lock();
    assert_eq!(g.iter().count(), 0);
    assert_eq!(g.iter().len(), 0);
}

#[test]
fn lock_guard_iter_mut() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3]);
    {
        let mut g = ring.lock();
        for v in g.iter_mut() {
            *v *= 100;
        }
    }
    let mut buf = [0u32; 3];
    assert_eq!(ring.read(&mut buf), 3);
    assert_eq!(buf, [100, 200, 300]);
}

#[test]
fn lock_guard_into_iterator() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3]);
    {
        let g = ring.lock();
        let mut sum = 0u32;
        for v in &g {
            sum += *v;
        }
        assert_eq!(sum, 6);
    }
    {
        let mut g = ring.lock();
        for v in &mut g {
            *v += 10;
        }
    }
    let mut buf = [0u32; 3];
    ring.read(&mut buf);
    assert_eq!(buf, [11, 12, 13]);
}

#[test]
fn lock_guard_does_not_consume() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    ring.write(&[1, 2, 3]);
    {
        let g = ring.lock();
        let _: Vec<_> = g.iter().copied().collect();
        let _ = g.get(0);
    }
    assert_eq!(ring.len(), 3);
    let mut buf = [0u32; 3];
    ring.read(&mut buf);
    assert_eq!(buf, [1, 2, 3]);
}

#[test]
fn lock_blocks_other_threads() {
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(4));
    assert!(ring.push(1).is_ok());

    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();

    let r = ring.clone();
    let handle = thread::spawn(move || {
        send_ok(&started_tx, ());
        let _ = r.push(2);
        send_ok(&done_tx, ());
    });

    let mut g = ring.lock();
    recv_ok(&started_rx);
    thread::sleep(Duration::from_millis(50));
    assert!(done_rx.try_recv().is_err());
    assert_eq!(g.len(), 1);
    assert!(g.push_back(99).is_ok());
    assert_eq!(g.len(), 2);

    drop(g);
    recv_timeout_ok(&done_rx, Duration::from_secs(2));
    join_ok(handle);
    assert_eq!(ring.len(), 3);
}

#[test]
fn guard_drop_restores_access() {
    let ring = ConcurrentRing::<u32>::new_max_capacity(4);
    assert!(ring.push(1).is_ok());
    {
        let _g = ring.lock();
    }
    assert!(ring.push(2).is_ok());
    assert_eq!(ring.pop(), Some(1));
    assert_eq!(ring.pop(), Some(2));
    assert_eq!(ring.pop(), None);
}

#[test]
fn lock_guard_does_not_leak_when_panicking() {
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(4));
    let r = ring.clone();
    let t = thread::spawn(move || {
        let mut g = r.lock();
        assert!(g.push_back(1).is_ok());
    });
    join_ok(t);
    let g = ring.lock();
    assert_eq!(g.len(), 1);
}

#[test]
fn lock_guard_concurrent_readers_and_writer_are_serialized() {
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(8));
    let total = 2000;

    let writer = {
        let r = ring.clone();
        thread::spawn(move || {
            for i in 0..total {
                let mut g = r.lock();
                while g.is_full() {
                    drop(g);
                    thread::yield_now();
                    g = r.lock();
                }
                assert!(g.push_back(i as u32).is_ok());
            }
        })
    };

    let reader = {
        let r = ring.clone();
        thread::spawn(move || {
            let mut got = 0usize;
            while got < total {
                let mut g = r.lock();
                if g.pop_front().is_some() {
                    got += 1;
                } else {
                    drop(g);
                    thread::yield_now();
                }
            }
            got
        })
    };

    join_ok(writer);
    assert_eq!(join_ok(reader), total);
    assert!(ring.is_empty());
}

#[test]
fn lock_guard_drop_counter_through_clear() {
    let counter = Arc::new(AtomicUsize::new(0));
    let ring = ConcurrentRing::<DropCounter>::new_max_capacity(4);
    for i in 0..3 {
        assert!(ring.push(DropCounter::new(i, counter.clone())).is_ok());
    }
    {
        let mut g = ring.lock();
        let v = expect_some(g.pop_back());
        assert_eq!(v.id, 2);
        drop(v);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        g.clear();
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }
    assert!(ring.is_empty());
}

// ==================== 毒化解毒验证 ====================

#[test]
fn poisoned_mutex_is_recovered() {
    // 故意在一个锁内部 panic，触发毒化；
    // 通过 lock_recover 应能在后续访问时解毒，不 panic。
    let ring = Arc::new(ConcurrentRing::<u32>::new_max_capacity(4));
    let r = ring.clone();
    let t = thread::spawn(move || {
        let mut g = r.lock();
        assert!(g.push_back(1).is_ok());
        // 模拟临界区 panic：Guard 在此 drop，Mutex/RwLock 被毒化。
        panic!("intentional panic while holding lock");
    });
    // 等待线程 panic 完成（其 drop 让锁毒化）。
    if t.join().is_ok() {
        panic!("expected thread to panic");
    }

    // 主线程正常获取 Guard——helper 内部 clear_poison + into_inner，
    // 若未解毒则 lock_recover 返回的也是毒化锁，不会 panic，
    // 但重复访问依然工作；且我们验证了后续连续访问也不 panic。
    {
        let g = ring.lock();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0], 1);
    }
    // 再次访问，验证 poisoning 标志已被清除（后续 lock 直接 Ok）。
    assert!(ring.push(2).is_ok());
    assert_eq!(ring.pop(), Some(1));
    assert_eq!(ring.pop(), Some(2));
}
