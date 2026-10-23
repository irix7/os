// SPDX-License-Identifier: GPL-3.0-only
//! Tests against the golden `mkfs.erofs` fixture.

use erofs::{DataLayout, DirEntry, Erofs, Error, FileType};

const IMAGE: &[u8] = include_bytes!("fixtures/root.erofs");
const EXTENDED: &[u8] = include_bytes!("fixtures/extended.erofs");
const IMAGE_48BIT: &[u8] = include_bytes!("fixtures/48bit.erofs");

fn mount() -> Erofs<&'static [u8]> {
    Erofs::mount(IMAGE).expect("mount fixture")
}

fn check_tree(fs: &mut Erofs<&'static [u8]>) {
    assert_eq!(
        fs.read_file_path("/etc/motd").expect("motd"),
        b"Welcome to IRIX 7\n"
    );
    assert_eq!(
        fs.read_file_path("/hello.txt").expect("hello"),
        b"hello, erofs\n"
    );
    let big = fs.read_file_path("/big.bin").expect("big.bin");
    assert_eq!(big.len(), 9000);
    for (offset, byte) in big.iter().enumerate() {
        assert_eq!(*byte, (offset as u32 * 7) as u8);
    }
}

fn names(entries: &[DirEntry]) -> Vec<&[u8]> {
    entries.iter().map(|entry| entry.name.as_slice()).collect()
}

#[test]
fn superblock_and_root() {
    let mut fs = mount();
    let sb = fs.superblock();
    assert_eq!(sb.block_size(), 4096);
    assert_eq!(sb.blocks, 3);
    assert_eq!(sb.root_nid, 36);
    assert_eq!(sb.inos, 9);
    assert_eq!(sb.feature_incompat, 0);
    assert_eq!(sb.feature_compat & 3, 3);
    assert_eq!(sb.epoch, 1_700_000_000);
    assert_eq!(sb.volume_name, [0; 16]);
    assert_eq!(sb.uuid, [0; 16]);

    let root = fs.root().expect("root inode");
    assert!(root.is_dir());
    assert_eq!(root.size, 130);
    assert_eq!(root.inode_size, 32);
    assert_eq!(root.layout, DataLayout::Inline);
    assert_eq!(root.mode, 0o40_755);
    assert_eq!(root.uid, 0);
    assert_eq!(root.gid, 0);

    let entries = fs.read_dir(&root).expect("root directory");
    let expected: &[&[u8]] = &[
        b".",
        b"..",
        b"big.bin",
        b"dir",
        b"empty",
        b"etc",
        b"hello.txt",
        b"link",
    ];
    assert_eq!(names(&entries).as_slice(), expected);
    assert_eq!(entries[0].file_type, FileType::Directory);
    assert_eq!(entries[2].file_type, FileType::Regular);
    assert_eq!(entries[7].file_type, FileType::Symlink);
    assert_eq!(entries[2].name_str(), Some("big.bin"));
}

#[test]
fn small_files_and_directories() {
    let mut fs = mount();
    assert_eq!(
        fs.read_file_path("/etc/motd").expect("motd"),
        b"Welcome to IRIX 7\n"
    );
    assert_eq!(
        fs.read_file_path("/hello.txt").expect("hello"),
        b"hello, erofs\n"
    );
    assert_eq!(
        fs.read_file_path("/dir/nested.txt").expect("nested"),
        b"nested\n"
    );
    assert_eq!(fs.read_file_path("/empty").expect("empty"), b"");
    assert_eq!(
        fs.read_file_path("//etc//motd").expect("duplicate slashes"),
        b"Welcome to IRIX 7\n"
    );

    let etc = fs.lookup_path("/etc").expect("etc dir");
    assert!(etc.is_dir());
    let motd = fs
        .lookup(&etc, b"motd")
        .expect("lookup motd")
        .expect("motd exists");
    assert_eq!(motd.file_type, FileType::Regular);
    assert_eq!(motd.mode, 0o100_644);
    assert_eq!(motd.nlink, 1);

    assert!(fs
        .lookup(&etc, b"missing")
        .expect("lookup missing")
        .is_none());
    assert!(matches!(
        fs.lookup_path("/etc/missing"),
        Err(Error::NotFound)
    ));
}

#[test]
fn multi_block_inline_file() {
    let mut fs = mount();
    let big = fs.lookup_path("/big.bin").expect("big.bin");
    assert_eq!(big.size, 9000);
    assert!(big.is_regular());
    assert_eq!(big.layout, DataLayout::Inline);
    assert_eq!(big.start_blk, 1);

    let data = fs.read_file(&big).expect("read big.bin");
    assert_eq!(data.len(), 9000);
    for (offset, byte) in data.iter().enumerate() {
        assert_eq!(*byte, (offset as u32 * 7) as u8, "offset {offset}");
    }

    // The full-block part is 8192 bytes; this range crosses into the inline tail.
    let mut cross = [0u8; 300];
    fs.read_at(&big, 8000, &mut cross).expect("crossing range");
    for (offset, byte) in cross.iter().enumerate() {
        assert_eq!(*byte, ((8000 + offset) as u32 * 7) as u8);
    }

    // The last bytes come from the inline tail.
    let mut tail = [0u8; 8];
    fs.read_at(&big, 8992, &mut tail).expect("tail range");
    for (offset, byte) in tail.iter().enumerate() {
        assert_eq!(*byte, ((8992 + offset) as u32 * 7) as u8);
    }

    let mut past = [0u8; 2];
    assert_eq!(fs.read_at(&big, 8999, &mut past), Err(Error::OutOfBounds));
    assert!(fs.read_at(&big, 9000, &mut []).is_ok());
}

#[test]
fn symlink_and_rejections() {
    let mut fs = mount();
    let link = fs.lookup_path("/link").expect("link");
    assert!(link.is_symlink());
    assert_eq!(link.size, 9);
    assert_eq!(fs.read_link(&link).expect("readlink"), b"hello.txt");

    let big = fs.lookup_path("/big.bin").expect("big.bin");
    assert_eq!(fs.read_link(&big), Err(Error::NotSymlink));
    assert!(matches!(fs.read_dir(&big), Err(Error::NotDirectory)));
    assert!(matches!(
        fs.lookup_path("/big.bin/child"),
        Err(Error::NotDirectory)
    ));
    assert!(matches!(
        fs.lookup_path("/link/.."),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn bad_magic_is_rejected() {
    let mut image = IMAGE.to_vec();
    image[1024..1028].copy_from_slice(&[0, 0, 0, 0]);
    assert!(matches!(
        Erofs::mount(image.as_slice()),
        Err(Error::BadMagic)
    ));
}

#[test]
fn unknown_incompatible_feature_is_rejected() {
    let mut image = IMAGE.to_vec();
    let bits = 0x8000_0000u32;
    image[1024 + 80..1024 + 84].copy_from_slice(&bits.to_le_bytes());
    assert!(matches!(
        Erofs::mount(image.as_slice()),
        Err(Error::UnsupportedFeature(0x8000_0000))
    ));
}

#[test]
fn truncated_image_is_rejected() {
    assert!(matches!(Erofs::mount(&IMAGE[..1024]), Err(Error::Io)));
    assert!(matches!(Erofs::mount(&IMAGE[..1100]), Err(Error::Io)));
}

#[test]
fn invalid_block_size_is_rejected() {
    let mut image = IMAGE.to_vec();
    image[1024 + 12] = 3; // 8-byte blocks
    assert!(matches!(
        Erofs::mount(image.as_slice()),
        Err(Error::InvalidSuperblock(_))
    ));
}

#[test]
fn compressed_layout_is_recognised_not_misread() {
    let mut image = IMAGE.to_vec();
    // big.bin: nid 42, metadata block 0, compact inode at 42 * 32 = 0x540.
    let nid_offset = 42 * 32;
    let format: u16 = 1 << 1; // datalayout 1: compressed full, compact inode
    image[nid_offset..nid_offset + 2].copy_from_slice(&format.to_le_bytes());

    let mut fs = Erofs::mount(image.as_slice()).expect("mount");
    let big = fs.lookup_path("/big.bin").expect("big.bin");
    assert_eq!(big.layout, DataLayout::CompressedFull);
    assert_eq!(fs.read_file(&big), Err(Error::UnsupportedLayout(1)));
}

#[test]
fn zero_length_reads_at_eof_are_allowed() {
    let mut fs = mount();
    let hello = fs.lookup_path("/hello.txt").expect("hello");
    assert!(fs.read_at(&hello, hello.size, &mut []).is_ok());
    assert_eq!(
        fs.read_at(&hello, hello.size + 1, &mut []),
        Err(Error::OutOfBounds)
    );
}

#[test]
fn extended_inodes_are_decoded() {
    let mut fs = Erofs::mount(EXTENDED).expect("mount extended image");
    assert!(fs.superblock().has_checksum());
    assert!(fs.superblock().has_mtime());
    let root = fs.root().expect("extended root");
    assert_eq!(root.inode_size, 64);
    assert_eq!(root.size, 130);
    assert!(root.is_dir());
    check_tree(&mut fs);
}

#[test]
fn block_addresses_48bit_are_recognised() {
    let mut fs = Erofs::mount(IMAGE_48BIT).expect("mount 48-bit image");
    assert!(fs.superblock().has_48bit());
    assert_eq!(fs.superblock().root_nid, 36);
    let root = fs.root().expect("48-bit root");
    assert!(root.is_dir());
    check_tree(&mut fs);
}
