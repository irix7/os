// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the read-ahead window arithmetic.
//!
//! `src/readahead.rs` is pure logic with no kernel dependencies, so the
//! window the HTTP and HTTPS block device requests can be checked on the host
//! without the emulator.

#![no_std]
// The helper is compiled on its own here, so its public entry points have no
// callers in this test binary.
#![allow(dead_code)]

#[path = "../src/readahead.rs"]
mod readahead;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;

#[test]
fn the_window_is_256_kib_and_the_probe_is_4_kib() {
    assert_eq!(readahead::CACHE_SIZE, 256 * 1024);
    assert_eq!(readahead::PROBE_SIZE, 4096);
    assert!(readahead::CACHE_SIZE.is_power_of_two());
}

#[test]
fn a_window_covers_256_kib_from_an_aligned_start() {
    let window = readahead::window(0, Some(MIB)).unwrap();
    assert_eq!(window.start, 0);
    assert_eq!(window.len, 256 * 1024);
    assert_eq!(window.end(), 256 * 1024 - 1);

    // A position inside a window fetches that whole window, not the next one.
    let inside = readahead::window(64 * KIB + 7, Some(MIB)).unwrap();
    assert_eq!(inside, window);

    // The second and third windows start at their 256 KiB boundaries.
    let second = readahead::window(300 * KIB, Some(MIB)).unwrap();
    assert_eq!(second.start, 256 * KIB);
    assert_eq!(second.len, 256 * 1024);
    assert_eq!(second.end(), 512 * KIB - 1);

    let third = readahead::window(512 * KIB, Some(MIB)).unwrap();
    assert_eq!(third.start, 512 * KIB);
}

#[test]
fn the_final_window_is_clamped_to_the_image_end() {
    // A 600 KiB image ends inside the third window.
    let size = 600 * KIB;
    let last = readahead::window(size - 1, Some(size)).unwrap();
    assert_eq!(last.start, 512 * KIB);
    assert_eq!(last.len, 88 * 1024);
    assert_eq!(last.end(), size - 1);
}

#[test]
fn a_position_past_the_image_end_has_no_window() {
    let size = 600 * KIB;
    assert_eq!(readahead::window(size, Some(size)), None);
    assert_eq!(readahead::window(size + 1, Some(size)), None);
    assert_eq!(readahead::window(0, Some(0)), None);
}

#[test]
fn the_probe_fetches_only_one_block_before_the_size_is_known() {
    let probe = readahead::window(1024, None).unwrap();
    assert_eq!(probe.start, 0);
    assert_eq!(probe.len, 4096);
    assert_eq!(probe.end(), 4095);
}
