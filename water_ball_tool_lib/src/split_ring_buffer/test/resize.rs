//! 缓冲区调整：扩容复制、缩小只改容量、并发读端继续运行。

use crate::split_ring_buffer::*;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

#[test]
fn grow_preserves_data_and_capacity() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    assert_eq!(w.capacity(), 4);
    assert_eq!(w.write(&[1, 2, 3]), 3);

    w.resize(8);
    assert_eq!(w.capacity(), 8);
    assert_eq!(r.len(), 3);

    let mut buf = [0u8; 3];
    assert_eq!(r.read(&mut buf), 3);
    assert_eq!(buf, [1, 2, 3]);

    assert_eq!(w.write(&[4, 5, 6, 7, 8, 9, 10, 11]), 8);
}

#[test]
fn grow_preserves_wrapped_data() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    assert_eq!(w.write(&[1, 2, 3, 4]), 4);

    let mut tmp = [0u8; 2];
    assert_eq!(r.read(&mut tmp), 2);
    assert_eq!(tmp, [1, 2]);
    assert_eq!(w.write(&[5, 6]), 2);

    w.resize(8);

    let mut buf = [0u8; 4];
    assert_eq!(r.read(&mut buf), 4);
    assert_eq!(buf, [3, 4, 5, 6]);
}

#[test]
fn shrink_only_updates_capacity() {
    let (r, w) = locked_ring_buffer::<u8>(8);
    assert_eq!(w.write(&[1, 2, 3]), 3);

    w.resize(4);
    assert_eq!(w.capacity(), 4);
    assert_eq!(r.len(), 3);
    assert_eq!(w.write(&[4, 5, 6]), 1);
    assert_eq!(w.write(&[7]), 0);

    let mut buf = [0u8; 4];
    assert_eq!(r.read(&mut buf), 4);
    assert_eq!(buf, [1, 2, 3, 4]);

    // 仍在物理分配（8）内扩大：不重新分配
    // Grow within the physical allocation (8): no reallocation.
    w.resize(6);
    assert_eq!(w.capacity(), 6);
    assert_eq!(w.write(&[1, 2, 3, 4, 5, 6]), 6);
}

#[test]
fn shrink_below_len_keeps_data_and_stalls_writers() {
    let (r, w) = locked_ring_buffer::<u8>(8);
    assert_eq!(w.write(&[1, 2, 3, 4, 5]), 5);

    w.resize(2);
    assert_eq!(w.capacity(), 2);
    assert_eq!(r.len(), 5);
    assert!(w.is_full());
    assert_eq!(w.write(&[9]), 0);

    let mut buf = [0u8; 5];
    assert_eq!(r.read(&mut buf), 5);
    assert_eq!(buf, [1, 2, 3, 4, 5]);

    assert_eq!(w.write(&[6, 7]), 2);
    assert_eq!(w.write(&[8]), 0);
}

#[test]
fn grow_after_shrink_reallocates() {
    let (r, w) = locked_ring_buffer::<u8>(8);
    w.resize(4);
    assert_eq!(w.write(&[1, 2, 3, 4]), 4);

    w.resize(12);
    assert_eq!(w.capacity(), 12);
    assert_eq!(r.len(), 4);

    let mut buf = [0u8; 4];
    assert_eq!(r.read(&mut buf), 4);
    assert_eq!(buf, [1, 2, 3, 4]);
}

#[test]
fn unlocked_writer_resize() {
    let (mut r, mut w) = unlocked_ring_buffer::<u8>(4);
    assert_eq!(w.write(&[1, 2, 3]), 3);

    w.resize(8);
    assert_eq!(w.capacity(), 8);

    let mut buf = [0u8; 3];
    assert_eq!(r.read(&mut buf), 3);
    assert_eq!(buf, [1, 2, 3]);
}

#[test]
fn readers_continue_during_grow() {
    let expected = 10_000u32;
    let (r, w) = locked_ring_buffer::<u32>(8);
    let done = Arc::new(AtomicBool::new(false));

    let reader = {
        let r = r.clone();
        let done = done.clone();
        thread::spawn(move || {
            let mut next = 0u32;
            while next < expected {
                let value = r.pop_wait().expect("pop_wait");
                assert_eq!(value, next);
                next += 1;
            }
            done.store(true, Ordering::Release);
        })
    };

    let resizer = {
        let w = w.clone();
        let done = done.clone();
        thread::spawn(move || {
            let sizes = [8usize, 16, 64, 32];
            let mut i = 0usize;
            while !done.load(Ordering::Acquire) {
                w.resize(sizes[i % sizes.len()]);
                i += 1;
                thread::yield_now();
            }
        })
    };

    for value in 0..expected {
        w.push_wait(value).expect("push_wait");
    }

    reader.join().expect("reader thread");
    done.store(true, Ordering::Release);
    resizer.join().expect("resizer thread");
}
