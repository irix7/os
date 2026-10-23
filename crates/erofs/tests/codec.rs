// SPDX-License-Identifier: GPL-3.0-only
//! Byte-order codec and generic superblock decoding.

use erofs::codec::{u16_at, u32_at, u64_at, BigEndian, ByteOrder, LittleEndian};
use erofs::Superblock;

#[test]
fn little_and_big_endian_values() {
    let bytes = [0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab, 0x90];
    assert_eq!(LittleEndian::read_u16([bytes[0], bytes[1]]), 0x5678);
    assert_eq!(
        LittleEndian::read_u32([bytes[0], bytes[1], bytes[2], bytes[3]]),
        0x1234_5678
    );
    assert_eq!(
        LittleEndian::read_u64([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]),
        0x90ab_cdef_1234_5678
    );
    assert_eq!(BigEndian::read_u16([bytes[0], bytes[1]]), 0x7856);
    assert_eq!(
        BigEndian::read_u32([bytes[0], bytes[1], bytes[2], bytes[3]]),
        0x7856_3412
    );
    assert_eq!(
        BigEndian::read_u64([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]),
        0x7856_3412_efcd_ab90
    );
}

#[test]
fn accessors_check_bounds() {
    let data = [1u8, 2, 3];
    assert_eq!(u16_at::<LittleEndian>(&data, 0), Some(0x0201));
    assert_eq!(u16_at::<LittleEndian>(&data, 2), None);
    assert_eq!(u32_at::<LittleEndian>(&data, 0), None);
    assert_eq!(u64_at::<LittleEndian>(&data, 0), None);
    assert_eq!(u16_at::<LittleEndian>(&data, usize::MAX), None);
}

#[test]
fn superblock_decoder_is_byte_order_parameterised() {
    let mut raw = [0u8; 128];
    put_u32(&mut raw, 0, 0xE0F5_E1E2);
    put_u32(&mut raw, 4, 0x1234_5678);
    put_u32(&mut raw, 8, 3);
    raw[12] = 12;
    raw[13] = 0;
    put_u16(&mut raw, 14, 36);
    put_u64(&mut raw, 16, 9);
    put_u64(&mut raw, 24, 1_700_000_000);
    put_u32(&mut raw, 32, 123);
    put_u32(&mut raw, 36, 3);
    put_u32(&mut raw, 80, 0);
    put_u16(&mut raw, 86, 0);
    put_u32(&mut raw, 108, 7);

    let sb = Superblock::parse_with::<BigEndian>(&raw).expect("big-endian superblock");
    assert_eq!(sb.checksum, 0x1234_5678);
    assert_eq!(sb.block_size(), 4096);
    assert_eq!(sb.root_nid, 36);
    assert_eq!(sb.inos, 9);
    assert_eq!(sb.epoch, 1_700_000_000);
    assert_eq!(sb.fixed_nsec, 123);
    assert_eq!(sb.blocks, 3);
    assert_eq!(sb.build_time, 7);

    let le = Superblock::parse(&raw);
    assert!(
        le.is_err(),
        "little-endian parse must reject big-endian magic"
    );
}

// The test superblock is written big-endian; the reader's LE path rejects it.
fn put_u16(raw: &mut [u8], offset: usize, value: u16) {
    raw[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn put_u32(raw: &mut [u8], offset: usize, value: u32) {
    raw[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn put_u64(raw: &mut [u8], offset: usize, value: u64) {
    raw[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}
