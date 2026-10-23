// SPDX-License-Identifier: GPL-3.0-only
//! The merged namespace: an in-memory upper tree over a read-only lower.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use crate::lower::{DirEntry, EntryKind, Lower};
use crate::path::{self, normalise};
use crate::{Error, Result};

/// What a path names in the merged namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    /// Whether the path is a file or a directory.
    pub kind: EntryKind,
}

/// A read-only-lower, in-memory-upper overlay.
///
/// The upper tree holds complete file contents, explicit directories and
/// whiteouts.  Writing a path that also exists in the lower simply shadows the
/// lower file; there is no copy-up because the lower is never modified.
#[derive(Debug)]
pub struct Overlay<L> {
    lower: L,
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
    whiteouts: BTreeSet<String>,
}

impl<L: Lower> Overlay<L> {
    /// Create an empty upper tree over `lower`.
    #[must_use]
    pub fn new(lower: L) -> Self {
        Self {
            lower,
            files: BTreeMap::new(),
            dirs: BTreeSet::new(),
            whiteouts: BTreeSet::new(),
        }
    }

    /// The lower namespace.
    #[must_use]
    pub fn lower(&self) -> &L {
        &self.lower
    }

    /// The lower namespace, mutably.
    pub fn lower_mut(&mut self) -> &mut L {
        &mut self.lower
    }

    /// Store a complete upper file at `path`.
    ///
    /// The parent directory must already exist in the merged namespace.  A
    /// lower file at the same path is shadowed, and a whiteout there is
    /// cleared.  A visible directory at `path` returns [`Error::IsDirectory`].
    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<()> {
        let path = normalise(path)?;
        if path == "/" {
            return Err(Error::IsDirectory);
        }
        self.check_parent(&path)?;
        if self.lookup_kind(&path)? == Some(EntryKind::Directory) {
            return Err(Error::IsDirectory);
        }

        let mut contents = Vec::new();
        contents
            .try_reserve_exact(data.len())
            .map_err(|_| Error::OutOfMemory)?;
        contents.extend_from_slice(data);

        self.whiteouts.remove(&path);
        self.files.insert(path, contents);
        Ok(())
    }

    /// Read a whole file, preferring the upper tree.
    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        let path = normalise(path)?;
        match self.lookup_kind(&path)? {
            Some(EntryKind::File) => {}
            Some(EntryKind::Directory) => return Err(Error::IsDirectory),
            None => return Err(Error::NotFound),
        }

        if let Some(data) = self.files.get(&path) {
            let mut contents = Vec::new();
            contents
                .try_reserve_exact(data.len())
                .map_err(|_| Error::OutOfMemory)?;
            contents.extend_from_slice(data);
            return Ok(contents);
        }

        self.lower.read_file(&path)?.ok_or(Error::NotFound)
    }

    /// Remove a file, hiding any lower file at the same path with a whiteout.
    pub fn remove_file(&mut self, path: &str) -> Result<()> {
        let path = normalise(path)?;
        if path == "/" {
            return Err(Error::IsDirectory);
        }
        self.check_parent(&path)?;
        match self.lookup_kind(&path)? {
            Some(EntryKind::File) => {}
            Some(EntryKind::Directory) => return Err(Error::IsDirectory),
            None => return Err(Error::NotFound),
        }

        self.files.remove(&path);
        self.whiteouts.insert(path);
        Ok(())
    }

    /// Create a directory.
    ///
    /// Creating an existing directory succeeds without changing anything.  A
    /// visible file at the same path returns [`Error::AlreadyExists`].  The
    /// parent directory must already exist in the merged namespace.
    pub fn create_dir(&mut self, path: &str) -> Result<()> {
        let path = normalise(path)?;
        if path == "/" {
            return Ok(());
        }
        self.check_parent(&path)?;
        match self.lookup_kind(&path)? {
            Some(EntryKind::Directory) => return Ok(()),
            Some(EntryKind::File) => return Err(Error::AlreadyExists),
            None => {}
        }

        self.whiteouts.remove(&path);
        self.dirs.insert(path);
        Ok(())
    }

    /// Remove a directory that is empty in the merged namespace.
    ///
    /// A whiteout is recorded so that a lower directory, and everything below
    /// it, stays hidden.  A directory with visible children returns
    /// [`Error::NotEmpty`].
    pub fn remove_dir(&mut self, path: &str) -> Result<()> {
        let path = normalise(path)?;
        if path == "/" {
            return Err(Error::InvalidPath("cannot remove the root directory"));
        }
        self.check_parent(&path)?;
        match self.lookup_kind(&path)? {
            Some(EntryKind::Directory) => {}
            Some(EntryKind::File) => return Err(Error::NotDirectory),
            None => return Err(Error::NotFound),
        }
        if !self.read_dir(&path)?.is_empty() {
            return Err(Error::NotEmpty);
        }

        self.dirs.remove(&path);
        self.whiteouts.insert(path);
        Ok(())
    }

    /// Whether `path` exists in the merged namespace.
    pub fn exists(&mut self, path: &str) -> Result<bool> {
        let path = normalise(path)?;
        Ok(self.lookup_kind(&path)?.is_some())
    }

    /// Whether `path` is a directory in the merged namespace.
    pub fn is_dir(&mut self, path: &str) -> Result<bool> {
        let path = normalise(path)?;
        Ok(self.lookup_kind(&path)? == Some(EntryKind::Directory))
    }

    /// Whether `path` is a regular file in the merged namespace.
    pub fn is_file(&mut self, path: &str) -> Result<bool> {
        let path = normalise(path)?;
        Ok(self.lookup_kind(&path)? == Some(EntryKind::File))
    }

    /// Look up a path's kind.
    pub fn stat(&mut self, path: &str) -> Result<Stat> {
        let path = normalise(path)?;
        let kind = self.lookup_kind(&path)?.ok_or(Error::NotFound)?;
        Ok(Stat { kind })
    }

    /// List the direct child names of a directory, sorted.
    pub fn list(&mut self, path: &str) -> Result<Vec<String>> {
        let entries = self.read_dir(path)?;
        let mut names = Vec::new();
        names
            .try_reserve(entries.len())
            .map_err(|_| Error::OutOfMemory)?;
        for entry in entries {
            names.push(entry.name);
        }
        Ok(names)
    }

    /// List a directory, merging upper entries over lower ones.
    ///
    /// Upper files and directories shadow lower entries with the same name,
    /// and a whiteout hides a lower entry.  The result is sorted by name.
    pub fn read_dir(&mut self, path: &str) -> Result<Vec<DirEntry>> {
        let path = normalise(path)?;
        match self.lookup_kind(&path)? {
            Some(EntryKind::Directory) => {}
            Some(EntryKind::File) => return Err(Error::NotDirectory),
            None => return Err(Error::NotFound),
        }

        let mut entries = Vec::new();
        self.collect_upper(&path, &mut entries)?;
        if let Some(lower) = self.lower.read_dir(&path)? {
            for entry in lower {
                if !is_name(&entry.name) {
                    return Err(Error::Lower("invalid directory entry name"));
                }
                let child = path::join(&path, &entry.name)?;
                if self.whiteouts.contains(&child)
                    || self.files.contains_key(&child)
                    || self.dirs.contains(&child)
                {
                    continue;
                }
                push_entry(&mut entries, &entry.name, entry.kind)?;
            }
        }
        entries.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }

    /// Resolve a path to its kind, if it is visible.
    fn lookup_kind(&mut self, path: &str) -> Result<Option<EntryKind>> {
        if self.is_hidden(path) || self.ancestor_is_file(path) {
            return Ok(None);
        }
        if self.files.contains_key(path) {
            return Ok(Some(EntryKind::File));
        }
        if path == "/" || self.dirs.contains(path) {
            return Ok(Some(EntryKind::Directory));
        }
        self.lower_kind(path)
    }

    /// Ask the lower tree for a path's kind by listing its parent.
    ///
    /// `Lower` intentionally has no metadata call, so the overlay reads the
    /// parent directory rather than the file itself.
    fn lower_kind(&mut self, path: &str) -> Result<Option<EntryKind>> {
        if path == "/" {
            return Ok(Some(EntryKind::Directory));
        }
        let parent = path::parent(path).ok_or(Error::NotFound)?;
        let name = path::name(path);
        let Some(entries) = self.lower.read_dir(parent)? else {
            return Ok(None);
        };
        Ok(entries
            .into_iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.kind))
    }

    /// Check that `path` has a visible directory as its parent.
    fn check_parent(&mut self, path: &str) -> Result<()> {
        let parent = path::parent(path).ok_or(Error::NotFound)?;
        match self.lookup_kind(parent)? {
            Some(EntryKind::Directory) => Ok(()),
            Some(EntryKind::File) => Err(Error::NotDirectory),
            None => Err(Error::NotFound),
        }
    }

    /// Whether a whiteout at `path` or an ancestor hides the path.
    fn is_hidden(&self, path: &str) -> bool {
        let mut current = Some(path);
        while let Some(candidate) = current {
            if self.whiteouts.contains(candidate) {
                return true;
            }
            current = path::parent(candidate);
        }
        false
    }

    /// Whether an upper file at an ancestor prevents descending to `path`.
    fn ancestor_is_file(&self, path: &str) -> bool {
        let mut current = path::parent(path);
        while let Some(candidate) = current {
            if self.files.contains_key(candidate) {
                return true;
            }
            current = path::parent(candidate);
        }
        false
    }

    /// Add the direct upper children of `path` to `entries`.
    fn collect_upper(&self, path: &str, entries: &mut Vec<DirEntry>) -> Result<()> {
        let prefix = path::prefix(path)?;
        for (key, _) in self.files.range(prefix.clone()..) {
            if !key.starts_with(prefix.as_str()) {
                break;
            }
            let name = &key[prefix.len()..];
            if !name.contains('/') {
                push_entry(entries, name, EntryKind::File)?;
            }
        }
        for key in self.dirs.range(prefix.clone()..) {
            if !key.starts_with(prefix.as_str()) {
                break;
            }
            let name = &key[prefix.len()..];
            if !name.contains('/') {
                push_entry(entries, name, EntryKind::Directory)?;
            }
        }
        Ok(())
    }
}

/// Whether `name` is a single, non-special path component.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.as_bytes().contains(&0)
}

/// Append a copied directory entry, reserving fallibly.
fn push_entry(entries: &mut Vec<DirEntry>, name: &str, kind: EntryKind) -> Result<()> {
    entries.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
    let mut owned = String::new();
    owned
        .try_reserve(name.len())
        .map_err(|_| Error::OutOfMemory)?;
    owned.push_str(name);
    entries.push(DirEntry { name: owned, kind });
    Ok(())
}
