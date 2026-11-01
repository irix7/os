// SPDX-License-Identifier: GPL-3.0-only
//! The `key=value` boot configuration, shared by every IRIX 7 machine.
//!
//! Each machine reads its configuration block from whatever source it has
//! (NVRAM, a disk sector, a fetched file) and hands the bytes here.  The lookup
//! is the same everywhere, so a third machine does not get a third parser.

#![no_std]

/// Copy the value of `key` from a configuration block into `out`.
///
/// Lines are separated by `\n`; a line is `key=value`; the value ends at the
/// newline.  Returns the value length, 0 when the key is absent, or -1 for a bad
/// argument.
pub fn get(config: &[u8], key: &[u8], out: &mut [u8]) -> isize {
    if key.is_empty() {
        return -1;
    }
    for line in config.split(|byte| *byte == b'\n') {
        let Some(equals) = line.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        if &line[..equals] == key {
            let value = &line[equals + 1..];
            let length = value.len().min(out.len());
            out[..length].copy_from_slice(&value[..length]);
            return length as isize;
        }
    }
    0
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_values_and_reports_absence() {
        let config = b"mesh.peer=10.0.2.2:7000\ntoken=abc=mno\nempty=\n";
        let mut out = [0u8; 32];
        assert_eq!(get(config, b"mesh.peer", &mut out), 13);
        assert_eq!(&out[..13], b"10.0.2.2:7000");
        assert_eq!(get(config, b"token", &mut out), 7);
        assert_eq!(&out[..7], b"abc=mno");
        assert_eq!(get(config, b"empty", &mut out), 0);
        assert_eq!(get(config, b"absent", &mut out), 0);
        assert_eq!(get(config, b"", &mut out), -1);
    }

    #[test]
    fn truncates_to_the_destination() {
        let config = b"long=0123456789\n";
        let mut out = [0u8; 4];
        assert_eq!(get(config, b"long", &mut out), 4);
        assert_eq!(&out, b"0123");
    }
}
