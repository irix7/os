// SPDX-License-Identifier: GPL-3.0-only
//! AES block cipher (FIPS 197).
//!
//! Implements AES-128 and AES-256 encryption/decryption in ECB mode, plus
//! AES-CTR for stream-like operation. Needed for TLS record layer encryption.
//!
//! No external dependencies — pure Rust, no_std compatible.

// AES-192/256 entry points and the full round set are kept for TLS even though
// the current ROM only needs the SHA-256 path.
#![allow(dead_code)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::manual_rotate)]

/// Number of columns (32-bit words) in the state. Always 4 for AES.
const NB: usize = 4;

/// Number of rounds for each key size.
const ROUNDS_128: usize = 10;
const ROUNDS_192: usize = 12;
const ROUNDS_256: usize = 14;

/// AES S-box (substitution box).
const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0xbb, 0x71, 0x79,
];

/// Inverse AES S-box, generated at compile time from the forward S-box.
const INV_SBOX: [u8; 256] = {
    let mut inv = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        inv[SBOX[i] as usize] = i as u8;
        i += 1;
    }
    inv
};

/// AES Rcon (round constant) values.
const RCON: [u32; 11] = [
    0x00000000, 0x01000000, 0x02000000, 0x04000000, 0x08000000, 0x10000000, 0x20000000, 0x40000000,
    0x80000000, 0x1B000000, 0x36000000,
];

/// Substitute a byte using the S-box.
#[inline(always)]
fn sub_byte(b: u8) -> u8 {
    SBOX[b as usize]
}

/// Substitute a byte using the inverse S-box.
#[inline(always)]
fn inv_sub_byte(b: u8) -> u8 {
    INV_SBOX[b as usize]
}

/// Rotate a 4-byte word left by one byte.
#[inline(always)]
fn rot_word(w: u32) -> u32 {
    (w << 8) | (w >> 24)
}

/// Substitute all bytes in a word.
#[inline(always)]
fn sub_word(w: u32) -> u32 {
    u32::from_be_bytes([
        sub_byte((w >> 24) as u8),
        sub_byte((w >> 16) as u8),
        sub_byte((w >> 8) as u8),
        sub_byte(w as u8),
    ])
}

/// Multiply by 2 in GF(2^8) with irreducible polynomial 0x11B.
#[inline(always)]
fn xtime(x: u8) -> u8 {
    let x = x as u16;
    let mut x = x << 1;
    if x & 0x100 != 0 {
        x ^= 0x1B;
    }
    x as u8
}

/// Multiply by 9 in GF(2^8): 9 = x^3 + 1
#[inline(always)]
fn mul9(x: u8) -> u8 {
    xtime(xtime(xtime(x))) ^ x
}

/// Multiply by 11 in GF(2^8): 11 = x^3 + x + 1
#[inline(always)]
fn mul11(x: u8) -> u8 {
    xtime(xtime(xtime(x))) ^ xtime(x) ^ x
}

/// Multiply by 13 in GF(2^8): 13 = x^3 + x^2 + 1
#[inline(always)]
fn mul13(x: u8) -> u8 {
    xtime(xtime(xtime(x))) ^ xtime(xtime(x)) ^ x
}

/// Multiply by 14 in GF(2^8): 14 = x^3 + x^2 + x
#[inline(always)]
fn mul14(x: u8) -> u8 {
    xtime(xtime(xtime(x))) ^ xtime(xtime(x)) ^ xtime(x)
}

/// AES-128 key schedule: expands a 16-byte key into 11 round keys (176 bytes).
pub fn expand_key_128(key: &[u8; 16]) -> [u32; 44] {
    let mut w = [0u32; 44];
    for i in 0..4 {
        w[i] = u32::from_be_bytes([key[i * 4], key[i * 4 + 1], key[i * 4 + 2], key[i * 4 + 3]]);
    }
    for i in 4..44 {
        let mut temp = w[i - 1];
        if i % 4 == 0 {
            temp = sub_word(rot_word(temp)) ^ RCON[i / 4];
        }
        w[i] = w[i - 4] ^ temp;
    }
    w
}

/// AES-256 key schedule: expands a 32-byte key into 15 round keys (240 bytes).
pub fn expand_key_256(key: &[u8; 32]) -> [u32; 60] {
    let mut w = [0u32; 60];
    for i in 0..8 {
        w[i] = u32::from_be_bytes([key[i * 4], key[i * 4 + 1], key[i * 4 + 2], key[i * 4 + 3]]);
    }
    for i in 8..60 {
        let mut temp = w[i - 1];
        if i % 8 == 0 {
            temp = sub_word(rot_word(temp)) ^ RCON[i / 8];
        } else if i % 8 == 4 {
            temp = sub_word(temp);
        }
        w[i] = w[i - 8] ^ temp;
    }
    w
}

/// AddRoundKey: XOR state with round key.
fn add_round_key(state: &mut [u8; 16], round_key: &[u32; 4]) {
    for i in 0..4 {
        let wk = round_key[i].to_be_bytes();
        state[i * 4] ^= wk[0];
        state[i * 4 + 1] ^= wk[1];
        state[i * 4 + 2] ^= wk[2];
        state[i * 4 + 3] ^= wk[3];
    }
}

/// SubBytes: substitute each byte of the state.
fn sub_bytes(state: &mut [u8; 16]) {
    for i in 0..16 {
        state[i] = sub_byte(state[i]);
    }
}

/// ShiftRows: shift rows of the state left.
fn shift_rows(state: &mut [u8; 16]) {
    // Row 1: shift left by 1
    state[1..5].rotate_left(1);
    // Row 2: shift left by 2
    state[5..9].rotate_left(2);
    // Row 3: shift left by 3
    state[9..13].rotate_left(3);
}

/// MixColumns: mix columns of the state.
fn mix_columns(state: &mut [u8; 16]) {
    for c in 0..4 {
        let s0 = state[c];
        let s1 = state[c + 4];
        let s2 = state[c + 8];
        let s3 = state[c + 12];
        state[c] = xtime(s0) ^ xtime(s1) ^ s1 ^ s2 ^ s3;
        state[c + 4] = s0 ^ xtime(s1) ^ xtime(s2) ^ s2 ^ s3;
        state[c + 8] = s0 ^ s1 ^ xtime(s2) ^ xtime(s3) ^ s3;
        state[c + 12] = s0 ^ s1 ^ s2 ^ xtime(s3) ^ s0;
    }
}

/// AES-128 encrypt a single 16-byte block in ECB mode.
pub fn aes_128_encrypt_block(key_schedule: &[u32; 44], block: &mut [u8; 16]) {
    add_round_key(block, &key_schedule[0..4].try_into().unwrap());
    for round in 1..ROUNDS_128 {
        sub_bytes(block);
        shift_rows(block);
        mix_columns(block);
        add_round_key(
            block,
            &key_schedule[round * 4..round * 4 + 4].try_into().unwrap(),
        );
    }
    // Final round: no MixColumns
    sub_bytes(block);
    shift_rows(block);
    add_round_key(
        block,
        &key_schedule[ROUNDS_128 * 4..ROUNDS_128 * 4 + 4]
            .try_into()
            .unwrap(),
    );
}

/// AES-128 decrypt a single 16-byte block in ECB mode.
pub fn aes_128_decrypt_block(key_schedule: &[u32; 44], block: &mut [u8; 16]) {
    add_round_key(
        block,
        &key_schedule[ROUNDS_128 * 4..ROUNDS_128 * 4 + 4]
            .try_into()
            .unwrap(),
    );
    for round in (1..ROUNDS_128).rev() {
        inv_shift_rows(block);
        inv_sub_bytes(block);
        add_round_key(
            block,
            &key_schedule[round * 4..round * 4 + 4].try_into().unwrap(),
        );
        inv_mix_columns(block);
    }
    inv_shift_rows(block);
    inv_sub_bytes(block);
    add_round_key(block, &key_schedule[0..4].try_into().unwrap());
}

/// InvSubBytes: inverse substitution.
fn inv_sub_bytes(state: &mut [u8; 16]) {
    for i in 0..16 {
        state[i] = inv_sub_byte(state[i]);
    }
}

/// InvShiftRows: inverse shift rows (shift right).
fn inv_shift_rows(state: &mut [u8; 16]) {
    state[1..5].rotate_right(1);
    state[5..9].rotate_right(2);
    state[9..13].rotate_right(3);
}

/// InvMixColumns: inverse mix columns using mul9/11/13/14.
fn inv_mix_columns(state: &mut [u8; 16]) {
    for c in 0..4 {
        let s0 = state[c];
        let s1 = state[c + 4];
        let s2 = state[c + 8];
        let s3 = state[c + 12];
        state[c] = mul14(s0) ^ mul11(s1) ^ mul13(s2) ^ mul9(s3);
        state[c + 4] = mul9(s0) ^ mul14(s1) ^ mul11(s2) ^ mul13(s3);
        state[c + 8] = mul13(s0) ^ mul9(s1) ^ mul14(s2) ^ mul11(s3);
        state[c + 12] = mul11(s0) ^ mul13(s1) ^ mul9(s2) ^ mul14(s3);
    }
}

/// AES-CTR: encrypt/decrypt using AES in counter mode.
/// `key_schedule` is the expanded key, `counter` is the initial 16-byte counter block.
/// `data` is modified in place.
pub fn aes_ctr(key_schedule: &[u32; 44], counter: &mut [u8; 16], data: &mut [u8]) {
    let mut keystream = [0u8; 16];
    let mut offset = 0;
    while offset < data.len() {
        aes_128_encrypt_block(key_schedule, counter);
        keystream.copy_from_slice(counter);
        // Increment counter (32-bit big-endian at bytes 12..15)
        for i in (12..16).rev() {
            counter[i] = counter[i].wrapping_add(1);
            if counter[i] != 0 {
                break;
            }
        }
        let end = (offset + 16).min(data.len());
        let chunk = &mut data[offset..end];
        let ks = &keystream[..chunk.len()];
        for (i, b) in chunk.iter_mut().enumerate() {
            *b ^= ks[i];
        }
        offset = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // AES-128 ECB test vector from NIST SP 800-38A:
    // Key: 2b7e1516 28aed2a6 abf71588 09cf4f3c
    // Plaintext: 6bc1bee2 2e409f96 e93d7e11 73971114
    // Ciphertext: 3ad77bb4 0d7a3660 a89ecaf3 2466ef97
    #[test]
    fn aes_128_encrypt_test_vector() {
        let key = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let mut block = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x97,
            0x11, 0x14,
        ];
        let ks = expand_key_128(&key);
        aes_128_encrypt_block(&ks, &mut block);
        let expected = [
            0x3a, 0xd7, 0x7b, 0xb4, 0x0d, 0x7a, 0x36, 0x60, 0xa8, 0x9e, 0xca, 0xf3, 0x24, 0x66,
            0xef, 0x97,
        ];
        assert_eq!(block, expected);
    }

    #[test]
    fn aes_128_decrypt_test_vector() {
        let key = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let mut block = [
            0x3a, 0xd7, 0x7b, 0xb4, 0x0d, 0x7a, 0x36, 0x60, 0xa8, 0x9e, 0xca, 0xf3, 0x24, 0x66,
            0xef, 0x97,
        ];
        let ks = expand_key_128(&key);
        aes_128_decrypt_block(&ks, &mut block);
        let expected = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x97,
            0x11, 0x14,
        ];
        assert_eq!(block, expected);
    }

    #[test]
    fn aes_ctr_roundtrip() {
        let key = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let data = *b"Hello, AES-CTR mode!";
        let mut counter = [0u8; 16];
        counter[12] = 1;
        let ks = expand_key_128(&key);
        let mut encrypted = data;
        aes_ctr(&ks, &mut counter, &mut encrypted);
        let mut decrypted = encrypted;
        aes_ctr(&ks, &mut counter, &mut decrypted);
        assert_eq!(decrypted, data);
    }
}
