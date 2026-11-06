// SPDX-License-Identifier: GPL-3.0-only
//! The PowerPC G3 machine's service platform.
//!
//! The service table, its guards and its refusals are shared in `kernel-core`.
//! This machine has console output, the decrementer clock and fixed
//! information blocks; everything else keeps the shared refusal.  There is no
//! allocator, network stack, block service or module arena in this slice, and
//! the module now says so with the same error every machine uses.

use kernel_core::service::Platform;

use crate::{console, timer};

/// The PowerPC platform instance.
pub struct PpcPlatform;

static PLATFORM: PpcPlatform = PpcPlatform;

/// Install the platform.  Called once during boot, before the hello demo.
pub fn install() {
    kernel_core::service::install(&PLATFORM);
}

impl Platform for PpcPlatform {
    fn write(&self, bytes: &[u8]) {
        console::write(bytes);
    }

    /// The decrementer runs at 100 Hz, so one tick is 10 ms.
    fn clock_ms(&self) -> u64 {
        u64::from(timer::ticks()) * 10
    }

    /// Fixed information blocks.  Query 0 is the machine description the
    /// demo writes through `write`; the others name the absent network and
    /// reserved memory.
    fn info(&self, which: u32, out: &mut [u8]) -> isize {
        let text: &[u8] = match which {
            0 => b"ppc: g3beige\n",
            1 => b"net: none\n",
            2 => b"mem: reserved\n",
            3 => b"time: monotonic\n",
            _ => return -1,
        };
        let count = text.len().min(out.len());
        out[..count].copy_from_slice(&text[..count]);
        count as isize
    }
}
