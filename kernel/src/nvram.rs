// SPDX-License-Identifier: GPL-3.0-only
//! Dallas DS1386 NVRAM environment.
//!
//! The PROM environment is stored as NUL-separated `name=value` strings.  The
//! 8 KiB NVRAM is one live byte per 32-bit word in the low lane; the RTC
//! registers occupy 0x00–0x0F and the environment starts at 0x10.  The MAC
//! address lives at sparse byte index 314 (physical `0x1FBE_04E8`).
//!
//! A dead DS1386 battery makes the SRAM read as `0xFF`.  `init` validates the
//! environment and MAC at boot and writes a minimal default environment when
//! the chip is blank or corrupt.  This is an actual battery-backed chip on an
//! Indy, not a release distribution channel: boot-time module URLs live in
//! `boot/config.conf` in the repository.

const NVRAM_BASE: usize = 0xBFBE_0000;
const ENV_START: usize = 0x10;
const ENV_MAX: usize = 0x3F0;
const MAC_INDEX: usize = 314;
const DEFAULT_MAC: [u8; 6] = [0x02, 0x00, 0x69, 0x12, 0x34, 0x56];

fn byte(index: usize) -> u8 {
    // SAFETY: the DS1386 window is mapped and the index is bounded by callers.
    unsafe { (core::ptr::read_volatile((NVRAM_BASE + index * 4) as *const u32) & 0xff) as u8 }
}

fn write_byte(index: usize, value: u8) {
    // SAFETY: the DS1386 window is mapped; the HPC3 PBUS BBRAM path writes the
    // low byte of the 32-bit word and ignores the upper lanes.
    unsafe {
        core::ptr::write_volatile((NVRAM_BASE + index * 4) as *mut u32, value as u32);
    }
}

fn read_environment(out: &mut [u8; ENV_MAX]) {
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = byte(ENV_START + index);
    }
}

fn read_mac(out: &mut [u8; 6]) {
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = byte(MAC_INDEX + index);
    }
}

/// Validate the NVRAM environment at boot and repair a blank/corrupt chip.
pub fn init() {
    let mut env = [0u8; ENV_MAX];
    read_environment(&mut env);
    if !environment_valid(&env) {
        let message = b"console=serial0";
        for (index, &value) in message.iter().enumerate() {
            write_byte(ENV_START + index, value);
        }
        write_byte(ENV_START + message.len(), 0);
        for index in (ENV_START + 1 + message.len())..(ENV_START + ENV_MAX) {
            write_byte(index, 0);
        }
        read_environment(&mut env);
        if environment_valid(&env) {
            crate::console_println("nvram: environment repaired");
        } else {
            crate::console_println("nvram: environment repair failed");
        }
    }

    let mut mac = [0u8; 6];
    read_mac(&mut mac);
    if !mac_valid(&mac) {
        for (index, &value) in DEFAULT_MAC.iter().enumerate() {
            write_byte(MAC_INDEX + index, value);
        }
        read_mac(&mut mac);
        if mac_valid(&mac) {
            crate::console_println("nvram: MAC restored");
        } else {
            crate::console_println("nvram: MAC repair failed");
        }
    }
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

/// Structural validation used both at boot and by host tests.
///
/// A battery-loss image is usually all `0xFF`; an erased image may be all zero.
/// Both are treated as blank, as is any string without a `name=value` shape or
/// with non-printable bytes.  A leading terminator is accepted because an
/// otherwise-valid chip may deliberately carry no variables.
fn environment_valid(env: &[u8]) -> bool {
    if env.is_empty() {
        return false;
    }
    if env.iter().all(|byte| *byte == 0) {
        return false;
    }
    if env[0] == 0 {
        return true;
    }
    let mut position = 0usize;
    let mut saw_assignment = false;
    while position < env.len() {
        let start = position;
        while position < env.len() && env[position] != 0 {
            let byte = env[position];
            if !(0x20..=0x7e).contains(&byte) {
                return false;
            }
            position += 1;
        }
        if position == env.len() {
            return false;
        }
        if position > start {
            let word = &env[start..position];
            let Some(equals) = word.iter().position(|&byte| byte == b'=') else {
                return false;
            };
            if equals == 0 {
                return false;
            }
            saw_assignment = true;
        } else {
            // Empty string terminator.
            return saw_assignment;
        }
        position += 1;
    }
    false
}

fn mac_valid(mac: &[u8; 6]) -> bool {
    *mac != [0x00; 6] && *mac != [0xFF; 6]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_environment_is_invalid() {
        assert!(!environment_valid(&[0xFF; 64]));
        assert!(!environment_valid(&[0; 64]));
    }

    #[test]
    fn valid_environment_is_accepted() {
        let env = b"console=serial0\0eaddr=02:00:69:12:34:56\0\0";
        assert!(environment_valid(env));
    }

    #[test]
    fn unterminated_or_corrupt_environment_is_rejected() {
        assert!(!environment_valid(b"console=serial0"));
        assert!(!environment_valid(b"console=\xFF\0\0"));
        assert!(!environment_valid(b"no-assignment\0\0"));
    }

    #[test]
    fn mac_validation_rejects_blank_cells() {
        assert!(mac_valid(&DEFAULT_MAC));
        assert!(!mac_valid(&[0; 6]));
        assert!(!mac_valid(&[0xFF; 6]));
    }
}
