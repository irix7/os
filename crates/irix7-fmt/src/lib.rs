// SPDX-License-Identifier: GPL-3.0-only
//! Number and address formatting for the kernels' console output.
//!
//! The kernels have no `alloc` on their console path, so each function writes
//! bytes through a sink rather than building a string.  The sink is the
//! machine's serial write, so the same formatting serves every architecture.

#![no_std]

/// Write `value` in decimal.
pub fn decimal(mut value: u64, put: &mut dyn FnMut(u8)) {
    let mut digits = [0u8; 20];
    let mut count = 0;
    loop {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
        if value == 0 {
            break;
        }
    }
    for digit in digits[..count].iter().rev() {
        put(*digit);
    }
}

/// Write `value` in hexadecimal, zero-padded to at least `width` digits.
pub fn hex(value: u64, width: usize, upper: bool, put: &mut dyn FnMut(u8)) {
    let digits: &[u8; 16] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let significant = (64 - value.leading_zeros()).div_ceil(4) as usize;
    let count = significant.max(width).max(1);
    for index in (0..count).rev() {
        let nibble = ((value >> (index * 4)) & 0xF) as usize;
        put(digits[nibble]);
    }
}

/// Write `value` in decimal, zero-padded to exactly `width` digits.  Digits
/// beyond `width` are dropped from the high end, as a fixed-width field does.
pub fn decimal_padded(value: u64, width: usize, put: &mut dyn FnMut(u8)) {
    assert!(width <= 20, "decimal_padded: width exceeds a u64");
    let mut digits = [0u8; 20];
    let mut rest = value;
    for index in (0..width).rev() {
        digits[index] = b'0' + (rest % 10) as u8;
        rest /= 10;
    }
    for digit in &digits[..width] {
        put(*digit);
    }
}

/// Write a dotted-quad IPv4 address in decimal.
pub fn dotted(octets: [u8; 4], put: &mut dyn FnMut(u8)) {
    for (index, octet) in octets.iter().enumerate() {
        if index > 0 {
            put(b'.');
        }
        decimal(u64::from(*octet), put);
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::{String, ToString};
    use std::vec::Vec;

    fn collect(f: impl FnOnce(&mut dyn FnMut(u8))) -> String {
        let mut bytes = Vec::new();
        f(&mut |b| bytes.push(b));
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn decimal_matches_std() {
        for value in [0u64, 1, 9, 10, 255, 4096, u64::MAX] {
            assert_eq!(collect(|p| decimal(value, p)), value.to_string());
        }
    }

    #[test]
    fn hex_pads_to_width_and_keeps_case() {
        assert_eq!(collect(|p| hex(0xAB, 2, true, p)), "AB");
        assert_eq!(collect(|p| hex(0x1, 2, true, p)), "01");
        assert_eq!(collect(|p| hex(0xdeadbeef, 8, false, p)), "deadbeef");
        assert_eq!(collect(|p| hex(0, 1, false, p)), "0");
        assert_eq!(collect(|p| hex(0x1F, 1, false, p)), "1f");
    }

    #[test]
    fn decimal_padded_fixed_width() {
        assert_eq!(collect(|p| decimal_padded(7, 3, p)), "007");
        assert_eq!(collect(|p| decimal_padded(12345, 3, p)), "345");
        assert_eq!(collect(|p| decimal_padded(0, 4, p)), "0000");
    }

    #[test]
    fn dotted_quad() {
        assert_eq!(collect(|p| dotted([10, 0, 2, 15], p)), "10.0.2.15");
    }
}
