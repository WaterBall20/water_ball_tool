//! 基础功能、模式组合、len/empty/full、回绕、零长度。

use crate::split_ring_buffer::*;

#[test]
fn basic_pop_push() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    assert!(r.pop().is_none());
    assert!(w.push(1).is_ok());
    assert!(w.push(2).is_ok());
    assert_eq!(r.pop(), Some(1));
    assert_eq!(r.pop(), Some(2));
    assert!(r.pop().is_none());
}

#[test]
fn push_full_returns_full_error() {
    let (_, w) = locked_ring_buffer::<u8>(2);
    assert!(w.push(1).is_ok());
    assert!(w.push(2).is_ok());
    match w.push(3) {
        Err(e) => match e.kind {
            Kind::Full(3) => {}
            other => panic!("expected Full(3), got {:?}", other),
        },
        Ok(()) => panic!("expected Full error"),
    }
}

#[test]
fn basic_read_write() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    assert_eq!(r.read(&mut [0u8; 2]), 0);
    assert_eq!(w.write(&[1, 2, 3]), 3);
    let mut buf = [0u8; 2];
    assert_eq!(r.read(&mut buf), 2);
    assert_eq!(buf, [1, 2]);
    let mut buf = [0u8; 2];
    assert_eq!(r.read(&mut buf), 1);
    assert_eq!(buf[0], 3);
}

#[test]
fn len_empty_full() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    assert!(r.is_empty());
    assert!(w.is_empty());
    assert!(!r.is_full());
    assert!(!w.is_full());
    w.write(&[1, 2, 3]);
    assert_eq!(r.len(), 3);
    assert_eq!(w.len(), 3);
    assert!(!r.is_empty());
    assert!(!r.is_full());
    w.write(&[4]);
    assert_eq!(r.len(), 4);
    assert!(r.is_full());
    assert!(w.is_full());
}

#[test]
fn wraparound() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.write(&[1, 2, 3]);
    let mut buf = [0u8; 3];
    r.read(&mut buf);
    assert_eq!(buf, [1, 2, 3]);
    let data = [10, 20, 30, 40];
    assert_eq!(w.write(&data), 4);
    let mut buf = [0u8; 4];
    assert_eq!(r.read(&mut buf), 4);
    assert_eq!(buf, data);
}

#[test]
fn zero_length_operations() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    let mut empty_r: [u8; 0] = [];
    let empty_w: [u8; 0] = [];
    assert_eq!(r.read(&mut empty_r), 0);
    assert_eq!(w.write(&empty_w), 0);
    assert_eq!(r.read_wait(&mut empty_r).unwrap(), 0);
    assert_eq!(w.write_wait(&empty_w).unwrap(), 0);
    assert!(r.read_exact(&mut empty_r).is_ok());
    assert!(w.write_all(&empty_w).is_ok());
}

#[test]
fn write_all_fits_in_capacity() {
    let (r, w) = locked_ring_buffer::<u8>(8);
    w.write_all(&[1, 2, 3, 4, 5]).unwrap();
    assert_eq!(r.len(), 5);
    let mut buf = [0u8; 5];
    r.read_exact(&mut buf).unwrap();
    assert_eq!(buf, [1, 2, 3, 4, 5]);
}

#[test]
fn read_wait_returns_partial() {
    let (r, w) = locked_ring_buffer::<u8>(8);
    w.write(&[1, 2, 3]);
    let mut buf = [0u8; 8];
    let n = r.read_wait(&mut buf).unwrap();
    assert_eq!(n, 3);
    assert_eq!(&buf[..3], &[1, 2, 3]);
}

#[test]
fn write_wait_returns_partial() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.write(&[1, 2]);
    let n = w.write_wait(&[3, 4, 5, 6]).unwrap();
    assert_eq!(n, 2);
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf).unwrap();
    assert_eq!(buf, [1, 2, 3, 4]);
}

// ---------- 四种模式组合 ----------

#[test]
fn mode_locked_locked() {
    let (r, w) = locked_ring_buffer::<u8>(4);
    w.push(1).unwrap();
    assert_eq!(r.pop(), Some(1));
}

#[test]
fn mode_unlocked_unlocked() {
    let (mut r, mut w) = unlocked_ring_buffer::<u8>(4);
    w.push(1).unwrap();
    assert_eq!(r.pop(), Some(1));
    w.push(2).unwrap();
    let mut buf = [0u8; 2];
    assert_eq!(r.read(&mut buf), 1);
    assert_eq!(buf[0], 2);
}

#[test]
fn mode_locked_read_unlocked_write() {
    let (r, mut w) = locked_read_unlocked_write_ring_buffer::<u8>(4);
    w.push(1).unwrap();
    assert_eq!(r.pop(), Some(1));
    let mut buf = [0u8; 2];
    w.write(&[2, 3]);
    assert_eq!(r.read(&mut buf), 2);
}

#[test]
fn mode_unlocked_read_locked_write() {
    let (mut r, w) = unlocked_read_locked_write_ring_buffer::<u8>(4);
    w.push(1).unwrap();
    assert_eq!(r.pop(), Some(1));
    let mut buf = [0u8; 2];
    w.write(&[2, 3]);
    assert_eq!(r.read(&mut buf), 2);
}

// ---------- 枚举模式构造 ----------

#[test]
fn enum_mode_locked_locked() {
    let (reader, writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Locked);
    assert!(reader.is_locked());
    assert!(writer.is_locked());
    let r = match reader.into_locked() {
        Ok(x) => x,
        Err(_) => panic!("expected locked reader"),
    };
    let w = match writer.into_locked() {
        Ok(x) => x,
        Err(_) => panic!("expected locked writer"),
    };
    w.push(1).unwrap();
    assert_eq!(r.pop(), Some(1));
}

#[test]
fn enum_mode_unlocked_unlocked() {
    let (reader, writer) = ring_buffer::<u8>(4, ReadMode::Unlocked, WriteMode::Unlocked);
    assert!(reader.is_unlocked());
    assert!(writer.is_unlocked());
    let mut r = match reader.into_unlocked() {
        Ok(x) => x,
        Err(_) => panic!("expected unlocked reader"),
    };
    let mut w = match writer.into_unlocked() {
        Ok(x) => x,
        Err(_) => panic!("expected unlocked writer"),
    };
    w.push(1).unwrap();
    assert_eq!(r.pop(), Some(1));
}

#[test]
fn enum_mode_mixed() {
    let (reader, writer) = ring_buffer::<u8>(4, ReadMode::Locked, WriteMode::Unlocked);
    assert!(reader.is_locked());
    assert!(writer.is_unlocked());
}