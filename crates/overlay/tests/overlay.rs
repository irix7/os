// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the overlay.

use std::collections::{BTreeMap, BTreeSet};

use overlay::{normalise, DirEntry, EntryKind, Error, Lower, Overlay};

/// A small read-only lower tree built from in-memory files and directories.
#[derive(Debug, Default)]
struct MemoryLower {
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
}

impl MemoryLower {
    fn new() -> Self {
        let mut lower = Self::default();
        lower.dirs.insert(String::from("/"));
        lower
    }

    fn dir(mut self, path: &str) -> Self {
        assert!(path.starts_with('/'), "test path must be absolute");
        let mut current = String::new();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            current.push('/');
            current.push_str(part);
            self.dirs.insert(current.clone());
        }
        self
    }

    fn file(mut self, path: &str, data: &[u8]) -> Self {
        assert!(path.starts_with('/'), "test path must be absolute");
        let parent = match path.rfind('/') {
            Some(0) | None => "/",
            Some(slash) => &path[..slash],
        };
        self = self.dir(parent);
        self.files.insert(path.to_string(), data.to_vec());
        self
    }
}

impl Lower for MemoryLower {
    fn read_file(&mut self, path: &str) -> overlay::Result<Option<Vec<u8>>> {
        Ok(self.files.get(path).cloned())
    }

    fn read_dir(&mut self, path: &str) -> overlay::Result<Option<Vec<DirEntry>>> {
        if !self.dirs.contains(path) {
            return Ok(None);
        }
        let prefix = if path == "/" {
            String::from("/")
        } else {
            format!("{path}/")
        };
        let mut entries = Vec::new();
        for file in self.files.keys() {
            if let Some(name) = file.strip_prefix(&prefix) {
                if !name.is_empty() && !name.contains('/') {
                    entries.push(DirEntry {
                        name: name.to_string(),
                        kind: EntryKind::File,
                    });
                }
            }
        }
        for dir in &self.dirs {
            if let Some(name) = dir.strip_prefix(&prefix) {
                if !name.is_empty() && !name.contains('/') {
                    entries.push(DirEntry {
                        name: name.to_string(),
                        kind: EntryKind::Directory,
                    });
                }
            }
        }
        Ok(Some(entries))
    }
}

fn overlay(lower: MemoryLower) -> Overlay<MemoryLower> {
    Overlay::new(lower)
}

#[test]
fn reads_a_lower_file() {
    let mut fs = overlay(MemoryLower::new().file("/etc/motd", b"welcome"));
    assert_eq!(fs.read_file("/etc/motd").unwrap(), b"welcome");
    assert!(fs.exists("/etc/motd").unwrap());
    assert!(fs.is_file("/etc/motd").unwrap());
    assert!(!fs.is_dir("/etc/motd").unwrap());
    assert_eq!(fs.stat("/etc/motd").unwrap().kind, EntryKind::File);
    assert_eq!(fs.stat("/etc").unwrap().kind, EntryKind::Directory);
}

#[test]
fn upper_file_shadows_lower_file() {
    let mut fs = overlay(MemoryLower::new().file("/etc/motd", b"lower"));
    fs.write_file("/etc/motd", b"upper").unwrap();
    assert_eq!(fs.read_file("/etc/motd").unwrap(), b"upper");
}

#[test]
fn whiteout_hides_a_lower_file() {
    let mut fs = overlay(
        MemoryLower::new()
            .file("/etc/motd", b"welcome")
            .file("/etc/passwd", b"root"),
    );
    fs.remove_file("/etc/motd").unwrap();

    assert!(!fs.exists("/etc/motd").unwrap());
    assert_eq!(fs.read_file("/etc/motd"), Err(Error::NotFound));
    assert_eq!(fs.list("/etc").unwrap(), ["passwd"]);
}

#[test]
fn writing_after_removal_replaces_the_lower_file() {
    let mut fs = overlay(MemoryLower::new().file("/etc/motd", b"lower"));
    fs.remove_file("/etc/motd").unwrap();
    fs.write_file("/etc/motd", b"new").unwrap();

    assert_eq!(fs.read_file("/etc/motd").unwrap(), b"new");
    assert_eq!(fs.list("/etc").unwrap(), ["motd"]);
}

#[test]
fn merges_upper_and_lower_directories() {
    let mut fs = overlay(
        MemoryLower::new()
            .file("/etc/motd", b"welcome")
            .dir("/etc/ssl"),
    );
    fs.write_file("/etc/hostname", b"indy").unwrap();

    let entries = fs.read_dir("/etc").unwrap();
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, ["hostname", "motd", "ssl"]);
    assert_eq!(entries[0].kind, EntryKind::File);
    assert_eq!(entries[2].kind, EntryKind::Directory);
    assert_eq!(fs.list("/etc").unwrap(), ["hostname", "motd", "ssl"]);
}

#[test]
fn lists_only_direct_children() {
    let mut fs = overlay(MemoryLower::new().file("/var/log/messages", b"log"));
    fs.write_file("/var/log/current", b"now").unwrap();
    fs.create_dir("/var/log/old").unwrap();

    assert_eq!(fs.list("/var").unwrap(), ["log"]);
    assert_eq!(fs.list("/var/log").unwrap(), ["current", "messages", "old"]);
}

#[test]
fn removes_an_empty_lower_directory() {
    let mut fs = overlay(MemoryLower::new().dir("/empty"));
    assert!(fs.is_dir("/empty").unwrap());

    fs.remove_dir("/empty").unwrap();
    assert!(!fs.exists("/empty").unwrap());
    assert_eq!(fs.read_dir("/empty"), Err(Error::NotFound));
    assert_eq!(fs.list("/").unwrap(), Vec::<String>::new());
}

#[test]
fn refuses_to_remove_a_non_empty_directory() {
    let mut fs = overlay(MemoryLower::new().file("/etc/motd", b"welcome"));
    assert_eq!(fs.remove_dir("/etc"), Err(Error::NotEmpty));
    assert!(fs.is_dir("/etc").unwrap());
}

#[test]
fn directory_whiteout_hides_the_lower_subtree() {
    let mut fs = overlay(MemoryLower::new().file("/etc/ssl/openssl.cnf", b"config"));
    fs.remove_file("/etc/ssl/openssl.cnf").unwrap();
    fs.remove_dir("/etc/ssl").unwrap();

    assert!(!fs.exists("/etc/ssl").unwrap());
    assert!(!fs.exists("/etc/ssl/openssl.cnf").unwrap());
    assert_eq!(fs.read_file("/etc/ssl/openssl.cnf"), Err(Error::NotFound));
}

#[test]
fn normalises_absolute_paths() {
    assert_eq!(normalise("//tmp///a/./b/").unwrap(), "/tmp/a/b");
    assert_eq!(normalise("/").unwrap(), "/");
    assert_eq!(normalise("/a/../b").unwrap(), "/b");
    assert_eq!(normalise("/a/..").unwrap(), "/");

    let mut fs = overlay(MemoryLower::new());
    fs.create_dir("//tmp").unwrap();
    fs.create_dir("//tmp/./work").unwrap();
    fs.write_file("/tmp//work/./file", b"data").unwrap();
    assert_eq!(fs.read_file("//tmp/work/file").unwrap(), b"data");
}

#[test]
fn rejects_parent_traversal_above_the_root() {
    assert_eq!(normalise("/.."), Err(Error::EscapesRoot));
    assert_eq!(normalise("/tmp/../../etc"), Err(Error::EscapesRoot));

    let mut fs = overlay(MemoryLower::new());
    assert_eq!(fs.read_file("/../etc/passwd"), Err(Error::EscapesRoot));
    assert_eq!(fs.create_dir("/a/../../b"), Err(Error::EscapesRoot));
}

#[test]
fn rejects_relative_paths() {
    assert_eq!(normalise("tmp/x"), Err(Error::NotAbsolute));
    assert_eq!(normalise(""), Err(Error::NotAbsolute));

    let mut fs = overlay(MemoryLower::new());
    assert_eq!(fs.create_dir("tmp"), Err(Error::NotAbsolute));
    assert_eq!(fs.write_file("file", b"x"), Err(Error::NotAbsolute));
}

#[test]
fn missing_paths_return_not_found() {
    let mut fs = overlay(MemoryLower::new());
    assert!(!fs.exists("/missing").unwrap());
    assert_eq!(fs.read_file("/missing"), Err(Error::NotFound));
    assert_eq!(fs.read_dir("/missing"), Err(Error::NotFound));
    assert_eq!(fs.remove_file("/missing"), Err(Error::NotFound));
    assert_eq!(fs.remove_dir("/missing"), Err(Error::NotFound));
    assert_eq!(fs.stat("/missing"), Err(Error::NotFound));
}

#[test]
fn handles_empty_files() {
    let mut fs = overlay(MemoryLower::new().dir("/tmp"));
    fs.write_file("/tmp/empty", b"").unwrap();

    assert!(fs.exists("/tmp/empty").unwrap());
    assert_eq!(fs.read_file("/tmp/empty").unwrap(), Vec::<u8>::new());
    assert_eq!(fs.stat("/tmp/empty").unwrap().kind, EntryKind::File);

    fs.write_file("/tmp/empty", b"full").unwrap();
    assert_eq!(fs.read_file("/tmp/empty").unwrap(), b"full");
}

#[test]
fn write_requires_an_existing_parent() {
    let mut fs = overlay(MemoryLower::new());
    assert_eq!(fs.write_file("/tmp/x", b"x"), Err(Error::NotFound));
    assert_eq!(fs.create_dir("/a/b"), Err(Error::NotFound));

    fs.create_dir("/tmp").unwrap();
    fs.write_file("/tmp/x", b"x").unwrap();
    assert_eq!(fs.read_file("/tmp/x").unwrap(), b"x");
}

#[test]
fn writes_and_removals_check_node_kind() {
    let mut fs = overlay(MemoryLower::new().file("/etc/motd", b"welcome"));
    assert_eq!(fs.write_file("/", b"x"), Err(Error::IsDirectory));
    assert_eq!(fs.write_file("/etc", b"x"), Err(Error::IsDirectory));
    assert_eq!(fs.read_file("/etc"), Err(Error::IsDirectory));
    assert_eq!(fs.remove_file("/etc"), Err(Error::IsDirectory));

    fs.create_dir("/tmp").unwrap();
    assert_eq!(fs.remove_dir("/tmp/../etc"), Err(Error::NotEmpty));
    assert_eq!(fs.remove_file("/tmp"), Err(Error::IsDirectory));
    assert_eq!(
        fs.remove_dir("/"),
        Err(Error::InvalidPath("cannot remove the root directory"))
    );
}

#[test]
fn create_dir_is_idempotent_and_rejects_files() {
    let mut fs = overlay(MemoryLower::new().file("/etc/motd", b"welcome"));
    fs.create_dir("/tmp").unwrap();
    fs.create_dir("/tmp").unwrap();
    assert!(fs.is_dir("/tmp").unwrap());
    assert!(fs.is_dir("/").unwrap());
    assert_eq!(fs.create_dir("/etc/motd"), Err(Error::AlreadyExists));
}

#[test]
fn read_file_returns_an_independent_copy() {
    let mut fs = overlay(MemoryLower::new().dir("/tmp"));
    fs.write_file("/tmp/file", b"abc").unwrap();

    let mut data = fs.read_file("/tmp/file").unwrap();
    data[0] = b'z';
    assert_eq!(fs.read_file("/tmp/file").unwrap(), b"abc");
}

#[test]
fn lower_trait_is_object_safe() {
    let mut lower = MemoryLower::new().file("/hello", b"world");
    let mut fs = Overlay::new(&mut lower as &mut dyn Lower);
    assert_eq!(fs.read_file("/hello").unwrap(), b"world");
}
