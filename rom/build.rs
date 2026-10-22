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
const MF_LIMIT: usize = 12;
const HASH_LOG: u32 = 16;
const HASH_SIZE: usize = 1 << HASH_LOG;

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
/// A single hash chain: the table holds the most recent position for each
/// 4-byte hash, and a candidate match is accepted when it is long enough and
/// close enough to be addressable by the 16-bit offset field.
fn lz4_compress(src: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(src.len() + src.len() / 200 + 32);
    let mut table = vec![u32::MAX; HASH_SIZE];
    let n = src.len();

    let mut anchor = 0usize; // start of the pending literal run
    let mut pos = 0usize;

    while pos + MIN_MATCH <= n {
        let seq = u32::from_le_bytes([src[pos], src[pos + 1], src[pos + 2], src[pos + 3]]);
        let h = hash4(seq);
        let cand = table[h] as usize;
        table[h] = pos as u32;

        if cand == u32::MAX as usize || pos - cand > 0xFFFF {
            pos += 1;
            continue;
        }

        // How far does the match run?
        let max = (n - pos).min(0x7FFF);
        let mut len = 0usize;
        while len < max && src[cand + len] == src[pos + len] {
            len += 1;
        }
        if len < MIN_MATCH {
            pos += 1;
            continue;
        }

        // The last sequence must be literals only, and the last five bytes of
        // the block are always literals, so stop the match short of the end.
        if pos + len > n - LAST_LITERALS {
            len = n.saturating_sub(LAST_LITERALS).saturating_sub(pos);
            if len < MIN_MATCH {
                pos += 1;
                continue;
            }
        }

        let lit_len = pos - anchor;
        let mlen = len - MIN_MATCH;
        let mut token = ((lit_len.min(15) as u8) << 4) | (mlen.min(15) as u8);
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

        // Index the positions the match covered so later matches can find
        // them.
        for i in (pos + 1)..(pos + len) {
            if i + MIN_MATCH <= n {
                let s = u32::from_le_bytes([src[i], src[i + 1], src[i + 2], src[i + 3]]);
                table[hash4(s)] = i as u32;
            }
        }
        pos += len;
        anchor = pos;
    }

    // The final sequence: literals only.
    let lit_len = n - anchor;
    let mut token = (lit_len.min(15) as u8) << 4;
    out.push(token);
    if lit_len >= 15 {
        write_len(&mut out, lit_len - 15);
    }
    out.extend_from_slice(&src[anchor..]);
    out
}

/// Build the compressed payload: magic, uncompressed size, LZ4 block.
fn build_payload(image: &[u8]) -> Vec<u8> {
    let block = lz4_compress(image);
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
        let offset = u32::from_be_bytes([
            elf[base + 4],
            elf[base + 5],
            elf[base + 6],
            elf[base + 7],
        ]) as usize;
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
