// SPDX-License-Identifier: GPL-3.0-only
//! Mutable in-memory filesystem for the bring-up userland.
//!
//! The tree is a fixed inode table.  Every node records its parent, its name,
//! a mode and a fixed 4 KiB content buffer.  Names are unique within a
//! directory; there are no hard links and no symbolic links.  An unlinked
//! inode stays alive while a descriptor still refers to it, which is the POSIX
//! unlink-while-open rule; [`release`] frees it when the last descriptor goes.
//! Inode numbers are the node index plus one and are reused after the node is
//! freed, so `st_ino` identifies a live node, not a durable history.
//!
//! The limits are bring-up limits, not a claim about the final VFS:
//! [`MAX_NODES`] inodes, [`FILE_CAPACITY`] bytes per regular file and
//! [`NAME_MAX`] bytes per name.  Exhausting the inode table or writing past a
//! file's capacity is `ENOSPC`.

use crate::syserr::{
    SysResult, CELL_E_EXIST, CELL_E_INVAL, CELL_E_ISDIR, CELL_E_NAMETOOLONG, CELL_E_NOENT,
    CELL_E_NOSPC, CELL_E_NOTDIR, CELL_E_NOTEMPTY, CELL_E_ROFS,
};

pub const MAX_NODES: usize = 32;
pub const FILE_CAPACITY: usize = 4096;
pub const NAME_MAX: usize = 63;
pub const ROOT: u16 = 0;
/// Largest executable image a ramfs `Image` node can name.
///
/// The staging slot is the same 256 KiB slot the target-2 loader uses, so a
/// staged child ELF fits without a second large `.bss` allocation.
pub const IMAGE_CAPACITY: usize = 256 * 1024;

static mut IMAGE_DATA: Option<&'static mut [u8]> = None;
static mut IMAGE_LEN: usize = 0;

/// The writable staging buffer behind `/bin/child`.
///
/// The boot path reads SCSI target 3 into it before userland starts.  The
/// contents survive [`reset`]: the node is recreated, the bytes are not.
pub fn image_buffer() -> &'static mut [u8] {
    // SAFETY: the console owns the machine while staging, and user code never
    // receives this pointer.  The loader copies out of it.
    crate::bigbuf::get(
        unsafe { &mut *core::ptr::addr_of_mut!(IMAGE_DATA) },
        IMAGE_CAPACITY,
    )
}

/// Publish the staged image length.  Longer than [`IMAGE_CAPACITY`] is refused.
pub fn set_image_size(size: usize) -> bool {
    if size > IMAGE_CAPACITY {
        return false;
    }
    // SAFETY: single-threaded kernel; the staging buffer is not mapped into a
    // user address space.
    unsafe {
        IMAGE_LEN = size;
    }
    true
}

/// Length of the staged image.
pub fn image_size() -> usize {
    // SAFETY: single-threaded kernel.
    unsafe { IMAGE_LEN }
}

/// The staged image bytes, for the ELF loader.
pub fn image_data() -> &'static [u8] {
    // SAFETY: the length is set once by the boot staging path and never exceeds
    // IMAGE_CAPACITY.
    let image_len = unsafe { IMAGE_LEN };
    &crate::bigbuf::get(
        unsafe { &mut *core::ptr::addr_of_mut!(IMAGE_DATA) },
        IMAGE_CAPACITY,
    )[..image_len]
}

/// `dirent.d_type` values used by the native ABI.
pub const TYPE_CHR: u8 = 2;
pub const TYPE_DIR: u8 = 4;
pub const TYPE_REG: u8 = 8;

const FREE: u8 = 0;
const FILE: u8 = 1;
const DIR: u8 = 2;
const LOGICAL: u8 = 3;
const NULL_DEVICE: u8 = 4;
const CONSOLE_DEVICE: u8 = 5;
/// An executable image backed by the staging buffer, not by `Node::data`.
const IMAGE: u8 = 6;

#[derive(Clone, Copy)]
struct Node {
    kind: u8,
    /// The name is gone, but an open descriptor still refers to the inode.
    unlinked: bool,
    parent: u16,
    name_len: u8,
    size: u32,
    mode: u32,
    name: [u8; NAME_MAX],
    data: [u8; FILE_CAPACITY],
}

const EMPTY: Node = Node {
    kind: FREE,
    unlinked: false,
    parent: ROOT,
    name_len: 0,
    size: 0,
    mode: 0,
    name: [0; NAME_MAX],
    data: [0; FILE_CAPACITY],
};

static mut NODES: [Node; MAX_NODES] = [EMPTY; MAX_NODES];

/// The kind of object an inode names.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    File,
    Dir,
    /// `/etc/motd`: content lives in the `logical` policy namespace.
    Logical,
    Null,
    Console,
    /// `/bin/child`: the staged executable image.
    Image,
}

/// A resolved path: either an inode or a missing final name in a directory.
pub enum Resolved<'a> {
    Node(u16),
    Missing { parent: u16, name: &'a [u8] },
}

fn nodes() -> *mut Node {
    // SAFETY: the table is a plain static array; the kernel is single-threaded
    // and every access goes through this pointer for its whole lifetime.
    core::ptr::addr_of_mut!(NODES) as *mut Node
}

fn node(index: u16) -> &'static mut Node {
    // SAFETY: every index given to this function is below MAX_NODES; callers
    // are kernel code that does not run concurrently.
    unsafe { &mut *nodes().add(index as usize) }
}

fn kind_of(index: u16) -> u8 {
    node(index).kind
}

/// Allocate an inode under `parent` with a copy of `name`.
fn allocate(kind: u8, parent: u16, name: &[u8], mode: u32) -> SysResult<u16> {
    if name.is_empty() || name.len() > NAME_MAX {
        return Err(CELL_E_NAMETOOLONG);
    }
    for index in 0..MAX_NODES {
        let slot = node(index as u16);
        if slot.kind != FREE {
            continue;
        }
        let mut entry = EMPTY;
        entry.kind = kind;
        entry.parent = parent;
        entry.name_len = name.len() as u8;
        entry.mode = mode;
        entry.name[..name.len()].copy_from_slice(name);
        *slot = entry;
        return Ok(index as u16);
    }
    Err(CELL_E_NOSPC)
}

/// Restore the fixture tree.  Called for every user image start.
pub fn reset() {
    // SAFETY: no descriptor can refer to the old tree after a reset; the fd
    // table is reset next to this call.
    unsafe {
        for index in 0..MAX_NODES {
            *nodes().add(index) = EMPTY;
        }
    }
    let mut root = EMPTY;
    root.kind = DIR;
    root.parent = ROOT;
    root.mode = 0o040755;
    *node(ROOT) = root;

    if let Ok(hello) = allocate(FILE, ROOT, b"hello.txt", 0o100644) {
        let _ = write(hello, 0, b"hello from the IRIX 7 ramfs\n");
    }
    let _ = allocate(DIR, ROOT, b"etc", 0o040755);
    let _ = allocate(DIR, ROOT, b"dev", 0o040755);
    let _ = allocate(LOGICAL, 2, b"motd", 0o100644);
    let _ = allocate(NULL_DEVICE, 3, b"null", 0o020666);
    let _ = allocate(CONSOLE_DEVICE, 3, b"tty", 0o020666);
    let _ = allocate(CONSOLE_DEVICE, 3, b"console", 0o020666);
    // `/bin` is installed only when a child image has actually been staged;
    // the fixture tree stays exactly the five entries the ramfs probe expects.
    unsafe {
        IMAGE_LEN = 0;
    }
}

/// Install the staged image as `/bin/child`.
///
/// It is called only after [`set_image_size`] has accepted a real image, so a
/// node that exists always names valid bytes.  `reset` removes it again.
pub fn install_image() -> bool {
    let bin = match find_child(ROOT, b"bin") {
        Some(node) if is_dir(node) => node,
        Some(_) => return false,
        None => match create(ROOT, b"bin", true, 0o040755) {
            Ok(node) => node,
            Err(_) => return false,
        },
    };
    if find_child(bin, b"child").is_some() {
        return true;
    }
    allocate(IMAGE, bin, b"child", 0o100755).is_ok()
}

fn find_child(dir: u16, name: &[u8]) -> Option<u16> {
    for index in 0..MAX_NODES {
        if index as u16 == dir {
            continue;
        }
        let candidate = node(index as u16);
        if candidate.kind == FREE || candidate.unlinked || candidate.parent != dir {
            continue;
        }
        if candidate.name[..candidate.name_len as usize] == *name {
            return Some(index as u16);
        }
    }
    None
}

fn step(dir: u16, component: &[u8]) -> SysResult<u16> {
    if component == b"." {
        return Ok(dir);
    }
    if component == b".." {
        return Ok(parent(dir));
    }
    if !is_dir(dir) {
        return Err(CELL_E_NOTDIR);
    }
    find_child(dir, component).ok_or(CELL_E_NOENT)
}

/// Resolve `path` against `cwd`.  The final component may be missing; every
/// earlier component must exist and be a directory.
pub fn resolve<'a>(cwd: u16, path: &'a [u8]) -> SysResult<Resolved<'a>> {
    if path.is_empty() {
        return Err(CELL_E_NOENT);
    }
    let trailing = path.last() == Some(&b'/');
    let mut current = if path[0] == b'/' { ROOT } else { cwd };
    let mut previous: Option<&[u8]> = None;
    for component in path.split(|byte| *byte == b'/') {
        if component.is_empty() {
            continue;
        }
        if let Some(component) = previous {
            current = step(current, component)?;
        }
        previous = Some(component);
    }
    let Some(last) = previous else {
        // The path named a directory itself: "/", ".", "a/..", "//".
        return Ok(Resolved::Node(current));
    };
    if last == b"." || last == b".." {
        if !is_dir(current) {
            return Err(CELL_E_NOTDIR);
        }
        return Ok(Resolved::Node(step(current, last)?));
    }
    if !is_dir(current) {
        return Err(CELL_E_NOTDIR);
    }
    match find_child(current, last) {
        Some(index) => {
            if trailing && !is_dir(index) {
                return Err(CELL_E_NOTDIR);
            }
            Ok(Resolved::Node(index))
        }
        None if trailing => Err(CELL_E_NOTDIR),
        None => Ok(Resolved::Missing {
            parent: current,
            name: last,
        }),
    }
}

/// Create a regular file or directory.  The name must not exist.
pub fn create(parent: u16, name: &[u8], dir: bool, mode: u32) -> SysResult<u16> {
    if !is_dir(parent) {
        return Err(CELL_E_NOTDIR);
    }
    if find_child(parent, name).is_some() {
        return Err(CELL_E_EXIST);
    }
    if name == b"." || name == b".." {
        return Err(CELL_E_EXIST);
    }
    allocate(if dir { DIR } else { FILE }, parent, name, mode)
}

pub fn is_dir(index: u16) -> bool {
    kind_of(index) == DIR
}

pub fn node_type(index: u16) -> NodeType {
    match kind_of(index) {
        FILE => NodeType::File,
        DIR => NodeType::Dir,
        LOGICAL => NodeType::Logical,
        NULL_DEVICE => NodeType::Null,
        CONSOLE_DEVICE => NodeType::Console,
        _ => NodeType::Image,
    }
}

pub fn type_of(index: u16) -> u8 {
    match kind_of(index) {
        DIR => TYPE_DIR,
        FILE | LOGICAL | IMAGE => TYPE_REG,
        _ => TYPE_CHR,
    }
}

pub fn parent(index: u16) -> u16 {
    if index == ROOT {
        return ROOT;
    }
    let parent = node(index).parent;
    if parent as usize >= MAX_NODES || kind_of(parent) == FREE {
        ROOT
    } else {
        parent
    }
}

pub fn name(index: u16, out: &mut [u8]) -> usize {
    if index == ROOT || out.is_empty() {
        return 0;
    }
    let entry = node(index);
    let len = (entry.name_len as usize).min(out.len());
    out[..len].copy_from_slice(&entry.name[..len]);
    len
}

pub fn inode(index: u16) -> u64 {
    index as u64 + 1
}

pub fn mode(index: u16) -> u32 {
    match kind_of(index) {
        FREE => 0,
        LOGICAL => 0o100644,
        IMAGE => 0o100755,
        _ => node(index).mode,
    }
}

pub fn size(index: u16) -> usize {
    match kind_of(index) {
        FILE => node(index).size as usize,
        LOGICAL => crate::logical::file_size(b"/etc/motd").unwrap_or(0),
        IMAGE => image_size(),
        _ => 0,
    }
}

/// Number of children, excluding `.` and `..`.
pub fn children(dir: u16) -> usize {
    (0..MAX_NODES)
        .filter(|index| {
            if *index as u16 == dir {
                // The root is its own parent; it is not one of its children.
                return false;
            }
            let candidate = node(*index as u16);
            candidate.kind != FREE && !candidate.unlinked && candidate.parent == dir
        })
        .count()
}

/// The `ordinal`-th child, in inode order.
pub fn child_at(dir: u16, ordinal: usize) -> Option<u16> {
    let mut seen = 0usize;
    for index in 0..MAX_NODES {
        if index as u16 == dir {
            continue;
        }
        let candidate = node(index as u16);
        if candidate.kind == FREE || candidate.unlinked || candidate.parent != dir {
            continue;
        }
        if seen == ordinal {
            return Some(index as u16);
        }
        seen += 1;
    }
    None
}

pub fn is_empty_dir(dir: u16) -> bool {
    children(dir) == 0
}

/// Build the absolute path of `index` into `out`.  Returns `None` if the path
/// does not fit, which `getcwd` reports as a range error.
pub fn path_of(index: u16, out: &mut [u8; 64]) -> Option<usize> {
    let mut end = out.len();
    let mut cursor = index;
    let mut guard = 0usize;
    while cursor != ROOT {
        if guard >= MAX_NODES {
            return None;
        }
        guard += 1;
        let mut component = [0u8; NAME_MAX];
        let len = name(cursor, &mut component);
        if len == 0 || end < len + 1 {
            return None;
        }
        end -= len;
        out[end..end + len].copy_from_slice(&component[..len]);
        end -= 1;
        out[end] = b'/';
        cursor = parent(cursor);
    }
    if end == out.len() {
        out[0] = b'/';
        return Some(1);
    }
    let len = out.len() - end;
    out.copy_within(end.., 0);
    Some(len)
}

pub fn read(index: u16, start: usize, buffer: &mut [u8]) -> SysResult<usize> {
    match kind_of(index) {
        FILE => {}
        IMAGE => {
            let data = image_data();
            if start >= data.len() {
                return Ok(0);
            }
            let count = buffer.len().min(data.len() - start);
            buffer[..count].copy_from_slice(&data[start..start + count]);
            return Ok(count);
        }
        _ => return Err(CELL_E_ISDIR),
    }
    let entry = node(index);
    let len = entry.size as usize;
    if start >= len {
        return Ok(0);
    }
    let count = buffer.len().min(len - start);
    buffer[..count].copy_from_slice(&entry.data[start..start + count]);
    Ok(count)
}

/// Write at `start`.  A write that reaches the capacity is short; the next
/// write at end-of-file is `ENOSPC`.
pub fn write(index: u16, start: usize, buffer: &[u8]) -> SysResult<usize> {
    match kind_of(index) {
        FILE => {}
        IMAGE => return Err(CELL_E_ROFS),
        _ => return Err(CELL_E_ISDIR),
    }
    if buffer.is_empty() {
        return Ok(0);
    }
    if start >= FILE_CAPACITY {
        return Err(CELL_E_NOSPC);
    }
    let count = buffer.len().min(FILE_CAPACITY - start);
    let entry = node(index);
    entry.data[start..start + count].copy_from_slice(&buffer[..count]);
    entry.size = entry.size.max((start + count) as u32);
    Ok(count)
}

/// Set the size of a regular file.  Growing zero-fills; past the capacity is
/// `ENOSPC`.
pub fn truncate(index: u16, length: usize) -> SysResult<()> {
    match kind_of(index) {
        FILE => {}
        DIR => return Err(CELL_E_ISDIR),
        LOGICAL | IMAGE => return Err(CELL_E_ROFS),
        _ => return Err(CELL_E_INVAL),
    }
    if length > FILE_CAPACITY {
        return Err(CELL_E_NOSPC);
    }
    let entry = node(index);
    let old = entry.size as usize;
    if length > old {
        entry.data[old..length].fill(0);
    } else if length < old {
        entry.data[length..old].fill(0);
    }
    entry.size = length as u32;
    Ok(())
}

/// Remove a name.  The inode stays allocated and readable through any open
/// descriptor; the caller reclaims it with [`release`] when no descriptor
/// refers to it.
pub fn unlink(index: u16) -> u16 {
    if index != ROOT {
        node(index).unlinked = true;
    }
    index
}

/// Free an unlinked inode.  Called when the last descriptor to it closes.
pub fn release(index: u16) {
    if index == ROOT {
        return;
    }
    let entry = node(index);
    if entry.kind != FREE && entry.unlinked {
        *entry = EMPTY;
    }
}

/// Rename `index` to `new_parent/new_name`, replacing an existing target.
/// Returns the replaced inode, if any, so the caller can reclaim it.
pub fn rename(index: u16, new_parent: u16, new_name: &[u8]) -> SysResult<Option<u16>> {
    if index == ROOT {
        return Err(CELL_E_INVAL);
    }
    if !is_dir(new_parent) {
        return Err(CELL_E_NOTDIR);
    }
    if new_name.is_empty() || new_name.len() > NAME_MAX {
        return Err(CELL_E_NAMETOOLONG);
    }
    if new_name == b"." || new_name == b".." {
        return Err(CELL_E_INVAL);
    }
    // A directory cannot move inside itself.
    let mut ancestor = new_parent;
    for _ in 0..MAX_NODES {
        if ancestor == index {
            return Err(CELL_E_INVAL);
        }
        if ancestor == ROOT {
            break;
        }
        ancestor = parent(ancestor);
    }

    let mut replaced = None;
    if let Some(target) = find_child(new_parent, new_name) {
        if target == index {
            return Ok(None);
        }
        let moving_dir = is_dir(index);
        let target_dir = is_dir(target);
        if moving_dir && !target_dir {
            return Err(CELL_E_NOTDIR);
        }
        if !moving_dir && target_dir {
            return Err(CELL_E_ISDIR);
        }
        if target_dir && !is_empty_dir(target) {
            return Err(CELL_E_NOTEMPTY);
        }
        unlink(target);
        replaced = Some(target);
    }

    let entry = node(index);
    entry.parent = new_parent;
    entry.name_len = new_name.len() as u8;
    entry.name = [0; NAME_MAX];
    entry.name[..new_name.len()].copy_from_slice(new_name);
    Ok(replaced)
}
