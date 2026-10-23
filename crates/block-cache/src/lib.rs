// SPDX-License-Identifier: GPL-3.0-only
//! A small read-only LRU block cache.
//!
//! The cache sits above a byte-addressed device.  It rounds reads to the
//! configured block size, keeps `N` blocks in least-recently-used order, and
//! copies the requested range out of the cached block.  It does not write
//! back and has no `unsafe`: the block cell will own the cache policy, while
//! the EROFS reader and a future SCSI driver only provide `ReadAt`.
//!
//! ```
//! use block_cache::{Lru, ReadAt};
//!
//! struct Memory(&'static [u8]);
//! impl ReadAt for Memory {
//!     type Error = ();
//!     fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), ()> {
//!         let start = offset as usize;
//!         buf.copy_from_slice(&self.0[start..start + buf.len()]);
//!         Ok(())
//!     }
//! }
//!
//! let mut cache = Lru::<_, 4>::new(Memory(&[0; 16]), 4).unwrap();
//! let mut byte = [0u8; 1];
//! cache.read_at(3, &mut byte).unwrap();
//! ```

#![no_std]
#![deny(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::cmp::min;

/// A byte-addressed read-only device.
pub trait ReadAt {
    /// The device error type.
    type Error;

    /// Read exactly `buf.len()` bytes starting at `offset`.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error>;
}

/// A cache configuration or device error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheError<E> {
    /// The cache block size or capacity was zero.
    InvalidBlockSize,
    /// An allocation for a cache block failed.
    OutOfMemory,
    /// A block address overflowed a 64-bit offset.
    Overflow,
    /// The underlying device failed.
    Device(E),
}

/// One cached block.
struct Slot {
    block: u64,
    last_used: u64,
    valid: bool,
    data: Vec<u8>,
}

/// An `N`-block LRU cache over a [`ReadAt`] device.
pub struct Lru<D, const N: usize> {
    device: D,
    block_size: usize,
    slots: Vec<Slot>,
    tick: u64,
    hits: u64,
    misses: u64,
}

impl<D: ReadAt, const N: usize> Lru<D, N> {
    /// Create a cache with `block_size` bytes per slot and `N` slots.
    pub fn new(device: D, block_size: usize) -> Result<Self, CacheError<D::Error>> {
        if block_size == 0 || N == 0 {
            return Err(CacheError::InvalidBlockSize);
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(N)
            .map_err(|_| CacheError::OutOfMemory)?;
        for _ in 0..N {
            let mut data = Vec::new();
            data.try_reserve_exact(block_size)
                .map_err(|_| CacheError::OutOfMemory)?;
            data.resize(block_size, 0);
            slots.push(Slot {
                block: 0,
                last_used: 0,
                valid: false,
                data,
            });
        }
        Ok(Self {
            device,
            block_size,
            slots,
            tick: 0,
            hits: 0,
            misses: 0,
        })
    }

    /// The configured block size in bytes.
    #[must_use]
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// The number of cache slots.
    #[must_use]
    pub fn capacity(&self) -> usize {
        N
    }

    /// The number of cache hits since creation.
    #[must_use]
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// The number of cache misses since creation.
    #[must_use]
    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// Access the underlying device.
    #[must_use]
    pub fn device(&self) -> &D {
        &self.device
    }

    /// Mutably access the underlying device.
    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }

    /// Consume the cache and return the device.
    #[must_use]
    pub fn into_device(self) -> D {
        self.device
    }

    /// Invalidate every slot; the statistics are kept.
    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            slot.valid = false;
        }
    }

    /// Read a byte range through the cache.
    pub fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), CacheError<D::Error>> {
        let block_size = self.block_size as u64;
        let mut position = offset;
        let mut done = 0usize;

        while done < buf.len() {
            let block = position / block_size;
            let within = (position % block_size) as usize;
            let start = block.checked_mul(block_size).ok_or(CacheError::Overflow)?;
            let index = self.load(block, start)?;

            let count = min(self.block_size - within, buf.len() - done);
            buf[done..done + count]
                .copy_from_slice(&self.slots[index].data[within..within + count]);
            position += count as u64;
            done += count;
        }
        Ok(())
    }

    fn load(&mut self, block: u64, start: u64) -> Result<usize, CacheError<D::Error>> {
        if let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.valid && slot.block == block)
        {
            self.hits += 1;
            self.tick = self.tick.wrapping_add(1);
            self.slots[index].last_used = self.tick;
            return Ok(index);
        }

        self.misses += 1;
        let index = self
            .slots
            .iter()
            .position(|slot| !slot.valid)
            .unwrap_or_else(|| {
                self.slots
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, slot)| slot.last_used)
                    .map(|(index, _)| index)
                    .unwrap_or(0)
            });

        self.device
            .read_at(start, &mut self.slots[index].data)
            .map_err(CacheError::Device)?;
        self.tick = self.tick.wrapping_add(1);
        self.slots[index].block = block;
        self.slots[index].last_used = self.tick;
        self.slots[index].valid = true;
        Ok(index)
    }
}

impl ReadAt for &[u8] {
    type Error = ();

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let start = usize::try_from(offset).map_err(|_| ())?;
        let end = start.checked_add(buf.len()).ok_or(())?;
        let source = self.get(start..end).ok_or(())?;
        buf.copy_from_slice(source);
        Ok(())
    }
}
