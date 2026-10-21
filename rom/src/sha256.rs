//! SHA-256 hash function (FIPS 180-4).
//!
//! Implemented in-house to avoid pulling in the `sha2` crate and its
//! dependencies into the freestanding ROM. Supports streaming multi-chunk
//! updates so the kernel/boot-bundle can be hashed as it is fetched over
//! HTTP without buffering the whole image.

use crate::hex::encode_into;

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const BLOCK_LEN: usize = 64;

fn rotr(x: u32, n: u32) -> u32 {
    x.rotate_right(n)
}

fn choice(e: u32, f: u32, g: u32) -> u32 {
    (e & f) ^ ((!e) & g)
}

fn majority(a: u32, b: u32, c: u32) -> u32 {
    (a & b) ^ (a & c) ^ (b & c)
}

fn sigma0(x: u32) -> u32 {
    rotr(x, 2) ^ rotr(x, 13) ^ rotr(x, 22)
}

fn sigma1(x: u32) -> u32 {
    rotr(x, 6) ^ rotr(x, 11) ^ rotr(x, 25)
}

fn gamma0(x: u32) -> u32 {
    rotr(x, 7) ^ rotr(x, 18) ^ (x >> 3)
}

fn gamma1(x: u32) -> u32 {
    rotr(x, 17) ^ rotr(x, 19) ^ (x >> 10)
}

/// Compress a single 64-byte block, updating the 8-word state in place.
fn compress_block(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..64 {
        w[i] = gamma1(w[i - 2])
            .wrapping_add(w[i - 7])
            .wrapping_add(gamma0(w[i - 15]))
            .wrapping_add(w[i - 16]);
    }
    let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
        state[0], state[1], state[2], state[3], state[4], state[5], state[6], state[7],
    );
    for i in 0..64 {
        let t1 = h
            .wrapping_add(sigma1(e))
            .wrapping_add(choice(e, f, g))
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let t2 = sigma0(a).wrapping_add(majority(a, b, c));
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

/// A SHA-256 digest (32 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Sha256(pub [u8; 32]);

impl Sha256 {
    /// Compute the SHA-256 digest of `data` in one shot.
    pub fn digest(data: &[u8]) -> Sha256 {
        let mut hasher = Hasher::new();
        hasher.update(data);
        hasher.finalize()
    }

    /// Compare this digest against another for equality (constant-time).
    pub fn eq_digest(&self, other: &Sha256) -> bool {
        let mut diff = 0u8;
        for i in 0..32 {
            diff |= self.0[i] ^ other.0[i];
        }
        diff == 0
    }

    /// Return the digest as lowercase hex bytes.
    pub fn to_hex(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        encode_into(&self.0, &mut out);
        out
    }
}

/// Streaming SHA-256 hasher.
pub struct Hasher {
    state: [u32; 8],
    buffer: [u8; BLOCK_LEN],
    buffer_len: usize,
    total_len: u64,
}

impl Hasher {
    /// Create a new hasher.
    pub fn new() -> Hasher {
        Hasher {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: [0; BLOCK_LEN],
            buffer_len: 0,
            total_len: 0,
        }
    }

    /// Feed `data` into the hash. May be called multiple times.
    pub fn update(&mut self, data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        let mut data = data;
        while !data.is_empty() {
            if self.buffer_len > 0 {
                // Finish any partially-filled block.
                let need = BLOCK_LEN - self.buffer_len;
                let take = data.len().min(need);
                self.buffer[self.buffer_len..self.buffer_len + take].copy_from_slice(&data[..take]);
                self.buffer_len += take;
                data = &data[take..];
                if self.buffer_len == BLOCK_LEN {
                    compress_block(&mut self.state, &self.buffer);
                    self.buffer_len = 0;
                }
            } else {
                // Buffer is empty: process full blocks directly, buffer remainder.
                while data.len() >= BLOCK_LEN {
                    compress_block(&mut self.state, &data[..BLOCK_LEN].try_into().unwrap());
                    data = &data[BLOCK_LEN..];
                }
                if !data.is_empty() {
                    self.buffer[..data.len()].copy_from_slice(data);
                    self.buffer_len = data.len();
                    data = &[];
                }
            }
        }
    }

    /// Finalize and return the digest.
    pub fn finalize(mut self) -> Sha256 {
        let total_bits = self.total_len.wrapping_mul(8);

        // Append 0x80 then zeros until length mod 64 == 56.
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;
        if self.buffer_len > 56 {
            for i in self.buffer_len..BLOCK_LEN {
                self.buffer[i] = 0;
            }
            compress_block(&mut self.state, &self.buffer);
            self.buffer_len = 0;
        }
        for i in self.buffer_len..56 {
            self.buffer[i] = 0;
        }
        // Append 64-bit big-endian bit length.
        for i in 0..8 {
            self.buffer[56 + i] = ((total_bits >> ((7 - i) * 8)) & 0xFF) as u8;
        }
        compress_block(&mut self.state, &self.buffer);

        let mut out = [0u8; 32];
        for i in 0..8 {
            out[i * 4..i * 4 + 4].copy_from_slice(&self.state[i].to_be_bytes());
        }
        Sha256(out)
    }
}

impl Default for Hasher {
    fn default() -> Hasher {
        Hasher::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // FIPS 180-4 example: SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
    #[test]
    fn sha256_abc() {
        let d = Sha256::digest(b"abc");
        assert_eq!(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            core::str::from_utf8(&d.to_hex()).unwrap()
        );
    }

    // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
    #[test]
    fn sha256_empty() {
        let d = Sha256::digest(b"");
        assert_eq!(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            core::str::from_utf8(&d.to_hex()).unwrap()
        );
    }

    // Two different inputs must differ.
    #[test]
    fn sha256_different_inputs_differ() {
        let a = Sha256::digest(b"hello");
        let b = Sha256::digest(b"world");
        assert!(!a.eq_digest(&b));
    }

    // Streaming update must equal one-shot digest.
    #[test]
    fn sha256_streaming_matches_oneshot() {
        let data = b"The quick brown fox jumps over the lazy dog";
        let one = Sha256::digest(data);
        let mut hasher = Hasher::new();
        hasher.update(&data[..10]);
        hasher.update(&data[10..20]);
        hasher.update(&data[20..]);
        let streamed = hasher.finalize();
        assert!(one.eq_digest(&streamed));
    }
}
