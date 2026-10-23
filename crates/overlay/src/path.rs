// SPDX-License-Identifier: GPL-3.0-only
//! Path normalisation shared by the overlay operations.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{Error, Result};

/// Normalise an absolute path for use as an upper-tree key.
///
/// A leading `/` is required.  Duplicate slashes, trailing slashes and `.`
/// components are removed.  A `..` component removes the previous component;
/// if there is no component left, the path escapes the root and is rejected
/// with [`Error::EscapesRoot`].  The root itself normalises to `/`.
pub fn normalise(path: &str) -> Result<String> {
    if !path.starts_with('/') {
        return Err(Error::NotAbsolute);
    }
    if path.as_bytes().contains(&0) {
        return Err(Error::InvalidPath("embedded NUL"));
    }

    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(Error::EscapesRoot);
                }
            }
            name => {
                parts.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
                parts.push(name);
            }
        }
    }

    let mut normalised = String::new();
    if parts.is_empty() {
        normalised.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
        normalised.push('/');
        return Ok(normalised);
    }
    for part in parts {
        normalised
            .try_reserve(part.len() + 1)
            .map_err(|_| Error::OutOfMemory)?;
        normalised.push('/');
        normalised.push_str(part);
    }
    Ok(normalised)
}

/// The parent of a normalised path, or `None` for the root.
pub(crate) fn parent(path: &str) -> Option<&str> {
    if path == "/" {
        return None;
    }
    let slash = path.rfind('/')?;
    if slash == 0 {
        Some("/")
    } else {
        Some(&path[..slash])
    }
}

/// The final component of a normalised path.
pub(crate) fn name(path: &str) -> &str {
    match path.rfind('/') {
        Some(slash) => &path[slash + 1..],
        None => path,
    }
}

/// Join a single component to a normalised directory path.
pub(crate) fn join(parent: &str, name: &str) -> Result<String> {
    let separator = usize::from(!parent.ends_with('/'));
    let len = parent
        .len()
        .checked_add(separator)
        .and_then(|len| len.checked_add(name.len()))
        .ok_or(Error::OutOfMemory)?;
    let mut path = String::new();
    path.try_reserve(len).map_err(|_| Error::OutOfMemory)?;
    path.push_str(parent);
    if separator != 0 {
        path.push('/');
    }
    path.push_str(name);
    Ok(path)
}

/// The key prefix shared by all descendants of a directory.
pub(crate) fn prefix(path: &str) -> Result<String> {
    let mut prefix = String::new();
    prefix
        .try_reserve(path.len() + 1)
        .map_err(|_| Error::OutOfMemory)?;
    prefix.push_str(path);
    if !prefix.ends_with('/') {
        prefix.push('/');
    }
    Ok(prefix)
}
