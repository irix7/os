// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the LRU block cache.

use block_cache::{CacheError, Lru, ReadAt};

struct Counting {
    data: Vec<u8>,
    reads: u64,
}

impl Counting {
    fn new(size: usize) -> Self {
        Self {
            data: (0..size).map(|index| (index * 3) as u8).collect(),
            reads: 0,
        }
    }
}

impl ReadAt for Counting {
    type Error = ();

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), ()> {
        let start = usize::try_from(offset).map_err(|_| ())?;
        let end = start.checked_add(buf.len()).ok_or(())?;
        let source = self.data.get(start..end).ok_or(())?;
        buf.copy_from_slice(source);
        self.reads += 1;
        Ok(())
    }
}

fn pattern(offset: usize, len: usize) -> Vec<u8> {
    (offset..offset + len)
        .map(|index| (index * 3) as u8)
        .collect()
}

#[test]
fn hits_keep_the_lru_order() {
    let mut cache = Lru::<_, 2>::new(Counting::new(16), 4).expect("cache");
    let mut out = [0u8; 4];

    cache.read_at(0, &mut out).unwrap();
    assert_eq!(out, pattern(0, 4).as_slice());
    cache.read_at(4, &mut out).unwrap();
    assert_eq!(out, pattern(4, 4).as_slice());

    // Hit block 0; block 4 is now the least recently used.
    cache.read_at(0, &mut out).unwrap();
    assert_eq!(out, pattern(0, 4).as_slice());
    assert_eq!(cache.hits(), 1);

    // Block 8 evicts block 4; block 0 stays.
    cache.read_at(8, &mut out).unwrap();
    assert_eq!(out, pattern(8, 4).as_slice());
    assert_eq!(cache.misses(), 3);
    assert_eq!(cache.device().reads, 3);

    cache.read_at(0, &mut out).unwrap();
    assert_eq!(cache.hits(), 2);
    cache.read_at(4, &mut out).unwrap();
    assert_eq!(out, pattern(4, 4).as_slice());
    assert_eq!(cache.misses(), 4);
    assert_eq!(cache.device().reads, 4);
}

#[test]
fn reads_cross_block_boundaries() {
    let mut cache = Lru::<_, 2>::new(Counting::new(32), 8).expect("cache");
    let mut out = [0u8; 6];
    cache.read_at(6, &mut out).unwrap();
    assert_eq!(out, pattern(6, 6).as_slice());
    assert_eq!(cache.misses(), 2);
    assert_eq!(cache.device().reads, 2);

    let mut tail = [0u8; 4];
    cache.read_at(20, &mut tail).unwrap();
    assert_eq!(tail, pattern(20, 4).as_slice());
    cache.read_at(28, &mut tail).unwrap();
    assert_eq!(tail, pattern(28, 4).as_slice());
    assert_eq!(cache.misses(), 4);
}

#[test]
fn zero_length_reads_do_not_touch_the_device() {
    let mut cache = Lru::<_, 1>::new(Counting::new(8), 4).expect("cache");
    cache.read_at(8, &mut []).unwrap();
    cache.read_at(1000, &mut []).unwrap();
    assert_eq!(cache.hits(), 0);
    assert_eq!(cache.misses(), 0);
    assert_eq!(cache.device().reads, 0);
}

#[test]
fn invalid_configuration_is_rejected() {
    assert!(matches!(
        Lru::<_, 1>::new(Counting::new(8), 0),
        Err(CacheError::InvalidBlockSize)
    ));
    assert!(matches!(
        Lru::<_, 0>::new(Counting::new(8), 4),
        Err(CacheError::InvalidBlockSize)
    ));
}

#[test]
fn device_errors_propagate() {
    struct Failing;
    impl ReadAt for Failing {
        type Error = &'static str;
        fn read_at(&mut self, _offset: u64, _buf: &mut [u8]) -> Result<(), Self::Error> {
            Err("device failed")
        }
    }

    let mut cache = Lru::<_, 1>::new(Failing, 4).expect("cache");
    let mut out = [0u8; 1];
    assert_eq!(
        cache.read_at(0, &mut out),
        Err(CacheError::Device("device failed"))
    );
    assert_eq!(cache.misses(), 1);
}

#[test]
fn clear_forces_a_reload() {
    let mut cache = Lru::<_, 2>::new(Counting::new(16), 4).expect("cache");
    let mut out = [0u8; 4];
    cache.read_at(0, &mut out).unwrap();
    assert_eq!(cache.device().reads, 1);
    cache.clear();
    cache.read_at(0, &mut out).unwrap();
    assert_eq!(cache.device().reads, 2);
    assert_eq!(cache.misses(), 2);
}

#[test]
fn slice_backend_is_available() {
    let data = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let mut cache = Lru::<_, 1>::new(&data[..], 4).expect("cache");
    let mut out = [0u8; 4];
    cache.read_at(4, &mut out).unwrap();
    assert_eq!(out, [5, 6, 7, 8]);
}
