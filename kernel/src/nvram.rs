// SPDX-License-Identifier: GPL-3.0-only
//! Dallas DS1386 NVRAM environment.
//!
//! The PROM environment is stored as NUL-separated `name=value` strings.  The
//! 8 KiB NVRAM is one live byte per 32-bit word in the low lane; the RTC
//! registers occupy 0x00–0x0F and the environment starts at 0x10.  The MAC
//! address lives at 0x4E8 (see `net::read_nvram_mac`).

const NVRAM_BASE: usize = 0xBFBE_0000;
const ENV_START: usize = 0x10;
const ENV_MAX: usize = 0x3F0;

fn byte(index: usize) -> u8 {
    // SAFETY: the DS1386 window is mapped and the index is bounded by callers.
    unsafe { (core::ptr::read_volatile((NVRAM_BASE + index * 4) as *const u32) & 0xff) as u8 }
}

/// Copy the value of `name` from the PROM environment into `out`.
///
/// Returns the value length, or `None` if the name is absent.
pub fn get(name: &str, out: &mut [u8]) -> Option<usize> {
    let end = ENV_START + ENV_MAX;
    let mut p = ENV_START;
    while p < end {
        let start = p;
        while p < end && byte(p) != 0 {
            p += 1;
        }
        if p == start {
            // An empty string terminates the environment.
            break;
        }
        let mut equals = None;
        for i in start..p {
            if byte(i) == b'=' {
                equals = Some(i);
                break;
            }
        }
        if let Some(equals) = equals {
            let name_len = equals - start;
            let matches = name_len == name.len()
                && name
                    .as_bytes()
                    .iter()
                    .enumerate()
                    .all(|(i, &b)| byte(start + i) == b);
            if matches {
                let value_len = (p - equals - 1).min(out.len());
                for (i, slot) in out.iter_mut().take(value_len).enumerate() {
                    *slot = byte(equals + 1 + i);
                }
                return Some(value_len);
            }
        }
        p += 1;
    }
    None
}
