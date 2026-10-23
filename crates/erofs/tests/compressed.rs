// SPDX-License-Identifier: GPL-3.0-only
//! Tests against the compressed `mkfs.erofs -zlz4` fixtures.

use erofs::{DataLayout, Erofs, Error};

const LZ4: &[u8] = include_bytes!("fixtures/lz4.erofs");
const LZ4_FULL: &[u8] = include_bytes!("fixtures/lz4-full.erofs");
const LZ4_BIG: &[u8] = include_bytes!("fixtures/lz4-big.erofs");
const ROOT: &[u8] = include_bytes!("fixtures/root.erofs");

const BIG_LEN: usize = 9_000;
const MANY_LEN: usize = 153_400;

fn big_bin() -> Vec<u8> {
    (0..BIG_LEN)
        .map(|offset| (offset as u32 * 7) as u8)
        .collect()
}

fn many_bin() -> Vec<u8> {
    let mut data = Vec::with_capacity(MANY_LEN);
    for line in 0..2600 {
        data.extend_from_slice(
            format!("line {line:08}: the quick brown fox jumps over the lazy dog\n").as_bytes(),
        );
    }
    data
}

fn inode(image: &'static [u8], path: &str) -> erofs::Inode {
    let mut fs = Erofs::mount(image).expect("mount");
    fs.lookup_path(path).expect("lookup")
}

/// The byte offset of the `z_erofs_map_header` for a compressed inode.
fn map_header_offset(inode: &erofs::Inode) -> usize {
    usize::try_from((inode.inline_offset + 7) & !7).expect("offset fits")
}

#[test]
fn compact_fixture_matches_the_uncompressed_tree() {
    let mut root = Erofs::mount(ROOT).expect("mount root");
    let mut fs = Erofs::mount(LZ4).expect("mount lz4");
    assert!(fs.superblock().has_zero_padding());
    assert_eq!(fs.superblock().block_size(), 4096);

    for path in [
        "/etc/motd",
        "/hello.txt",
        "/dir/nested.txt",
        "/empty",
        "/link",
        "/big.bin",
    ] {
        let expected = root.read_file_path(path).expect("uncompressed file");
        assert_eq!(fs.read_file_path(path).expect(path), expected, "{path}");
    }

    let big = fs.lookup_path("/big.bin").expect("big.bin");
    assert_eq!(big.layout, DataLayout::CompressedCompact);
    assert_eq!(big.size, BIG_LEN as u64);
    assert_eq!(fs.read_file(&big).expect("big.bin"), big_bin());

    let many = fs.lookup_path("/many.bin").expect("many.bin");
    assert_eq!(many.layout, DataLayout::CompressedCompact);
    assert_eq!(many.size, MANY_LEN as u64);
    assert_eq!(fs.read_file(&many).expect("many.bin"), many_bin());
}

#[test]
fn full_index_fixture_is_decoded() {
    let mut fs = Erofs::mount(LZ4_FULL).expect("mount lz4-full");
    assert!(fs.superblock().has_zero_padding());

    let big = fs.lookup_path("/big.bin").expect("big.bin");
    assert_eq!(big.layout, DataLayout::CompressedFull);
    assert_eq!(fs.read_file(&big).expect("big.bin"), big_bin());

    let many = fs.lookup_path("/many.bin").expect("many.bin");
    assert_eq!(many.layout, DataLayout::CompressedFull);
    assert_eq!(fs.read_file(&many).expect("many.bin"), many_bin());

    assert_eq!(
        fs.read_file_path("/etc/motd").expect("motd"),
        b"Welcome to IRIX 7\n"
    );
}

#[test]
fn big_pclusters_are_decoded() {
    let mut fs = Erofs::mount(LZ4_BIG).expect("mount lz4-big");
    let sb = fs.superblock();
    assert!(sb.has_zero_padding());
    // compr_cfgs: the LZ4 configuration lives in an extra superblock slot.
    assert_ne!(sb.feature_incompat & 0x0000_0002, 0);

    let big = fs.lookup_path("/big.bin").expect("big.bin");
    assert_eq!(big.layout, DataLayout::CompressedCompact);
    assert_eq!(fs.read_file(&big).expect("big.bin"), big_bin());

    // many.bin is one 16 KiB pcluster of four blocks that decodes to 153400
    // bytes, so the CBLKCNT chain in the compact index is exercised.
    let many = fs.lookup_path("/many.bin").expect("many.bin");
    assert_eq!(many.layout, DataLayout::CompressedCompact);
    assert_eq!(fs.read_file(&many).expect("many.bin"), many_bin());

    let expected = many_bin();
    for offset in [0, 4095, 4096, 65_536, MANY_LEN - 1] {
        let mut byte = [0u8; 1];
        fs.read_at(&many, offset as u64, &mut byte).expect("byte");
        assert_eq!(byte[0], expected[offset], "offset {offset}");
    }
}

#[test]
fn tail_pclusters_are_inline() {
    for image in [LZ4, LZ4_FULL] {
        let big = inode(image, "/big.bin");
        let header = map_header_offset(&big);
        let advise = u16::from_le_bytes([image[header + 4], image[header + 5]]);
        let idata_size = u16::from_le_bytes([image[header + 2], image[header + 3]]);
        assert_ne!(advise & 0x0008, 0, "inline pcluster advise");
        assert_eq!(idata_size, 301, "big.bin is one inline pcluster");

        let many = inode(image, "/many.bin");
        let header = map_header_offset(&many);
        let advise = u16::from_le_bytes([image[header + 4], image[header + 5]]);
        let idata_size = u16::from_le_bytes([image[header + 2], image[header + 3]]);
        assert_ne!(advise & 0x0008, 0, "inline pcluster advise");
        assert_eq!(idata_size, 1282, "many.bin tail pcluster");
    }
}

#[test]
fn reads_cross_cluster_and_extent_boundaries() {
    let mut fs = Erofs::mount(LZ4).expect("mount");
    let many = fs.lookup_path("/many.bin").expect("many.bin");
    let expected = many_bin();

    for (offset, len) in [
        (0usize, 16usize),
        (4090, 32),
        (47_100, 200),
        (47_162, 64),
        (93_100, 300),
        (139_050, 64),
        (139_083, 512),
        (153_300, 100),
        (MANY_LEN - 1, 1),
    ] {
        let mut buffer = vec![0u8; len];
        fs.read_at(&many, offset as u64, &mut buffer)
            .expect("range");
        assert_eq!(buffer, expected[offset..offset + len], "offset {offset}");
    }

    let mut buffer = [0u8; 1];
    fs.read_at(&many, MANY_LEN as u64, &mut buffer)
        .expect_err("read past EOF");
    assert!(fs.read_at(&many, MANY_LEN as u64, &mut []).is_ok());
}

#[test]
fn interlaced_and_fragment_headers_are_rejected() {
    for (byte, expected) in [
        (0x20u8, Error::UnsupportedCompression("fragments")),
        (0x10, Error::UnsupportedCompression("interlaced pcluster")),
    ] {
        let big = inode(LZ4, "/big.bin");
        let header = map_header_offset(&big);
        let mut image = LZ4.to_vec();
        // Keep COMPACTED_2B (bit 0) and add the unsupported advise bit.
        let advise = 0x0001 | u16::from(byte);
        image[header + 4..header + 6].copy_from_slice(&advise.to_le_bytes());
        let mut fs = Erofs::mount(image.as_slice()).expect("mount patched image");
        let patched = fs.lookup_path("/big.bin").expect("big.bin");
        assert!(matches!(fs.read_file(&patched), Err(error) if error == expected));
    }
}

#[test]
fn non_lz4_algorithm_is_rejected() {
    let big = inode(LZ4, "/big.bin");
    let header = map_header_offset(&big);
    let mut image = LZ4.to_vec();
    image[header + 6] = 0x01; // LZMA for head 1 and head 2
    let mut fs = Erofs::mount(image.as_slice()).expect("mount patched image");
    let patched = fs.lookup_path("/big.bin").expect("big.bin");
    assert_eq!(
        fs.read_file(&patched),
        Err(Error::UnsupportedCompression("lzma"))
    );
}

#[test]
fn unknown_logical_cluster_size_is_rejected() {
    let big = inode(LZ4, "/big.bin");
    let header = map_header_offset(&big);
    let mut image = LZ4.to_vec();
    image[header + 7] = 1; // one block larger than the filesystem block
    let mut fs = Erofs::mount(image.as_slice()).expect("mount patched image");
    let patched = fs.lookup_path("/big.bin").expect("big.bin");
    assert_eq!(
        fs.read_file(&patched),
        Err(Error::UnsupportedCompression("logical cluster size"))
    );
}

#[test]
fn empty_pcluster_is_rejected() {
    // lz4.erofs stores many.bin's first pcluster in block 1.  Zeroing the
    // block leaves only the zero padding, which is not a valid LZ4 stream.
    let mut image = LZ4.to_vec();
    image[4096..8192].fill(0);
    let mut fs = Erofs::mount(image.as_slice()).expect("mount patched image");
    let many = fs.lookup_path("/many.bin").expect("many.bin");
    assert!(matches!(
        fs.read_file(&many),
        Err(Error::InvalidCompressedData("pcluster is all padding"))
    ));
}

#[test]
fn truncated_pcluster_stream_is_rejected() {
    let mut image = LZ4.to_vec();
    // A five-literal token followed by a zero match offset: the stream ends
    // long before the extent is decoded.
    image[4096] = 0x50;
    image[4097..4102].copy_from_slice(b"hello");
    let mut fs = Erofs::mount(image.as_slice()).expect("mount patched image");
    let many = fs.lookup_path("/many.bin").expect("many.bin");
    assert!(matches!(
        fs.read_file(&many),
        Err(Error::InvalidCompressedData(_))
    ));
}

#[test]
fn compressed_reads_are_bounded_by_the_inode_size() {
    let mut fs = Erofs::mount(LZ4).expect("mount");
    let many = fs.lookup_path("/many.bin").expect("many.bin");
    let mut buffer = [0u8; 2];
    assert_eq!(
        fs.read_at(&many, MANY_LEN as u64 - 1, &mut buffer),
        Err(Error::OutOfBounds)
    );
    let mut empty = [];
    assert!(fs.read_at(&many, MANY_LEN as u64, &mut empty).is_ok());
}
