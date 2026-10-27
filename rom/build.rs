// SPDX-License-Identifier: GPL-3.0-only
// Build script for the ROM.
//
// Builds the PROM image out of two parts:
//   1. the boot stub (rom/src/boot/reset.S), which is plain assembly;
//   2. the kernel image, compressed with LZ4 and embedded in the PROM so the
//      stub can decompress it into RAM and jump to it.
//
// The kernel must already be built before this runs:
//   cargo build --release --target targets/mips3-sgi-irix7-n32.json -p kernel
//
// The compressed payload has this layout:
//   [4 bytes] magic 0x184D2204
//   [4 bytes] uncompressed size, big-endian
//   [n bytes] LZ4 block (RFC 7857), literal/match sequences
//
// The stub decompresses it to 0x0804_0000 physical and jumps there.

use std::env;
use std::fs;
use std::path::PathBuf;

/// Magic that introduces a compressed payload.
const PAYLOAD_MAGIC: u32 = 0x184D_2204;

/// LZ4 block format constants.
const MIN_MATCH: usize = 4;
const LAST_LITERALS: usize = 5;
const HASH_LOG: u32 = 16;
const HASH_SIZE: usize = 1 << HASH_LOG;
/// Candidates walked in one hash chain before the match search gives up.
const CHAIN_MAX: usize = 64;
/// LZ4's 16-bit offset field.
const MAX_DIST: usize = 0xFFFF;

/// Hash a 4-byte sequence.
fn hash4(v: u32) -> usize {
    (v.wrapping_mul(2_654_435_761) >> (32 - HASH_LOG)) as usize
}

/// Emit an LZ4 length extension: `len` counts the bytes beyond the 15 the
/// token can hold.
fn write_len(out: &mut Vec<u8>, mut len: usize) {
    while len >= 255 {
        out.push(255);
        len -= 255;
    }
    out.push(len as u8);
}

/// Compress `src` into an LZ4 block.
///
/// This is a hash-chain match finder with lazy matching, not the one-candidate
/// greedy scan the first encoder used.  The block format is unchanged, so the
/// assembly decompressor in `src/boot` needs no new code; the encoder only
/// spends host build time to leave more of the fixed 512 KiB PROM window for
/// the kernel.  A round-trip check below proves the emitted block decodes to
/// the exact input before the image is written.
fn lz4_compress(src: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(src.len() / 2 + 32);
    let n = src.len();
    let mut head = vec![u32::MAX; HASH_SIZE];
    let mut prev = vec![u32::MAX; n];
    let mut anchor = 0usize;
    let mut pos = 0usize;

    /// Record one position in the hash chain.
    fn insert(src: &[u8], head: &mut [u32], prev: &mut [u32], pos: usize) {
        if pos + MIN_MATCH > src.len() {
            return;
        }
        let seq = u32::from_le_bytes([src[pos], src[pos + 1], src[pos + 2], src[pos + 3]]);
        let h = hash4(seq);
        prev[pos] = head[h];
        head[h] = pos as u32;
    }

    /// Longest match for `pos`, bounded by the hash chain and the LZ4 limits.
    fn find_best(src: &[u8], head: &[u32], prev: &[u32], pos: usize) -> (usize, usize) {
        if pos + MIN_MATCH > src.len() {
            return (0, 0);
        }
        let n = src.len();
        // The last five bytes of a block must be literals, so a match may not
        // extend into them.
        let max = (n - LAST_LITERALS).saturating_sub(pos).min(0x7FFF);
        if max < MIN_MATCH {
            return (0, 0);
        }
        let seq = u32::from_le_bytes([src[pos], src[pos + 1], src[pos + 2], src[pos + 3]]);
        let mut cand = head[hash4(seq)] as usize;
        let mut best_len = 0usize;
        let mut best_cand = 0usize;
        let mut tries = 0usize;
        while cand != u32::MAX as usize && pos > cand && pos - cand <= MAX_DIST && tries < CHAIN_MAX
        {
            // Cheap rejection before the byte loop.  `best_len < max`, so the
            // probe stays inside the match window and inside `src`.
            if src[cand + best_len] == src[pos + best_len] {
                let mut len = 0usize;
                while len < max && src[cand + len] == src[pos + len] {
                    len += 1;
                }
                if len > best_len {
                    best_len = len;
                    best_cand = cand;
                    if len == max {
                        break;
                    }
                }
            }
            cand = prev[cand] as usize;
            tries += 1;
        }
        (best_len, best_cand)
    }

    while pos + MIN_MATCH <= n {
        let (mut len, mut cand) = find_best(src, &head, &prev, pos);
        insert(src, &mut head, &mut prev, pos);
        if len < MIN_MATCH {
            pos += 1;
            continue;
        }
        // Lazy matching: if the next position has a strictly longer match,
        // leave the current byte as a literal instead of taking a short match
        // that hides a better one.
        if pos + 1 + MIN_MATCH <= n {
            let (next_len, _) = find_best(src, &head, &prev, pos + 1);
            if next_len > len + 1 {
                pos += 1;
                continue;
            }
        }
        // A match that runs into the final literal tail must be shortened.
        if pos + len > n - LAST_LITERALS {
            len = n - LAST_LITERALS - pos;
            if len < MIN_MATCH {
                pos += 1;
                continue;
            }
        }

        let lit_len = pos - anchor;
        let mlen = len - MIN_MATCH;
        let token = ((lit_len.min(15) as u8) << 4) | (mlen.min(15) as u8);
        out.push(token);
        if lit_len >= 15 {
            write_len(&mut out, lit_len - 15);
        }
        out.extend_from_slice(&src[anchor..pos]);

        let offset = (pos - cand) as u16;
        out.push((offset & 0xFF) as u8);
        out.push((offset >> 8) as u8);
        if mlen >= 15 {
            write_len(&mut out, mlen - 15);
        }

        // Index the positions the match covered so later matches can use them.
        for i in (pos + 1)..(pos + len) {
            insert(src, &mut head, &mut prev, i);
        }
        pos += len;
        anchor = pos;
        let _ = &mut cand;
    }

    // The final sequence: literals only.
    let lit_len = n - anchor;
    let token = (lit_len.min(15) as u8) << 4;
    out.push(token);
    if lit_len >= 15 {
        write_len(&mut out, lit_len - 15);
    }
    out.extend_from_slice(&src[anchor..]);
    out
}

/// Decode an LZ4 block.  Used only by the build-time round-trip check.
fn lz4_decompress(src: &[u8], expected: usize) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(expected);
    let mut at = 0usize;
    while at < src.len() {
        let token = src[at];
        at += 1;
        let mut literals = (token >> 4) as usize;
        if literals == 15 {
            loop {
                let byte = *src.get(at)?;
                at += 1;
                literals += byte as usize;
                if byte != 255 {
                    break;
                }
            }
        }
        let end = at.checked_add(literals)?;
        if end > src.len() {
            return None;
        }
        out.extend_from_slice(&src[at..end]);
        at = end;
        if at == src.len() {
            break;
        }
        let offset = u16::from_le_bytes([*src.get(at)?, *src.get(at + 1)?]) as usize;
        at += 2;
        if offset == 0 || offset > out.len() {
            return None;
        }
        let mut match_len = (token & 0x0F) as usize;
        if match_len == 15 {
            loop {
                let byte = *src.get(at)?;
                at += 1;
                match_len += byte as usize;
                if byte != 255 {
                    break;
                }
            }
        }
        match_len += MIN_MATCH;
        let start = out.len() - offset;
        for index in 0..match_len {
            let byte = out[start + index];
            out.push(byte);
        }
    }
    (out.len() == expected).then_some(out)
}

/// Build the compressed payload: magic, uncompressed size, LZ4 block.
fn build_payload(image: &[u8]) -> Vec<u8> {
    let block = lz4_compress(image);
    // An encoder bug must fail the build, not produce a ROM that cannot boot.
    match lz4_decompress(&block, image.len()) {
        Some(decoded) if decoded == image => {}
        _ => panic!("rom: LZ4 payload does not round-trip to the kernel image"),
    }
    let mut out = Vec::with_capacity(block.len() + 8);
    out.extend_from_slice(&PAYLOAD_MAGIC.to_be_bytes());
    out.extend_from_slice(&(image.len() as u32).to_be_bytes());
    out.extend_from_slice(&block);
    out
}

/// The kernel ELF's `PT_LOAD` segments, laid out at their physical addresses.
fn load_image(elf: &[u8]) -> Option<Vec<u8>> {
    if elf.len() < 52 || &elf[0..4] != b"\x7fELF" || elf[4] != 1 || elf[5] != 2 {
        return None;
    }
    let phoff = u32::from_be_bytes([elf[0x1C], elf[0x1D], elf[0x1E], elf[0x1F]]) as usize;
    let phentsize = u16::from_be_bytes([elf[0x2A], elf[0x2B]]) as usize;
    let phnum = u16::from_be_bytes([elf[0x2C], elf[0x2D]]) as usize;
    if phentsize < 32 || phnum == 0 {
        return None;
    }

    let mut segs: Vec<(u32, usize, usize)> = Vec::new(); // (paddr, file offset, size)
    for i in 0..phnum {
        let base = phoff + i * phentsize;
        if base + 32 > elf.len() {
            return None;
        }
        let p_type = u32::from_be_bytes([elf[base], elf[base + 1], elf[base + 2], elf[base + 3]]);
        if p_type != 1 {
            continue; // PT_LOAD only
        }
        let offset =
            u32::from_be_bytes([elf[base + 4], elf[base + 5], elf[base + 6], elf[base + 7]])
                as usize;
        let paddr = u32::from_be_bytes([
            elf[base + 12],
            elf[base + 13],
            elf[base + 14],
            elf[base + 15],
        ]);
        let filesz = u32::from_be_bytes([
            elf[base + 16],
            elf[base + 17],
            elf[base + 18],
            elf[base + 19],
        ]) as usize;
        if filesz == 0 {
            continue;
        }
        if offset.checked_add(filesz)? > elf.len() {
            return None;
        }
        segs.push((paddr, offset, filesz));
    }
    if segs.is_empty() {
        return None;
    }
    segs.sort_by_key(|s| s.0);

    let base = segs[0].0;
    let end = segs.iter().map(|s| s.0 + s.2 as u32).max()?;
    let mut image = vec![0u8; (end - base) as usize];
    for &(paddr, offset, size) in &segs {
        let at = (paddr - base) as usize;
        image[at..at + size].copy_from_slice(&elf[offset..offset + size]);
    }
    Some(image)
}

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let payload_dir = manifest_dir.join("src/boot");
    let payload_s = payload_dir.join("payload.S");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_TARGET_DIR");

    let target_dir = match env::var("CARGO_TARGET_DIR") {
        Ok(v) => PathBuf::from(v),
        Err(_) => manifest_dir
            .parent()
            .map(|p| p.join("target"))
            .unwrap_or_else(|| PathBuf::from("target")),
    };
    let target = env::var("TARGET").unwrap_or_default();
    let profile = env::var("PROFILE").unwrap_or_else(|_| "release".into());
    let kernel_elf = target_dir.join(&target).join(&profile).join("kernel");

    println!("cargo:rerun-if-changed={}", kernel_elf.display());

    let image = match fs::read(&kernel_elf).ok().and_then(|d| load_image(&d)) {
        Some(i) => i,
        None => {
            println!(
                "cargo:warning=kernel image not found at {}; embedding an empty payload",
                kernel_elf.display()
            );
            Vec::new()
        }
    };

    let payload = build_payload(&image);

    // Also drop the raw payload next to the ELF so the format can be checked
    // against a reference decoder without running the machine.
    if let Ok(out_dir) = env::var("OUT_DIR") {
        let dir = PathBuf::from(out_dir);
        let _ = fs::write(dir.join("payload.bin"), &payload);
        let _ = fs::write(dir.join("image.bin"), &image);
    }

    println!(
        "cargo:warning=kernel image {} bytes -> {} bytes compressed",
        image.len(),
        payload.len()
    );

    // Emit the payload as assembly the linker can place in the PROM image.
    let mut asm = String::new();
    asm.push_str("/* generated by rom/build.rs - do not edit */\n");
    asm.push_str(".section .payload, \"a\", @progbits\n");
    asm.push_str(".align 4\n");
    asm.push_str(".globl rom_payload_start\n");
    asm.push_str("rom_payload_start:\n");
    for chunk in payload.chunks(4) {
        let mut w: u32 = 0;
        for (i, &b) in chunk.iter().enumerate() {
            w |= (b as u32) << (24 - i * 8);
        }
        asm.push_str(&format!("    .word 0x{:08X}\n", w));
    }
    asm.push_str(".globl rom_payload_end\n");
    asm.push_str("rom_payload_end:\n");
    fs::write(&payload_s, asm).expect("write payload.S");
}
