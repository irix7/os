// SPDX-License-Identifier: GPL-3.0-only
//! Buffered output on a descriptor, usable with `write!`.

use core::fmt;

use crate::libc;

/// Write every byte, retrying short writes.  Returns `false` on an error.
pub fn write_all(fd: i32, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        // SAFETY: the slice is valid for `bytes.len()` bytes.
        let count = unsafe { libc::write(fd, bytes.as_ptr(), bytes.len()) };
        if count <= 0 {
            return false;
        }
        bytes = &bytes[count as usize..];
    }
    true
}

/// A descriptor with a small buffer.  It is flushed when full and on drop.
pub struct Out {
    fd: i32,
    buffer: [u8; 256],
    len: usize,
    /// Set when a write failed; later output is dropped.
    pub failed: bool,
}

impl Out {
    pub fn new(fd: i32) -> Self {
        Self {
            fd,
            buffer: [0; 256],
            len: 0,
            failed: false,
        }
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.len == self.buffer.len() {
                self.flush();
            }
            self.buffer[self.len] = byte;
            self.len += 1;
        }
    }

    pub fn flush(&mut self) {
        if self.len != 0 && !self.failed && !write_all(self.fd, &self.buffer[..self.len]) {
            self.failed = true;
        }
        self.len = 0;
    }
}

impl fmt::Write for Out {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.bytes(text.as_bytes());
        Ok(())
    }
}

impl Drop for Out {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Print `sh: <subject>: <strerror(errno)>` on standard error.
pub fn perror(program: &str, subject: &[u8]) {
    let error = libc::errno();
    let mut out = Out::new(2);
    out.bytes(program.as_bytes());
    out.bytes(b": ");
    out.bytes(subject);
    out.bytes(b": ");
    // SAFETY: strerror returns a NUL-terminated static string.
    out.bytes(unsafe { libc::cstr(libc::strerror(error)) });
    out.bytes(b"\n");
}

/// Parse an unsigned decimal number.
pub fn parse_u32(text: &[u8]) -> Option<u32> {
    if text.is_empty() {
        return None;
    }
    let mut value = 0u32;
    for &byte in text {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as u32)?;
    }
    Some(value)
}
