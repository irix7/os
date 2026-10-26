// SPDX-License-Identifier: GPL-3.0-only
//! Hidden calibration command: proves the timing harness can see a memcpy.

use std::hint::black_box;
use std::time::Instant;

use crate::stats::{header, row, us};

fn batch(iterations: usize, mut work: impl FnMut(usize) -> u64) -> u64 {
    let start = Instant::now();
    let mut acc = 0u64;
    for index in 0..iterations {
        acc = acc.wrapping_add(work(index));
    }
    let elapsed = start.elapsed().as_nanos() as u64;
    black_box(acc);
    (elapsed / iterations as u64).max(1)
}

/// Run the calibration.
pub fn run() {
    println!("\n## Calibration: raw copy cost in the same harness\n");
    header(&["bytes", "copy us", "GB/s"]);
    for size in [16usize, 1024, 8192, 65_212] {
        let bodies: Vec<Vec<u8>> = (0..64u8).map(|fill| vec![fill; size]).collect();
        let mut out = vec![0u8; size];
        let iterations = (200_000_000 / (size + 360)).clamp(2_000, 200_000);
        let ns = batch(iterations, |index| {
            let slot = index & 63;
            out.copy_from_slice(&bodies[slot]);
            u64::from(out[(index * 7) % size])
        });
        let gb_s = size as f64 / ns as f64;
        row(&[
            size.to_string(),
            format!("{:.1}", us(ns)),
            format!("{gb_s:.1}"),
        ]);
    }
}
